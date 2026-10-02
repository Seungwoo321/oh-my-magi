use crate::StorageError;
use magi_domain::{CoreId, Digest, QuestionKind, VoteValue, canonical_json};
use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use std::collections::BTreeSet;
pub const MAX_REPLAY_BYTES: usize = 10 * 1024 * 1024;
const MAX_TEXT: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayPublicContent {
    pub title: String,
    pub question: String,
    pub assessments: Vec<PublicAssessment>,
    pub proposal: PublicProposalContent,
    pub ballot_rationales: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicAssessment {
    pub assessment_id: String,
    pub core_id: CoreId,
    pub position_summary: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicProposalContent {
    pub kind: QuestionKind,
    pub body: String,
    pub conditions: Vec<String>,
    pub alternatives: Vec<String>,
    pub open_objections: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct SharedProposal {
    #[serde(flatten)]
    pub content: PublicProposalContent,
    pub proposal_digest: Digest,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicBallot {
    pub core_id: CoreId,
    pub vote: VoteValue,
    pub rationale: String,
    pub proposal_digest: Digest,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redaction {
    pub field: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct ReplayEvent {
    pub sequence: u64,
    pub offset_ms: u64,
    #[serde(flatten)]
    pub payload: ReplayPayload,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ReplayPayload {
    PhaseEntered { phase: ReplayPhase },
    AssessmentAvailable { assessment_id: String },
    ProposalFrozen { proposal_digest: Digest },
    BallotsRevealed { proposal_digest: Digest },
    RunEnded { status: ReplayEnd },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ReplayPhase {
    IndependentReview,
    CrossReview,
    Synthesis,
    Balloting,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayEnd {
    Completed,
    Cancelled,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedReplay {
    pub format: String,
    pub schema_version: u16,
    pub title: String,
    pub question: String,
    pub cores: Vec<CoreId>,
    pub assessments: Vec<PublicAssessment>,
    pub proposal: SharedProposal,
    pub ballots: Vec<PublicBallot>,
    pub events: Vec<ReplayEvent>,
    pub redactions: Vec<Redaction>,
    pub redacted: bool,
    pub attribution: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalReplay {
    pub external_replay_id: String,
    pub file_digest: Digest,
    pub schema_version: u16,
    pub imported_at_epoch_ms: u64,
    pub data: SharedReplay,
    pub external: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalReplaySummary {
    pub external_replay_id: String,
    pub file_digest: Digest,
    pub schema_version: u16,
    pub imported_at_epoch_ms: u64,
    pub title: String,
    pub question: String,
    pub external: bool,
}

impl ExternalReplay {
    pub fn parse_stored(json: &str) -> Result<Self, StorageError> {
        if json.len() > MAX_REPLAY_BYTES + 1024 {
            return Err(StorageError::Corrupt(
                "stored external replay byte limit exceeded".into(),
            ));
        }
        let mut deserializer = serde_json::Deserializer::from_str(json);
        let value = Bounded { depth: 0 }
            .deserialize(&mut deserializer)
            .map_err(|_| invalid_replay())?;
        deserializer.end().map_err(|_| invalid_replay())?;
        let replay: Self = serde_json::from_value(value).map_err(|_| invalid_replay())?;
        replay.validate()?;
        Ok(replay)
    }

    pub fn validate(&self) -> Result<(), StorageError> {
        self.data.validate()?;
        if !self.external
            || self.schema_version != self.data.schema_version
            || !self.file_digest.is_valid()
            || uuid::Uuid::parse_str(&self.external_replay_id).is_err()
        {
            return Err(StorageError::Integrity(
                "invalid external replay authority".into(),
            ));
        }
        Ok(())
    }
}

impl SharedReplay {
    pub fn validate(&self) -> Result<(), StorageError> {
        let bad = || StorageError::Corrupt("invalid shared replay contract".into());
        if self.format != "magi-replay"
            || self.schema_version != 1
            || self.cores != CoreId::ALL.to_vec()
            || self.events.len() > 10_000
            || self.redacted == self.redactions.is_empty()
        {
            return Err(bad());
        }
        if !self.proposal.proposal_digest.is_valid()
            || Digest::from_bytes(
                &canonical_json(&self.proposal.content).map_err(|_| invalid_replay())?,
            ) != self.proposal.proposal_digest
        {
            return Err(StorageError::Integrity(
                "shared proposal digest mismatch".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for a in &self.assessments {
            if a.assessment_id.is_empty() || !ids.insert(a.assessment_id.clone()) {
                return Err(bad());
            }
        }
        let mut cores = BTreeSet::new();
        for b in &self.ballots {
            if !cores.insert(b.core_id) || b.proposal_digest != self.proposal.proposal_digest {
                return Err(bad());
            }
        }
        if self.ballots.len() != 3 {
            return Err(bad());
        }
        let (mut sequence, mut offset, mut phase, mut frozen, mut revealed, mut ended) =
            (0, 0, None, false, false, false);
        let mut announced = BTreeSet::new();
        for e in &self.events {
            if e.sequence <= sequence || e.offset_ms < offset || ended {
                return Err(bad());
            }
            sequence = e.sequence;
            offset = e.offset_ms;
            match &e.payload {
                ReplayPayload::PhaseEntered { phase: p } => {
                    if phase.is_some_and(|prior| prior >= *p) {
                        return Err(bad());
                    }
                    phase = Some(*p);
                }
                ReplayPayload::AssessmentAvailable { assessment_id } => {
                    if !matches!(
                        phase,
                        Some(ReplayPhase::IndependentReview | ReplayPhase::CrossReview)
                    ) || !ids.contains(assessment_id)
                        || !announced.insert(assessment_id)
                    {
                        return Err(bad());
                    }
                }
                ReplayPayload::ProposalFrozen { proposal_digest } => {
                    if !matches!(phase, Some(ReplayPhase::Synthesis | ReplayPhase::Balloting))
                        || frozen
                        || proposal_digest != &self.proposal.proposal_digest
                    {
                        return Err(bad());
                    }
                    frozen = true;
                }
                ReplayPayload::BallotsRevealed { proposal_digest } => {
                    if phase != Some(ReplayPhase::Balloting)
                        || !frozen
                        || revealed
                        || proposal_digest != &self.proposal.proposal_digest
                    {
                        return Err(bad());
                    }
                    revealed = true;
                }
                ReplayPayload::RunEnded { status } => {
                    if matches!(status, ReplayEnd::Completed) && !revealed {
                        return Err(bad());
                    }
                    ended = true;
                }
            }
        }
        if !ended {
            return Err(bad());
        }
        let bytes = serde_json::to_vec(self).map_err(|_| invalid_replay())?;
        if bytes.len() > MAX_REPLAY_BYTES {
            return Err(bad());
        }
        bounded_value(&bytes)?;
        Ok(())
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, StorageError> {
        let value = bounded_value(bytes)?;
        let replay: Self = serde_json::from_value(value).map_err(|_| invalid_replay())?;
        replay.validate()?;
        Ok(replay)
    }
}
pub fn import_replay(bytes: &[u8], now: u64) -> Result<ExternalReplay, StorageError> {
    let data = SharedReplay::parse(bytes)?;
    Ok(ExternalReplay {
        external_replay_id: uuid::Uuid::new_v4().to_string(),
        file_digest: Digest::from_bytes(bytes),
        schema_version: data.schema_version,
        imported_at_epoch_ms: now,
        data,
        external: true,
    })
}

// The seed rejects duplicates before conversion to serde_json::Value can erase them.
struct Bounded {
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for Bounded {
    type Value = serde_json::Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        if self.depth > 32 {
            return Err(de::Error::custom("JSON depth exceeded"));
        }
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Bounded {
    type Value = serde_json::Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("bounded JSON")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(v.into())
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(v.into())
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(v.into())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Err(E::custom("floating numbers not allowed"))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        if v.len() > MAX_TEXT {
            return Err(E::custom("text limit exceeded"));
        }
        Ok(v.into())
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
        self.visit_str(&v)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = a.next_element_seed(Bounded {
            depth: self.depth + 1,
        })? {
            if out.len() >= 10_000 {
                return Err(de::Error::custom("array limit exceeded"));
            }
            out.push(v);
        }
        Ok(out.into())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
        let mut out = serde_json::Map::new();
        while let Some(k) = a.next_key::<String>()? {
            if k.len() > MAX_TEXT || out.contains_key(&k) || out.len() >= 10_000 {
                return Err(de::Error::custom("duplicate or excessive keys"));
            }
            let v = a.next_value_seed(Bounded {
                depth: self.depth + 1,
            })?;
            out.insert(k, v);
        }
        Ok(out.into())
    }
}
fn invalid_replay() -> StorageError {
    StorageError::Corrupt("invalid shared replay contract".into())
}

fn bounded_value(bytes: &[u8]) -> Result<serde_json::Value, StorageError> {
    if bytes.len() > MAX_REPLAY_BYTES {
        return Err(StorageError::Corrupt(
            "shared replay byte limit exceeded".into(),
        ));
    }
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let v = Bounded { depth: 0 }
        .deserialize(&mut d)
        .map_err(|_| invalid_replay())?;
    d.end().map_err(|_| invalid_replay())?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_duplicate_keys_before_parse() {
        assert!(bounded_value(br#"{"format":"magi-replay","format":"magi-replay"}"#).is_err());
    }
    #[test]
    fn enforces_depth_and_byte_limits() {
        let deep = format!("{}0{}", "[".repeat(34), "]".repeat(34));
        assert!(bounded_value(deep.as_bytes()).is_err());
        assert!(bounded_value(&vec![b' '; MAX_REPLAY_BYTES + 1]).is_err());
    }
    #[test]
    fn rejects_external_assets() {
        let json = br#"{"format":"magi-replay","schema_version":1,"url":"https://example.com"}"#;
        assert!(SharedReplay::parse(json).is_err());
    }
}

impl<'de> Deserialize<'de> for SharedProposal {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            kind: QuestionKind,
            body: String,
            conditions: Vec<String>,
            alternatives: Vec<String>,
            open_objections: Vec<String>,
            proposal_digest: Digest,
        }
        let w = Wire::deserialize(d)?;
        Ok(Self {
            content: PublicProposalContent {
                kind: w.kind,
                body: w.body,
                conditions: w.conditions,
                alternatives: w.alternatives,
                open_objections: w.open_objections,
            },
            proposal_digest: w.proposal_digest,
        })
    }
}
impl<'de> Deserialize<'de> for ReplayEvent {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            sequence: u64,
            offset_ms: u64,
            #[serde(rename = "type")]
            kind: String,
            payload: serde_json::Value,
        }
        let w = Wire::deserialize(d)?;
        let payload =
            serde_json::from_value(serde_json::json!({"type":w.kind,"payload":w.payload}))
                .map_err(|_| de::Error::custom("invalid replay event payload"))?;
        Ok(Self {
            sequence: w.sequence,
            offset_ms: w.offset_ms,
            payload,
        })
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    fn sample() -> SharedReplay {
        let content = PublicProposalContent {
            kind: QuestionKind::Answer,
            body: "Public answer".into(),
            conditions: vec![],
            alternatives: vec![],
            open_objections: vec![],
        };
        let digest = Digest::from_bytes(&canonical_json(&content).unwrap());
        SharedReplay {
            format: "magi-replay".into(),
            schema_version: 1,
            title: "Replay".into(),
            question: "Public question".into(),
            cores: CoreId::ALL.to_vec(),
            assessments: vec![],
            proposal: SharedProposal {
                content,
                proposal_digest: digest.clone(),
            },
            ballots: CoreId::ALL
                .iter()
                .map(|c| PublicBallot {
                    core_id: *c,
                    vote: VoteValue::Support,
                    rationale: "Recorded display vote".into(),
                    proposal_digest: digest.clone(),
                })
                .collect(),
            events: vec![
                ReplayEvent {
                    sequence: 1,
                    offset_ms: 0,
                    payload: ReplayPayload::PhaseEntered {
                        phase: ReplayPhase::IndependentReview,
                    },
                },
                ReplayEvent {
                    sequence: 2,
                    offset_ms: 10,
                    payload: ReplayPayload::PhaseEntered {
                        phase: ReplayPhase::CrossReview,
                    },
                },
                ReplayEvent {
                    sequence: 3,
                    offset_ms: 20,
                    payload: ReplayPayload::PhaseEntered {
                        phase: ReplayPhase::Synthesis,
                    },
                },
                ReplayEvent {
                    sequence: 4,
                    offset_ms: 30,
                    payload: ReplayPayload::ProposalFrozen {
                        proposal_digest: digest.clone(),
                    },
                },
                ReplayEvent {
                    sequence: 5,
                    offset_ms: 40,
                    payload: ReplayPayload::PhaseEntered {
                        phase: ReplayPhase::Balloting,
                    },
                },
                ReplayEvent {
                    sequence: 6,
                    offset_ms: 50,
                    payload: ReplayPayload::BallotsRevealed {
                        proposal_digest: digest,
                    },
                },
                ReplayEvent {
                    sequence: 7,
                    offset_ms: 60,
                    payload: ReplayPayload::RunEnded {
                        status: ReplayEnd::Completed,
                    },
                },
            ],
            redactions: vec![Redaction {
                field: "sources".into(),
                reason: "omitted".into(),
            }],
            redacted: true,
            attribution: "Unofficial fan project; recorded replay".into(),
        }
    }
    #[test]
    fn valid_closed_replay_roundtrips_as_inert_external() {
        let r = sample();
        let b = serde_json::to_vec(&r).unwrap();
        let imported = import_replay(&b, 123).unwrap();
        assert!(imported.external);
        assert_eq!(imported.file_digest, Digest::from_bytes(&b));
        assert_eq!(imported.imported_at_epoch_ms, 123);
    }
    #[test]
    fn rejects_digest_tampering_and_ballot_reference_substitution() {
        let mut r = sample();
        r.proposal.content.body = "tampered".into();
        assert!(r.validate().is_err());
        let mut r = sample();
        r.ballots[1].proposal_digest = Digest::from_bytes(b"other");
        assert!(r.validate().is_err());
    }
    #[test]
    fn rejects_premature_reveal_and_unknown_nested_fields() {
        let mut r = sample();
        r.events.swap(3, 5);
        assert!(r.validate().is_err());
        let mut v = serde_json::to_value(sample()).unwrap();
        v["proposal"]["source_path"] = serde_json::json!("/private/path");
        assert!(SharedReplay::parse(&serde_json::to_vec(&v).unwrap()).is_err());
        let mut v = serde_json::to_value(sample()).unwrap();
        v["events"][0]["payload"]["command"] = serde_json::json!("execute");
        assert!(SharedReplay::parse(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    #[test]
    fn rejects_single_field_text_limit() {
        let mut r = sample();
        r.question = "x".repeat(MAX_TEXT + 1);
        assert!(r.validate().is_err());
    }
    #[test]
    fn imported_private_fields_and_invalid_variants_never_enter_error_messages() {
        const CANARY: &str = "PRIVATE_IMPORT_CANARY_DO_NOT_ECHO";
        for fault in 0..6 {
            let mut value = serde_json::to_value(sample()).unwrap();
            match fault {
                0 => value[CANARY] = serde_json::json!(CANARY),
                1 => value["proposal"][CANARY] = serde_json::json!(CANARY),
                2 => value["events"][0]["type"] = serde_json::json!(CANARY),
                3 => value["events"][0]["payload"]["phase"] = serde_json::json!(CANARY),
                4 => value["ballots"][0]["core_id"] = serde_json::json!(CANARY),
                _ => value["proposal"]["proposal_digest"] = serde_json::json!(CANARY),
            }
            let bytes = serde_json::to_vec(&value).unwrap();
            let error = SharedReplay::parse(&bytes).unwrap_err().to_string();
            assert!(!error.contains(CANARY));
            let stored = serde_json::json!({"externalReplayId":uuid::Uuid::new_v4().to_string(),"fileDigest":Digest::from_bytes(&bytes),"schemaVersion":1,"importedAtEpochMs":0,"data":value,"external":true});
            let error = ExternalReplay::parse_stored(&stored.to_string())
                .unwrap_err()
                .to_string();
            assert!(!error.contains(CANARY));
        }
    }

    #[test]
    fn closed_replay_rejects_order_references_versions_and_early_reveal() {
        for fault in 0..9 {
            let mut replay = sample();
            match fault {
                0 => replay.schema_version = 2,
                1 => replay.cores.swap(0, 1),
                2 => replay.ballots[1].core_id = replay.ballots[0].core_id,
                3 => replay.events[1].sequence = replay.events[0].sequence,
                4 => replay.events[2].offset_ms = 0,
                5 => {
                    replay.events.remove(5);
                }
                6 => replay.events.push(ReplayEvent {
                    sequence: 8,
                    offset_ms: 70,
                    payload: ReplayPayload::PhaseEntered {
                        phase: ReplayPhase::Balloting,
                    },
                }),
                7 => {
                    replay.events[1].payload = ReplayPayload::AssessmentAvailable {
                        assessment_id: "unknown-assessment".into(),
                    }
                }
                _ => {
                    replay.events[3].payload = ReplayPayload::BallotsRevealed {
                        proposal_digest: replay.proposal.proposal_digest.clone(),
                    }
                }
            };
            assert!(replay.validate().is_err(), "fault {fault}");
            assert!(
                SharedReplay::parse(&serde_json::to_vec(&replay).unwrap()).is_err(),
                "fault {fault}"
            );
        }
    }

    #[test]
    fn bounded_parser_rejects_floats_trailing_json_and_excessive_collections() {
        for bytes in [
            b"1.25".as_slice(),
            b"{} {}".as_slice(),
            b"{\"private\":0,\"private\":1}".as_slice(),
        ] {
            assert!(bounded_value(bytes).is_err());
        }
        let array = format!("[{}]", vec!["0"; 10_001].join(","));
        assert!(bounded_value(array.as_bytes()).is_err());
        let mut replay = sample();
        replay.title = "x".repeat(MAX_TEXT + 1);
        assert!(replay.validate().is_err());
    }

    #[test]
    fn stored_replay_is_inert_and_roundtrips_without_native_authority() {
        let bytes = serde_json::to_vec(&sample()).unwrap();
        let imported = import_replay(&bytes, 123).unwrap();
        let json = serde_json::to_string(&imported).unwrap();
        let stored = ExternalReplay::parse_stored(&json).unwrap();
        assert_eq!(stored.external_replay_id, imported.external_replay_id);
        assert_eq!(stored.file_digest, Digest::from_bytes(&bytes));
        assert!(stored.external);
        let mut untrusted = stored.clone();
        untrusted.external = false;
        assert!(untrusted.validate().is_err());
        let mut malformed = stored.clone();
        malformed.external_replay_id = "not-an-id".into();
        assert!(malformed.validate().is_err());
    }
}
