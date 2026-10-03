import type { RecordEvidenceLocator as EvidenceLocator } from "./desktop-api";

export function resolveEvidenceLocator(sourceId: string, objectDigest: string, locator: string, captured: EvidenceLocator[] = []): EvidenceLocator | null {
  const range = /:lines:(\d+)-(\d+):of-(\d+)$/.exec(locator);
  if (range) {
    const [start, end, total] = range.slice(1).map(Number);
    if (start < 1 || end < start || end > total) return null;
    return { source_id: sourceId, object_digest: objectDigest, start_line: start, end_line: end, total_lines: total };
  }
  const page = /^pdf-v1:[^:]+:page:(\d+)$/.exec(locator);
  if (page) return captured.find((item) => item.source_id === sourceId && item.object_digest === objectDigest && item.page === Number(page[1])) ?? null;
  const image = /^image-v1:[^:]+:(\d+)x(\d+)$/.exec(locator);
  if (image) return { source_id: sourceId, object_digest: objectDigest, start_line: null, end_line: null, total_lines: null, width: Number(image[1]), height: Number(image[2]) };
  return null;
}

export function safeEvidenceImage(dataUrl?: string | null): string | null {
  return dataUrl && /^data:image\/(?:png|jpeg);base64,[A-Za-z0-9+/=]+$/.test(dataUrl) ? dataUrl : null;
}
