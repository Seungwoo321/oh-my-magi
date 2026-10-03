use super::*;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const RUN_ACTIVE_LIMIT_MILLIS: u64 = 45 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveClockBudget {
    pub run_id: String,
    pub observed_at: String,
    pub active_millis: u64,
    pub remaining_millis: u64,
    pub limit_millis: u64,
    pub live_event_high_water: u64,
}

fn timestamp_millis(value: &str) -> Result<u64, StorageError> {
    let time = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| StorageError::Corrupt("active clock timestamp is not RFC3339".into()))?;
    u64::try_from(time.unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| StorageError::Corrupt("active clock timestamp is unsupported".into()))
}

fn running_intervals(
    events: &[(u64, Option<String>, String)],
    now: u64,
) -> Result<Vec<(u64, u64)>, StorageError> {
    let mut previous_time = 0;
    let mut previous_sequence = None;
    let mut start = None;
    let mut intervals = Vec::new();
    for (sequence, status, at) in events {
        let at = timestamp_millis(at)?;
        if previous_sequence.is_some_and(|previous| *sequence <= previous)
            || at < previous_time
            || at > now
        {
            return Err(StorageError::Corrupt(
                "active clock evidence is out of order or in the future".into(),
            ));
        }
        previous_sequence = Some(*sequence);
        previous_time = at;
        let Some(status) = status else {
            continue;
        };
        let status = parse_live_run_status(status)?;
        if status == LiveRunStatus::Unknown {
            return Err(StorageError::Corrupt(
                "active clock contains unresolved unknown execution".into(),
            ));
        }
        if status == LiveRunStatus::Running {
            if start.is_none() {
                start = Some(at);
            }
        } else if let Some(from) = start.take() {
            intervals.push((from, at));
        }
    }
    if let Some(from) = start {
        intervals.push((from, now));
    }
    Ok(intervals)
}

#[cfg(test)]
fn measured_active_millis(
    events: &[(u64, Option<String>, String)],
    now: u64,
) -> Result<u64, StorageError> {
    union_millis(running_intervals(events, now)?)
}

fn union_millis(mut intervals: Vec<(u64, u64)>) -> Result<u64, StorageError> {
    intervals.sort_unstable();
    let mut total = 0u64;
    let mut current: Option<(u64, u64)> = None;
    for (start, end) in intervals {
        if end < start {
            return Err(StorageError::Corrupt(
                "active clock interval is reversed".into(),
            ));
        }
        match current {
            Some((from, to)) if start <= to => current = Some((from, to.max(end))),
            Some((from, to)) => {
                total = total
                    .checked_add(to - from)
                    .ok_or_else(|| StorageError::Corrupt("active clock overflow".into()))?;
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    if let Some((from, to)) = current {
        total = total
            .checked_add(to - from)
            .ok_or_else(|| StorageError::Corrupt("active clock overflow".into()))?;
    }
    Ok(total)
}

struct AuthenticationPhaseEvent {
    sequence: u64,
    store_generation: u64,
    claim_generation: u64,
    token: String,
    kind: String,
    at: String,
}
fn load_authentication_phase_events(
    connection: &Connection,
    run_id: &str,
) -> Result<Vec<AuthenticationPhaseEvent>, StorageError> {
    let mut statement=connection.prepare("SELECT sequence,store_generation,claim_generation,phase_token,kind,created_at FROM execution_clock_phase_events WHERE run_id=?1 ORDER BY sequence")?;
    let rows = statement.query_map([run_id], |row| {
        Ok(AuthenticationPhaseEvent {
            sequence: row.get(0)?,
            store_generation: row.get(1)?,
            claim_generation: row.get(2)?,
            token: row.get(3)?,
            kind: row.get(4)?,
            at: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(StorageError::from)
}
fn authentication_intervals(
    events: &[AuthenticationPhaseEvent],
    now: u64,
    allow_open: bool,
) -> Result<Vec<(u64, u64)>, StorageError> {
    let mut intervals = Vec::new();
    let mut open: Option<&AuthenticationPhaseEvent> = None;
    let mut previous_time = 0;
    let mut previous_sequence = 0;
    for event in events {
        let at = timestamp_millis(&event.at)?;
        if event.sequence <= previous_sequence || at < previous_time || at > now {
            return Err(StorageError::Corrupt(
                "authentication clock evidence is out of order".into(),
            ));
        }
        previous_time = at;
        previous_sequence = event.sequence;
        match event.kind.as_str() {
            "authentication_wait_started" if open.is_none() => open = Some(event),
            "authenticated" | "authentication_failed" => {
                let start = open.take().ok_or_else(|| {
                    StorageError::Corrupt("authentication clock end has no start".into())
                })?;
                if start.token != event.token
                    || start.store_generation != event.store_generation
                    || start.claim_generation != event.claim_generation
                {
                    return Err(StorageError::Corrupt(
                        "authentication clock lineage mismatch".into(),
                    ));
                }
                intervals.push((timestamp_millis(&start.at)?, at));
            }
            _ => {
                return Err(StorageError::Corrupt(
                    "authentication clock phase is invalid".into(),
                ));
            }
        }
    }
    if open.is_some() && !allow_open {
        return Err(StorageError::Corrupt(
            "authentication clock phase is unresolved".into(),
        ));
    }
    Ok(intervals)
}
fn active_millis_excluding_authentication(
    running: Vec<(u64, u64)>,
    authentication: Vec<(u64, u64)>,
) -> Result<u64, StorageError> {
    let mut excluded = Vec::new();
    for &(from, to) in &running {
        for &(auth_from, auth_to) in &authentication {
            let start = from.max(auth_from);
            let end = to.min(auth_to);
            if end > start {
                excluded.push((start, end));
            }
        }
    }
    union_millis(running)?
        .checked_sub(union_millis(excluded)?)
        .ok_or_else(|| {
            StorageError::Corrupt("authentication clock exclusion exceeds active time".into())
        })
}

fn validate_phase_timestamp(
    connection: &Connection,
    run_id: &str,
    now: u64,
) -> Result<(), StorageError> {
    let latest: String = connection.query_row(
        "SELECT created_at FROM live_run_events WHERE run_id=?1 ORDER BY sequence DESC LIMIT 1",
        [run_id],
        |row| row.get(0),
    )?;
    if timestamp_millis(&latest)? > now {
        return Err(StorageError::Corrupt(
            "authentication phase precedes durable execution evidence".into(),
        ));
    }
    Ok(())
}

impl Storage {
    pub fn begin_live_run_authentication_wait(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        at: &str,
    ) -> Result<String, StorageError> {
        let now = timestamp_millis(at)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        ensure_active_live_run_claim(&load_live_run_claim_state(&transaction, claim)?, claim)?;
        validate_phase_timestamp(&transaction, &claim.run_id, now)?;
        let phases = load_authentication_phase_events(&transaction, &claim.run_id)?;
        authentication_intervals(&phases, now, false)?;
        let token = Uuid::new_v4().simple().to_string();
        transaction.execute("INSERT INTO execution_clock_phase_events(run_id,store_generation,claim_generation,phase_token,kind,created_at) VALUES(?1,?2,?3,?4,'authentication_wait_started',?5)",params![claim.run_id,expected.store_generation,claim.claim_generation,token,at])?;
        transaction.commit()?;
        Ok(token)
    }
    pub fn finish_live_run_authentication_wait(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        token: &str,
        at: &str,
    ) -> Result<(), StorageError> {
        self.finish_authentication_phase(expected, claim, token, at, "authenticated")
    }
    /// The native caller must confirm local owned-process cleanup before recording failure.
    /// This outcome does not attest successful authentication or remote completion.
    pub fn finish_failed_authentication_after_local_stop(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        token: &str,
        at: &str,
    ) -> Result<(), StorageError> {
        self.finish_authentication_phase(expected, claim, token, at, "authentication_failed")
    }
    fn finish_authentication_phase(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        token: &str,
        at: &str,
        kind: &str,
    ) -> Result<(), StorageError> {
        let now = timestamp_millis(at)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        ensure_active_live_run_claim(&load_live_run_claim_state(&transaction, claim)?, claim)?;
        validate_phase_timestamp(&transaction, &claim.run_id, now)?;
        let phases = load_authentication_phase_events(&transaction, &claim.run_id)?;
        authentication_intervals(&phases, now, true)?;
        let last = phases.last().ok_or(StorageError::DispatchFenced)?;
        if last.kind != "authentication_wait_started"
            || last.token != token
            || last.store_generation != expected.store_generation
            || last.claim_generation != claim.claim_generation
        {
            return Err(StorageError::DispatchFenced);
        }
        transaction.execute("INSERT INTO execution_clock_phase_events(run_id,store_generation,claim_generation,phase_token,kind,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![claim.run_id,expected.store_generation,claim.claim_generation,token,kind,at])?;
        transaction.commit()?;
        Ok(())
    }
    /// Counts durable Running time, excluding only paired authentication waits.
    /// An unresolved phase cannot authorize a new provider turn.
    pub fn live_run_active_clock(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        now: &str,
    ) -> Result<ActiveClockBudget, StorageError> {
        let now_millis = timestamp_millis(now)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        let current = load_live_run_claim_state(&transaction, claim)?;
        ensure_active_live_run_claim(&current, claim)?;
        let events = {
            let mut statement=transaction.prepare("SELECT sequence,status,created_at FROM live_run_events WHERE run_id=?1 ORDER BY sequence")?;
            let rows = statement.query_map([&claim.run_id], |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if events.is_empty() {
            return Err(StorageError::Corrupt(
                "active clock durable evidence is missing".into(),
            ));
        }
        let phases = load_authentication_phase_events(&transaction, &claim.run_id)?;
        let active = active_millis_excluding_authentication(
            running_intervals(&events, now_millis)?,
            authentication_intervals(&phases, now_millis, false)?,
        )?;
        let result = ActiveClockBudget {
            run_id: claim.run_id.clone(),
            observed_at: now.into(),
            active_millis: active,
            remaining_millis: RUN_ACTIVE_LIMIT_MILLIS.saturating_sub(active),
            limit_millis: RUN_ACTIVE_LIMIT_MILLIS,
            live_event_high_water: events.last().expect("nonempty checked").0,
        };
        transaction.commit()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct OwnedTestPaths(std::cell::RefCell<Vec<PathBuf>>);
    impl OwnedTestPaths {
        fn register(&self, path: PathBuf) {
            self.0.borrow_mut().push(path);
        }
    }
    impl Drop for OwnedTestPaths {
        fn drop(&mut self) {
            for path in self.0.get_mut() {
                if path.exists()
                    && let Err(error) = std::fs::remove_dir_all(&*path)
                {
                    if std::thread::panicking() {
                        eprintln!("Owned authentication clock fixture cleanup failed: {error}");
                    } else {
                        panic!("Owned authentication clock fixture cleanup failed: {error}");
                    }
                }
            }
        }
    }

    #[test]
    fn schema_sixteen_migration_preserves_frozen_input_and_does_not_reactivate_prior_claim() {
        let (root, storage, mut aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        let _cleanup = OwnedTestPaths(std::cell::RefCell::new(vec![root.clone()]));
        let claim =
            super::super::saved_model_selection_tests::cancellation_claim(&storage, &mut aggregate);
        let digest = storage
            .load_run_dossier(&claim.run_id)
            .unwrap()
            .snapshot
            .run
            .input_digest;
        {
            let connection = storage.connection().unwrap();
            connection.execute_batch("DROP TABLE execution_clock_phase_events; ALTER TABLE admission_request_bindings DROP COLUMN common_context_budget_json; DELETE FROM schema_migrations WHERE version>16; PRAGMA user_version=16; UPDATE store_meta SET value='16' WHERE key='schema_version';").unwrap();
            super::super::verify_migration_ledger(&connection, 16).unwrap();
        }
        drop(storage);
        let migrated = Storage::open_or_create(&root).unwrap();
        let schema: u32 = migrated
            .connection()
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schema, super::super::SCHEMA_VERSION);
        assert_eq!(
            migrated
                .load_run_dossier(&claim.run_id)
                .unwrap()
                .snapshot
                .run
                .input_digest,
            digest
        );
        assert!(
            load_authentication_phase_events(&migrated.connection().unwrap(), &claim.run_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            migrated
                .live_run_active_clock(
                    &migrated.admission_execution_authority().unwrap(),
                    &claim,
                    "2026-10-01T00:10:01Z"
                )
                .is_err()
        );
        drop(migrated);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn schema_seventeen_pending_intent_migrates_without_policy_backfill_or_digest_change() {
        let prepared = super::super::saved_model_selection_tests::cancellation_fixture_build(
            false, None, false,
        );
        let _cleanup = OwnedTestPaths(std::cell::RefCell::new(vec![prepared.0.clone()]));
        let (root, storage, aggregate) = prepared;
        let intent = super::super::admission_intent_from_input(
            "legacy-pending",
            "legacy-pending-key",
            aggregate.input(),
        )
        .unwrap();
        assert!(intent.common_context_budget.is_none());
        let binding = storage
            .register_admission_request(&intent, "2026-10-01T00:00:00Z")
            .unwrap();
        {
            let connection = storage.connection().unwrap();
            connection.execute_batch("ALTER TABLE admission_request_bindings DROP COLUMN common_context_budget_json; DELETE FROM schema_migrations WHERE version>17; PRAGMA user_version=17; UPDATE store_meta SET value='17' WHERE key='schema_version';").unwrap();
            super::super::verify_migration_ledger(&connection, 17).unwrap();
        }
        drop(storage);
        let storage = Storage::open_or_create(&root).unwrap();
        assert_eq!(
            storage.identity().schema_version,
            super::super::SCHEMA_VERSION
        );
        assert_eq!(
            storage
                .load_admission_request_budget(&intent.command_id, &intent.idempotency_key)
                .unwrap(),
            None
        );
        assert_eq!(
            storage
                .register_admission_request(&intent, "2026-10-01T00:01:00Z")
                .unwrap(),
            binding
        );
        let receipt = storage
            .cancel_admission_request(
                &intent,
                "legacy-cancel",
                "legacy-cancel-key",
                "2026-10-01T00:02:00Z",
            )
            .unwrap();
        drop(storage);
        let storage = Storage::open_or_create(&root).unwrap();
        assert_eq!(
            storage
                .load_admission_request_cancellation(&intent)
                .unwrap(),
            Some(receipt)
        );
        drop(storage);
    }

    #[test]
    fn confirmed_authentication_failure_survives_backup_restore_without_authority_and_full_delete_cleans_phase_rows()
     {
        let (root, storage, mut aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        let _cleanup = OwnedTestPaths(std::cell::RefCell::new(vec![root.clone()]));
        let claim =
            super::super::saved_model_selection_tests::cancellation_claim(&storage, &mut aggregate);
        let authority = storage.admission_execution_authority().unwrap();
        let token = storage
            .begin_live_run_authentication_wait(&authority, &claim, "2026-10-01T00:05:01Z")
            .unwrap();
        storage
            .finish_failed_authentication_after_local_stop(
                &authority,
                &claim,
                &token,
                "2026-10-01T00:25:01Z",
            )
            .unwrap();
        assert_eq!(
            storage
                .live_run_active_clock(&authority, &claim, "2026-10-01T00:30:01Z")
                .unwrap()
                .active_millis,
            600_000
        );
        let container = root.parent().unwrap();
        let backup = container.join(format!("clock-backup-{}", Uuid::new_v4()));
        let restored = container.join(format!("clock-restored-{}", Uuid::new_v4()));
        _cleanup.register(backup.clone());
        _cleanup.register(restored.clone());
        let backup_manifest = storage.create_backup(&backup).unwrap();
        assert_eq!(backup_manifest.schema_version, super::super::SCHEMA_VERSION);
        let receipt = Storage::restore_backup(&backup, &restored).unwrap();
        assert!(receipt.requires_reauthorization);
        let copy = Storage::open_or_create(&restored).unwrap();
        assert_eq!(
            load_authentication_phase_events(&copy.connection().unwrap(), &claim.run_id)
                .unwrap()
                .len(),
            2
        );
        assert!(!copy.admission_execution_authority().unwrap().active);
        assert!(
            copy.live_run_active_clock(&authority, &claim, "2026-10-01T00:30:01Z")
                .is_err()
        );
        drop(copy);
        let cancellation = storage
            .begin_deliberation_cancel(&claim.run_id, "2026-10-01T00:31:01Z")
            .unwrap();
        storage
            .finish_live_run_cancel(
                &claim.run_id,
                cancellation.3,
                LiveRunProviderOutcome::Confirmed,
                "2026-10-01T00:31:02Z",
            )
            .unwrap();
        let revision = storage
            .load_run_dossier(&claim.run_id)
            .unwrap()
            .snapshot
            .run
            .revision;
        assert!(
            storage
                .delete_run(&claim.run_id, revision, "2026-10-01T00:32:01Z")
                .unwrap()
                .deleted
        );
        assert!(
            load_authentication_phase_events(&storage.connection().unwrap(), &claim.run_id)
                .unwrap()
                .is_empty()
        );
        drop(storage);
        for path in [root, backup, restored] {
            std::fs::remove_dir_all(path).unwrap();
        }
    }
    #[test]
    fn later_slot_authentication_wait_is_durable_excluded_and_cannot_reset_prior_active_time() {
        let (root, storage, mut aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        let _cleanup = OwnedTestPaths(std::cell::RefCell::new(vec![root.clone()]));
        let claim =
            super::super::saved_model_selection_tests::cancellation_claim(&storage, &mut aggregate);
        let authority = storage.admission_execution_authority().unwrap();
        let token = storage
            .begin_live_run_authentication_wait(&authority, &claim, "2026-10-01T00:05:01Z")
            .unwrap();
        assert!(
            storage
                .begin_live_run_authentication_wait(&authority, &claim, "2026-10-01T00:06:01Z")
                .is_err()
        );
        assert!(
            storage
                .live_run_active_clock(&authority, &claim, "2026-10-01T00:20:01Z")
                .is_err()
        );
        assert!(
            storage
                .finish_live_run_authentication_wait(
                    &authority,
                    &claim,
                    "wrong-token",
                    "2026-10-01T00:25:01Z"
                )
                .is_err()
        );
        storage
            .finish_live_run_authentication_wait(&authority, &claim, &token, "2026-10-01T00:25:01Z")
            .unwrap();
        assert!(
            storage
                .finish_live_run_authentication_wait(
                    &authority,
                    &claim,
                    &token,
                    "2026-10-01T00:25:01Z"
                )
                .is_err()
        );
        let clock = storage
            .live_run_active_clock(&authority, &claim, "2026-10-01T00:30:01Z")
            .unwrap();
        assert_eq!(clock.active_millis, 600_000);
        assert_eq!(clock.remaining_millis, 35 * 60 * 1000);
        assert!(
            storage
                .begin_live_run_authentication_wait(&authority, &claim, "2026-10-01T00:24:01Z")
                .is_err()
        );
        assert_eq!(
            active_millis_excluding_authentication(vec![(100, 200)], vec![(0, 150)]).unwrap(),
            50
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn parallel_slots_share_one_durable_running_interval_and_restart_does_not_reset_it() {
        let (root, storage, mut aggregate) =
            super::super::saved_model_selection_tests::cancellation_fixture();
        let _cleanup = OwnedTestPaths(std::cell::RefCell::new(vec![root.clone()]));
        let claim =
            super::super::saved_model_selection_tests::cancellation_claim(&storage, &mut aggregate);
        storage
            .activate_live_run_dispatch_slot(&claim, 1, "2026-10-01T00:00:01Z")
            .unwrap();
        storage
            .activate_live_run_dispatch_slot(&claim, 2, "2026-10-01T00:00:01Z")
            .unwrap();
        let authority = storage.admission_execution_authority().unwrap();
        let clock = storage
            .live_run_active_clock(&authority, &claim, "2026-10-01T00:10:01Z")
            .unwrap();
        assert_eq!(clock.active_millis, 600_000);
        assert_eq!(clock.remaining_millis, 35 * 60 * 1000);
        assert!(
            storage
                .live_run_active_clock(&authority, &claim, "2026-09-30T23:59:59Z")
                .is_err()
        );
        assert!(
            storage
                .live_run_active_clock(&authority, &claim, "fixture")
                .is_err()
        );
        let mut stale = authority.clone();
        stale.store_generation += 1;
        assert!(
            storage
                .live_run_active_clock(&stale, &claim, "2026-10-01T00:10:01Z")
                .is_err()
        );
        drop(storage);
        let storage = Storage::open_or_create(&root).unwrap();
        let authority = storage.admission_execution_authority().unwrap();
        assert!(
            storage
                .live_run_active_clock(&authority, &claim, "2026-10-01T00:10:01Z")
                .is_err()
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn waiting_and_pause_intervals_are_excluded_and_overlaps_are_unioned() {
        let event = |sequence, status: &str, at: &str| (sequence, Some(status.into()), at.into());
        let events = [
            event(1, "queued", "2026-10-01T00:00:00Z"),
            event(2, "session_creation_intent", "2026-10-01T00:10:00Z"),
            event(3, "running", "2026-10-01T00:20:00Z"),
            event(4, "paused", "2026-10-01T00:25:00Z"),
            event(5, "running", "2026-10-01T01:00:00Z"),
        ];
        let now = timestamp_millis("2026-10-01T01:05:00Z").unwrap();
        assert_eq!(measured_active_millis(&events, now).unwrap(), 600_000);
        assert_eq!(
            union_millis(vec![(0, 600_000), (0, 600_000), (100_000, 600_000)]).unwrap(),
            600_000
        );
        let bad = [
            event(1, "running", "2026-10-01T00:00:00Z"),
            event(2, "unknown", "2026-10-01T00:01:00Z"),
        ];
        assert!(measured_active_millis(&bad, now).is_err());
    }
}
