import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { generateKeyPairSync, verify } from 'node:crypto';
import { artifactIdentity, assembleReleaseResources, cleanupOwnedTarget, atomicJson, digest, resourceBuildEnvironment, signReleaseDocument, storageCompatibility, validateBuildReceipt } from './release-contract.mjs';

const root = process.cwd();
const owned = fs.mkdtempSync(path.join(os.tmpdir(), 'magi-release-contract-'));
try {
  const target = path.join(owned, 'fresh-target');
  fs.mkdirSync(target);
  const source = path.join(owned, 'signed-resource');
  fs.writeFileSync(source, 'isolated immutable resource fixture');
  const expected = digest(fs.readFileSync(source));
  const admitted = path.join(target, 'resource');
  const fakeRun = (command, args, transcript, env) => {
    assert.equal(command, 'cargo'); assert.ok(args.includes('--release')); assert.ok(args.includes('--locked'));
    assert.equal(env.CARGO_TARGET_DIR, target); assert.equal(env.MAGI_RESOURCE_BUILD_PURPOSE, 'admit');
    if (digest(fs.readFileSync(source)) !== expected) return 1;
    fs.copyFileSync(source, admitted); return 0;
  };
  const consume = env => {
    assert.equal(env.MAGI_RESOURCE_BUILD_PURPOSE, 'verify');
    assert.equal(digest(fs.readFileSync(admitted)), expected);
  };
  assert.throws(() => consume(resourceBuildEnvironment({}, 'verify', target)));
  consume(assembleReleaseResources(fakeRun, { MAGI_RESOURCE_BUILD_PURPOSE: 'publish-quiescent' }, target));
  fs.writeFileSync(source, 'changed resource');
  assert.throws(() => assembleReleaseResources(fakeRun, {}, target), /admission failed/);
  assert.throws(() => resourceBuildEnvironment({ TAURI_CONFIG: '{}' }, 'admit', target));
  assert.throws(() => resourceBuildEnvironment({}, 'release', target));
  console.log('PASS fresh target requires explicit admitted immutable inputs before Verify; drift and ambient override denied');

  const compatibility = storageCompatibility(root);
  assert.equal(compatibility.targetSchema, 18);
  const fixtureStorage = path.join(owned, 'src-tauri/crates/magi-storage');
  fs.mkdirSync(path.join(fixtureStorage, 'src'), { recursive: true });
  fs.cpSync(path.join(root, 'src-tauri/crates/magi-storage/schema_contracts'), path.join(fixtureStorage, 'schema_contracts'), { recursive: true });
  fs.cpSync(path.join(root, 'src-tauri/crates/magi-storage/migrations'), path.join(fixtureStorage, 'migrations'), { recursive: true });
  fs.copyFileSync(path.join(root, 'src-tauri/crates/magi-storage/src/migration_guard.rs'), path.join(fixtureStorage, 'src/migration_guard.rs'));
  const currentStore = fs.readFileSync(path.join(root, 'src-tauri/crates/magi-storage/src/store.rs'), 'utf8');
  // A synthetic future contract tests predecessor admission without changing product schema.
  const futureStore = currentStore.replace('const SCHEMA_VERSION: u32 = 18;', 'const SCHEMA_VERSION: u32 = 19;')
    .replace('fn migration_sql(version: u32)', 'const MIGRATION_19: &str = include_str!("../migrations/0019_fixture.sql");\nfn migration_sql(version: u32)')
    .replace('18 => Some(MIGRATION_18),', '18 => Some(MIGRATION_18),\n        19 => Some(MIGRATION_19),');
  fs.writeFileSync(path.join(fixtureStorage, 'src/store.rs'), futureStore);
  fs.writeFileSync(path.join(fixtureStorage, 'migrations/0019_fixture.sql'), 'CREATE TABLE future_fixture (id INTEGER PRIMARY KEY);');
  const future = storageCompatibility(owned);
  assert.equal(future.targetSchema, 19);
  const migrate = spawnSync('python3', ['-c', `import sqlite3,pathlib,sys,hashlib
c=sqlite3.connect(':memory:')
c.execute('CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,checksum TEXT,applied_at TEXT)')
for p in sorted(pathlib.Path(sys.argv[1]).glob('*.sql')):
 version=int(p.name[:4])
 c.executescript(p.read_text())
 c.execute('INSERT INTO schema_migrations VALUES (?,?,?)',(version,hashlib.sha256(p.read_bytes()).hexdigest(),'fixture'))
 c.execute('PRAGMA user_version='+str(version))
assert [r[0] for r in c.execute('SELECT version FROM schema_migrations ORDER BY version')]==list(range(1,20))
assert c.execute('PRAGMA user_version').fetchone()[0]==19
print('19')`, path.join(fixtureStorage, 'migrations')], { encoding: 'utf8', timeout: 10000 });
  assert.equal(migrate.status, 0, migrate.stderr); assert.equal(migrate.stdout.trim(), '19');
  const { privateKey, publicKey } = generateKeyPairSync('ed25519');
  const manifest = { version: 'fixture-next', target: 'darwin-aarch64', minimum_macos: '12.0', schema_min: future.schemaMin, schema_max: future.schemaMax, payload: { url: 'https://github.com/Seungwoo321/oh-my-magi/releases/download/fixture/a.tar.gz', sha256: expected, size: 1, signature: 'fixture-updater-signature' }, notes: '' };
  const document = signReleaseDocument(manifest, privateKey, publicKey);
  assert.ok(verify(null, Buffer.from(document.signedPayload, 'base64'), publicKey, Buffer.from(document.manifestSignature, 'base64')));
  const signed = JSON.parse(Buffer.from(document.signedPayload, 'base64'));
  const accepts = schema => signed.schema_min <= schema && schema <= signed.schema_max;
  assert.ok(accepts(18)); assert.ok(accepts(19)); assert.ok(!accepts(0)); assert.ok(!accepts(20));
  fs.writeFileSync(path.join(fixtureStorage, 'src/store.rs'), futureStore.replace('18 => Some(MIGRATION_18),', ''));
  assert.throws(() => storageCompatibility(owned), /incomplete/);
  console.log('PASS actual SQL1..18 plus synthetic19 migration; signed predecessor18 accepted, unsupported0/20 and incomplete registry denied');

  const archive = path.join(owned, 'artifact.tar.gz'); fs.writeFileSync(archive, 'verified archive fixture');
  const receipt = { format: 'magi-verified-release-v2', version: 'fixture-next', bundleVersion: 'fixture-next', archive, artifact: artifactIdentity(archive), storageCompatibility: future, target: 'darwin-aarch64', codeSignatureValid: true, gatekeeperAccepted: true, notarizationStapleValid: true, localSigned: false };
  const published = path.join(owned, 'build-verification.json'); const pending = path.join(owned, 'pending-build-verification.json');
  const foreignTemporary = `${published}.${process.pid}.pending`; fs.writeFileSync(foreignTemporary, 'foreign');
  assert.throws(() => atomicJson(published, receipt)); assert.equal(fs.readFileSync(foreignTemporary, 'utf8'), 'foreign'); fs.rmSync(foreignTemporary);
  atomicJson(published, { previous: true }); atomicJson(pending, receipt, { exclusive: true });
  assert.throws(() => atomicJson(pending, receipt, { exclusive: true }));
  assert.deepEqual(JSON.parse(fs.readFileSync(published)), { previous: true });
  validateBuildReceipt(receipt, { artifact: archive, version: 'fixture-next', compatibility: future });
  assert.throws(() => validateBuildReceipt(receipt, { artifact: archive, version: 'fixture-next', compatibility }));
  fs.writeFileSync(archive, 'changed archive fixture');
  assert.throws(() => validateBuildReceipt(receipt, { artifact: archive, version: 'fixture-next', compatibility: future }));
  assert.deepEqual(JSON.parse(fs.readFileSync(published)), { previous: true });
  fs.writeFileSync(archive, 'verified archive fixture'); atomicJson(published, receipt); fs.rmSync(pending);
  validateBuildReceipt(JSON.parse(fs.readFileSync(published)), { artifact: archive, version: 'fixture-next', compatibility: future });
  assert.equal(fs.readdirSync(owned).filter(name => name.endsWith('.pending')).length, 0);
  console.log('PASS atomic pending/published receipts preserve prior authority on failure; changed artifact or schema proof denied');
  const cleanup = path.join(owned, 'readonly-target'); fs.mkdirSync(cleanup);
  const cleanupIdentity = fs.lstatSync(cleanup);
  fs.mkdirSync(path.join(cleanup, 'sealed')); fs.writeFileSync(path.join(cleanup, 'sealed/input'), 'fixture');
  fs.symlinkSync(archive, path.join(cleanup, 'foreign-reference'));
  fs.chmodSync(path.join(cleanup, 'sealed'), 0o555); fs.chmodSync(cleanup, 0o555);
  cleanupOwnedTarget(cleanup, cleanupIdentity);
  assert.equal(fs.existsSync(cleanup), false); assert.equal(fs.existsSync(archive), true);
  const replaced = path.join(owned, 'replaced-target'); fs.mkdirSync(replaced); const original = fs.lstatSync(replaced);
  fs.renameSync(replaced, replaced + '-original'); fs.mkdirSync(replaced);
  assert.throws(() => cleanupOwnedTarget(replaced, original), /identity changed/);
  assert.equal(fs.existsSync(replaced), true);
  console.log('PASS sealed owned target cleanup; foreign symlink target preserved; replacement directory refused');

} finally {
  fs.rmSync(owned, { recursive: true, force: true });
  assert.equal(fs.existsSync(owned), false);
}
