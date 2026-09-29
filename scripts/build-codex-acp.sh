#!/usr/bin/env bash
set -euo pipefail

readonly codex_acp_package='@agentclientprotocol/codex-acp'
readonly codex_acp_version='1.13.1'
readonly codex_acp_commit='b1b8490cd165c18626dc3fe83836cdacdef94cd3'
readonly codex_acp_integrity='sha512-NAbXTb6GRReox7B+8RN9VRB+sKUqFJh5Vg7Ex7RskYCO8EsYPJKN1WJvN7mOqjCKKK9lVDrNxtP7Bp/OUKPyAg=='
readonly codex_version='0.156.1'
readonly codex_integrity='sha512-nI1iVl/n2SO2lSvlwEsJx63zdSI4C4Me2gR7AG0OWMJiGSakz2tY2hx43E39Zq5aEoeB5bZjJXzp5Sqhog6vyA=='

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
target_arch="${1:-$(uname -m)}"
case "$target_arch" in
  arm64|aarch64)
    target_arch='arm64'
    codex_target_triple='aarch64-apple-darwin'
    codex_platform_package='@openai/codex-darwin-arm64'
    codex_platform_version='0.156.1-darwin-arm64'
    codex_platform_integrity='sha512-Jg6wbdV+wmMZczhwE74GSxOYEZlViKXn6KyCw/yfrz3PAKFD14xljuPopmdhWC1+8IKU2WdN5fdmXNPt2q4HPA=='
    ;;
  x86_64|x64)
    target_arch='x64'
    codex_target_triple='x86_64-apple-darwin'
    codex_platform_package='@openai/codex-darwin-x64'
    codex_platform_version='0.156.1-darwin-x64'
    codex_platform_integrity='sha512-BVjqNOoltWrnNUrgMRepvDIIBmd4XY+ikAE4pYVHlrwiizNlEWQuCQOTrFMJKp54LFRG8jDTjLZyYQ1QrliuOg=='
    ;;
  *)
    printf 'Usage: %s [arm64|x64]\n' "$0" >&2
    exit 2
    ;;
esac

if [[ "$(uname -s)" != 'Darwin' ]]; then
  printf 'This pinned build is supported only on macOS.\n' >&2
  exit 1
fi
host_arch="$(uname -m)"
case "$host_arch" in
  arm64|aarch64) host_arch='arm64' ;;
  x86_64|x64) host_arch='x64' ;;
esac
if [[ "$target_arch" != "$host_arch" ]]; then
  printf 'Build only the current macOS architecture: %s.\n' "$host_arch" >&2
  exit 1
fi

for required_command in git node npm bun shasum; do
  if ! command -v "$required_command" >/dev/null 2>&1; then
    printf 'Missing required command: %s\n' "$required_command" >&2
    exit 1
  fi
done

mkdir -p "$repo_root/tmp" "$repo_root/.local/provider-build"
output_dir="$repo_root/.local/provider-build/codex-acp-$codex_acp_version/darwin-$target_arch"
version_dir="$(dirname "$output_dir")"
staging_dir="$version_dir/.darwin-$target_arch-candidate-$$"
if [[ -e "$output_dir" || -e "$staging_dir" ]]; then
  printf 'Refusing to overwrite an existing provider candidate.\n' >&2
  exit 1
fi
mkdir -p "$version_dir"
temporary_dir="$(mktemp -d "$repo_root/tmp/codex-acp-build.XXXXXX")"
cleanup() {
  rm -rf "$temporary_dir"
  if [[ -d "$staging_dir" ]]; then
    rm -rf "$staging_dir"
  fi
}
trap cleanup EXIT
readonly npm_cache="$temporary_dir/npm-cache"

checkout="$temporary_dir/source"
git clone --filter=blob:none --no-checkout https://github.com/agentclientprotocol/codex-acp.git "$checkout"
git -C "$checkout" fetch --depth=1 origin "$codex_acp_commit"
git -C "$checkout" checkout --detach FETCH_HEAD
actual_commit="$(git -C "$checkout" rev-parse HEAD)"
if [[ "$actual_commit" != "$codex_acp_commit" ]]; then
  printf 'Unexpected codex-acp source commit: %s\n' "$actual_commit" >&2
  exit 1
fi

node - "$checkout/package.json" "$checkout/package-lock.json" "$codex_platform_package" "$codex_platform_version" "$codex_platform_integrity" <<'NODE'
const fs = require('node:fs');
const [packagePath, lockPath, platformPackage, platformVersion, platformIntegrity] = process.argv.slice(2);
const pkg = JSON.parse(fs.readFileSync(packagePath, 'utf8'));
const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
const expected = {
  packageName: '@agentclientprotocol/codex-acp',
  packageVersion: '1.13.1',
  codexVersion: '0.156.1',
  codexIntegrity: 'sha512-nI1iVl/n2SO2lSvlwEsJx63zdSI4C4Me2gR7AG0OWMJiGSakz2tY2hx43E39Zq5aEoeB5bZjJXzp5Sqhog6vyA==',
};
const bundledCodex = lock.packages?.['node_modules/@openai/codex'];
const platformCodex = lock.packages?.[`node_modules/${platformPackage}`];
if (pkg.name !== expected.packageName || pkg.version !== expected.packageVersion) {
  throw new Error('The pinned checkout package identity does not match the approved profile.');
}
if (pkg.dependencies?.['@openai/codex'] !== '^0.156.1'
  || lock.packages?.['']?.version !== expected.packageVersion
  || bundledCodex?.version !== expected.codexVersion
  || bundledCodex?.integrity !== expected.codexIntegrity
  || platformCodex?.version !== platformVersion
  || platformCodex?.integrity !== platformIntegrity) {
  throw new Error('The bundled Codex dependency does not match the pinned profile.');
}
NODE

pack_json="$temporary_dir/npm-pack.json"
npm pack "$codex_acp_package@$codex_acp_version" --json --pack-destination "$temporary_dir" --cache "$npm_cache" > "$pack_json"
node - "$pack_json" "$codex_acp_integrity" <<'NODE'
const fs = require('node:fs');
const [manifestPath, expectedIntegrity] = process.argv.slice(2);
const [pack] = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
if (!pack || pack.integrity !== expectedIntegrity) {
  throw new Error('The registry package integrity does not match the pinned profile.');
}
NODE

(
  cd "$checkout"
  npm ci --cache "$npm_cache" --no-audit --no-fund
  npm cache clean --force --cache "$npm_cache" >/dev/null
  npm run "bundle:darwin-$target_arch"
)

binary="$checkout/dist/bin/codex-acp-$target_arch-darwin"
if [[ ! -f "$binary" ]]; then
  printf 'Pinned build did not produce the expected macOS executable.\n' >&2
  exit 1
fi

mkdir -p "$staging_dir"
chmod 755 "$binary"
mv "$binary" "$staging_dir/codex-acp"
artifact_sha256="$(shasum -a 256 "$staging_dir/codex-acp" | awk '{ print $1 }')"
codex_vendor="$checkout/node_modules/$codex_platform_package/vendor/$codex_target_triple"
if [[ ! -d "$codex_vendor" || ! -f "$codex_vendor/bin/codex" || ! -f "$codex_vendor/codex-path/rg" ]]; then
  printf 'Pinned Codex package is missing required native runtime files.\n' >&2
  exit 1
fi
mkdir -p "$staging_dir/vendor"
mv "$codex_vendor" "$staging_dir/vendor/$codex_target_triple"
codex_executable="$staging_dir/vendor/$codex_target_triple/bin/codex"
ripgrep_executable="$staging_dir/vendor/$codex_target_triple/codex-path/rg"
chmod 755 "$codex_executable" "$ripgrep_executable"
codex_executable_sha256="$(shasum -a 256 "$codex_executable" | awk '{ print $1 }')"
ripgrep_sha256="$(shasum -a 256 "$ripgrep_executable" | awk '{ print $1 }')"
cat > "$staging_dir/build-manifest.json" <<EOF
{
  "schema_version": 2,
  "provider": "codex-acp",
  "package": "$codex_acp_package",
  "package_version": "$codex_acp_version",
  "package_integrity": "$codex_acp_integrity",
  "source_commit": "$codex_acp_commit",
  "bundled_codex_package": "@openai/codex",
  "bundled_codex_version": "$codex_version",
  "bundled_codex_integrity": "$codex_integrity",
  "codex_platform_package": "$codex_platform_package",
  "codex_platform_version": "$codex_platform_version",
  "codex_platform_integrity": "$codex_platform_integrity",
  "codex_target_triple": "$codex_target_triple",
  "target": "darwin-$target_arch",
  "artifact_sha256": "$artifact_sha256",
  "codex_executable_sha256": "$codex_executable_sha256",
  "ripgrep_sha256": "$ripgrep_sha256",
  "status": "built_unadmitted"
}
EOF

if [[ -e "$output_dir" ]]; then
  printf 'Refusing to overwrite an existing provider candidate: %s\n' "$output_dir" >&2
  exit 1
fi
mv "$staging_dir" "$output_dir"

printf 'Built candidate: %s\n' "$output_dir/codex-acp"
printf 'SHA-256: %s\n' "$artifact_sha256"
printf 'The candidate is not enabled or wired into the application.\n'
