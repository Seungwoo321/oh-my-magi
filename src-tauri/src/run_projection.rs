use magi_domain::{
    Ballot, CoreId, Outcome, RunSnapshot, RunStage, RunStatus, VoteValue, status_name,
};
use magi_storage::{RunHistoryFilter, RunHistoryRequest, RunStatusFilter, Storage, StorageError};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunClarificationView {
    pub schema_version: u16,
    pub run_id: String,
    pub run_revision: u64,
    pub input_digest: magi_domain::Digest,
    pub run_generation: u64,
    pub reason: &'static str,
    pub resume_stage: RunStage,
    pub gaps: Vec<ClarificationGapView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarificationGapView {
    pub attempt_id: String,
    pub core_id: CoreId,
    pub stage: magi_domain::AssessmentStage,
    pub gap_index: u64,
    pub missing_information: String,
    pub impact: String,
    pub essential: bool,
}

pub fn clarification_view(
    snapshot: &RunSnapshot,
) -> Result<Option<RunClarificationView>, &'static str> {
    let RunStatus::Paused {
        reason: magi_domain::PauseReason::NeedsInput,
        resume_stage,
        ..
    } = &snapshot.run.status
    else {
        return Ok(None);
    };
    if !snapshot.essential_input_pending
        || !snapshot
            .assessments
            .iter()
            .any(|assessment| assessment.has_essential_gap())
    {
        return Err("The paused clarification does not contain its accepted essential gaps.");
    }
    let gaps = snapshot
        .assessments
        .iter()
        .flat_map(|assessment| {
            assessment
                .information_gaps
                .iter()
                .enumerate()
                .map(move |(index, gap)| ClarificationGapView {
                    attempt_id: assessment.attempt_id.clone(),
                    core_id: assessment.core_id,
                    stage: assessment.stage,
                    gap_index: index as u64,
                    missing_information: gap.missing_information.clone(),
                    impact: gap.impact.clone(),
                    essential: gap.essential,
                })
        })
        .collect();
    Ok(Some(RunClarificationView {
        schema_version: 1,
        run_id: snapshot.run.run_id.clone(),
        run_revision: snapshot.run.revision,
        input_digest: snapshot.run.input_digest.clone(),
        run_generation: snapshot.run.generation,
        reason: "needs_input",
        resume_stage: *resume_stage,
        gaps,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDossierView {
    pub run_id: String,
    pub question: String,
    pub status: &'static str,
    pub stage: &'static str,
    pub proposal: Option<ProposalView>,
    pub votes: Vec<VoteView>,
    pub outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RunErrorView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalView {
    pub body: String,
    pub conditions: Vec<String>,
    pub alternatives: Vec<String>,
    pub open_objections: Vec<ObjectionView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectionView {
    pub claim_id: String,
    pub rationale: String,
    pub required_information: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoteView {
    pub core_id: CoreId,
    pub choice: VoteValue,
    pub rationale: String,
}

#[derive(Debug, Serialize)]
pub struct RunErrorView {
    #[serde(rename = "profileBinding", skip_serializing_if = "Option::is_none")]
    pub profile_binding: Option<magi_domain::AuthFailureProfileBinding>,
    pub code: String,
    pub message: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleRunSummary {
    pub id: String,
    pub question: String,
    pub stage: &'static str,
    pub status: &'static str,
    pub source_count: usize,
    pub roles: Vec<RoleView>,
    pub ballot_state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub votes: Option<Vec<VoteValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
}

#[derive(Debug, Serialize)]
pub struct RoleView {
    pub core: CoreId,
    pub label: String,
}

fn disclosed_ballots(snapshot: &RunSnapshot) -> Option<&[Ballot]> {
    matches!(snapshot.run.status, RunStatus::Completed { .. })
        .then_some(snapshot.ballots_revealed.as_deref())
        .flatten()
}

fn stage_name(status: &RunStatus) -> &'static str {
    match status.stage() {
        Some(RunStage::IndependentReview) => "independent_review",
        Some(RunStage::CrossReview) => "cross_review",
        Some(RunStage::Synthesis) => "synthesis",
        Some(RunStage::Balloting) => "balloting",
        None => status_name(status),
    }
}

pub fn dossier_view(snapshot: &RunSnapshot) -> RunDossierView {
    RunDossierView {
        run_id: snapshot.run.run_id.clone(),
        question: snapshot.input.question.prompt.clone(),
        status: status_name(&snapshot.run.status),
        stage: stage_name(&snapshot.run.status),
        proposal: snapshot.proposal.as_ref().map(|proposal| ProposalView {
            body: proposal.body.clone(),
            conditions: proposal.conditions.clone(),
            alternatives: proposal.alternatives.clone(),
            open_objections: proposal
                .open_objections
                .iter()
                .map(|objection| ObjectionView {
                    claim_id: objection.claim_id.clone(),
                    rationale: objection.rationale.clone(),
                    required_information: objection.required_information.clone(),
                })
                .collect(),
        }),
        votes: disclosed_ballots(snapshot)
            .unwrap_or_default()
            .iter()
            .map(|ballot| VoteView {
                core_id: ballot.core_id,
                choice: ballot.vote,
                rationale: ballot.rationale.clone(),
            })
            .collect(),
        outcome: match snapshot.run.status {
            RunStatus::Completed { outcome } => Some(outcome),
            _ => None,
        },
        error: match &snapshot.run.status {
            RunStatus::Failed { reason_code } => Some(RunErrorView {
                profile_binding: None,
                code: reason_code.clone(),
                message: "The persisted deliberation failed. Review the saved run state before retrying.",
            }),
            _ => None,
        },
    }
}

pub fn dossier_view_with_failure(
    snapshot: &RunSnapshot,
    failure: Option<&magi_domain::LiveRunFailure>,
) -> RunDossierView {
    let mut view = dossier_view(snapshot);
    if let (Some(error), Some(failure)) = (&mut view.error, failure)
        && error.code == failure.code
        && failure.validate_profile_binding()
    {
        error.profile_binding = failure.profile_binding.clone();
    }
    view
}

pub fn console_run_summary(snapshot: &RunSnapshot) -> ConsoleRunSummary {
    let ballots = disclosed_ballots(snapshot);
    ConsoleRunSummary {
        id: snapshot.run.run_id.clone(),
        question: snapshot.input.question.prompt.clone(),
        stage: stage_name(&snapshot.run.status),
        status: status_name(&snapshot.run.status),
        source_count: snapshot.input.context_manifest.sources.len(),
        roles: snapshot
            .input
            .role_set
            .roles
            .iter()
            .map(|role| RoleView {
                core: role.core_id,
                label: role.display_name.clone(),
            })
            .collect(),
        ballot_state: if ballots.is_some() {
            "public"
        } else if snapshot.submitted_ballot_count > 0 {
            "sealed"
        } else {
            "none"
        },
        votes: ballots.map(|ballots| ballots.iter().map(|ballot| ballot.vote).collect()),
        outcome: match snapshot.run.status {
            RunStatus::Completed { outcome } => Some(outcome),
            _ => None,
        },
    }
}

pub fn selected_console_snapshot(
    storage: &Storage,
) -> Result<Option<magi_storage::ConsoleSnapshot>, StorageError> {
    // The home screen selects a current run, otherwise its last persisted run,
    // using the same deterministic ordering as the records list (UI_SCREENS §3.0).
    let active = storage.list_runs(&RunHistoryRequest {
        filter: RunHistoryFilter {
            statuses: vec![
                RunStatusFilter::Preparing,
                RunStatusFilter::AwaitingConfirmation,
                RunStatusFilter::IndependentReview,
                RunStatusFilter::CrossReview,
                RunStatusFilter::Synthesis,
                RunStatusFilter::Balloting,
                RunStatusFilter::Paused,
                RunStatusFilter::Interrupted,
                RunStatusFilter::Cancelling,
            ],
            ..RunHistoryFilter::default()
        },
        cursor: None,
        page_size: 1,
    })?;
    let selected = if let Some(run) = active.items.into_iter().next() {
        Some(run)
    } else {
        storage
            .list_runs(&RunHistoryRequest {
                filter: RunHistoryFilter::default(),
                cursor: None,
                page_size: 1,
            })?
            .items
            .into_iter()
            .next()
    };
    selected
        .map(|run| storage.load_console_snapshot(&run.run_id))
        .transpose()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use magi_domain::*;

    #[test]
    fn clarification_preserves_accepted_gap_authority_and_rejects_corrupt_pause() {
        let source = balloting_aggregate(0).snapshot(0);
        let mut assessment = source.assessments[0].clone();
        assessment.information_gaps = vec![InformationGap {
            missing_information: "The selected provider's disclosure policy".to_owned(),
            impact: "The user needs to approve the actual disclosure scope".to_owned(),
            essential: true,
        }];
        let mut pending = aggregate();
        pending
            .request_confirmation(0, "2026-10-01T00:00:01Z".into())
            .unwrap();
        pending
            .confirm_and_start(pending.run().revision, "2026-10-01T00:00:02Z".into())
            .unwrap();
        pending
            .accept_assessment(assessment.clone(), "2026-10-01T00:00:03Z".into())
            .unwrap();
        assert!(clarification_view(&pending.snapshot(0)).unwrap().is_none());
        pending
            .pause(
                PauseReason::NeedsInput,
                "Additional input is required".into(),
                true,
                pending.run().revision,
                "2026-10-01T00:00:04Z".into(),
            )
            .unwrap();
        let snapshot = pending.snapshot(0);
        let value = serde_json::to_value(clarification_view(&snapshot).unwrap().unwrap()).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["runRevision"], snapshot.run.revision);
        assert_eq!(
            value["inputDigest"],
            serde_json::to_value(&snapshot.run.input_digest).unwrap()
        );
        assert_eq!(value["runGeneration"], snapshot.run.generation);
        assert_eq!(value["gaps"][0]["attemptId"], assessment.attempt_id);
        assert_eq!(value["gaps"][0]["gapIndex"], 0);
        assert_eq!(value["gaps"][0]["essential"], true);
        assert!(value.get("votes").is_none());
        assert!(value.get("proposal").is_none());
        let mut corrupt = snapshot.clone();
        corrupt.essential_input_pending = false;
        assert!(clarification_view(&corrupt).is_err());
        corrupt = snapshot.clone();
        corrupt.assessments[0].information_gaps[0].essential = false;
        assert!(clarification_view(&corrupt).is_err());
        for status in [
            RunStatus::Cancelled,
            RunStatus::Failed {
                reason_code: "preserved_failure".into(),
            },
            RunStatus::Completed {
                outcome: Outcome::Unresolved,
            },
        ] {
            corrupt.run.status = status;
            assert!(clarification_view(&corrupt).unwrap().is_none());
        }
    }

    #[test]
    fn authentication_projection_preserves_exact_binding_and_hides_generic_metadata() {
        let mut aggregate = aggregate();
        let revision = aggregate.run().revision;
        aggregate
            .fail(
                "authentication_status_rpc_failed".into(),
                revision,
                "2026-10-01T00:00:00Z".into(),
            )
            .unwrap();
        let snapshot = aggregate.snapshot(0);
        let mut failure = LiveRunFailure {
            code: "authentication_status_rpc_failed".into(),
            detail: "Verification unavailable".into(),
            external_effect_unknown: false,
            profile_binding: Some(AuthFailureProfileBinding {
                provider_profile_id: "frozen-profile".into(),
                profile_revision: 7,
            }),
        };
        let value =
            serde_json::to_value(dossier_view_with_failure(&snapshot, Some(&failure))).unwrap();
        assert_eq!(value["error"]["profileBinding"]["profileRevision"], 7);
        assert!(value["votes"].as_array().unwrap().is_empty());
        failure.code = "provider_timeout".into();
        let value =
            serde_json::to_value(dossier_view_with_failure(&snapshot, Some(&failure))).unwrap();
        assert!(value["error"].get("profileBinding").is_none());
    }

    pub(crate) fn aggregate() -> RunAggregate {
        let roles = CoreId::ALL.map(|core_id| CoreRoleProfile {
            core_id,
            profile_id: format!("role-{}", core_id.wire_name()),
            revision: 1,
            display_name: core_id.wire_name().to_owned(),
            review_purpose: "Review fixture evidence".to_owned(),
            evaluation_criteria: vec!["Evidence".to_owned()],
            falsification_questions: vec!["What disproves the finding?".to_owned()],
            response_language: "en".to_owned(),
            binding: ModelBindingSnapshot {
                provider_profile_id: "provider-fixture".to_owned(),
                revision: 1,
                adapter_id: "adapter-fixture".to_owned(),
                adapter_version: "1".to_owned(),
                adapter_digest: Digest::from_bytes(b"adapter-fixture"),
                model_id: "model-fixture".to_owned(),
                context_window_tokens: None,
                maximum_output_tokens: None,
            },
            catalog_binding: None,
        });
        let input = InputSnapshot::new(
            QuestionSnapshot::new(
                "question-fixture".to_owned(),
                QuestionKind::Answer,
                "Evaluate the fixture".to_owned(),
                vec![],
                vec![],
                vec![],
            )
            .unwrap(),
            ContextManifest::new("manifest-fixture".to_owned(), vec![]).unwrap(),
            RoleSetSnapshot::new("roles-fixture".to_owned(), roles).unwrap(),
            Digest::from_bytes(b"policy-fixture"),
        )
        .unwrap();
        let run = Run::new(
            "run-fixture".to_owned(),
            "conversation-fixture".to_owned(),
            None,
            &input,
            "2026-10-01T00:00:00Z".to_owned(),
        )
        .unwrap();
        RunAggregate::new(run, input).unwrap()
    }

    fn balloting_aggregate(ballot_count: usize) -> RunAggregate {
        let mut aggregate = aggregate();
        advance_ballots(&mut aggregate, ballot_count);
        aggregate
    }

    pub(crate) fn advance_ballots(aggregate: &mut RunAggregate, ballot_count: usize) {
        let at = "2026-10-01T00:00:01Z";
        aggregate.request_confirmation(0, at.to_owned()).unwrap();
        aggregate
            .confirm_and_start(aggregate.run().revision, at.to_owned())
            .unwrap();
        for stage in [
            magi_domain::AssessmentStage::IndependentReview,
            magi_domain::AssessmentStage::CrossReview,
        ] {
            for core_id in CoreId::ALL {
                aggregate
                    .accept_assessment(
                        magi_domain::RoleAssessment {
                            schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
                            run_id: "run-fixture".to_owned(),
                            attempt_id: format!("assessment-{stage:?}-{}", core_id.wire_name()),
                            core_id,
                            stage,
                            input_digest: aggregate.input().input_digest.clone(),
                            attempt_generation: aggregate.run().generation,
                            position_summary: "Fixture finding".to_owned(),
                            claims: vec![magi_domain::Claim {
                                claim_id: format!("claim-{stage:?}-{}", core_id.wire_name()),
                                kind: magi_domain::ClaimKind::Inference,
                                text: "Fixture inference".to_owned(),
                                evidence_refs: vec![],
                                limitations: vec![],
                            }],
                            assumptions: vec![],
                            information_gaps: vec![],
                            counterarguments: vec![],
                            claim_responses: vec![],
                            position_changes: vec![],
                            created_at: at.to_owned(),
                        },
                        at.to_owned(),
                    )
                    .unwrap();
            }
        }
        let input = aggregate.input();
        let proposal = magi_domain::ProposalSnapshot {
            schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
            proposal_id: "proposal-fixture".to_owned(),
            run_id: "run-fixture".to_owned(),
            question_digest: input.question.digest.clone(),
            context_digest: input.context_manifest.digest.clone(),
            roles_digest: input.role_set.digest.clone(),
            kind: QuestionKind::Answer,
            body: "Fixture proposal".to_owned(),
            claims: vec![],
            conditions: vec![],
            alternatives: vec![],
            open_objections: vec![],
            digest: Digest::from_bytes(b"unsealed"),
            created_at: at.to_owned(),
        }
        .seal()
        .unwrap();
        aggregate
            .freeze_proposal(proposal.clone(), at.to_owned())
            .unwrap();
        for core_id in CoreId::ALL.into_iter().take(ballot_count) {
            aggregate
                .accept_ballot(
                    magi_domain::Ballot {
                        schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
                        run_id: "run-fixture".to_owned(),
                        attempt_id: format!("ballot-{}", core_id.wire_name()),
                        attempt_generation: aggregate.run().generation,
                        core_id,
                        input_digest: aggregate.input().input_digest.clone(),
                        proposal_id: proposal.proposal_id.clone(),
                        proposal_digest: proposal.digest.clone(),
                        vote: magi_domain::VoteValue::Support,
                        rationale: "Private sealed rationale".to_owned(),
                        objection_refs: vec![],
                        created_at: at.to_owned(),
                    },
                    at.to_owned(),
                )
                .unwrap();
        }
    }

    #[test]
    fn sealed_ballots_stay_hidden_and_completed_ballots_match_the_ui_envelope() {
        let sealed = balloting_aggregate(2).snapshot(10);
        let dossier = serde_json::to_value(dossier_view(&sealed)).unwrap();
        assert_eq!(dossier["status"], "balloting");
        assert_eq!(dossier["votes"], serde_json::json!([]));
        assert!(dossier["outcome"].is_null());
        assert!(dossier.get("runId").is_some());
        assert!(dossier.get("run_id").is_none());
        assert!(!dossier.to_string().contains("Private sealed rationale"));
        let summary = serde_json::to_value(console_run_summary(&sealed)).unwrap();
        assert_eq!(summary["ballotState"], "sealed");
        assert!(summary.get("votes").is_none());
        assert_eq!(summary["sourceCount"], 0);
        assert_eq!(summary["roles"].as_array().unwrap().len(), 3);
        let completed = balloting_aggregate(3).snapshot(11);
        let dossier = serde_json::to_value(dossier_view(&completed)).unwrap();
        assert_eq!(dossier["status"], "completed");
        assert_eq!(dossier["votes"].as_array().unwrap().len(), 3);
        assert_eq!(dossier["votes"][0]["choice"], "support");
        assert_eq!(dossier["outcome"], "unanimous");
        assert_eq!(
            serde_json::to_value(console_run_summary(&completed)).unwrap()["ballotState"],
            "public"
        );
    }

    #[test]
    fn paused_and_interrupted_runs_keep_their_phase_without_revealing_votes() {
        let at = "2026-10-01T00:00:02Z".to_owned();
        let mut paused = balloting_aggregate(2);
        paused
            .pause(
                PauseReason::Quota,
                "Quota exhausted".to_owned(),
                true,
                paused.run().revision,
                at.clone(),
            )
            .unwrap();
        let mut interrupted = balloting_aggregate(2);
        interrupted
            .mark_interrupted("Provider disconnected".to_owned(), true, at)
            .unwrap();
        for (aggregate, status) in [(paused, "paused"), (interrupted, "interrupted")] {
            let snapshot = aggregate.snapshot(12);
            let dossier = serde_json::to_value(dossier_view(&snapshot)).unwrap();
            let summary = serde_json::to_value(console_run_summary(&snapshot)).unwrap();
            assert_eq!(dossier["status"], status);
            assert_eq!(summary["status"], status);
            assert_eq!(dossier["stage"], "balloting");
            assert_eq!(summary["stage"], "balloting");
            assert_eq!(dossier["votes"], serde_json::json!([]));
            assert!(dossier["outcome"].is_null());
            assert!(summary.get("votes").is_none());
            assert_eq!(summary["ballotState"], "sealed");
            assert!(!dossier.to_string().contains("Private sealed rationale"));
        }
    }

    #[test]
    fn persisted_selection_and_missing_run_queries_fail_closed() {
        let root = std::env::temp_dir().join(format!("magi-projection-{}", uuid::Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        assert!(selected_console_snapshot(&storage).unwrap().is_none());
        assert!(storage.load_run_dossier("missing-run").is_err());
        assert!(storage.load_run_dossier("").is_err());
        let mut aggregate = aggregate();
        let command = CommandEnvelope {
            command_id: "projection-create".to_owned(),
            idempotency_key: "projection-create".to_owned(),
            command_kind: CommandKind::CreateRun,
            target_id: "run-fixture".to_owned(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        storage
            .commit_run(&command, &mut aggregate, "2026-10-01T00:00:00Z", None)
            .unwrap();
        let selected = selected_console_snapshot(&storage).unwrap().unwrap();
        assert_eq!(
            selected.run,
            storage.load_console_snapshot("run-fixture").unwrap().run
        );
        assert_eq!(selected.run.run.run_id, "run-fixture");
        assert!(selected.high_water.sequence > 0);
        advance_ballots(&mut aggregate, 3);
        let complete = CommandEnvelope {
            command_id: "projection-complete".to_owned(),
            idempotency_key: "projection-complete".to_owned(),
            command_kind: CommandKind::AddBallot,
            ..command.clone()
        };
        storage
            .commit_run(&complete, &mut aggregate, "2026-10-01T00:00:01Z", None)
            .unwrap();
        let completed = selected_console_snapshot(&storage).unwrap().unwrap();
        assert!(matches!(
            completed.run.run.status,
            RunStatus::Completed { .. }
        ));
        assert_eq!(
            dossier_view(&storage.load_run_dossier("run-fixture").unwrap().snapshot)
                .votes
                .len(),
            3
        );
        let input = aggregate.input().clone();
        let run = Run::new(
            "older-active".to_owned(),
            "conversation-active".to_owned(),
            None,
            &input,
            "2026-09-30T00:00:00Z".to_owned(),
        )
        .unwrap();
        let mut active = RunAggregate::new(run, input).unwrap();
        let create_active = CommandEnvelope {
            command_id: "projection-active".to_owned(),
            idempotency_key: "projection-active".to_owned(),
            target_id: "older-active".to_owned(),
            ..command
        };
        storage
            .commit_run(&create_active, &mut active, "2026-09-30T00:00:00Z", None)
            .unwrap();
        assert_eq!(
            selected_console_snapshot(&storage)
                .unwrap()
                .unwrap()
                .run
                .run
                .run_id,
            "older-active"
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }
}
