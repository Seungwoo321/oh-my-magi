use serde::{Deserialize, Serialize};

use crate::{CoreId, Digest, DomainError, OpaqueId, ProposalSnapshot, ValidationIssue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VoteValue {
    Support,
    Oppose,
    Abstain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ballot {
    pub schema_version: u16,
    pub run_id: OpaqueId,
    pub attempt_id: OpaqueId,
    pub attempt_generation: u64,
    pub core_id: CoreId,
    pub input_digest: Digest,
    pub proposal_id: OpaqueId,
    pub proposal_digest: Digest,
    pub vote: VoteValue,
    pub rationale: String,
    #[serde(default)]
    pub objection_refs: Vec<OpaqueId>,
    pub created_at: String,
}

impl Ballot {
    pub fn validate_for(
        &self,
        run_id: &str,
        input_digest: &Digest,
        generation: u64,
        proposal: &ProposalSnapshot,
    ) -> Result<(), DomainError> {
        let mut issues = Vec::new();
        if self.schema_version != crate::CONTRACT_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_schema_version",
                format!("expected {}", crate::CONTRACT_SCHEMA_VERSION),
            ));
        }
        if self.run_id != run_id {
            issues.push(ValidationIssue::new(
                "run_id",
                "run_mismatch",
                "ballot belongs to a different run",
            ));
        }
        if &self.input_digest != input_digest {
            issues.push(ValidationIssue::new(
                "input_digest",
                "input_digest_mismatch",
                "ballot input digest differs from the frozen run",
            ));
        }
        if self.attempt_generation != generation {
            issues.push(ValidationIssue::new(
                "attempt_generation",
                "stale_generation",
                "ballot was produced by a fenced attempt",
            ));
        }
        if self.proposal_id != proposal.proposal_id {
            issues.push(ValidationIssue::new(
                "proposal_id",
                "proposal_mismatch",
                "ballot targets a different proposal",
            ));
        }
        if self.proposal_digest != proposal.digest {
            issues.push(ValidationIssue::new(
                "proposal_digest",
                "proposal_digest_mismatch",
                "ballot does not reference the exact frozen proposal",
            ));
        }
        check_id("run_id", &self.run_id, &mut issues);
        check_id("attempt_id", &self.attempt_id, &mut issues);
        check_id("proposal_id", &self.proposal_id, &mut issues);
        check_digest("input_digest", &self.input_digest, &mut issues);
        check_digest("proposal_digest", &self.proposal_digest, &mut issues);
        check_text("rationale", &self.rationale, 8_000, &mut issues);
        check_text("created_at", &self.created_at, 64, &mut issues);
        for (index, objection_id) in self.objection_refs.iter().enumerate() {
            check_id(
                &format!("objection_refs[{index}]"),
                objection_id,
                &mut issues,
            );
            if !proposal
                .open_objections
                .iter()
                .any(|objection| objection.claim_id == *objection_id)
            {
                issues.push(ValidationIssue::new(
                    format!("objection_refs[{index}]"),
                    "unknown_objection",
                    "ballot objection reference must match an unresolved proposal claim",
                ));
            }
        }
        finish(issues)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Unanimous,
    Majority,
    Rejected,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoteCounts {
    pub support: u8,
    pub oppose: u8,
    pub abstain: u8,
}

impl VoteCounts {
    pub const fn total(self) -> u8 {
        self.support + self.oppose + self.abstain
    }

    pub const fn outcome(self) -> Option<Outcome> {
        match (self.support, self.oppose, self.abstain) {
            (3, 0, 0) => Some(Outcome::Unanimous),
            (2, 1, 0) | (2, 0, 1) => Some(Outcome::Majority),
            (1, 2, 0) | (0, 3, 0) | (0, 2, 1) => Some(Outcome::Rejected),
            (1, 1, 1) | (1, 0, 2) | (0, 1, 2) | (0, 0, 3) => Some(Outcome::Unresolved),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tally {
    pub run_id: OpaqueId,
    pub input_digest: Digest,
    pub proposal_id: OpaqueId,
    pub proposal_digest: Digest,
    pub counts: VoteCounts,
    pub outcome: Outcome,
}

impl Tally {
    pub fn from_ballots(
        run_id: &str,
        input_digest: &Digest,
        proposal: &ProposalSnapshot,
        ballots: &[Ballot],
        generation: u64,
    ) -> Result<Self, DomainError> {
        if ballots.len() != CoreId::ALL.len() {
            return Err(DomainError::Precondition {
                required: "exactly three valid ballots".to_owned(),
                actual: format!("{} ballots", ballots.len()),
            });
        }

        let mut seen = [false; 3];
        let mut counts = VoteCounts {
            support: 0,
            oppose: 0,
            abstain: 0,
        };
        for ballot in ballots {
            if ballot.attempt_generation > generation {
                return Err(DomainError::Precondition {
                    required: format!("ballot attempt generation no newer than {generation}"),
                    actual: format!("future generation {}", ballot.attempt_generation),
                });
            }
            ballot.validate_for(run_id, input_digest, ballot.attempt_generation, proposal)?;
            let index = match ballot.core_id {
                CoreId::Melchior1 => 0,
                CoreId::Balthasar2 => 1,
                CoreId::Casper3 => 2,
            };
            if seen[index] {
                return Err(DomainError::Validation(vec![ValidationIssue::new(
                    "ballots",
                    "duplicate_core_ballot",
                    format!("{} has more than one ballot", ballot.core_id),
                )]));
            }
            seen[index] = true;
            match ballot.vote {
                VoteValue::Support => counts.support += 1,
                VoteValue::Oppose => counts.oppose += 1,
                VoteValue::Abstain => counts.abstain += 1,
            }
        }
        if seen.iter().any(|was_seen| !was_seen) || counts.total() != 3 {
            return Err(DomainError::Precondition {
                required: "one ballot from each of MELCHIOR-1, BALTHASAR-2, and CASPER-3"
                    .to_owned(),
                actual: "one or more core ballots are missing".to_owned(),
            });
        }
        let outcome = counts.outcome().ok_or_else(|| DomainError::Precondition {
            required: "one of the ten valid three-ballot count combinations".to_owned(),
            actual: format!(
                "support={}, oppose={}, abstain={}",
                counts.support, counts.oppose, counts.abstain
            ),
        })?;

        Ok(Self {
            run_id: run_id.to_owned(),
            input_digest: input_digest.clone(),
            proposal_id: proposal.proposal_id.clone(),
            proposal_digest: proposal.digest.clone(),
            counts,
            outcome,
        })
    }
}

fn check_id(path: &str, value: &str, issues: &mut Vec<ValidationIssue>) {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        issues.push(ValidationIssue::new(path, "invalid_id", "opaque IDs must be non-empty, at most 128 UTF-8 bytes, and contain no control characters"));
    }
}

fn check_digest(path: &str, value: &Digest, issues: &mut Vec<ValidationIssue>) {
    if !value.is_valid() {
        issues.push(ValidationIssue::new(
            path,
            "digest_format",
            "value must be a SHA-256 digest",
        ));
    }
}

fn check_text(path: &str, value: &str, max_bytes: usize, issues: &mut Vec<ValidationIssue>) {
    if value.trim().is_empty() || value.len() > max_bytes {
        issues.push(ValidationIssue::new(
            path,
            "text_bounds",
            format!("value must be non-empty and at most {max_bytes} UTF-8 bytes"),
        ));
    }
}

fn finish(issues: Vec<ValidationIssue>) -> Result<(), DomainError> {
    if issues.is_empty() {
        Ok(())
    } else {
        Err(DomainError::Validation(issues))
    }
}
