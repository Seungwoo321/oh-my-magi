import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { loadSigningEnvironment } from './load-signing-env.mjs';

const options = process.argv.slice(2);
const option = (name) => { const index = options.indexOf(name); if (index < 0) return undefined; if (!options[index + 1] || options[index + 1].startsWith('--')) throw new Error(`Missing ${name} value.`); return options[index + 1]; };
const { environment } = loadSigningEnvironment({ envFile: option('--env-file'), notaryProfile: option('--notary-profile') });
const localSigned = process.argv.includes('--local-signed');
const buildEnvironment = { ...environment };
for (const key of ['APPLE_ID', 'APPLE_PASSWORD', 'APPLE_API_ISSUER', 'APPLE_API_KEY', 'APPLE_API_KEY_PATH', 'APPLE_KEYCHAIN_PROFILE', 'APPLE_NOTARY_PROFILE']) delete buildEnvironment[key];
environment.CARGO_BUILD_JOBS = '2';
buildEnvironment.CARGO_BUILD_JOBS = '2';
buildEnvironment.RUSTFLAGS = `${buildEnvironment.RUSTFLAGS ?? ''} --remap-path-prefix=${process.env.HOME}/.cargo=/cargo --remap-path-prefix=${process.env.HOME}=/build`;
const root = process.cwd();
const evidence = path.join(root, '.local/release');
fs.mkdirSync(evidence, { recursive: true });
const staging = fs.mkdtempSync(path.join(evidence, 'build-'));
buildEnvironment.CARGO_TARGET_DIR = staging;
const stagedBundle = path.join(staging, 'release/bundle');
const publishedBundle = path.join(root, 'src-tauri/target/release/bundle');
function run(command, args, transcript, childEnvironment = buildEnvironment) {
  const fd = fs.openSync(path.join(evidence, transcript), 'w', 0o600);
  const result = spawnSync(command, args, { cwd: root, env: childEnvironment, stdio: ['ignore', fd, fd] });
  fs.closeSync(fd);
  return result.status;
}
try {
run('security', ['find-identity', '-v', '-p', 'codesigning'], 'signing-identities.log');
if (environment.APPLE_TEAM_ID !== '95B7J2U49K' || environment.APPLE_SIGNING_IDENTITY !== 'Developer ID Application: SEUNGWOO LEE (95B7J2U49K)') throw new Error('The configured Apple signing identity differs from the approved identity.');
if (run('pnpm', ['tauri', 'build'], localSigned ? 'local-signed-build.log' : 'signed-build.log', buildEnvironment) !== 0) throw new Error('The signed build failed; inspect the local release transcript.');
const app = path.join(stagedBundle, 'macos/MAGI CONSOLE.app');
if (run('codesign', ['--verify', '--deep', '--strict', '--verbose=2', app], 'codesign-verification.log') !== 0) throw new Error('The macOS code signature is invalid.');
run('codesign', ['--display', '--verbose=4', app], 'codesign-identity.log');
if (!localSigned) {
  const dmg = path.join(stagedBundle, `dmg/MAGI CONSOLE_${JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8')).version}_${process.arch === 'arm64' ? 'aarch64' : 'x64'}.dmg`);
  if (run('python3', ['scripts/notarize-release.py', 'submit', dmg], 'notarization-receipt.json', environment) !== 0) throw new Error('Apple notarization did not accept the signed artifact. Inspect the safe local receipt.');
  if (run('xcrun', ['stapler', 'staple', app], 'app-staple.log') !== 0 || run('xcrun', ['stapler', 'staple', dmg], 'dmg-staple.log') !== 0) throw new Error('Notarization tickets could not be attached.');
  if (run('xcrun', ['stapler', 'validate', dmg], 'dmg-stapler-verification.log') !== 0 || run('spctl', ['--assess', '--type', 'install', '--verbose=4', dmg], 'dmg-gatekeeper-verification.log') !== 0) throw new Error('The signed DMG failed notarization or Gatekeeper verification.');
  const archive = `${app}.tar.gz`;
  const signerEnvironment = { ...buildEnvironment };
  delete signerEnvironment.TAURI_SIGNING_PRIVATE_KEY;
  fs.rmSync(`${archive}.sig`, { force: true });
  if (run('tar', ['-czf', archive, '-C', path.dirname(app), path.basename(app)], 'notarized-updater-archive.log') !== 0 || run('pnpm', ['tauri', 'signer', 'sign', '--private-key-path', environment.TAURI_SIGNING_PRIVATE_KEY, archive], 'notarized-updater-signature.log', signerEnvironment) !== 0) throw new Error('The notarized updater archive could not be signed.');
}
const gatekeeper = run('spctl', ['--assess', '--type', 'execute', '--verbose=4', app], 'gatekeeper-verification.log');
const staple = run('xcrun', ['stapler', 'validate', app], 'stapler-verification.log');
fs.writeFileSync(path.join(evidence, 'build-verification.json'), JSON.stringify({ app: path.join(publishedBundle, 'macos/MAGI CONSOLE.app'), archive: path.join(publishedBundle, 'macos/MAGI CONSOLE.app.tar.gz'), target: process.arch === 'arm64' ? 'darwin-aarch64' : 'darwin-x86_64', codeSignatureValid: true, gatekeeperAccepted: gatekeeper === 0, notarizationStapleValid: staple === 0, localSigned }, null, 2) + '\n');
if (gatekeeper !== 0 || staple !== 0) { console.log('Signed macOS application built; notarization and Gatekeeper acceptance remain unverified.'); process.exitCode = 1; }
else {
  const previous = path.join(evidence, 'previous-bundle');
  if (fs.existsSync(previous)) throw new Error('Accept or recover the previous release before publishing another bundle.');
  if (run('python3', ['scripts/publish-release-bundle.py', stagedBundle, publishedBundle], 'bundle-publication.log') !== 0) throw new Error('Verified release publication failed. The previous published bundle remains available.');
  if (fs.existsSync(stagedBundle)) {
    fs.renameSync(stagedBundle, previous);
  }
  fs.writeFileSync(path.join(evidence, 'publication.json'), JSON.stringify({ publishedBundle, retainedPreviousBundle: fs.existsSync(previous) ? previous : null, releaseCondition: 'Remove previous-bundle after the published release is accepted; preserve it only for rollback.' }, null, 2) + '\n');
  console.log('Signed macOS application published; code signature, notarization and Gatekeeper verification passed.');
}

} finally {
  fs.rmSync(staging, { recursive: true, force: true });
}
