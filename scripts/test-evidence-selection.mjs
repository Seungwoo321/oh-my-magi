import { test } from 'node:test';
import assert from 'node:assert/strict';
import { resolveEvidenceLocator, safeEvidenceImage } from '../src/lib/evidence-selection.ts';
test('text locators preserve exact approved line identity and reject invalid ranges', () => {
  assert.deepEqual(resolveEvidenceLocator('s', 'original', 'utf8-text-v1:derived:lines:2-4:of-5'), { source_id: 's', object_digest: 'original', start_line: 2, end_line: 4, total_lines: 5 });
  assert.equal(resolveEvidenceLocator('s', 'o', 'utf8-text-v1:d:lines:0-4:of-5'), null);
  assert.equal(resolveEvidenceLocator('s', 'o', 'utf8-text-v1:d:lines:2-6:of-5'), null);
});
test('PDF pages use exact captured raster metadata and reject unavailable pages', () => {
  const page = { source_id: 's', object_digest: 'o', start_line: null, end_line: null, total_lines: null, page: 2, width: 600, height: 800 };
  assert.equal(resolveEvidenceLocator('s', 'o', 'pdf-v1:d:page:2', [page]), page);
  assert.equal(resolveEvidenceLocator('s', 'o', 'pdf-v1:d:page:3', [page]), null);
});
test('image evidence never interprets remote URLs or executable data URIs', () => {
  assert.equal(safeEvidenceImage('data:image/png;base64,AA=='), 'data:image/png;base64,AA==');
  for (const value of ['https://external/image.png', 'data:image/svg+xml;base64,AA==', 'javascript:alert(1)', 'data:text/html;base64,AA==']) assert.equal(safeEvidenceImage(value), null);
});
