import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { collectProviderLicenses } from './collect-provider-licenses.mjs';
const root = fs.mkdtempSync(path.join(os.tmpdir(), 'magi-license-test-'));
try {
  const add = (directory, name, text) => {
    fs.mkdirSync(directory, { recursive: true });
    fs.writeFileSync(path.join(directory, 'package.json'), JSON.stringify({ name, version: '1.0.0', license: 'MIT' }));
    fs.writeFileSync(path.join(directory, 'LICENSE'), text);
  };
  add(root, 'provider', 'Root attribution');
  const dependency = path.join(root, 'node_modules/@scope/dependency');
  add(dependency, '@scope/dependency', 'Dependency attribution');
  add(path.join(dependency, 'node_modules/nested'), 'nested', 'Nested attribution');
  const output = collectProviderLicenses(root);
  assert.match(output, /Root attribution/);
  assert.match(output, /Dependency attribution/);
  assert.match(output, /Nested attribution/);
  assert.equal(output, collectProviderLicenses(root));
  fs.symlinkSync(path.join(root, 'LICENSE'), path.join(dependency, 'NOTICE'));
  assert.throws(() => collectProviderLicenses(root), /bounded regular file/);
  fs.unlinkSync(path.join(dependency, 'NOTICE'));
  add(path.join(root, 'node_modules/conflict'), 'nested', 'Conflicting attribution');
  assert.throws(() => collectProviderLicenses(root), /Conflicting/);
  console.log('PASS provider licenses preserve root, scoped and nested attribution; deterministic bytes; reject symlinks and conflicting package identities');
} finally {
  fs.rmSync(root, { recursive: true, force: true });
}
