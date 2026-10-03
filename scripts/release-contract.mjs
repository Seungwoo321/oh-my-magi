import fs from 'node:fs';
import path from 'node:path';
import { createHash, sign, verify } from 'node:crypto';

export const digest = bytes => createHash('sha256').update(bytes).digest('hex');

export function storageCompatibility(root) {
  const directory = path.join(root, 'src-tauri/crates/magi-storage');
  const source = fs.readFileSync(path.join(directory, 'src/store.rs'), 'utf8');
  const target = Number(source.match(/const SCHEMA_VERSION: u32 = (\d+);/)?.[1]);
  const registry = source.match(/fn migration_sql\(version: u32\)[\s\S]*?\n}\n/)?.[0];
  const initialization = source.match(/fn initialize_schema\([\s\S]*?\n}\n/)?.[0];
  if (!Number.isSafeInteger(target) || target < 1 || !registry || !initialization
      || !initialization.includes('read_only_store_identity(&transaction)?;')
      || !initialization.includes('while schema_version < SCHEMA_VERSION')
      || !initialization.includes('migration_sql(next_version)')
      || !initialization.includes('Digest::from_bytes(sql.as_bytes())')) throw new Error('Unsupported storage migration contract.');
  const registered = [...registry.matchAll(/(\d+) => Some\(MIGRATION_(\d+)\)/g)];
  if (registered.length !== target) throw new Error('Storage migration registry is incomplete.');
  const migrations = registered.map(([_, version, binding], index) => {
    if (Number(version) !== index + 1 || version !== binding) throw new Error('Storage migration registry is not contiguous.');
    const reference = source.match(new RegExp(`const MIGRATION_${version}: &str =\\s*include_str!\\("(\\.\\./migrations/[^"/]+\\.sql)"\\);`))?.[1];
    if (!reference) throw new Error('Storage migration source is missing.');
    const file = path.resolve(directory, 'src', reference);
    const bytes = fs.readFileSync(file);
    if (!bytes.length) throw new Error('Storage migration is empty.');
    return { version: Number(version), sha256: digest(bytes) };
  });
  const contracts = path.join(directory, 'schema_contracts');
  const schemaContracts = fs.readdirSync(contracts).sort().map(name => ({ name, sha256: digest(fs.readFileSync(path.join(contracts, name))) }));
  const proof = { targetSchema: target, migrations, schemaContracts, storeSourceSha256: digest(source), registrySha256: digest(registry), initializationSha256: digest(initialization), migrationGuardSha256: digest(fs.readFileSync(path.join(directory, 'src/migration_guard.rs'))) };
  // Schema zero denotes an uninitialized database, not a supported installed store.
  return { schemaMin: migrations[0].version, schemaMax: target, targetSchema: target, migrationContractSha256: digest(JSON.stringify(proof)) };
}

export function artifactIdentity(file) {
  const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
  try {
    const before = fs.fstatSync(fd);
    if (!before.isFile() || before.nlink !== 1 || before.size < 1 || before.size > 512 * 1024 * 1024) throw new Error('Invalid release artifact.');
    const hash = createHash('sha256');
    const buffer = Buffer.alloc(64 * 1024);
    let size = 0;
    while (size <= before.size) {
      const count = fs.readSync(fd, buffer, 0, Math.min(buffer.length, before.size + 1 - size), null);
      if (!count) break;
      size += count; hash.update(buffer.subarray(0, count));
    }
    const after = fs.fstatSync(fd);
    const current = fs.lstatSync(file);
    if (size !== before.size || ['dev', 'ino', 'size', 'mtimeMs', 'ctimeMs'].some(key => before[key] !== after[key] || after[key] !== current[key])) throw new Error('Release artifact changed while verifying.');
    return { sha256: hash.digest('hex'), size };
  } finally { fs.closeSync(fd); }
}

export function atomicJson(file, value, { exclusive = false } = {}) {
  const temporary = `${file}.${process.pid}.pending`;
  let fd;
  let created = false;
  try {
    if (exclusive && fs.existsSync(file)) throw new Error('Resolve the previous pending release before proceeding.');
    fd = fs.openSync(temporary, 'wx', 0o600);
    created = true;
    fs.writeFileSync(fd, JSON.stringify(value, null, 2) + '\n');
    fs.fsyncSync(fd);
    fs.closeSync(fd); fd = undefined;
    if (exclusive) { fs.linkSync(temporary, file); fs.unlinkSync(temporary); }
    else fs.renameSync(temporary, file);
    const directory = fs.openSync(path.dirname(file), 'r');
    try { fs.fsyncSync(directory); } finally { fs.closeSync(directory); }
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
    if (created) fs.rmSync(temporary, { force: true });
  }
}

export function validateBuildReceipt(receipt, { artifact, version, compatibility }) {
  const identity = artifactIdentity(artifact);
  if (receipt.format !== 'magi-verified-release-v2' || receipt.version !== version || receipt.bundleVersion !== version
      || !receipt.codeSignatureValid || !receipt.gatekeeperAccepted || !receipt.notarizationStapleValid || receipt.localSigned
      || path.resolve(artifact) !== path.resolve(receipt.archive)
      || !['darwin-aarch64', 'darwin-x86_64'].includes(receipt.target)
      || identity.sha256 !== receipt.artifact?.sha256 || identity.size !== receipt.artifact?.size
      || JSON.stringify(receipt.storageCompatibility) !== JSON.stringify(compatibility)) throw new Error('Artifact or migration contract differs from the verified published release.');
  return identity;
}

export function resourceBuildEnvironment(environment, purpose, target) {
  if (!['admit', 'verify'].includes(purpose)) throw new Error('Unsupported release resource purpose.');
  if (environment.TAURI_CONFIG !== undefined) throw new Error('Ambient Tauri configuration is forbidden.');
  return { ...environment, CARGO_TARGET_DIR: target, MAGI_RESOURCE_BUILD_PURPOSE: purpose };
}

export function assembleReleaseResources(run, environment, target) {
  const admit = resourceBuildEnvironment(environment, 'admit', target);
  if (run('cargo', ['build', '--manifest-path', 'src-tauri/Cargo.toml', '--release', '--lib', '--features', 'custom-protocol', '--locked'], 'resource-admission.log', admit) !== 0) throw new Error('Verified release resource admission failed.');
  return resourceBuildEnvironment(environment, 'verify', target);
}

export function signReleaseDocument(manifest, privateKey, publicKey) {
  const signedBytes = Buffer.from(JSON.stringify(manifest));
  const signature = sign(null, signedBytes, privateKey);
  if (!verify(null, signedBytes, publicKey, signature)) throw new Error('Manifest signing verification failed.');
  return { version: manifest.version, notes: manifest.notes, platforms: { [manifest.target]: { url: manifest.payload.url, signature: manifest.payload.signature } }, signedPayload: signedBytes.toString('base64'), manifestSignature: signature.toString('base64') };
}

export function cleanupOwnedTarget(target, expected) {
  const identity = fs.lstatSync(target);
  if (!identity.isDirectory() || identity.isSymbolicLink() || identity.dev !== expected.dev || identity.ino !== expected.ino || identity.uid !== expected.uid) throw new Error('Owned release target identity changed; cleanup refused.');
  const pending = [target];
  let visited = 0;
  while (pending.length) {
    if (++visited > 1_000_000) throw new Error('Owned release cleanup inventory exceeds its bound.');
    const directory = pending.pop();
    const held = fs.lstatSync(directory);
    if (!held.isDirectory() || held.isSymbolicLink() || held.uid !== expected.uid || held.dev !== expected.dev) throw new Error('Owned release directory custody changed; cleanup refused.');
    fs.chmodSync(directory, held.mode | 0o700);
    for (const name of fs.readdirSync(directory)) {
      const child = path.join(directory, name);
      const metadata = fs.lstatSync(child);
      if (metadata.isDirectory() && !metadata.isSymbolicLink()) pending.push(child);
    }
  }
  fs.rmSync(target, { recursive: true });
}
