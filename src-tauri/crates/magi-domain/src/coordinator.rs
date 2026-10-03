use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::{
    AssessmentStage, Ballot, CoreId, DomainError, InputSnapshot, OpaqueId, PauseReason,
    ProposalSnapshot, RoleAssessment, Run, RunStage, RunStatus, Tally, VoteValue,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    CreateRun,
    ConfirmRun,
    PauseRun,
    ResumeRun,
    CancelRun,
    AddAssessment,
    FreezeProposal,
    AddBallot,
    DeleteRun,
    CreateChildRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEnvelope {
    pub command_id: OpaqueId,
    pub idempotency_key: OpaqueId,
    pub command_kind: CommandKind,
    pub target_id: OpaqueId,
    pub expected_revision: u64,
    pub payload_digest: crate::Digest,
}

impl CommandEnvelope {
    pub fn validate(&self) -> Result<(), DomainError> {
        let mut errors = Vec::new();
        for (path, value) in [
            ("command_id", &self.command_id),
            ("idempotency_key", &self.idempotency_key),
            ("target_id", &self.target_id),
        ] {
            if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
                errors.push(crate::ValidationIssue::new(
                    path,
                    "invalid_id",
                    "value must be a non-empty opaque ID",
                ));
            }
        }
        if !self.payload_digest.is_valid() {
            errors.push(crate::ValidationIssue::new(
                "payload_digest",
                "digest_format",
                "payload digest must be SHA-256",
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(DomainError::Validation(errors))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReceipt {
    pub command_id: OpaqueId,
    pub command_kind: CommandKind,
    pub target_id: OpaqueId,
    pub payload_digest: crate::Digest,
    pub accepted_revision: u64,
    pub event_position: Option<EventPosition>,
    pub accepted_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    RunCreated,
    ConfirmationRequired,
    RunStarted,
    PhaseAdvanced,
    AssessmentAccepted,
    EssentialInformationNeeded,
    ProposalFrozen,
    BallotSealed,
    BallotsRevealed,
    RunPaused,
    RunInterrupted,
    CancellationRequested,
    RunCancelled,
    RunFailed,
    AttemptFenced,
}

impl EventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RunCreated => "run_created",
            Self::ConfirmationRequired => "confirmation_required",
            Self::RunStarted => "run_started",
            Self::PhaseAdvanced => "phase_advanced",
            Self::AssessmentAccepted => "assessment_accepted",
            Self::EssentialInformationNeeded => "essential_information_needed",
            Self::ProposalFrozen => "proposal_frozen",
            Self::BallotSealed => "ballot_sealed",
            Self::BallotsRevealed => "ballots_revealed",
            Self::RunPaused => "run_paused",
            Self::RunInterrupted => "run_interrupted",
            Self::CancellationRequested => "cancellation_requested",
            Self::RunCancelled => "run_cancelled",
            Self::RunFailed => "run_failed",
            Self::AttemptFenced => "attempt_fenced",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum EventPayload {
    RunCreated {
        run_id: OpaqueId,
        input_digest: crate::Digest,
    },
    ConfirmationRequired {
        run_id: OpaqueId,
    },
    RunStarted {
        run_id: OpaqueId,
        stage: RunStage,
    },
    PhaseAdvanced {
        from: RunStage,
        to: RunStage,
    },
    AssessmentAccepted {
        assessment_id: OpaqueId,
        core_id: CoreId,
        stage: AssessmentStage,
        accepted_count: u8,
    },
    EssentialInformationNeeded {
        gap_count: u16,
        resume_stage: RunStage,
    },
    ProposalFrozen {
        proposal_id: OpaqueId,
        proposal_digest: crate::Digest,
    },
    BallotSealed {
        submitted_count: u8,
    },
    BallotsRevealed {
        ballots: Vec<Ballot>,
        tally: Tally,
    },
    RunPaused {
        reason: PauseReason,
        detail: String,
        resume_stage: RunStage,
    },
    RunInterrupted {
        detail: String,
        resume_stage: RunStage,
        cancellation_requested: bool,
        external_effect_unknown: bool,
    },
    CancellationRequested {
        generation: u64,
    },
    RunCancelled,
    RunFailed {
        reason_code: String,
    },
    AttemptFenced {
        generation: u64,
    },
}

impl EventPayload {
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::RunCreated { .. } => EventKind::RunCreated,
            Self::ConfirmationRequired { .. } => EventKind::ConfirmationRequired,
            Self::RunStarted { .. } => EventKind::RunStarted,
            Self::PhaseAdvanced { .. } => EventKind::PhaseAdvanced,
            Self::AssessmentAccepted { .. } => EventKind::AssessmentAccepted,
            Self::EssentialInformationNeeded { .. } => EventKind::EssentialInformationNeeded,
            Self::ProposalFrozen { .. } => EventKind::ProposalFrozen,
            Self::BallotSealed { .. } => EventKind::BallotSealed,
            Self::BallotsRevealed { .. } => EventKind::BallotsRevealed,
            Self::RunPaused { .. } => EventKind::RunPaused,
            Self::RunInterrupted { .. } => EventKind::RunInterrupted,
            Self::CancellationRequested { .. } => EventKind::CancellationRequested,
            Self::RunCancelled => EventKind::RunCancelled,
            Self::RunFailed { .. } => EventKind::RunFailed,
            Self::AttemptFenced { .. } => EventKind::AttemptFenced,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainEvent {
    pub run_id: OpaqueId,
    pub run_revision: u64,
    pub generation: u64,
    pub event_type: EventKind,
    pub payload: EventPayload,
    pub created_at: String,
}

impl DomainEvent {
    fn new(run: &Run, payload: EventPayload, created_at: String) -> Self {
        Self {
            run_id: run.run_id.clone(),
            run_revision: run.revision,
            generation: run.generation,
            event_type: payload.kind(),
            payload,
            created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventPosition {
    pub store_id: OpaqueId,
    pub generation: u64,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCheckpoint {
    pub run_id: OpaqueId,
    pub input_digest: crate::Digest,
    pub status: RunStatus,
    pub resume_stage: Option<RunStage>,
    pub generation: u64,
    pub completed_assessment_attempt_ids: Vec<OpaqueId>,
    pub pending_dispatch_ids: Vec<OpaqueId>,
    pub essential_input_pending: bool,
    pub external_effect_unknown: bool,
    pub latest_event_sequence: u64,
    pub revision: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSnapshot {
    pub run: Run,
    pub input: InputSnapshot,
    pub assessments: Vec<RoleAssessment>,
    pub proposal: Option<ProposalSnapshot>,
    pub submitted_ballot_count: u8,
    pub ballots_revealed: Option<Vec<Ballot>>,
    pub tally: Option<Tally>,
    pub essential_input_pending: bool,
    pub external_effect_unknown: bool,
    pub latest_event_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPersistenceState {
    pub run: Run,
    pub input: InputSnapshot,
    pub assessments: Vec<RoleAssessment>,
    pub proposal: Option<ProposalSnapshot>,
    pub sealed_ballots: Vec<Ballot>,
    pub tally: Option<Tally>,
    pub essential_input_pending: bool,
    pub external_effect_unknown: bool,
    pub cancel_resume_stage: Option<RunStage>,
    pub event_drafts: Vec<DomainEvent>,
}

#[derive(Debug, Clone)]
pub struct RunAggregate {
    run: Run,
    input: InputSnapshot,
    assessments: BTreeMap<(AssessmentStage, CoreId), RoleAssessment>,
    proposal: Option<ProposalSnapshot>,
    sealed_ballots: BTreeMap<CoreId, Ballot>,
    tally: Option<Tally>,
    events: Vec<DomainEvent>,
    cancel_resume_stage: Option<RunStage>,
    needs_input: bool,
    unresolved_external_effect: bool,
}

impl RunAggregate {
    pub fn new(run: Run, input: InputSnapshot) -> Result<Self, DomainError> {
        run.validate()?;
        input.validate()?;
        if !matches!(run.status, RunStatus::Preparing) {
            return Err(DomainError::Precondition {
                required: "new runs to start in preparing".to_owned(),
                actual: super::model::status_name(&run.status).to_owned(),
            });
        }
        if run.input_digest != input.input_digest
            || run.question_id != input.question.question_id
            || run.context_manifest_id != input.context_manifest.manifest_id
            || run.role_set_id != input.role_set.role_set_id
        {
            return Err(DomainError::Precondition {
                required: "run IDs and digest to identify its immutable input snapshot".to_owned(),
                actual: "run does not match the supplied input snapshot".to_owned(),
            });
        }
        let mut aggregate = Self {
            run,
            input,
            assessments: BTreeMap::new(),
            proposal: None,
            sealed_ballots: BTreeMap::new(),
            tally: None,
            events: Vec::new(),
            cancel_resume_stage: None,
            needs_input: false,
            unresolved_external_effect: false,
        };
        aggregate.push(
            EventPayload::RunCreated {
                run_id: aggregate.run.run_id.clone(),
                input_digest: aggregate.run.input_digest.clone(),
            },
            aggregate.run.created_at.clone(),
        );
        Ok(aggregate)
    }

    pub fn run(&self) -> &Run {
        &self.run
    }
    pub fn input(&self) -> &InputSnapshot {
        &self.input
    }
    pub fn events(&self) -> &[DomainEvent] {
        &self.events
    }
    pub fn acknowledge_committed_events(
        &mut self,
        committed: &[DomainEvent],
    ) -> Result<(), DomainError> {
        if committed.len() > self.events.len() || self.events[..committed.len()] != *committed {
            return Err(DomainError::Precondition {
                required: "the committed event drafts to be a prefix of the aggregate event buffer"
                    .to_owned(),
                actual: "event drafts changed or were not committed in order".to_owned(),
            });
        }
        self.events.drain(..committed.len());
        Ok(())
    }

    pub fn persistence_state(&self) -> RunPersistenceState {
        RunPersistenceState {
            run: self.run.clone(),
            input: self.input.clone(),
            assessments: self.assessments.values().cloned().collect(),
            proposal: self.proposal.clone(),
            sealed_ballots: self.ordered_ballots(),
            tally: self.tally.clone(),
            essential_input_pending: self.needs_input,
            external_effect_unknown: self.unresolved_external_effect,
            cancel_resume_stage: self.cancel_resume_stage,
            event_drafts: self.events.clone(),
        }
    }

    pub fn restore(state: RunPersistenceState) -> Result<Self, DomainError> {
        state.run.validate()?;
        state.input.validate()?;
        if state.run.input_digest != state.input.input_digest
            || state.run.question_id != state.input.question.question_id
            || state.run.context_manifest_id != state.input.context_manifest.manifest_id
            || state.run.role_set_id != state.input.role_set.role_set_id
        {
            return Err(DomainError::Precondition {
                required: "persisted run IDs and digest to identify its immutable input snapshot"
                    .to_owned(),
                actual: "persisted run does not match its input snapshot".to_owned(),
            });
        }
        let mut aggregate = Self {
            run: state.run,
            input: state.input,
            assessments: BTreeMap::new(),
            proposal: None,
            sealed_ballots: BTreeMap::new(),
            tally: None,
            events: Vec::new(),
            cancel_resume_stage: None,
            needs_input: false,
            unresolved_external_effect: false,
        };
        let mut claim_ids = Vec::new();
        for assessment in state.assessments {
            if assessment.run_id != aggregate.run.run_id
                || assessment.input_digest != aggregate.run.input_digest
            {
                return Err(DomainError::Precondition {
                    required: "persisted assessment to match its run".to_owned(),
                    actual: "assessment references another run/input".to_owned(),
                });
            }
            if assessment.attempt_generation > aggregate.run.generation {
                return Err(DomainError::Precondition {
                    required: format!(
                        "assessment generation no newer than {}",
                        aggregate.run.generation
                    ),
                    actual: format!("future generation {}", assessment.attempt_generation),
                });
            }
            let allowed_claims = if assessment.stage == AssessmentStage::CrossReview {
                aggregate.claim_ids_for_cross_review()
            } else {
                Vec::new()
            };
            assessment.validate(&aggregate.input.context_manifest, &allowed_claims)?;
            if assessment
                .claims
                .iter()
                .any(|claim| claim_ids.contains(&claim.claim_id))
            {
                return Err(DomainError::DuplicateResult(
                    "persisted claim ID".to_owned(),
                ));
            }
            claim_ids.extend(assessment.claims.iter().map(|claim| claim.claim_id.clone()));
            if aggregate
                .assessments
                .insert((assessment.stage, assessment.core_id), assessment)
                .is_some()
            {
                return Err(DomainError::DuplicateResult(
                    "persisted core assessment".to_owned(),
                ));
            }
        }
        if let Some(proposal) = state.proposal {
            proposal.validate(Some(&aggregate.input.context_manifest))?;
            if proposal.run_id != aggregate.run.run_id
                || proposal.question_digest != aggregate.input.question.digest
                || proposal.context_digest != aggregate.input.context_manifest.digest
                || proposal.roles_digest != aggregate.input.role_set.digest
                || proposal.kind != aggregate.input.question.kind
            {
                return Err(DomainError::Precondition {
                    required: "persisted proposal to match its frozen run".to_owned(),
                    actual: "proposal references another input snapshot".to_owned(),
                });
            }
            aggregate.validate_proposal_sources(&proposal)?;
            aggregate.proposal = Some(proposal);
        }
        for ballot in state.sealed_ballots {
            let proposal = aggregate
                .proposal
                .as_ref()
                .ok_or(DomainError::ProposalMissing)?;
            ballot.validate_for(
                &aggregate.run.run_id,
                &aggregate.run.input_digest,
                ballot.attempt_generation,
                proposal,
            )?;
            if ballot.attempt_generation > aggregate.run.generation
                || aggregate
                    .sealed_ballots
                    .insert(ballot.core_id, ballot)
                    .is_some()
            {
                return Err(DomainError::DuplicateResult(
                    "persisted ballot or future attempt generation".to_owned(),
                ));
            }
        }
        aggregate.needs_input = state.essential_input_pending;
        aggregate.unresolved_external_effect = state.external_effect_unknown;
        aggregate.cancel_resume_stage = state.cancel_resume_stage;
        aggregate.events = state.event_drafts;
        if let Some(tally) = state.tally {
            let proposal = aggregate
                .proposal
                .as_ref()
                .ok_or(DomainError::ProposalMissing)?;
            let expected = Tally::from_ballots(
                &aggregate.run.run_id,
                &aggregate.run.input_digest,
                proposal,
                &aggregate.ordered_ballots(),
                aggregate.run.generation,
            )?;
            if tally != expected
                || !matches!(aggregate.run.status, RunStatus::Completed { outcome } if outcome == tally.outcome)
            {
                return Err(DomainError::Precondition {
                    required: "persisted tally and completed status to match all ballots"
                        .to_owned(),
                    actual: "stored tally is inconsistent".to_owned(),
                });
            }
            aggregate.tally = Some(tally);
        } else if matches!(aggregate.run.status, RunStatus::Completed { .. }) {
            return Err(DomainError::Precondition {
                required: "completed run to have a durable tally".to_owned(),
                actual: "tally missing".to_owned(),
            });
        }
        if aggregate.sealed_ballots.len() == CoreId::ALL.len() && aggregate.tally.is_none() {
            return Err(DomainError::Precondition {
                required: "three sealed ballots to be committed with outcome".to_owned(),
                actual: "three ballots exist without a tally".to_owned(),
            });
        }
        Ok(aggregate)
    }

    pub fn snapshot(&self, latest_event_sequence: u64) -> RunSnapshot {
        let ballots_revealed = if matches!(self.run.status, RunStatus::Completed { .. }) {
            Some(self.ordered_ballots())
        } else {
            None
        };
        RunSnapshot {
            run: self.run.clone(),
            input: self.input.clone(),
            assessments: self.assessments.values().cloned().collect(),
            proposal: self.proposal.clone(),
            submitted_ballot_count: self.sealed_ballots.len() as u8,
            ballots_revealed,
            tally: self.tally.clone(),
            essential_input_pending: self.needs_input,
            external_effect_unknown: self.unresolved_external_effect,
            latest_event_sequence,
        }
    }

    pub fn checkpoint(
        &self,
        latest_event_sequence: u64,
        pending_dispatch_ids: Vec<OpaqueId>,
    ) -> RunCheckpoint {
        RunCheckpoint {
            run_id: self.run.run_id.clone(),
            input_digest: self.run.input_digest.clone(),
            status: self.run.status.clone(),
            resume_stage: self.run.status.stage().or(self.cancel_resume_stage),
            generation: self.run.generation,
            completed_assessment_attempt_ids: self
                .assessments
                .values()
                .map(|a| a.attempt_id.clone())
                .collect(),
            pending_dispatch_ids,
            essential_input_pending: self.needs_input,
            external_effect_unknown: self.unresolved_external_effect,
            latest_event_sequence,
            revision: self.run.revision,
            updated_at: self.run.updated_at.clone(),
        }
    }

    pub fn request_confirmation(
        &mut self,
        expected_revision: u64,
        at: String,
    ) -> Result<(), DomainError> {
        self.change_status(
            RunStatus::AwaitingConfirmation,
            expected_revision,
            at.clone(),
        )?;
        self.push(
            EventPayload::ConfirmationRequired {
                run_id: self.run.run_id.clone(),
            },
            at,
        );
        Ok(())
    }

    pub fn confirm_and_start(
        &mut self,
        expected_revision: u64,
        at: String,
    ) -> Result<(), DomainError> {
        self.change_status(RunStatus::IndependentReview, expected_revision, at.clone())?;
        self.push(
            EventPayload::RunStarted {
                run_id: self.run.run_id.clone(),
                stage: RunStage::IndependentReview,
            },
            at,
        );
        Ok(())
    }

    pub fn accept_assessment(
        &mut self,
        assessment: RoleAssessment,
        at: String,
    ) -> Result<(), DomainError> {
        if self.needs_input {
            return Err(DomainError::Precondition {
                required: "a child run with the missing information".to_owned(),
                actual: "this run is fenced pending essential input".to_owned(),
            });
        }
        let expected_stage = match self.run.status {
            RunStatus::IndependentReview => AssessmentStage::IndependentReview,
            RunStatus::CrossReview => AssessmentStage::CrossReview,
            _ => {
                return Err(DomainError::Precondition {
                    required: "independent_review or cross_review stage".to_owned(),
                    actual: super::model::status_name(&self.run.status).to_owned(),
                });
            }
        };
        if assessment.stage != expected_stage {
            return Err(DomainError::Precondition {
                required: format!("{expected_stage:?}"),
                actual: format!("{:?}", assessment.stage),
            });
        }
        if assessment.run_id != self.run.run_id || assessment.input_digest != self.run.input_digest
        {
            return Err(DomainError::Precondition {
                required: "assessment bound to this run and frozen input".to_owned(),
                actual: "assessment references a different run or input".to_owned(),
            });
        }
        if assessment.attempt_generation != self.run.generation {
            return Err(DomainError::Precondition {
                required: format!("attempt generation {}", self.run.generation),
                actual: format!("stale attempt generation {}", assessment.attempt_generation),
            });
        }
        let key = (assessment.stage, assessment.core_id);
        if self.assessments.contains_key(&key) {
            return Err(DomainError::DuplicateResult(format!(
                "{} {:?}",
                assessment.core_id, assessment.stage
            )));
        }
        let available_claim_ids = self.claim_ids_for_cross_review();
        assessment.validate(&self.input.context_manifest, &available_claim_ids)?;
        let existing_claim_ids = self.all_claim_ids();
        for claim in &assessment.claims {
            if existing_claim_ids.iter().any(|id| id == &claim.claim_id) {
                return Err(DomainError::Validation(vec![crate::ValidationIssue::new(
                    "claims.claim_id",
                    "duplicate_global_claim",
                    "claim IDs must be unique across all assessments in this run",
                )]));
            }
        }

        let assessment_id = assessment.attempt_id.clone();
        let core_id = assessment.core_id;
        let stage = assessment.stage;
        let essential_gap_count = assessment
            .information_gaps
            .iter()
            .filter(|gap| gap.essential)
            .count();
        self.assessments.insert(key, assessment);
        self.run.revision += 1;
        self.run.updated_at = at.clone();
        let accepted_count = self
            .assessments
            .keys()
            .filter(|(saved_stage, _)| *saved_stage == stage)
            .count() as u8;
        self.push(
            EventPayload::AssessmentAccepted {
                assessment_id,
                core_id,
                stage,
                accepted_count,
            },
            at.clone(),
        );

        if essential_gap_count > 0 {
            let resume_stage = match stage {
                AssessmentStage::IndependentReview => RunStage::IndependentReview,
                AssessmentStage::CrossReview => RunStage::CrossReview,
            };
            self.fence_generation(at.clone());
            self.needs_input = true;
            self.push(
                EventPayload::EssentialInformationNeeded {
                    gap_count: essential_gap_count as u16,
                    resume_stage,
                },
                at.clone(),
            );
            return Ok(());
        }

        let all_cores_accepted = CoreId::ALL
            .iter()
            .all(|core| self.assessments.contains_key(&(stage, *core)));
        if all_cores_accepted {
            match stage {
                AssessmentStage::IndependentReview => self.advance_phase(
                    RunStatus::CrossReview,
                    RunStage::IndependentReview,
                    RunStage::CrossReview,
                    at,
                )?,
                AssessmentStage::CrossReview => self.advance_phase(
                    RunStatus::Synthesis,
                    RunStage::CrossReview,
                    RunStage::Synthesis,
                    at,
                )?,
            }
        }
        Ok(())
    }

    pub fn freeze_proposal(
        &mut self,
        proposal: ProposalSnapshot,
        at: String,
    ) -> Result<(), DomainError> {
        if !matches!(self.run.status, RunStatus::Synthesis) {
            return Err(DomainError::Precondition {
                required: "synthesis stage".to_owned(),
                actual: super::model::status_name(&self.run.status).to_owned(),
            });
        }
        if self.proposal.is_some() {
            return Err(DomainError::ProposalAlreadyFrozen);
        }
        if proposal.run_id != self.run.run_id
            || proposal.question_digest != self.input.question.digest
            || proposal.context_digest != self.input.context_manifest.digest
            || proposal.roles_digest != self.input.role_set.digest
            || proposal.kind != self.input.question.kind
        {
            return Err(DomainError::Precondition {
                required:
                    "proposal bound to this run's frozen question, context, roles, and answer kind"
                        .to_owned(),
                actual: "proposal references different input snapshots".to_owned(),
            });
        }
        proposal.validate(Some(&self.input.context_manifest))?;
        self.validate_proposal_sources(&proposal)?;
        let proposal_id = proposal.proposal_id.clone();
        let digest = proposal.digest.clone();
        self.proposal = Some(proposal);
        self.change_status(RunStatus::Balloting, self.run.revision, at.clone())?;
        self.push(
            EventPayload::ProposalFrozen {
                proposal_id,
                proposal_digest: digest,
            },
            at,
        );
        Ok(())
    }

    pub fn accept_ballot(&mut self, ballot: Ballot, at: String) -> Result<(), DomainError> {
        if !matches!(self.run.status, RunStatus::Balloting) {
            return Err(DomainError::Precondition {
                required: "balloting stage".to_owned(),
                actual: super::model::status_name(&self.run.status).to_owned(),
            });
        }
        let proposal = self.proposal.as_ref().ok_or(DomainError::ProposalMissing)?;
        if ballot.attempt_generation != self.run.generation {
            return Err(DomainError::Precondition {
                required: format!("attempt generation {}", self.run.generation),
                actual: format!("stale attempt generation {}", ballot.attempt_generation),
            });
        }
        ballot.validate_for(
            &self.run.run_id,
            &self.run.input_digest,
            self.run.generation,
            proposal,
        )?;
        if self.sealed_ballots.contains_key(&ballot.core_id) {
            return Err(DomainError::DuplicateResult(format!(
                "{} ballot",
                ballot.core_id
            )));
        }
        let core_id = ballot.core_id;
        let mut prospective_ballots = self.ordered_ballots();
        prospective_ballots.push(ballot.clone());
        let prospective_tally = if prospective_ballots.len() == CoreId::ALL.len() {
            Some(Tally::from_ballots(
                &self.run.run_id,
                &self.run.input_digest,
                proposal,
                &prospective_ballots,
                self.run.generation,
            )?)
        } else {
            None
        };

        self.sealed_ballots.insert(core_id, ballot);
        self.run.revision += 1;
        self.run.updated_at = at.clone();
        self.push(
            EventPayload::BallotSealed {
                submitted_count: self.sealed_ballots.len() as u8,
            },
            at.clone(),
        );

        if let Some(tally) = prospective_tally {
            let ordered = self.ordered_ballots();
            self.run.transition(
                RunStatus::Completed {
                    outcome: tally.outcome,
                },
                self.run.revision,
                at.clone(),
            )?;
            self.tally = Some(tally.clone());
            self.push(
                EventPayload::BallotsRevealed {
                    ballots: ordered,
                    tally,
                },
                at,
            );
        }
        Ok(())
    }

    pub fn pause(
        &mut self,
        reason: PauseReason,
        detail: String,
        quiescence_confirmed: bool,
        expected_revision: u64,
        at: String,
    ) -> Result<(), DomainError> {
        if self.run.revision != expected_revision {
            return Err(DomainError::RevisionConflict {
                expected: expected_revision,
                actual: self.run.revision,
            });
        }
        if !quiescence_confirmed {
            return Err(DomainError::Precondition {
                required: "provider calls to be observed stopped or settled before pausing"
                    .to_owned(),
                actual: "external work may still be in flight".to_owned(),
            });
        }
        let resume_stage = self
            .run
            .status
            .stage()
            .ok_or_else(|| DomainError::Precondition {
                required: "active run stage".to_owned(),
                actual: super::model::status_name(&self.run.status).to_owned(),
            })?;
        self.fence_generation(at.clone());
        let status = RunStatus::Paused {
            reason,
            detail: detail.clone(),
            resume_stage,
        };
        self.change_status(status, self.run.revision, at.clone())?;
        self.push(
            EventPayload::RunPaused {
                reason,
                detail,
                resume_stage,
            },
            at,
        );
        Ok(())
    }

    pub fn mark_interrupted(
        &mut self,
        detail: String,
        external_effect_unknown: bool,
        at: String,
    ) -> Result<(), DomainError> {
        let resume_stage = self
            .run
            .status
            .stage()
            .ok_or_else(|| DomainError::Precondition {
                required: "active run stage".to_owned(),
                actual: super::model::status_name(&self.run.status).to_owned(),
            })?;
        self.fence_generation(at.clone());
        self.unresolved_external_effect = external_effect_unknown;
        self.change_status(
            RunStatus::Interrupted {
                resume_stage,
                detail: detail.clone(),
                cancellation_requested: false,
            },
            self.run.revision,
            at.clone(),
        )?;
        self.push(
            EventPayload::RunInterrupted {
                detail,
                resume_stage,
                cancellation_requested: false,
                external_effect_unknown,
            },
            at,
        );
        Ok(())
    }

    pub fn request_cancel(
        &mut self,
        expected_revision: u64,
        at: String,
    ) -> Result<(), DomainError> {
        let resume_stage = self.run.status.stage();
        if resume_stage.is_none()
            && !matches!(
                self.run.status,
                RunStatus::Preparing | RunStatus::AwaitingConfirmation
            )
        {
            return Err(DomainError::Precondition {
                required: "active run stage or local preparation".to_owned(),
                actual: super::model::status_name(&self.run.status).to_owned(),
            });
        }
        if self.run.revision != expected_revision {
            return Err(DomainError::RevisionConflict {
                expected: expected_revision,
                actual: self.run.revision,
            });
        }
        self.cancel_resume_stage = resume_stage;
        self.fence_generation(at.clone());
        self.change_status(RunStatus::Cancelling, self.run.revision, at.clone())?;
        self.push(
            EventPayload::CancellationRequested {
                generation: self.run.generation,
            },
            at,
        );
        Ok(())
    }

    pub fn confirm_cancelled(
        &mut self,
        stopped: bool,
        detail: String,
        at: String,
    ) -> Result<(), DomainError> {
        if !matches!(self.run.status, RunStatus::Cancelling) {
            return Err(DomainError::Precondition {
                required: "cancelling state".to_owned(),
                actual: super::model::status_name(&self.run.status).to_owned(),
            });
        }
        if stopped {
            self.change_status(RunStatus::Cancelled, self.run.revision, at.clone())?;
            self.push(EventPayload::RunCancelled, at);
        } else {
            let resume_stage =
                self.cancel_resume_stage
                    .ok_or_else(|| DomainError::Precondition {
                        required: "saved pre-cancellation stage".to_owned(),
                        actual: "missing cancellation checkpoint".to_owned(),
                    })?;
            self.change_status(
                RunStatus::Interrupted {
                    resume_stage,
                    detail: detail.clone(),
                    cancellation_requested: true,
                },
                self.run.revision,
                at.clone(),
            )?;
            self.unresolved_external_effect = true;
            self.push(
                EventPayload::RunInterrupted {
                    detail,
                    resume_stage,
                    cancellation_requested: true,
                    external_effect_unknown: true,
                },
                at,
            );
        }
        Ok(())
    }

    pub fn resume(
        &mut self,
        expected_revision: u64,
        input_digest: &crate::Digest,
        readiness_ok: bool,
        external_effects_resolved: bool,
        at: String,
    ) -> Result<RunStage, DomainError> {
        if self.run.revision != expected_revision {
            return Err(DomainError::RevisionConflict {
                expected: expected_revision,
                actual: self.run.revision,
            });
        }
        if &self.run.input_digest != input_digest {
            return Err(DomainError::Precondition {
                required: "same frozen input digest".to_owned(),
                actual: "input changed; create a child run".to_owned(),
            });
        }
        if !readiness_ok {
            return Err(DomainError::Precondition {
                required: "readiness checks to pass".to_owned(),
                actual: "one or more readiness checks failed".to_owned(),
            });
        }
        if self.needs_input {
            return Err(DomainError::Precondition {
                required: "a child run with the missing information".to_owned(),
                actual: "same-input resume would omit a required input".to_owned(),
            });
        }
        if self.unresolved_external_effect && !external_effects_resolved {
            return Err(DomainError::Precondition {
                required: "provider request state to be reconciled".to_owned(),
                actual: "external effect remains unknown; automatic redispatch is unsafe"
                    .to_owned(),
            });
        }
        if matches!(
            self.run.status,
            RunStatus::Interrupted {
                cancellation_requested: true,
                ..
            }
        ) {
            return Err(DomainError::Precondition {
                required: "cancelled run or separate child run".to_owned(),
                actual: "cancellation was requested".to_owned(),
            });
        }
        let stage = match self.run.status {
            RunStatus::Paused { resume_stage, .. }
            | RunStatus::Interrupted { resume_stage, .. } => resume_stage,
            _ => {
                return Err(DomainError::Precondition {
                    required: "paused or interrupted run".to_owned(),
                    actual: super::model::status_name(&self.run.status).to_owned(),
                });
            }
        };
        let next = match stage {
            RunStage::IndependentReview => RunStatus::IndependentReview,
            RunStage::CrossReview => RunStatus::CrossReview,
            RunStage::Synthesis => RunStatus::Synthesis,
            RunStage::Balloting => RunStatus::Balloting,
        };
        self.change_status(next, expected_revision, at.clone())?;
        self.unresolved_external_effect = false;
        self.push(
            EventPayload::RunStarted {
                run_id: self.run.run_id.clone(),
                stage,
            },
            at,
        );
        Ok(stage)
    }

    pub fn fail(
        &mut self,
        reason_code: String,
        expected_revision: u64,
        at: String,
    ) -> Result<(), DomainError> {
        if reason_code.trim().is_empty() || reason_code.len() > 128 {
            return Err(DomainError::Validation(vec![crate::ValidationIssue::new(
                "reason_code",
                "text_bounds",
                "reason code must be non-empty and at most 128 bytes",
            )]));
        }
        self.change_status(
            RunStatus::Failed {
                reason_code: reason_code.clone(),
            },
            expected_revision,
            at.clone(),
        )?;
        self.push(EventPayload::RunFailed { reason_code }, at);
        Ok(())
    }

    fn advance_phase(
        &mut self,
        next: RunStatus,
        from: RunStage,
        to: RunStage,
        at: String,
    ) -> Result<(), DomainError> {
        self.change_status(next, self.run.revision, at.clone())?;
        self.push(EventPayload::PhaseAdvanced { from, to }, at);
        Ok(())
    }

    fn change_status(
        &mut self,
        next: RunStatus,
        expected_revision: u64,
        at: String,
    ) -> Result<(), DomainError> {
        self.run.transition(next, expected_revision, at)
    }

    fn fence_generation(&mut self, at: String) {
        self.run.generation += 1;
        self.run.revision += 1;
        self.run.updated_at = at.clone();
        self.push(
            EventPayload::AttemptFenced {
                generation: self.run.generation,
            },
            at,
        );
    }

    fn push(&mut self, payload: EventPayload, at: String) {
        self.events.push(DomainEvent::new(&self.run, payload, at));
    }

    fn claim_ids_for_cross_review(&self) -> Vec<OpaqueId> {
        self.assessments
            .iter()
            .filter(|((stage, _), _)| *stage == AssessmentStage::IndependentReview)
            .flat_map(|(_, assessment)| {
                assessment.claims.iter().map(|claim| claim.claim_id.clone())
            })
            .collect()
    }

    fn validate_proposal_sources(&self, proposal: &ProposalSnapshot) -> Result<(), DomainError> {
        let all_claim_ids = self.all_claim_ids();
        for objection in &proposal.open_objections {
            if !all_claim_ids.contains(&objection.claim_id) {
                return Err(DomainError::Validation(vec![crate::ValidationIssue::new(
                    "open_objections.claim_id",
                    "unknown_source_claim",
                    "open objection must point to a claim from an accepted assessment",
                )]));
            }
        }
        Ok(())
    }

    fn all_claim_ids(&self) -> Vec<OpaqueId> {
        self.assessments
            .values()
            .flat_map(|assessment| assessment.claims.iter().map(|claim| claim.claim_id.clone()))
            .collect()
    }

    fn ordered_ballots(&self) -> Vec<Ballot> {
        CoreId::ALL
            .iter()
            .filter_map(|core| self.sealed_ballots.get(core).cloned())
            .collect()
    }
}

#[derive(Debug, Default)]
pub struct Coordinator {
    runs: HashMap<OpaqueId, RunAggregate>,
}

impl Coordinator {
    pub fn insert(&mut self, run: Run, input: InputSnapshot) -> Result<(), DomainError> {
        if self.runs.contains_key(&run.run_id) {
            return Err(DomainError::DuplicateResult(format!("run {}", run.run_id)));
        }
        let aggregate = RunAggregate::new(run.clone(), input)?;
        self.runs.insert(run.run_id, aggregate);
        Ok(())
    }

    pub fn run(&self, run_id: &str) -> Option<&RunAggregate> {
        self.runs.get(run_id)
    }

    pub fn run_mut(&mut self, run_id: &str) -> Option<&mut RunAggregate> {
        self.runs.get_mut(run_id)
    }

    pub fn remove_terminal(&mut self, run_id: &str) -> Result<Option<RunAggregate>, DomainError> {
        if self
            .runs
            .get(run_id)
            .is_some_and(|aggregate| !aggregate.run.status.is_terminal())
        {
            return Err(DomainError::Precondition {
                required: "terminal run".to_owned(),
                actual: "run is still active".to_owned(),
            });
        }
        Ok(self.runs.remove(run_id))
    }

    pub fn active_run_for_conversation(&self, conversation_id: &str) -> Option<&Run> {
        self.runs
            .values()
            .map(|aggregate| &aggregate.run)
            .find(|run| run.conversation_id == conversation_id && !run.status.is_terminal())
    }

    pub fn sealed_vote_values(&self, run_id: &str) -> Option<BTreeMap<CoreId, VoteValue>> {
        self.runs.get(run_id).map(|aggregate| {
            if matches!(aggregate.run.status, RunStatus::Completed { .. }) {
                aggregate
                    .sealed_ballots
                    .iter()
                    .map(|(core, ballot)| (*core, ballot.vote))
                    .collect()
            } else {
                BTreeMap::new()
            }
        })
    }
}
