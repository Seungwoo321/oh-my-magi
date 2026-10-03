import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { sourceBinding, publishExtractionTree } from './build-extraction-resource.mjs';

function fixture() {
  const bytes = Buffer.alloc(248);
  bytes.writeUInt32LE(0xfeedfacf, 0);
  bytes.writeUInt32LE(1, 16);
  bytes.writeUInt32LE(152, 20);
  bytes.writeUInt32LE(0x19, 32);
  bytes.writeUInt32LE(152, 36);
  bytes.writeUInt32LE(1, 96);
  bytes.write('__magi_source', 104);
  bytes.write('__TEXT', 120);
  bytes.writeBigUInt64LE(64n, 144);
  bytes.writeUInt32LE(184, 152);
  bytes.write('a'.repeat(64), 184);
  return bytes;
}
assert.equal(sourceBinding(fixture()), 'a'.repeat(64));
for (const mutate of [
  bytes => bytes.writeUInt32LE(4097, 16),
  bytes => bytes.writeUInt32LE(4, 36),
  bytes => bytes.writeBigUInt64LE(65n, 144),
  bytes => bytes.writeUInt32LE(220, 152),
  bytes => bytes.write('__DATA', 120),
  bytes => bytes.write('g', 184),
]) {
  const bytes = fixture(); mutate(bytes); assert.throws(() => sourceBinding(bytes));
}
assert.throws(() => sourceBinding(fixture().subarray(0, 20)));
console.log('Extraction source-binding parser rejects malformed commands, sections, bounds and digests.');
const fixtureRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'magi-extraction-publication-'));
const source = path.join(fixtureRoot, 'staged'), destination = path.join(fixtureRoot, 'extraction');
try {
  fs.mkdirSync(source, { mode: 0o700 });
  fs.writeFileSync(path.join(source, 'magi-extract'), 'signed fixture', { mode: 0o555 });
  fs.writeFileSync(path.join(source, 'magi-extract.sha256'), 'digest fixture', { mode: 0o444 });
  const original = fs.statSync(source).ino;
  fs.chmodSync(source, 0o555);
  publishExtractionTree(source, destination);
  assert.equal(fs.existsSync(source), false);
  assert.equal(fs.statSync(destination).ino, original);
  assert.equal(fs.statSync(destination).mode & 0o777, 0o555);
  assert.equal(fs.statSync(path.join(destination, 'magi-extract')).mode & 0o777, 0o555);
  assert.equal(fs.statSync(path.join(destination, 'magi-extract.sha256')).mode & 0o777, 0o444);
  assert.equal(fs.readFileSync(path.join(destination, 'magi-extract'), 'utf8'), 'signed fixture');
  fs.mkdirSync(source, { mode: 0o700 });
  assert.throws(() => publishExtractionTree(source, destination));
  assert.equal(fs.statSync(destination).ino, original);
} finally {
  if (fs.existsSync(destination)) fs.chmodSync(destination, 0o700);
  fs.rmSync(fixtureRoot, { recursive: true });
}
console.log('Read-only extraction publication preserves identity, final seals and existing generations.');
