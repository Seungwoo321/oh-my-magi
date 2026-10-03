#!/usr/bin/env bash
set -euo pipefail
: "${HOME:?HOME must be set}"

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"
requested_env_file=''
notary_profile=''

usage() {
  cat <<'USAGE'
Usage: scripts/build-macos-release.sh [--env-file PATH] --notary-profile NAME

Build and validate the signed macOS app and notarized DMG using the shared Apple
environment by default. An explicit --env-file replaces all inherited APPLE_*
values. Paths beginning with ~ are resolved from HOME.

The notary profile is a macOS Keychain profile name created with
`xcrun notarytool store-credentials`; its password is entered at Apple's
protected prompt and is never passed as a command argument by this script.
USAGE
}

error() {
  printf '%s\n' "$1" >&2
}

while (($#)); do
  case "$1" in
    --env-file)
      if [[ -n "$requested_env_file" || $# -lt 2 || "$2" == --* ]]; then
        error 'Specify --env-file once, followed by a path.'
        exit 2
      fi
      requested_env_file="$2"
      shift 2
      ;;
    --notary-profile)
      if [[ -n "$notary_profile" || $# -lt 2 || "$2" == --* ]]; then
        error 'Specify --notary-profile once, followed by a Keychain profile name.'
        exit 2
      fi
      notary_profile="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      error 'Unknown option. Run scripts/build-macos-release.sh --help.'
      exit 2
      ;;
  esac
done

if [[ -z "$notary_profile" ]]; then
  error 'A Keychain notary profile is required through --notary-profile.'
  exit 2
fi

expand_home_path() {
  case "$1" in
    \~) printf '%s\n' "$HOME" ;;
    \~/*) printf '%s/%s\n' "$HOME" "${1#\~/}" ;;
    \~*)
      error 'Only ~ and ~/ paths are supported in profile paths.'
      return 2
      ;;
    *) printf '%s\n' "$1" ;;
  esac
}

if [[ -n "$requested_env_file" ]]; then
  env_file="$(expand_home_path "$requested_env_file")"
else
  env_file="$HOME/.config/tauri-signing/apple.env"
fi

if [[ ! -f "$env_file" || ! -r "$env_file" ]]; then
  error 'The selected Apple environment file is missing or unreadable.'
  exit 1
fi

clear_inherited_apple_environment() {
  local name
  while IFS='=' read -r name _; do
    case "$name" in
      APPLE_*) unset "$name" ;;
    esac
  done < <(env)
  unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PATH \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD
}

clear_inherited_apple_environment
set -a
# shellcheck disable=SC1090
if ! source "$env_file" >/dev/null 2>&1; then
  set +a
  error 'The selected Apple environment file could not be loaded.'
  exit 1
fi
set +a

# App-specific updater keys are intentionally not part of Apple release signing.
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PATH \
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD

if [[ -z "${APPLE_NOTARY_PROFILE:-}" ]]; then
  export APPLE_NOTARY_PROFILE="$notary_profile"
elif [[ "$APPLE_NOTARY_PROFILE" != "$notary_profile" ]]; then
  error 'The selected Keychain profile conflicts with APPLE_NOTARY_PROFILE.'
  exit 1
fi

missing=()
for name in APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  if [[ -z "${!name:-}" ]]; then
    missing+=("$name")
  fi
done
if ((${#missing[@]})); then
  error 'The selected Apple profile is incomplete; missing variable names:'
  printf '  %s\n' "${missing[@]}" >&2
  exit 1
fi

if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then
  error 'The packaged Codex ACP resource currently supports macOS Apple Silicon builds only.'
  exit 1
fi

for tool in codesign node pnpm spctl stat xcrun; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    error "Missing required release command: $tool"
    exit 1
  fi
done

product_name="$(node -e 'const fs=require("node:fs");process.stdout.write(JSON.parse(fs.readFileSync(process.argv[1],"utf8")).productName)' \
  "$repo_root/src-tauri/tauri.conf.json")"
bundle_root="$repo_root/src-tauri/target/release/bundle"
app_path="$bundle_root/macos/$product_name.app"
dmg_dir="$bundle_root/dmg"
build_started="$(date +%s)"

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$HOME=/build"

printf 'Loading one Apple signing profile: %s\n' \
  "$([[ -n "$requested_env_file" ]] && printf 'explicit --env-file' || printf 'default shared Apple environment')"
printf 'Building signed macOS app and DMG. This command does not publish a release.\n'
(cd -- "$repo_root" && pnpm tauri build --bundles app,dmg)

if [[ ! -d "$app_path" ]]; then
  error 'Tauri did not produce the expected macOS app bundle.'
  exit 1
fi

dmg_path=''
newest_mtime=0
for candidate in "$dmg_dir"/*.dmg; do
  [[ -f "$candidate" ]] || continue
  modified_at="$(stat -f '%m' "$candidate")"
  if ((modified_at >= build_started && modified_at >= newest_mtime)); then
    newest_mtime="$modified_at"
    dmg_path="$candidate"
  fi
done
if [[ -z "$dmg_path" ]]; then
  error 'Tauri did not produce a new DMG for this build.'
  exit 1
fi

codesign --verify --deep --strict "$app_path"
xcrun stapler validate "$app_path"
spctl --assess --type execute --verbose "$app_path"

# Use the Keychain profile so the app-specific password never appears in argv.
xcrun notarytool submit "$dmg_path" \
  --keychain-profile "$notary_profile" --wait --timeout 30m
xcrun stapler staple "$dmg_path"
xcrun stapler validate "$dmg_path"
spctl --assess --type install --verbose "$dmg_path"

printf 'Release artifacts passed signature, notarization, staple, and Gatekeeper checks: %s, %s\n' \
  "${app_path##*/}" "${dmg_path##*/}"
