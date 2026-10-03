export type DossierCursor = { runId: string; generation: number; revision: number };
export class DossierPublicationFence {
  private readonly requests = new Map<string, number>();
  private readonly heads = new Map<string, DossierCursor>();
  begin(runId: string): number {
    const request = (this.requests.get(runId) ?? 0) + 1;
    this.requests.set(runId, request);
    return request;
  }
  isCurrent(runId: string, request: number): boolean { return this.requests.get(runId) === request; }
  accept(next: DossierCursor, request?: number): boolean {
    if (!next.runId || !Number.isSafeInteger(next.generation) || next.generation < 0 || !Number.isSafeInteger(next.revision) || next.revision < 0) return false;
    if (request !== undefined && !this.isCurrent(next.runId, request)) return false;
    const previous = this.heads.get(next.runId);
    if (previous && (next.generation < previous.generation || next.revision < previous.revision)) return false;
    this.heads.set(next.runId, { runId: next.runId, generation: next.generation, revision: next.revision });
    if (request === undefined) this.begin(next.runId);
    return true;
  }
}
