import type { RecordReplayEvent, RunDossierView } from "./desktop-api";

export function projectRecordedReplay(events: readonly RecordReplayEvent[], cursor: number) {
  const assessments = new Set<string>();
  let phase: string | null = null;
  let status: string | null = null;
  let proposalVisible = false;
  let votes: RunDossierView["votes"] = [];
  let outcome: RunDossierView["outcome"] = null;
  for (const event of events.slice(0, Math.max(0, cursor + 1))) {
    if (event.phase) phase = event.phase;
    if (event.kind === "assessment_accepted" && event.assessmentId) assessments.add(event.assessmentId);
    if (event.kind === "proposal_frozen") proposalVisible = true;
    if (event.kind === "ballots_revealed" && event.votes) { votes = event.votes; outcome = event.outcome ?? null; }
    if (["run_completed", "run_cancelled", "run_failed", "run_paused", "run_interrupted"].includes(event.kind)) status = event.kind.slice(4);
  }
  return { assessments, phase, status, proposalVisible, votes, outcome };
}
