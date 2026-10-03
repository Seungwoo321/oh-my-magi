import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createPrivateKey, createPublicKey, sign, verify } from 'node:crypto';

function protectedFile(file, mode) {
  const entry = fs.lstatSync(file);
  if (!entry.isFile() || entry.isSymbolicLink() || (entry.mode & 0o777) !== mode || entry.uid !== process.getuid()) throw new Error('Signing material ownership or permissions are invalid.');
  return fs.readFileSync(file, 'utf8');
}

export function loadSigningEnvironment({ envFile, notaryProfile } = {}) {
  const root = path.join(os.homedir(), '.config/tauri-signing');
  for (const directory of [root, path.join(root, 'oh-my-magi')]) {
    const entry = fs.lstatSync(directory);
    if (!entry.isDirectory() || entry.isSymbolicLink() || (entry.mode & 0o777) !== 0o700 || entry.uid !== process.getuid()) throw new Error('Signing directory ownership or permissions are invalid.');
  }
  const allowed = new Set(['APPLE_SIGNING_IDENTITY', 'APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID', 'APPLE_API_ISSUER', 'APPLE_API_KEY', 'APPLE_API_KEY_PATH', 'APPLE_KEYCHAIN_PROFILE', 'APPLE_NOTARY_PROFILE']);
  const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('APPLE_') && !key.startsWith('TAURI_SIGNING_')));
  const selected = envFile?.replace(/^~(?=\/|$)/, os.homedir()) ?? path.join(root, 'apple.env');
  const appleFile = path.resolve(selected);
  protectedFile(appleFile, 0o600);
  const appleParent = fs.lstatSync(path.dirname(appleFile));
  if ((appleParent.mode & 0o777) !== 0o700 || appleParent.uid !== process.getuid()) throw new Error('Apple signing environment directory permissions are invalid.');
  for (const line of protectedFile(appleFile, 0o600).split(/\r?\n/)) {
    if (!line.trim() || line.trim().startsWith('#')) continue;
    const match = line.match(/^\s*(?:export\s+)?([A-Z][A-Z0-9_]*)\s*=\s*(.*?)\s*$/);
    if (!match || !allowed.has(match[1])) throw new Error('The Apple signing environment contains an unsupported assignment.');
    let value = match[2];
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) value = value.slice(1, -1);
    if (value.includes('\0') || value.includes('\n')) throw new Error('The signing environment contains an invalid value.');
    environment[match[1]] = value;
  }
  const configuredProfile = environment.APPLE_NOTARY_PROFILE ?? environment.APPLE_KEYCHAIN_PROFILE;
  if (notaryProfile && configuredProfile && configuredProfile !== notaryProfile) throw new Error('The selected notarization profile conflicts with the signing environment.');
  environment.APPLE_KEYCHAIN_PROFILE = notaryProfile ?? configuredProfile;
  if (!environment.APPLE_SIGNING_IDENTITY || !environment.APPLE_TEAM_ID) throw new Error('The Apple signing profile requires identity and Team ID.');
  const app = path.join(root, 'oh-my-magi');
  const updater = path.join(app, 'updater.key');
  protectedFile(updater, 0o600);
  const privateKey = createPrivateKey(protectedFile(path.join(app, 'release-manifest-private.pem'), 0o600));
  const publicKey = createPublicKey(protectedFile(path.join(app, 'release-manifest-public.pem'), 0o644));
  if (privateKey.asymmetricKeyType !== 'ed25519' || publicKey.asymmetricKeyType !== 'ed25519' || !verify(null, Buffer.from('release-key-verification'), publicKey, sign(null, Buffer.from('release-key-verification'), privateKey))) throw new Error('The manifest signing key pair is invalid.');
  environment.TAURI_SIGNING_PRIVATE_KEY = updater;
  environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = '';
  return { environment, privateKey, publicKey, appKeyDirectory: app };
}
