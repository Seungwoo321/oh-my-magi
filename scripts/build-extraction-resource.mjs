import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash, randomUUID } from 'node:crypto';
import { spawnSync } from 'node:child_process';

const digest = bytes => createHash('sha256').update(bytes).digest('hex');
export function sourceBinding(bytes) {
  const word = offset => { if (offset < 0 || offset + 4 > bytes.length) throw new Error('Truncated Mach-O.'); return bytes.readUInt32LE(offset); };
  if (word(0) !== 0xfeedfacf) throw new Error('Expected thin 64-bit Mach-O.');
  const count = word(16), end = 32 + word(20);
  if (count > 4096 || end > bytes.length) throw new Error('Invalid Mach-O commands.');
  let cursor = 32, found;
  for (let index = 0; index < count; index++) {
    const command = word(cursor), size = word(cursor + 4), next = cursor + size;
    if (size < 8 || next > end) throw new Error('Invalid Mach-O command size.');
    if (command === 0x19) {
      if (size < 72) throw new Error('Truncated Mach-O segment.');
      const sections = word(cursor + 64);
      if (sections > Math.floor((size - 72) / 80)) throw new Error('Invalid Mach-O sections.');
      for (let section = 0; section < sections; section++) {
        const at = cursor + 72 + section * 80;
        if (bytes.subarray(at, at + 16).equals(Buffer.from('__magi_source\0\0\0'))) {
          if (found || !bytes.subarray(at + 16, at + 32).equals(Buffer.from('__TEXT\0\0\0\0\0\0\0\0\0\0')) || bytes.readBigUInt64LE(at + 40) !== 64n) throw new Error('Invalid source binding section.');
          const offset = word(at + 48);
          found = bytes.subarray(offset, offset + 64).toString('utf8');
          if (!/^[a-f0-9]{64}$/.test(found)) throw new Error('Invalid source digest.');
        }
      }
    }
    cursor = next;
  }
  if (!found) throw new Error('Missing source binding section.');
  return found;
}
function run(command, args, timeout = 120000) {
  const result = spawnSync(command, args, { env: { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', LANG: 'en_US.UTF-8' }, stdio: ['ignore', 'pipe', 'pipe'], timeout, maxBuffer: 16 * 1024 });
  if (result.error || result.status !== 0) throw new Error(`Extraction resource command failed: ${path.basename(command)}.`);
  return result;
}
function regular(file, limit) {
  const metadata = fs.lstatSync(file);
  if (!metadata.isFile() || metadata.uid !== process.getuid() || metadata.nlink !== 1 || (metadata.mode & 0o022) || metadata.size > limit) throw new Error('Untrusted extraction input.');
  const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
  const opened = fs.fstatSync(fd);
  if (opened.ino !== metadata.ino || opened.dev !== metadata.dev) { fs.closeSync(fd); throw new Error('Extraction input changed.'); }
  const bytes = Buffer.alloc(limit + 1);
  let length = 0;
  while (length < bytes.length) { const count = fs.readSync(fd, bytes, length, bytes.length - length, null); if (!count) break; length += count; }
  if (length > limit) { fs.closeSync(fd); throw new Error('Extraction input grew past its limit.'); }
  return { fd, metadata, bytes: bytes.subarray(0, length) };
}
function unchanged(file, held) {
  const now = fs.lstatSync(file), opened = fs.fstatSync(held.fd);
  for (const key of ['dev', 'ino', 'size', 'mtimeMs', 'ctimeMs', 'mode', 'uid', 'nlink']) if (now[key] !== held.metadata[key] || opened[key] !== held.metadata[key]) throw new Error('Extraction input changed during compilation.');
}
function syncDirectory(directory) { const fd = fs.openSync(directory, 'r'); try { fs.fsyncSync(fd); } finally { fs.closeSync(fd); } }
export function publishExtractionTree(output, destination) {
  const owned = fs.lstatSync(output);
  if (!owned.isDirectory() || owned.uid !== process.getuid() || fs.lstatSync(destination, { throwIfNoEntry: false })) throw new Error('Extraction publication directories are unsafe.');
  // macOS requires a writable directory for rename; admission rejects the brief unsealed generation.
  fs.chmodSync(output, 0o700);
  fs.renameSync(output, destination);
  try { fs.chmodSync(destination, 0o555); syncDirectory(destination); syncDirectory(path.dirname(destination)); }
  catch (error) {
    const current = fs.lstatSync(destination);
    if (current.ino !== owned.ino || current.dev !== owned.dev) throw new Error('Published extraction custody changed during sealing.');
    fs.chmodSync(destination, 0o700); fs.renameSync(destination, output);
    throw error;
  }
}
export function buildExtractionResource() {
  if (process.platform !== 'darwin' || !['arm64', 'x64'].includes(process.arch)) throw new Error('Extraction resources require supported macOS.');
  const identity = process.env.APPLE_SIGNING_IDENTITY;
  if (identity !== 'Developer ID Application: SEUNGWOO LEE (95B7J2U49K)') throw new Error('The approved Developer ID signing identity is required.');
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const parent = path.join(root, '.local/native-build');
  const destination = path.join(parent, 'extraction');
  run(process.execPath, [path.join(root, 'scripts/assert-repo-local-paths.cjs'), root, parent, destination, path.join(root, 'src-tauri/native/extract.swift')]);
  fs.mkdirSync(parent, { recursive: true, mode: 0o700 });
  const free = fs.statfsSync(parent);
  if (free.bavail * free.bsize < 512 * 1024 * 1024) throw new Error('Extraction compilation requires 512 MiB of free staging space.');
  const lock = path.join(parent, '.extraction-build.lock');
  fs.mkdirSync(lock, { mode: 0o700 });
  const lockIdentity = fs.lstatSync(lock);
  const stage = path.join(parent, `.extraction-build-${randomUUID()}`);
  let held, stageIdentity;
  try {
    if (fs.existsSync(destination) || fs.lstatSync(destination, { throwIfNoEntry: false })) throw new Error('An extraction generation already exists; admission must verify it instead of overwriting it.');
    fs.mkdirSync(stage, { mode: 0o700 }); stageIdentity = fs.lstatSync(stage);
    const source = path.join(root, 'src-tauri/native/extract.swift');
    held = regular(source, 1024 * 1024);
    const sourceDigest = digest(held.bytes), snapshot = path.join(stage, 'extract.swift'), binding = path.join(stage, 'source.sha256');
    fs.writeFileSync(snapshot, held.bytes, { flag: 'wx', mode: 0o444 });
    fs.writeFileSync(binding, sourceDigest, { flag: 'wx', mode: 0o444 });
    const output = path.join(stage, 'extraction'); fs.mkdirSync(output, { mode: 0o700 });
    const executable = path.join(output, 'magi-extract');
    run('/usr/bin/xcrun', ['swiftc', '-target', `${process.arch === 'arm64' ? 'arm64' : 'x86_64'}-apple-macosx13.0`, '-module-cache-path', path.join(stage, 'module-cache'), '-O', '-Xlinker', '-sectcreate', '-Xlinker', '__TEXT', '-Xlinker', '__magi_source', '-Xlinker', binding, snapshot, '-o', executable], 300000);
    unchanged(source, held);
    if (!fs.readFileSync(snapshot).equals(held.bytes) || fs.readFileSync(binding, 'utf8') !== sourceDigest) throw new Error('Captured extraction source changed.');
    run('/usr/bin/codesign', ['--force', '--options', 'runtime', '--timestamp', '--sign', identity, executable]);
    run('/usr/bin/codesign', ['--verify', '--strict', '--verbose=0', executable]);
    const display = run('/usr/bin/codesign', ['--display', '--verbose=4', executable]);
    if (!display.stderr.toString('utf8').split(/\r?\n/).includes('TeamIdentifier=95B7J2U49K')) throw new Error('Extraction signer Team ID differs from the runtime trust contract.');
    const binary = regular(executable, 32 * 1024 * 1024);
    try {
      if (sourceBinding(binary.bytes) !== sourceDigest) throw new Error('Signed extraction source binding differs from the current source.');
      fs.writeFileSync(path.join(output, 'magi-extract.sha256'), `${digest(binary.bytes)}\n`, { flag: 'wx', mode: 0o444 });
      fs.fsyncSync(binary.fd);
    } finally { fs.closeSync(binary.fd); }
    fs.chmodSync(executable, 0o555); fs.chmodSync(output, 0o555);
    const checksum = fs.openSync(path.join(output, 'magi-extract.sha256'), 'r'); try { fs.fsyncSync(checksum); } finally { fs.closeSync(checksum); }
    syncDirectory(output); unchanged(source, held);
    if (fs.lstatSync(lock).ino !== lockIdentity.ino || fs.lstatSync(destination, { throwIfNoEntry: false })) throw new Error('Extraction publication ownership changed.');
    publishExtractionTree(output, destination);
    console.log('Published a signed, source-bound extraction generation for independent resource admission.');
  } finally {
    if (held) fs.closeSync(held.fd);
    if (stageIdentity) {
      const current = fs.lstatSync(stage, { throwIfNoEntry: false });
      if (!current?.isDirectory() || current.ino !== stageIdentity.ino || current.dev !== stageIdentity.dev) throw new Error('Extraction staging custody changed; refusing cleanup.');
      const clear = directory => { for (const entry of fs.readdirSync(directory, { withFileTypes: true })) { if (entry.isSymbolicLink()) throw new Error('Extraction staging contains an unexpected symbolic link.'); if (entry.isDirectory()) clear(path.join(directory, entry.name)); } fs.chmodSync(directory, 0o700); };
      clear(stage); fs.rmSync(stage, { recursive: true });
    }
    const currentLock = fs.lstatSync(lock, { throwIfNoEntry: false });
    if (currentLock?.ino === lockIdentity.ino && currentLock.dev === lockIdentity.dev) fs.rmdirSync(lock);
  }
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) buildExtractionResource();
