use super::*;
use magi_context::{DisclosureContentKind, DisclosureGrant, Recipient, RepresentationKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const LIFETIME_MS: u64 = 120 * 60 * 1000;
const MAX_TURNS: usize = 20;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantReservation {
    run_id: String,
    token: String,
    attempt_id: String,
    ordinal: u16,
    slot: u8,
    recipient: Recipient,
    request_digest: Digest,
    kinds: BTreeSet<DisclosureContentKind>,
    lineage_id: String,
    store_generation: u64,
    claim_generation: u64,
    reserved_at_ms: u64,
    expires_at_ms: u64,
}
impl GrantReservation {
    pub fn expires_at_epoch_ms(&self) -> u64 {
        self.expires_at_ms
    }
    pub fn attempt_ordinal(&self) -> u16 {
        self.ordinal
    }
    pub fn recipient(&self) -> &Recipient {
        &self.recipient
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seal {
    version: u16,
    run_id: String,
    input_digest: Digest,
    context_digest: Digest,
    policy_digest: Digest,
    accepted_at: String,
    binding_digest: Digest,
    capture_digest: Option<Digest>,
    grants: Vec<DisclosureGrant>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    seal: Seal,
    seal_digest: Digest,
    attempts: Vec<GrantReservation>,
    last_observed_ms: u64,
    revocations: Vec<(String, u64, u64)>,
}
fn millis(at: &str) -> Result<u64, StorageError> {
    let time = OffsetDateTime::parse(at, &Rfc3339).map_err(|_| StorageError::DispatchFenced)?;
    u64::try_from(time.unix_timestamp_nanos() / 1_000_000).map_err(|_| StorageError::DispatchFenced)
}
fn digest<T: Serialize>(value: &T) -> Result<Digest, StorageError> {
    Ok(Digest::from_bytes(&canonical_json(value)?))
}
fn recipients(state: &RunPersistenceState) -> Result<Vec<Recipient>, StorageError> {
    let mut values = BTreeMap::new();
    for role in &state.input.role_set.roles {
        let binding = role
            .catalog_binding
            .as_ref()
            .ok_or(StorageError::DispatchFenced)?;
        let recipient = Recipient {
            provider_id: binding.provider_id.clone(),
            account_profile_id: binding.provider_profile_id.clone(),
        };
        values.insert(
            (
                recipient.provider_id.clone(),
                recipient.account_profile_id.clone(),
            ),
            recipient,
        );
    }
    Ok(values.into_values().collect())
}
fn scope(
    storage: &Storage,
    connection: &Connection,
    state: &RunPersistenceState,
) -> Result<(BTreeSet<DisclosureContentKind>, Option<Digest>), StorageError> {
    let mut kinds = [
        DisclosureContentKind::Question,
        DisclosureContentKind::RoleProfile,
        DisclosureContentKind::PriorAssessment,
        DisclosureContentKind::Proposal,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    let capture =
        load_capture_manifest_from_db(connection, &state.input.context_manifest.manifest_id)?;
    if let Some(capture) = &capture {
        if capture.disclosure_state != DisclosureState::Approved {
            return Err(StorageError::DispatchFenced);
        }
        let expected = recipients(state)?;
        let mut actual = capture.content.recipients.clone();
        actual.sort_by(|a, b| {
            (&a.provider_id, &a.account_profile_id).cmp(&(&b.provider_id, &b.account_profile_id))
        });
        if actual != expected {
            return Err(StorageError::DispatchFenced);
        }
        for recipient in &expected {
            if capture.run_manifest_for_recipient(recipient)? != state.input.context_manifest {
                return Err(StorageError::DispatchFenced);
            }
        }
        for source in &capture.content.sources {
            if source.state != ManifestSourceState::Captured {
                continue;
            }
            let original_digest = source
                .object_digest
                .as_ref()
                .ok_or(StorageError::DispatchFenced)?;
            let derived_digest = source
                .derived_digest
                .as_ref()
                .ok_or(StorageError::DispatchFenced)?;
            match source.representation_kind {
                Some(RepresentationKind::Utf8Text) => {
                    kinds.insert(if original_digest == derived_digest {
                        DisclosureContentKind::SourceOriginal
                    } else {
                        DisclosureContentKind::SourceDerivedText
                    });
                }
                Some(
                    RepresentationKind::PdfText
                    | RepresentationKind::PdfRaster
                    | RepresentationKind::Image,
                ) => {
                    let bytes =
                        verify_object_on_disk(connection, &storage.objects_root, derived_digest)?;
                    let extraction: magi_context::NativeExtraction =
                        serde_json::from_slice(&bytes)?;
                    if Some(extraction.kind) != source.representation_kind {
                        return Err(StorageError::DispatchFenced);
                    }
                    if extraction.kind == RepresentationKind::Image {
                        let original = verify_object_on_disk(
                            connection,
                            &storage.objects_root,
                            original_digest,
                        )?;
                        let data = extraction
                            .image_base64
                            .as_ref()
                            .ok_or(StorageError::DispatchFenced)?;
                        kinds.insert(if canonical_base64(&original) == *data {
                            DisclosureContentKind::SourceOriginal
                        } else {
                            DisclosureContentKind::SourceDerivedImage
                        });
                    } else {
                        for page in extraction.pages {
                            if page.text.is_some() {
                                kinds.insert(DisclosureContentKind::SourceDerivedText);
                            }
                            if page.image_base64.is_some() {
                                kinds.insert(DisclosureContentKind::SourceDerivedImage);
                            }
                        }
                    }
                }
                None => return Err(StorageError::DispatchFenced),
            }
        }
    } else if !state.input.context_manifest.sources.is_empty() {
        return Err(StorageError::DispatchFenced);
    }
    Ok((kinds, capture.map(|value| value.digest)))
}
fn canonical_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        output.push(ALPHABET[((bits >> 18) & 63) as usize] as char);
        output.push(ALPHABET[((bits >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}
fn binding_digest(state: &RunPersistenceState) -> Result<Digest, StorageError> {
    digest(&state.input.role_set)
}

pub(super) fn persist_admission(
    storage: &Storage,
    connection: &Connection,
    state: &RunPersistenceState,
    accepted_at: &str,
) -> Result<(), StorageError> {
    let issued = millis(accepted_at)?;
    let expires = issued
        .checked_add(LIFETIME_MS)
        .ok_or(StorageError::DispatchFenced)?;
    let (kinds, capture_digest) = scope(storage, connection, state)?;
    let grants = recipients(state)?
        .into_iter()
        .map(|recipient| DisclosureGrant {
            grant_id: Uuid::new_v4().to_string(),
            run_id: state.run.run_id.clone(),
            manifest_digest: state.input.context_manifest.digest.clone(),
            recipient,
            content_kinds: kinds.clone(),
            issued_at_epoch_ms: issued,
            expires_at_epoch_ms: expires,
            max_app_turns: MAX_TURNS as u16,
            revoked_at_epoch_ms: None,
        })
        .collect();
    let seal = Seal {
        version: 1,
        run_id: state.run.run_id.clone(),
        input_digest: state.input.input_digest.clone(),
        context_digest: state.input.context_manifest.digest.clone(),
        policy_digest: state.input.policy_digest.clone(),
        accepted_at: accepted_at.into(),
        binding_digest: binding_digest(state)?,
        capture_digest,
        grants,
    };
    let envelope = Envelope {
        seal_digest: digest(&seal)?,
        seal,
        attempts: vec![],
        last_observed_ms: issued,
        revocations: vec![],
    };
    connection.execute(
        "INSERT INTO disclosure_grants(run_id,payload_json,revoked_at_epoch_ms) VALUES(?1,?2,NULL)",
        params![state.run.run_id, serde_json::to_string(&envelope)?],
    )?;
    Ok(())
}
fn load(
    storage: &Storage,
    connection: &Connection,
    state: &RunPersistenceState,
) -> Result<(Envelope, Option<u64>), StorageError> {
    let (payload, revoked): (String, Option<u64>) = connection
        .query_row(
            "SELECT payload_json,revoked_at_epoch_ms FROM disclosure_grants WHERE run_id=?1",
            [&state.run.run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or(StorageError::DispatchFenced)?;
    if payload.len() > 128 * 1024 {
        return Err(StorageError::DispatchFenced);
    }
    let value: Envelope = serde_json::from_str(&payload)?;
    let (kinds, capture) = scope(storage, connection, state)?;
    let issued = millis(&value.seal.accepted_at)?;
    let expected_recipients = recipients(state)?;
    if value.seal.version != 1
        || value.seal.run_id != state.run.run_id
        || value.seal.input_digest != state.input.input_digest
        || value.seal.context_digest != state.input.context_manifest.digest
        || value.seal.policy_digest != state.input.policy_digest
        || value.seal.binding_digest != binding_digest(state)?
        || value.seal.capture_digest != capture
        || value.seal_digest != digest(&value.seal)?
        || value.seal.grants.len() != expected_recipients.len()
        || value.attempts.len() > MAX_TURNS
        || value.revocations.len() > MAX_TURNS
        || value.last_observed_ms < issued
    {
        return Err(StorageError::DispatchFenced);
    }
    let mut ids = BTreeSet::new();
    for (grant, recipient) in value.seal.grants.iter().zip(expected_recipients) {
        if grant.run_id != state.run.run_id
            || grant.manifest_digest != state.input.context_manifest.digest
            || grant.recipient != recipient
            || grant.content_kinds != kinds
            || grant.issued_at_epoch_ms != issued
            || grant.expires_at_epoch_ms
                != issued
                    .checked_add(LIFETIME_MS)
                    .ok_or(StorageError::DispatchFenced)?
            || grant.max_app_turns != MAX_TURNS as u16
            || grant.revoked_at_epoch_ms.is_some()
            || grant.grant_id.is_empty()
            || !ids.insert(&grant.grant_id)
        {
            return Err(StorageError::DispatchFenced);
        }
    }
    let mut attempts = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    let mut previous = issued;
    for (index, attempt) in value.attempts.iter().enumerate() {
        if attempt.run_id != state.run.run_id
            || attempt.ordinal != index as u16 + 1
            || attempt.slot >= 10
            || attempt.attempt_id.is_empty()
            || !attempts.insert(&attempt.attempt_id)
            || attempt.token.is_empty()
            || !tokens.insert(&attempt.token)
            || attempt.reserved_at_ms < previous
            || attempt.reserved_at_ms > value.last_observed_ms
            || attempt.kinds.is_empty()
            || !attempt.kinds.is_subset(&kinds)
            || attempt.reserved_at_ms
                >= issued
                    .checked_add(LIFETIME_MS)
                    .ok_or(StorageError::DispatchFenced)?
            || !value.seal.grants.iter().any(|g| {
                g.recipient == attempt.recipient && g.expires_at_epoch_ms == attempt.expires_at_ms
            })
        {
            return Err(StorageError::DispatchFenced);
        }
        previous = attempt.reserved_at_ms;
    }
    Ok((value, revoked))
}
pub(super) fn validate_admission_replay(
    storage: &Storage,
    connection: &Connection,
    state: &RunPersistenceState,
) -> Result<(), StorageError> {
    let (value, _) = load(storage, connection, state)?;
    let accepted: String = connection.query_row(
        "SELECT accepted_at FROM live_run_receipts WHERE run_id=?1",
        [&state.run.run_id],
        |row| row.get(0),
    )?;
    if value.seal.accepted_at != accepted {
        return Err(StorageError::IdempotencyConflict);
    }
    Ok(())
}
fn current(
    connection: &Connection,
    expected: &crate::AdmissionExecutionAuthority,
    claim: &LiveRunClaim,
    slot: u8,
) -> Result<(RunPersistenceState, Recipient), StorageError> {
    validate_expected_execution_authority(connection, Some(expected))?;
    ensure_active_live_run_claim(&load_live_run_claim_state(connection, claim)?, claim)?;
    let core = match slot {
        0..=2 => CoreId::ALL[slot as usize],
        3..=5 => CoreId::ALL[(slot - 3) as usize],
        6 => CoreId::Melchior1,
        7..=9 => CoreId::ALL[(slot - 7) as usize],
        _ => return Err(StorageError::DispatchFenced),
    };
    let role = Storage::frozen_deliberation_role_from(connection, claim, core)?;
    let binding = role.catalog_binding.ok_or(StorageError::DispatchFenced)?;
    let active:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM live_run_dispatch_reservations WHERE run_id=?1 AND slot_ordinal=?2 AND state='active')",params![claim.run_id,slot],|row|row.get(0))?;
    if !active {
        return Err(StorageError::DispatchFenced);
    }
    Ok((
        load_persistence_state(connection, &claim.run_id)?,
        Recipient {
            provider_id: binding.provider_id,
            account_profile_id: binding.provider_profile_id,
        },
    ))
}
fn check(
    value: &Envelope,
    revoked: Option<u64>,
    recipient: &Recipient,
    kinds: &BTreeSet<DisclosureContentKind>,
    now: u64,
) -> Result<(), StorageError> {
    if revoked.is_some() || now < value.last_observed_ms || kinds.is_empty() {
        return Err(StorageError::DispatchFenced);
    }
    let grant = value
        .seal
        .grants
        .iter()
        .find(|g| &g.recipient == recipient)
        .ok_or(StorageError::DispatchFenced)?;
    if now >= grant.expires_at_epoch_ms || !kinds.is_subset(&grant.content_kinds) {
        return Err(StorageError::DispatchFenced);
    }
    Ok(())
}
fn save(connection: &Connection, value: &Envelope) -> Result<(), StorageError> {
    let changed=connection.execute("UPDATE disclosure_grants SET payload_json=?1 WHERE run_id=?2 AND revoked_at_epoch_ms IS NULL",params![serde_json::to_string(value)?,value.seal.run_id])?;
    if changed != 1 {
        return Err(StorageError::DispatchFenced);
    }
    Ok(())
}
pub(super) fn revoke_cancel_in_transaction(
    storage: &Storage,
    connection: &Connection,
    state: &RunPersistenceState,
    command_id: &str,
    at: &str,
) -> Result<(), StorageError> {
    let expected = read_admission_execution_authority(connection)?;
    if expected.store_generation != storage.identity.generation {
        return Err(StorageError::DispatchFenced);
    }
    validate_expected_execution_authority(connection, Some(&expected))?;
    let (mut value, revoked) = load(storage, connection, state)?;
    if revoked.is_some() {
        return Ok(());
    }
    let now = millis(at)?;
    if now < value.last_observed_ms {
        return Err(StorageError::DispatchFenced);
    }
    value.last_observed_ms = now;
    value
        .revocations
        .push((command_id.into(), now, state.run.revision));
    let changed = connection.execute("UPDATE disclosure_grants SET payload_json=?1,revoked_at_epoch_ms=?2 WHERE run_id=?3 AND revoked_at_epoch_ms IS NULL",params![serde_json::to_string(&value)?,to_sql_integer(now)?,state.run.run_id])?;
    if changed != 1 {
        return Err(StorageError::DispatchFenced);
    }
    Ok(())
}

impl Storage {
    pub fn reserve_disclosure_turn(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        request: crate::DisclosureTurnRequest<'_>,
        now: &str,
    ) -> Result<GrantReservation, StorageError> {
        let crate::DisclosureTurnRequest {
            slot_ordinal,
            attempt_id,
            request_digest,
            content_kinds,
        } = request;
        validate_text("attempt_id", attempt_id, 128)?;
        let now = millis(now)?;
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (state, recipient) = current(&tx, expected, claim, slot_ordinal)?;
        let (mut value, revoked) = load(self, &tx, &state)?;
        check(&value, revoked, &recipient, content_kinds, now)?;
        if let Some(previous) = value.attempts.iter().find(|a| a.attempt_id == attempt_id) {
            if previous.slot != slot_ordinal
                || previous.recipient != recipient
                || previous.request_digest != *request_digest
                || previous.kinds != *content_kinds
                || previous.lineage_id != expected.lineage_id
                || previous.store_generation != expected.store_generation
                || previous.claim_generation != claim.claim_generation
            {
                return Err(StorageError::IdempotencyConflict);
            }
            let result = previous.clone();
            value.last_observed_ms = now;
            save(&tx, &value)?;
            tx.commit()?;
            return Ok(result);
        }
        if value.attempts.len() >= MAX_TURNS {
            return Err(StorageError::DispatchFenced);
        }
        let expires_at_ms = value
            .seal
            .grants
            .iter()
            .find(|grant| grant.recipient == recipient)
            .ok_or(StorageError::DispatchFenced)?
            .expires_at_epoch_ms;
        let result = GrantReservation {
            run_id: claim.run_id.clone(),
            token: Uuid::new_v4().to_string(),
            attempt_id: attempt_id.into(),
            ordinal: value.attempts.len() as u16 + 1,
            slot: slot_ordinal,
            recipient,
            request_digest: request_digest.clone(),
            kinds: content_kinds.clone(),
            lineage_id: expected.lineage_id.clone(),
            store_generation: expected.store_generation,
            claim_generation: claim.claim_generation,
            reserved_at_ms: now,
            expires_at_ms,
        };
        value.attempts.push(result.clone());
        value.last_observed_ms = now;
        save(&tx, &value)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn authorize_reserved_disclosure(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        reservation: &GrantReservation,
        request_digest: &Digest,
        content_kinds: &BTreeSet<DisclosureContentKind>,
        now: &str,
    ) -> Result<(), StorageError> {
        self.authorize_disclosure_clock(
            expected,
            claim,
            reservation,
            request_digest,
            content_kinds,
            || millis(now),
        )
    }

    pub fn authorize_reserved_disclosure_now(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        reservation: &GrantReservation,
        request_digest: &Digest,
        content_kinds: &BTreeSet<DisclosureContentKind>,
    ) -> Result<(), StorageError> {
        self.authorize_disclosure_clock(
            expected,
            claim,
            reservation,
            request_digest,
            content_kinds,
            || {
                let elapsed = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| StorageError::DispatchFenced)?;
                u64::try_from(elapsed.as_millis()).map_err(|_| StorageError::DispatchFenced)
            },
        )
    }

    fn authorize_disclosure_clock(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        reservation: &GrantReservation,
        request_digest: &Digest,
        content_kinds: &BTreeSet<DisclosureContentKind>,
        clock: impl FnOnce() -> Result<u64, StorageError>,
    ) -> Result<(), StorageError> {
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = clock()?;
        let (state, recipient) = current(&tx, expected, claim, reservation.slot)?;
        let (mut value, revoked) = load(self, &tx, &state)?;
        check(&value, revoked, &recipient, content_kinds, now)?;
        if reservation.run_id != claim.run_id
            || reservation.recipient != recipient
            || reservation.request_digest != *request_digest
            || *content_kinds != reservation.kinds
            || reservation.lineage_id != expected.lineage_id
            || reservation.store_generation != expected.store_generation
            || reservation.claim_generation != claim.claim_generation
            || !value.attempts.contains(reservation)
        {
            return Err(StorageError::DispatchFenced);
        }
        value.last_observed_ms = now;
        save(&tx, &value)?;
        tx.commit()?;
        Ok(())
    }
    pub fn revoke_disclosure_with_authority(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        run_id: &str,
        expected_revision: u64,
        command_id: &str,
        now: &str,
    ) -> Result<(), StorageError> {
        validate_text("command_id", command_id, 128)?;
        let now = millis(now)?;
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_expected_execution_authority(&tx, Some(expected))?;
        let state = load_persistence_state(&tx, run_id)?;
        let (mut value, revoked) = load(self, &tx, &state)?;
        if let Some((_, at, revision)) =
            value.revocations.iter().find(|(id, _, _)| id == command_id)
        {
            if revoked != Some(*at) || *revision != expected_revision {
                return Err(StorageError::IdempotencyConflict);
            }
            tx.commit()?;
            return Ok(());
        }
        if state.run.revision != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual: state.run.revision,
            });
        }
        if now < value.last_observed_ms {
            return Err(StorageError::DispatchFenced);
        }
        if revoked.is_some() {
            tx.commit()?;
            return Ok(());
        }
        value.last_observed_ms = now;
        value
            .revocations
            .push((command_id.into(), now, expected_revision));
        tx.execute("UPDATE disclosure_grants SET payload_json=?1,revoked_at_epoch_ms=?2 WHERE run_id=?3 AND revoked_at_epoch_ms IS NULL",params![serde_json::to_string(&value)?,to_sql_integer(now)?,run_id])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Cleanup(Option<PathBuf>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                std::fs::remove_dir_all(path)
                    .expect("close SQLite handles before removing fixture");
            }
        }
    }
    fn kinds() -> BTreeSet<DisclosureContentKind> {
        [
            DisclosureContentKind::Question,
            DisclosureContentKind::RoleProfile,
        ]
        .into_iter()
        .collect()
    }
    #[test]
    fn question_only_admission_persists_three_exact_recipients_without_capture() {
        let mut cleanup = Cleanup(None);
        let (root, storage, aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        cleanup.0 = Some(root);
        let connection = storage.connection().unwrap();
        let state = aggregate.persistence_state();
        let (value, revoked) = load(&storage, &connection, &state).unwrap();
        assert_eq!(value.seal.grants.len(), 3);
        assert!(value.seal.capture_digest.is_none());
        assert!(revoked.is_none());
        assert_eq!(
            value
                .seal
                .grants
                .iter()
                .map(|g| g.recipient.clone())
                .collect::<Vec<_>>(),
            recipients(&state).unwrap()
        );
        validate_admission_replay(&storage, &connection, &state).unwrap();
        let before: String = connection
            .query_row(
                "SELECT payload_json FROM disclosure_grants WHERE run_id=?1",
                [&state.run.run_id],
                |r| r.get(0),
            )
            .unwrap();
        validate_admission_replay(&storage, &connection, &state).unwrap();
        let after: String = connection
            .query_row(
                "SELECT payload_json FROM disclosure_grants WHERE run_id=?1",
                [&state.run.run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
    }
    #[test]
    fn global_twenty_attempts_idempotency_and_ttl_boundary_are_atomic() {
        let mut cleanup = Cleanup(None);
        let (root, storage, mut aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        cleanup.0 = Some(root);
        let claim =
            super::super::saved_model_selection_tests::cancellation_claim(&storage, &mut aggregate);
        let authority = storage.admission_execution_authority().unwrap();
        let digest = Digest::from_bytes(b"actual serialized prompt");
        for n in 1..=20 {
            let reservation = storage
                .reserve_disclosure_turn(
                    &authority,
                    &claim,
                    crate::DisclosureTurnRequest {
                        slot_ordinal: 0,
                        attempt_id: &format!("attempt-{n}"),
                        request_digest: &digest,
                        content_kinds: &kinds(),
                    },
                    "2026-10-01T00:00:02Z",
                )
                .unwrap();
            assert_eq!(reservation.attempt_ordinal(), n);
        }
        assert!(
            storage
                .reserve_disclosure_turn(
                    &authority,
                    &claim,
                    crate::DisclosureTurnRequest {
                        slot_ordinal: 0,
                        attempt_id: "attempt-21",
                        request_digest: &digest,
                        content_kinds: &kinds()
                    },
                    "2026-10-01T00:00:02Z"
                )
                .is_err()
        );
        let prior = storage
            .reserve_disclosure_turn(
                &authority,
                &claim,
                crate::DisclosureTurnRequest {
                    slot_ordinal: 0,
                    attempt_id: "attempt-1",
                    request_digest: &digest,
                    content_kinds: &kinds(),
                },
                "2026-10-01T00:00:03Z",
            )
            .unwrap();
        assert!(
            storage
                .reserve_disclosure_turn(
                    &authority,
                    &claim,
                    crate::DisclosureTurnRequest {
                        slot_ordinal: 0,
                        attempt_id: "attempt-1",
                        request_digest: &Digest::from_bytes(b"changed"),
                        content_kinds: &kinds()
                    },
                    "2026-10-01T00:00:03Z"
                )
                .is_err()
        );
        storage
            .authorize_reserved_disclosure(
                &authority,
                &claim,
                &prior,
                &digest,
                &kinds(),
                "2026-10-01T01:59:59.999Z",
            )
            .unwrap();
        assert!(
            storage
                .authorize_reserved_disclosure(
                    &authority,
                    &claim,
                    &prior,
                    &digest,
                    &kinds(),
                    "2026-10-01T02:00:00Z"
                )
                .is_err()
        );
    }
    #[test]
    fn revoked_wrong_claim_generation_and_mutated_reservations_cannot_authorize() {
        let mut cleanup = Cleanup(None);
        let (root, storage, mut aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        cleanup.0 = Some(root);
        let claim =
            super::super::saved_model_selection_tests::cancellation_claim(&storage, &mut aggregate);
        let authority = storage.admission_execution_authority().unwrap();
        let digest = Digest::from_bytes(b"prompt");
        let reservation = storage
            .reserve_disclosure_turn(
                &authority,
                &claim,
                crate::DisclosureTurnRequest {
                    slot_ordinal: 0,
                    attempt_id: "attempt",
                    request_digest: &digest,
                    content_kinds: &kinds(),
                },
                "2026-10-01T00:00:02Z",
            )
            .unwrap();
        let mut altered = reservation.clone();
        altered.recipient.account_profile_id = "other".into();
        assert!(
            storage
                .authorize_reserved_disclosure(
                    &authority,
                    &claim,
                    &altered,
                    &digest,
                    &kinds(),
                    "2026-10-01T00:00:03Z"
                )
                .is_err()
        );
        let mut stale = claim.clone();
        stale.claim_generation += 1;
        assert!(
            storage
                .authorize_reserved_disclosure(
                    &authority,
                    &stale,
                    &reservation,
                    &digest,
                    &kinds(),
                    "2026-10-01T00:00:03Z"
                )
                .is_err()
        );
        let mut stale_authority = authority.clone();
        stale_authority.store_generation += 1;
        assert!(
            storage
                .authorize_reserved_disclosure(
                    &stale_authority,
                    &claim,
                    &reservation,
                    &digest,
                    &kinds(),
                    "2026-10-01T00:00:03Z"
                )
                .is_err()
        );
        let revision = aggregate.persistence_state().run.revision;
        storage
            .revoke_disclosure_with_authority(
                &authority,
                &claim.run_id,
                revision,
                "revoke",
                "2026-10-01T00:00:03Z",
            )
            .unwrap();
        storage
            .revoke_disclosure_with_authority(
                &authority,
                &claim.run_id,
                revision,
                "revoke",
                "2026-10-01T00:00:04Z",
            )
            .unwrap();
        assert!(
            storage
                .revoke_disclosure_with_authority(
                    &authority,
                    &claim.run_id,
                    revision + 1,
                    "revoke",
                    "2026-10-01T00:00:04Z"
                )
                .is_err()
        );
        assert!(
            storage
                .authorize_reserved_disclosure(
                    &authority,
                    &claim,
                    &reservation,
                    &digest,
                    &kinds(),
                    "2026-10-01T00:00:04Z"
                )
                .is_err()
        );
        assert!(
            storage
                .reserve_disclosure_turn(
                    &authority,
                    &claim,
                    crate::DisclosureTurnRequest {
                        slot_ordinal: 0,
                        attempt_id: "after-revoke",
                        request_digest: &digest,
                        content_kinds: &kinds()
                    },
                    "2026-10-01T00:00:04Z"
                )
                .is_err()
        );
    }
    #[test]
    fn captured_ranges_and_seal_mutation_are_bound_to_actual_frozen_manifest() {
        let mut cleanup = Cleanup(None);
        let (root, storage, aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture_with_source(true);
        cleanup.0 = Some(root);
        let connection = storage.connection().unwrap();
        let state = aggregate.persistence_state();
        let (mut value, _) = load(&storage, &connection, &state).unwrap();
        assert!(value.seal.capture_digest.is_some());
        assert!(value.seal.grants.iter().all(|g| {
            g.content_kinds
                .contains(&DisclosureContentKind::SourceDerivedText)
        }));
        assert!(!state.input.context_manifest.sources.is_empty());
        value.seal.policy_digest = Digest::from_bytes(b"mutated policy");
        value.seal_digest = digest(&value.seal).unwrap();
        connection
            .execute(
                "UPDATE disclosure_grants SET payload_json=?1 WHERE run_id=?2",
                params![serde_json::to_string(&value).unwrap(), state.run.run_id],
            )
            .unwrap();
        assert!(load(&storage, &connection, &state).is_err());
    }
    #[test]
    fn grant_bundle_is_rolled_back_with_admission_transaction() {
        let mut cleanup = Cleanup(None);
        let (root, storage, aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture_build(
                false, None, false,
            );
        cleanup.0 = Some(root);
        let state = aggregate.persistence_state();
        let mut connection = storage.connection().unwrap();
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        insert_conversation_if_missing(
            &tx,
            &state.run.conversation_id,
            "grant rollback",
            "2026-10-01T00:00:00Z",
        )
        .unwrap();
        persist_input_snapshots(&tx, &state.input, "2026-10-01T00:00:00Z").unwrap();
        insert_run(&tx, &state.run, &state.input).unwrap();
        persist_admission(&storage, &tx, &state, "2026-10-01T00:00:00Z").unwrap();
        let count: u32 = tx
            .query_row("SELECT count(*) FROM disclosure_grants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        tx.rollback().unwrap();
        let count: u32 = connection
            .query_row("SELECT count(*) FROM disclosure_grants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        let count: u32 = connection
            .query_row("SELECT count(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    #[test]
    fn source_permission_is_not_inferred_for_question_only_or_inactive_slot() {
        let mut cleanup = Cleanup(None);
        let (root, storage, mut aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        cleanup.0 = Some(root);
        let claim =
            super::super::saved_model_selection_tests::cancellation_claim(&storage, &mut aggregate);
        let authority = storage.admission_execution_authority().unwrap();
        let digest = Digest::from_bytes(b"prompt");
        for kind in [
            DisclosureContentKind::SourceOriginal,
            DisclosureContentKind::SourceDerivedText,
            DisclosureContentKind::SourceDerivedImage,
        ] {
            assert!(
                storage
                    .reserve_disclosure_turn(
                        &authority,
                        &claim,
                        crate::DisclosureTurnRequest {
                            slot_ordinal: 0,
                            attempt_id: "source",
                            request_digest: &digest,
                            content_kinds: &[kind].into_iter().collect()
                        },
                        "2026-10-01T00:00:02Z"
                    )
                    .is_err()
            );
        }
        assert!(
            storage
                .reserve_disclosure_turn(
                    &authority,
                    &claim,
                    crate::DisclosureTurnRequest {
                        slot_ordinal: 1,
                        attempt_id: "inactive",
                        request_digest: &digest,
                        content_kinds: &kinds()
                    },
                    "2026-10-01T00:00:02Z"
                )
                .is_err()
        );
        let connection = storage.connection().unwrap();
        let (value, _) = load(&storage, &connection, &aggregate.persistence_state()).unwrap();
        assert!(value.attempts.is_empty());
    }
    #[test]
    fn stored_pdf_and_image_representations_grant_only_actual_wire_content_kinds() {
        for (representation, original, data, expected_kind) in [
            (
                RepresentationKind::PdfText,
                b"%PDF fixture".as_slice(),
                None,
                DisclosureContentKind::SourceDerivedText,
            ),
            (
                RepresentationKind::PdfRaster,
                b"%PDF fixture".as_slice(),
                Some("YWJj"),
                DisclosureContentKind::SourceDerivedImage,
            ),
            (
                RepresentationKind::Image,
                b"abc".as_slice(),
                Some("YWJj"),
                DisclosureContentKind::SourceOriginal,
            ),
            (
                RepresentationKind::Image,
                b"original".as_slice(),
                Some("YWJj"),
                DisclosureContentKind::SourceDerivedImage,
            ),
        ] {
            let mut cleanup = Cleanup(None);
            let (root, storage, base) =
                super::super::saved_model_selection_tests::cancellation_fixture_build(
                    false, None, false,
                );
            cleanup.0 = Some(root);
            let original = storage.put_source_object(original).unwrap();
            let is_image = representation == RepresentationKind::Image;
            let is_text = representation == RepresentationKind::PdfText;
            let extraction = serde_json::json!({
                "schema_version":1,"kind":representation,"mime_type":if is_image {"image/png"} else {"application/pdf"},
                "width":if is_image {Some(1)} else {None},"height":if is_image {Some(1)} else {None},
                "image_base64":if is_image {data} else {None},
                "pages":if is_image {vec![]} else {vec![serde_json::json!({"page":1,"text":if is_text {Some("selected text")} else {None},"mime_type":if is_text {None} else {Some("image/png")},"image_base64":if is_text {None} else {data},"width":if is_text {None} else {Some(1)},"height":if is_text {None} else {Some(1)}})]},
                "warnings":[],"total_pages":if is_image {None} else {Some(1)}
            });
            let derived = storage
                .put_source_object(&serde_json::to_vec(&extraction).unwrap())
                .unwrap();
            let source = magi_context::ManifestSource {
                source_id: "representation".into(),
                display_name: "selected source".into(),
                state: ManifestSourceState::Captured,
                byte_length: Some(original.byte_length),
                mime_type: Some(
                    if is_image {
                        "image/png"
                    } else {
                        "application/pdf"
                    }
                    .into(),
                ),
                object_digest: Some(original.digest.clone()),
                derived_digest: Some(derived.digest),
                representation_kind: Some(representation),
                extractor_id: Some("native-fixture".into()),
                extractor_version: Some("1".into()),
                included_locators: vec![magi_context::EvidenceLocator {
                    source_id: "representation".into(),
                    object_digest: original.digest,
                    start_line: None,
                    end_line: None,
                    total_lines: None,
                    page: if is_image { None } else { Some(1) },
                    width: if is_text { None } else { Some(1) },
                    height: if is_text { None } else { Some(1) },
                }],
                omission: None,
                captured_at_epoch_ms: Some(1),
                secret_pattern_findings: vec![],
                secret_scan_incomplete: false,
            };
            let draft = SourceCaptureManifest::draft(vec![source], 1).unwrap();
            let capture = draft
                .confirm_disclosure(
                    recipients(&base.persistence_state()).unwrap(),
                    &draft.digest,
                    2,
                )
                .unwrap();
            storage.commit_context_manifest(&capture).unwrap();
            let input = InputSnapshot::new_with_request_provenance(
                base.input().question.clone(),
                capture
                    .run_manifest_for_recipient(&recipients(&base.persistence_state()).unwrap()[0])
                    .unwrap(),
                base.input().role_set.clone(),
                base.input().policy_digest.clone(),
                base.input().request_provenance.clone().unwrap(),
            )
            .unwrap();
            let run = magi_domain::Run::new(
                "run-core".into(),
                "conversation-core".into(),
                None,
                &input,
                "2026-10-01T00:00:00Z".into(),
            )
            .unwrap();
            let mut aggregate = RunAggregate::new(run, input).unwrap();
            let references = aggregate
                .input()
                .role_set
                .frozen_core_selections
                .as_ref()
                .unwrap()
                .iter()
                .map(|item| crate::CoreBindingReference {
                    core_id: item.core_id,
                    provider_profile_id: item.provider_profile_id.clone(),
                    profile_revision: item.profile_revision,
                    model_selection_revision: item.model_selection_revision,
                    core_selection_revision: item.core_selection_revision,
                })
                .collect::<Vec<_>>();
            let command = CommandEnvelope {
                command_id: "representation-admission".into(),
                idempotency_key: "representation-admission".into(),
                command_kind: CommandKind::CreateRun,
                target_id: "run-core".into(),
                expected_revision: 0,
                payload_digest: aggregate.input().input_digest.clone(),
            };
            let request = LiveRunAdmissionRequest {
                command_id: command.command_id.clone(),
                idempotency_key: command.idempotency_key.clone(),
                question: aggregate.input().question.prompt.clone(),
                model_binding: aggregate.input().role_set.roles[0]
                    .catalog_binding
                    .clone()
                    .unwrap(),
            };
            storage
                .admit_deliberation_run(
                    &command,
                    &mut aggregate,
                    &request,
                    &references,
                    "2026-10-01T00:00:00Z",
                    None,
                )
                .unwrap();
            let claim = super::super::saved_model_selection_tests::cancellation_claim(
                &storage,
                &mut aggregate,
            );
            let authority = storage.admission_execution_authority().unwrap();
            let digest = Digest::from_bytes(b"serialized selected representation");
            for candidate in [
                DisclosureContentKind::SourceOriginal,
                DisclosureContentKind::SourceDerivedText,
                DisclosureContentKind::SourceDerivedImage,
            ] {
                let result = storage.reserve_disclosure_turn(
                    &authority,
                    &claim,
                    crate::DisclosureTurnRequest {
                        slot_ordinal: 0,
                        attempt_id: &format!("representation-{candidate:?}"),
                        request_digest: &digest,
                        content_kinds: &[candidate].into_iter().collect(),
                    },
                    "2026-10-01T00:00:02Z",
                );
                assert_eq!(result.is_ok(), candidate == expected_kind);
            }
        }
    }
    #[test]
    fn reservation_expiry_is_admission_expiry_and_authorization_samples_after_sql_lock() {
        let mut cleanup = Cleanup(None);
        let (root, storage, mut aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        cleanup.0 = Some(root);
        let claim =
            super::super::saved_model_selection_tests::cancellation_claim(&storage, &mut aggregate);
        let authority = storage.admission_execution_authority().unwrap();
        let digest = Digest::from_bytes(b"late reserved prompt");
        let reservation = storage
            .reserve_disclosure_turn(
                &authority,
                &claim,
                crate::DisclosureTurnRequest {
                    slot_ordinal: 0,
                    attempt_id: "late",
                    request_digest: &digest,
                    content_kinds: &kinds(),
                },
                "2026-10-01T01:59:59Z",
            )
            .unwrap();
        assert_eq!(
            reservation.expires_at_epoch_ms(),
            millis("2026-10-01T02:00:00Z").unwrap()
        );
        let mut sampled = false;
        let result = storage.authorize_disclosure_clock(
            &authority,
            &claim,
            &reservation,
            &digest,
            &kinds(),
            || {
                assert!(storage.connection.try_lock().is_err());
                let mut competing =
                    Connection::open(cleanup.0.as_ref().unwrap().join("state/magi.sqlite"))
                        .unwrap();
                competing.busy_timeout(Duration::ZERO).unwrap();
                assert!(
                    competing
                        .transaction_with_behavior(TransactionBehavior::Immediate)
                        .is_err()
                );
                sampled = true;
                Ok(reservation.expires_at_epoch_ms())
            },
        );
        assert!(sampled && result.is_err());
        assert!(
            storage
                .authorize_reserved_disclosure_now(
                    &authority,
                    &claim,
                    &reservation,
                    &digest,
                    &kinds()
                )
                .is_err()
        );
        let connection = storage.connection().unwrap();
        let (value, _) = load(&storage, &connection, &aggregate.persistence_state()).unwrap();
        assert_eq!(
            value.last_observed_ms,
            millis("2026-10-01T01:59:59Z").unwrap()
        );
    }

    #[test]
    fn cancel_and_grant_revoke_commit_or_rollback_together_with_revision_and_lineage_fences() {
        let mut cleanup = Cleanup(None);
        let (root, storage, mut aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        cleanup.0 = Some(root);
        let claim =
            super::super::saved_model_selection_tests::cancellation_claim(&storage, &mut aggregate);
        let live = storage.get_live_run_snapshot(&claim.run_id, 0).unwrap();
        assert!(
            storage
                .begin_live_run_cancel(
                    "wrong-revision",
                    "wrong-revision",
                    &claim.run_id,
                    live.revision + 1,
                    "2026-10-01T00:00:02Z"
                )
                .is_err()
        );
        {
            let connection = storage.connection().unwrap();
            assert!(
                load(&storage, &connection, &aggregate.persistence_state())
                    .unwrap()
                    .1
                    .is_none()
            );
            connection
                .execute(
                    "UPDATE store_meta SET value='inert' WHERE key='admission_activation_state'",
                    [],
                )
                .unwrap();
        }
        assert!(
            storage
                .begin_deliberation_cancel(&claim.run_id, "2026-10-01T00:00:02Z")
                .is_err()
        );
        {
            let connection = storage.connection().unwrap();
            assert!(
                load(&storage, &connection, &aggregate.persistence_state())
                    .unwrap()
                    .1
                    .is_none()
            );
            connection
                .execute(
                    "UPDATE store_meta SET value='active' WHERE key='admission_activation_state'",
                    [],
                )
                .unwrap();
            connection.execute_batch("CREATE TRIGGER deny_cancel_write BEFORE UPDATE ON live_run_outbox BEGIN SELECT RAISE(ABORT,'injected transaction failure'); END;").unwrap();
        }
        assert!(
            storage
                .begin_deliberation_cancel(&claim.run_id, "2026-10-01T00:00:02Z")
                .is_err()
        );
        {
            let connection = storage.connection().unwrap();
            assert!(
                load(&storage, &connection, &aggregate.persistence_state())
                    .unwrap()
                    .1
                    .is_none()
            );
            let count: u64 = connection
                .query_row(
                    "SELECT count(*) FROM live_run_cancellation_receipts",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0);
            connection
                .execute_batch("DROP TRIGGER deny_cancel_write;")
                .unwrap();
        }
        let first = storage
            .begin_deliberation_cancel(&claim.run_id, "2026-10-01T00:00:02Z")
            .unwrap();
        let second = storage
            .begin_deliberation_cancel(&claim.run_id, "2026-10-01T00:00:03Z")
            .unwrap();
        assert!(!first.5 && second.5);
        let connection = storage.connection().unwrap();
        let state = load_persistence_state(&connection, &claim.run_id).unwrap();
        let (value, revoked) = load(&storage, &connection, &state).unwrap();
        assert_eq!(revoked, Some(millis("2026-10-01T00:00:02Z").unwrap()));
        assert_eq!(value.revocations.len(), 1);
        let count: u64 = connection
            .query_row(
                "SELECT count(*) FROM live_run_cancellation_receipts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
