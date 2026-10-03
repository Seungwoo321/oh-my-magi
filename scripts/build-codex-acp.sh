#!/usr/bin/env bash
set -euo pipefail

readonly codex_acp_package='@agentclientprotocol/codex-acp'
readonly codex_acp_version='1.13.1'
readonly codex_acp_commit='b1b8490cd165c18626dc3fe83836cdacdef94cd3'
readonly codex_acp_integrity='sha512-NAbXTb6GRReox7B+8RN9VRB+sKUqFJh5Vg7Ex7RskYCO8EsYPJKN1WJvN7mOqjCKKK9lVDrNxtP7Bp/OUKPyAg=='
readonly codex_acp_previous_adapter_patch_id='magi-home-lock-auth-progress-v6'
readonly codex_version='0.156.1'
readonly codex_integrity='sha512-nI1iVl/n2SO2lSvlwEsJx63zdSI4C4Me2gR7AG0OWMJiGSakz2tY2hx43E39Zq5aEoeB5bZjJXzp5Sqhog6vyA=='
readonly apple_signing_team_identifier='95B7J2U49K'
readonly bun_version='1.4.2'
readonly public_ca_source_url='https://curl.se/ca/cacert-2026-09-25.pem'
readonly public_ca_sha256='a41b5d356aea97a529fe27e0f7316d2f9d946d75927476cf9cf1b90637d00505'
readonly codex_source_commit='b412ff32c417f855c2b2d1581b77058eed87c84b'
readonly codex_source_sha256='1ac6a92e7318b8acf3d767170c5c5e6dceeffdc074c73b1c5d422b46f0de4daf'
readonly codex_source_patch_id='codex-http-ca-preserve-backend-v1'
readonly codex_source_patch_sha256='b08f4099725b6394e5657691e10d2dc8d9696cd119623fa6155db28a70d76d54'
readonly codex_source_lock_sha256='d722f05fc760bcd1f5749ec452452d81058458b788df3b765b80500d757eba4a'
provider_bun="${MAGI_BUN_EXECUTABLE:-bun}"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
codex_acp_adapter_patch_id_file="$repo_root/src-tauri/crates/magi-provider/src/codex_acp_adapter_patch_id.txt"
if [[ ! -f "$codex_acp_adapter_patch_id_file" || -L "$codex_acp_adapter_patch_id_file" ]]; then
  printf 'The Codex ACP adapter patch identity is missing or not a regular file.\n' >&2
  exit 1
fi
codex_acp_adapter_patch_id="$(cat "$codex_acp_adapter_patch_id_file")"
if [[ ! "$codex_acp_adapter_patch_id" =~ ^magi-home-lock-auth-progress-v[0-9]+$ ]]; then
  printf 'The Codex ACP adapter patch identity has an invalid format.\n' >&2
  exit 1
fi
readonly codex_acp_adapter_patch_id
require_signature=false
candidate_only=false
target_arch=''
while (($#)); do
  case "$1" in
    --candidate-only)
      if [[ "$candidate_only" == true ]]; then
        printf 'Candidate-only mode may be specified only once.\n' >&2
        exit 2
      fi
      candidate_only=true
      ;;
    --require-signature)
      if [[ "$require_signature" == true ]]; then
        printf 'The required-signature mode may be specified only once.\n' >&2
        exit 2
      fi
      require_signature=true
      ;;
    arm64|aarch64|x86_64|x64)
      if [[ -n "$target_arch" ]]; then
        printf 'Specify only one target architecture.\n' >&2
        exit 2
      fi
      target_arch="$1"
      ;;
    *)
      printf 'Usage: %s [--require-signature | --candidate-only] [arm64]\n' "$0" >&2
      exit 2
      ;;
  esac
  shift
done
if [[ "$candidate_only" == true && "$require_signature" == true ]]; then
  printf 'Candidate-only builds do not publish signed resources.\n' >&2
  exit 2
fi
target_arch="${target_arch:-$(uname -m)}"
case "$target_arch" in
  arm64|aarch64)
    target_arch='arm64'
    ;;
  x86_64|x64)
    printf 'Codex ACP packaging is supported only on Apple Silicon; no independently pinned Intel artifact is available.\n' >&2
    exit 1
    ;;
  *)
    printf 'Usage: %s [--require-signature | --candidate-only] [arm64]\n' "$0" >&2
    exit 2
    ;;
esac
if [[ "$require_signature" == true ]]; then
  if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    printf 'Packaged ACP builds require the shared Apple signing identity.\n' >&2
    exit 1
  fi
  if ! command -v codesign >/dev/null 2>&1; then
    printf 'Packaged ACP builds require macOS codesign.\n' >&2
    exit 1
  fi
fi
case "$target_arch" in
  arm64)
    codex_target_triple='aarch64-apple-darwin'
    codex_platform_package='@openai/codex-darwin-arm64'
    codex_platform_version='0.156.1-darwin-arm64'
    codex_platform_integrity='sha512-Jg6wbdV+wmMZczhwE74GSxOYEZlViKXn6KyCw/yfrz3PAKFD14xljuPopmdhWC1+8IKU2WdN5fdmXNPt2q4HPA=='
    codex_acp_upstream_artifact_sha256='69a7752a9092ea7734518e59ddcba808bcf4b216737539b50382005055b6aa01'
    ;;
  *)
    printf 'Usage: %s [--require-signature | --candidate-only] [arm64]\n' "$0" >&2
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
  x86_64|x64)
    printf 'Codex ACP packaging is supported only on Apple Silicon; no independently pinned Intel artifact is available.\n' >&2
    exit 1
    ;;
esac
if [[ "$target_arch" != "$host_arch" ]]; then
  printf 'Build only the current macOS architecture: %s.\n' "$host_arch" >&2
  exit 1
fi
if [[ -z "$codex_acp_upstream_artifact_sha256" ]]; then
  printf 'No independently pinned Codex ACP artifact digest exists for darwin-%s; refusing cache reuse or build.\n' "$target_arch" >&2
  exit 1
fi

for required_command in node shasum; do
  if ! command -v "$required_command" >/dev/null 2>&1; then
    printf 'Missing required command: %s\n' "$required_command" >&2
    exit 1
  fi
done

provider_build_root="$repo_root/.local/provider-build/codex-http-ca-preserve-backend-v1-package"
codex_runtime_root="$repo_root/.local/provider-build/$codex_source_patch_id"
output_dir="$provider_build_root/codex-acp-$codex_acp_version/darwin-$target_arch"
version_dir="$(dirname "$output_dir")"
staging_dir="$version_dir/.darwin-$target_arch-candidate-$$"
resource_root="$provider_build_root/codex-acp-tauri-resources"
resource_target="$resource_root/darwin-$target_arch"
resource_staging_target="$resource_root/.darwin-$target_arch-resource-$$"
resource_backup_target="$provider_build_root/.codex-acp-resource-backup-$target_arch-$$"
resource_backup_moved=false
minimum_build_space_kib=1572864
available_build_space_kib="$(df -Pk "$repo_root" | awk 'NR == 2 { print $4 }')"
if [[ ! "$available_build_space_kib" =~ ^[0-9]+$ ]]; then
  printf 'Could not determine available disk space for the provider build.\n' >&2
  exit 1
fi
if ((available_build_space_kib < minimum_build_space_kib)); then
  printf 'Provider build requires at least 1536 MiB free before staging; currently available: %s MiB. No build files were created.\n' \
    "$((available_build_space_kib / 1024))" >&2
  exit 1
fi

apply_adapter_home_lock_patch() {
  node - "$1" <<'NODE'
const fs = require('node:fs');
const sourcePath = process.argv[2];
let source = fs.readFileSync(sourcePath, 'utf8');
const replaceExactlyOnce = (before, after) => {
  const first = source.indexOf(before);
  if (first < 0 || source.indexOf(before, first + before.length) >= 0) {
    throw new Error('Pinned ACP home-lock patch anchor did not match exactly once.');
  }
  source = `${source.slice(0, first)}${after}${source.slice(first + before.length)}`;
};

replaceExactlyOnce(
  'import type {ChildProcessWithoutNullStreams} from "node:child_process";\n',
  'import type {ChildProcess, ChildProcessWithoutNullStreams} from "node:child_process";\n',
);
replaceExactlyOnce(
  '    const spawnEnv = env ?? process.env;\n\n    let codex: ChildProcessWithoutNullStreams;\n',
  '    const spawnEnv = env ?? process.env;\n'
    + '    if (spawnEnv[\'MAGI_PROVIDER_HOME_LOCK_FD\'] !== \'3\') {\n'
    + '        throw new Error("The profile-home lock descriptor is required.");\n'
    + '    }\n'
    + '    const stdio: (number | \'pipe\')[] = [\'pipe\', \'pipe\', \'pipe\', 3];\n'
    + '    const requireStandardStreams = (child: ChildProcess): ChildProcessWithoutNullStreams => {\n'
    + '        if (child.stdin === null || child.stdout === null || child.stderr === null) {\n'
    + '            child.kill();\n'
    + '            throw new Error("The Codex App Server standard streams could not be established.");\n'
    + '        }\n'
    + '        return child as ChildProcessWithoutNullStreams;\n'
    + '    };\n'
    + '\n'
    + '    let codex: ChildProcessWithoutNullStreams;\n',
);
replaceExactlyOnce(
  "spawn(codexPath, ['app-server'], { env: spawnEnv })",
  "requireStandardStreams(spawn(codexPath, ['app-server'], { env: spawnEnv, stdio }))",
);
replaceExactlyOnce(
  "spawn(process.execPath, [bundledCodexPath, 'app-server'], {env: spawnEnv})",
  "requireStandardStreams(spawn(process.execPath, [bundledCodexPath, 'app-server'], {env: spawnEnv, stdio}))",
);
for (const output of [
  'import type {ChildProcess, ChildProcessWithoutNullStreams} from "node:child_process";',
  "if (spawnEnv['MAGI_PROVIDER_HOME_LOCK_FD'] !== '3')",
  "const stdio: (number | 'pipe')[] = ['pipe', 'pipe', 'pipe', 3];",
  'const requireStandardStreams = (child: ChildProcess): ChildProcessWithoutNullStreams => {',
  "requireStandardStreams(spawn(codexPath, ['app-server'], { env: spawnEnv, stdio }))",
  "requireStandardStreams(spawn(process.execPath, [bundledCodexPath, 'app-server'], {env: spawnEnv, stdio}))",
]) {
  if (source.indexOf(output) < 0 || source.indexOf(output, source.indexOf(output) + output.length) >= 0) {
    throw new Error('Pinned ACP home-lock patch output did not match exactly once.');
  }
}

fs.writeFileSync(sourcePath, source);
NODE
}

apply_adapter_auth_exit_patch() {
  node - "$1" <<'NODE'
const fs = require('node:fs');
const sourcePath = process.argv[2];
let source = fs.readFileSync(sourcePath, 'utf8');
const replaceExactlyOnce = (before, after) => {
  const first = source.indexOf(before);
  if (first < 0 || source.indexOf(before, first + before.length) >= 0) {
    throw new Error('Pinned ACP authentication-exit patch anchor did not match exactly once.');
  }
  source = `${source.slice(0, first)}${after}${source.slice(first + before.length)}`;
};

replaceExactlyOnce(
  'function startAcpServer() {\n',
  'async function authenticateWithAppServerExitFailure<T>(\n'
    + '    authenticate: () => Promise<T>,\n'
    + '    codexProcess: ReturnType<typeof startCodexConnection>["process"],\n'
    + '): Promise<T> {\n'
    + '    const exitedError = (exitCode: number | null): acp.RequestError =>\n'
    + '        acp.RequestError.internalError(\n'
    + '            { code: "codex_app_server_exited", exitCode },\n'
    + '            exitCode === null\n'
    + '                ? "Codex App Server exited during subscription authentication. Retry after checking the Codex runtime."\n'
    + '                : `Codex App Server exited during subscription authentication (exit status ${exitCode}). Retry after checking the Codex runtime.`,\n'
    + '        );\n'
    + '    if (codexProcess.exitCode !== null || codexProcess.signalCode !== null) {\n'
    + '        throw exitedError(codexProcess.exitCode);\n'
    + '    }\n'
    + '    let removeExitListener = () => {};\n'
    + '    const processExited = new Promise<never>((_resolve, reject) => {\n'
    + '        const onExit = (exitCode: number | null) => reject(exitedError(exitCode));\n'
    + '        codexProcess.once("exit", onExit);\n'
    + '        removeExitListener = () => codexProcess.removeListener("exit", onExit);\n'
    + '    });\n'
    + '    try {\n'
    + '        try {\n'
    + '            return await Promise.race([Promise.resolve().then(authenticate), processExited]);\n'
    + '        } catch (error) {\n'
    + '            if (codexProcess.exitCode !== null || codexProcess.signalCode !== null) {\n'
    + '                throw exitedError(codexProcess.exitCode);\n'
    + '            }\n'
    + '            throw error;\n'
    + '        }\n'
    + '    } finally {\n'
    + '        removeExitListener();\n'
    + '    }\n'
    + '}\n'
    + '\n'
    + 'function startAcpServer() {\n',
);
replaceExactlyOnce(
  'return hostAuth.connect(ctx.params);',
  'return authenticateWithAppServerExitFailure(\n'
    + '                () => hostAuth!.connect(ctx.params),\n'
    + '                codexProcessState.connection.process,\n'
    + '            );',
);
replaceExactlyOnce(
  '.onRequest(acp.methods.agent.authenticate, (ctx) => getAgent().authenticate(ctx.params, ctx.requestId))',
  '.onRequest(acp.methods.agent.authenticate, () => { throw acp.RequestError.internalError("existing_subscription_required"); })',
);

for (const output of [
  'async function authenticateWithAppServerExitFailure<T>(',
  'code: "codex_app_server_exited", exitCode',
  'codexProcess.once("exit", onExit)',
  'authenticateWithAppServerExitFailure(',
]) {
  if (source.indexOf(output) < 0 || source.indexOf(output, source.indexOf(output) + output.length) >= 0) {
    throw new Error('Pinned ACP authentication-exit patch output did not match exactly once.');
  }
}

fs.writeFileSync(sourcePath, source);
NODE
}

apply_adapter_existing_subscription_patch() {
  node - "$1" <<'NODE'
const fs = require('node:fs');
const path = require('node:path');
const sourcePath = path.join(process.argv[2], 'index.ts');
let source = fs.readFileSync(sourcePath, 'utf8');
const replaceExactlyOnce = (before, after) => {
  const first = source.indexOf(before);
  if (first < 0 || source.indexOf(before, first + before.length) >= 0) {
    throw new Error('Pinned ACP existing-subscription patch anchor did not match exactly once.');
  }
  source = `${source.slice(0, first)}${after}${source.slice(first + before.length)}`;
};
replaceExactlyOnce(
  'import {z} from "zod";\n',
  'import {z} from "zod";\nimport {createHostAuth, hostAuthParams} from "./MagiHostAuth";\n',
);
replaceExactlyOnce(
  '    function createAgent(connection: acp.AgentContext): CodexAcpServer {\n',
  '    let hostAuth: ReturnType<typeof createHostAuth> | null = null;\n\n'
    + '    function createAgent(connection: acp.AgentContext): CodexAcpServer {\n'
    + '        hostAuth = createHostAuth(codexProcessState.connection.connection, connection);\n',
);
replaceExactlyOnce(
  '                    codexAcpServer = null;\n',
  '                    codexAcpServer = null;\n                    hostAuth = null;\n',
);
replaceExactlyOnce(
  '        .onRequest(acp.methods.agent.authenticate,',
  '        .onRequest("_magi/auth/connect", hostAuthParams, (ctx) => {\n'
    + '            if (!hostAuth) throw acp.RequestError.internalError("auth_connection_unavailable");\n'
    + '            return hostAuth.connect(ctx.params);\n'
    + '        })\n'
    + '        .onRequest(acp.methods.agent.authenticate,',
);
const hostSource = String.raw`import { z } from "zod";
import type { MessageConnection } from "vscode-jsonrpc";
import type { AgentContext } from "@agentclientprotocol/sdk";

export const hostAuthParams = z.object({
    accessToken: z.string().min(1).max(32768),
    chatgptAccountId: z.string().min(1).max(512),
    chatgptPlanType: z.string().max(128).nullable().optional(),
}).strict();

export function createHostAuth(connection: MessageConnection, host: AgentContext) {
    let accountId: string | null = null;
    let connected = false;
    let busy = false;
    connection.onRequest("account/chatgptAuthTokens/refresh", async (params: unknown) => {
        const request = z.object({ reason: z.literal("unauthorized"), previousAccountId: z.string().nullable().optional() }).strict().safeParse(params);
        if (!connected || !accountId || !request.success || (request.data.previousAccountId && request.data.previousAccountId !== accountId)) throw new Error("auth_refresh_required");
        try {
            const response = hostAuthParams.safeParse(await host.request("_magi/auth/refresh", request.data));
            if (!response.success || response.data.chatgptAccountId !== accountId) throw new Error("auth_refresh_required");
            return response.data;
        } catch { throw new Error("auth_refresh_required"); }
    });
    return {
        async connect(params: z.infer<typeof hostAuthParams>): Promise<{ authenticated: true }> {
            if (busy || connected) throw new Error("auth_connection_already_bound");
            busy = true;
            try {
                // Credential requests bypass the public event observer used by session diagnostics.
                const response = await connection.sendRequest<{ type: string }>("account/login/start", { type: "chatgptAuthTokens", ...params });
                if (response.type !== "chatgptAuthTokens") throw new Error("auth_login_response_mismatch");
                accountId = params.chatgptAccountId;
                connected = true;
                return { authenticated: true };
            } catch (error: unknown) {
                if (error instanceof Error && error.message === "auth_login_response_mismatch") throw new Error("auth_login_response_mismatch");
                const code = typeof error === "object" && error !== null && "code" in error ? (error as { code: unknown }).code : null;
                const category = code === -32602 ? "auth_login_invalid_params"
                    : code === -32601 ? "auth_login_method_unavailable"
                    : code === -32603 ? "auth_login_internal_failure"
                    : typeof code === "number" ? "auth_login_request_failed" : "auth_login_transport_failure";
                throw new Error(category);
            }
            finally { busy = false; }
        },
    };
}
`;
fs.writeFileSync(path.join(process.argv[2], 'MagiHostAuth.ts'), hostSource, {flag: 'wx'});
fs.writeFileSync(sourcePath, source);
NODE
}

verify_tauri_resource_mapping() {
  node - "$repo_root" "$resource_root" <<'NODE'
const fs = require('node:fs');
const path = require('node:path');

const [repoRoot, expectedResourceRoot] = process.argv.slice(2);
const configPath = path.join(repoRoot, 'src-tauri', 'tauri.conf.json');
const config = JSON.parse(fs.readFileSync(configPath, 'utf8'));
const destination = 'provider/codex-acp/';
const mappings = Object.entries(config.bundle?.resources ?? {})
  .filter(([, target]) => target === destination);

if (mappings.length !== 1
  || path.resolve(path.dirname(configPath), mappings[0][0]) !== path.resolve(expectedResourceRoot)) {
  throw new Error('Tauri resource mapping does not match the staged Codex ACP resource root.');
}
const expectedHooks = {
  beforeDevCommand: 'pnpm dev',
  beforeBuildCommand: 'bash scripts/build-codex-acp.sh --require-signature "${TAURI_ENV_ARCH:?}" && pnpm build',
};
for (const [hook, expectedCommand] of Object.entries(expectedHooks)) {
  if (config.build?.[hook] !== expectedCommand) {
    const expectation = hook === 'beforeDevCommand'
      ? 'keep development independent from provider packaging'
      : 'stage the pinned provider before packaging';
    throw new Error(`Tauri ${hook} must ${expectation}.`);
  }
}
NODE
}

if [[ "$candidate_only" != true ]]; then
  verify_tauri_resource_mapping
fi
"$script_dir/build-codex-runtime.sh" --verify

assert_repo_local_paths() {
  node "$script_dir/assert-repo-local-paths.cjs" "$repo_root" "$@"
}

assert_repo_local_paths \
  "$repo_root/.local" "$provider_build_root" "$version_dir" "$output_dir" "$staging_dir" \
  "$resource_root" "$resource_target" "$resource_staging_target" "$resource_backup_target" "$repo_root/tmp"
mkdir -p "$provider_build_root"
assert_repo_local_paths \
  "$repo_root/.local" "$provider_build_root" "$version_dir" "$output_dir" "$staging_dir" \
  "$resource_root" "$resource_target" "$resource_staging_target" "$resource_backup_target" "$repo_root/tmp"

cleanup_resource_staging() {
  if [[ -n "$resource_staging_target" && -d "$resource_staging_target" ]]; then
    if ! assert_repo_local_paths "$resource_root" "$resource_staging_target"; then
      printf 'Refusing to remove provider resource staging after path containment changed; preserving it.\n' >&2
      return 1
    fi
    rm -rf "$resource_staging_target"
  fi
  if [[ "$resource_backup_moved" == true && ! -e "$resource_target" \
    && -d "$resource_backup_target" ]]; then
    if ! assert_repo_local_paths "$provider_build_root" "$resource_backup_target" "$resource_target" \
      || ! mv "$resource_backup_target" "$resource_target"; then
      printf 'Could not restore the previous Tauri provider resource; its recovery backup remains intact.\n' >&2
      return 1
    fi
    resource_backup_moved=false
  fi
}
trap cleanup_resource_staging EXIT

reject_unsafe_tree() {
  local tree_path="$1"
  local unsafe_entry

  if [[ ! -d "$tree_path" || -L "$tree_path" ]]; then
    printf 'Provider tree is not a regular directory: %s\n' "$tree_path" >&2
    return 1
  fi
  if ! unsafe_entry="$(find "$tree_path" -mindepth 1 ! -type d ! -type f -print -quit)"; then
    printf 'Could not inspect provider tree: %s\n' "$tree_path" >&2
    return 1
  fi
  if [[ -n "$unsafe_entry" ]]; then
    printf 'Provider tree contains a symlink or special file: %s\n' "$unsafe_entry" >&2
    return 1
  fi
  return 0
}

verify_public_ca_bundle() {
  node - "$1/cacert.pem" "$public_ca_sha256" <<'NODE'
const fs = require('node:fs');
const crypto = require('node:crypto');
const [certificatePath, expected] = process.argv.slice(2);
const stat = fs.lstatSync(certificatePath);
if (!stat.isFile() || stat.nlink !== 1 || (stat.mode & 0o222) !== 0
  || crypto.createHash('sha256').update(fs.readFileSync(certificatePath)).digest('hex') !== expected) {
  throw new Error('Provider public certificate authority bundle failed its immutable source pin.');
}
NODE
}

verify_closed_candidate_tree() {
  local tree_path="$1"
  local expected_triple="$2"
  local entry
  local -a root_entries
  local -a vendor_entries

  if [[ ! -d "$tree_path" || -L "$tree_path" ]]; then
    printf 'Provider candidate is not a regular directory: %s\n' "$tree_path" >&2
    return 1
  fi

  shopt -s dotglob nullglob
  root_entries=("$tree_path"/*)
  vendor_entries=("$tree_path/vendor"/*)
  shopt -u dotglob nullglob

  if [[ "${#root_entries[@]}" -ne 6 ]]; then
    printf 'Provider candidate must contain only the pinned executables, manifest, public CA bundle, license notice, and vendor.\n' >&2
    return 1
  fi
  for entry in "${root_entries[@]}"; do
    case "${entry##*/}" in
      codex-acp|codex-acp.sha256|build-manifest.json|cacert.pem|ca-notice.txt|vendor) ;;
      *)
        printf 'Provider candidate contains an unexpected root entry: %s\n' "${entry##*/}" >&2
        return 1
        ;;
    esac
  done
  if [[ ! -f "$tree_path/codex-acp" || -L "$tree_path/codex-acp" \
    || ! -f "$tree_path/codex-acp.sha256" || -L "$tree_path/codex-acp.sha256" \
    || ! -f "$tree_path/build-manifest.json" || -L "$tree_path/build-manifest.json" \
    || ! -f "$tree_path/cacert.pem" || -L "$tree_path/cacert.pem" \
    || ! -f "$tree_path/ca-notice.txt" || -L "$tree_path/ca-notice.txt" \
    || ! -d "$tree_path/vendor" || -L "$tree_path/vendor" ]]; then
    printf 'Provider candidate root entries have invalid types.\n' >&2
    return 1
  fi
  if [[ "${#vendor_entries[@]}" -ne 1 ]]; then
    printf 'Provider candidate must contain only vendor/%s.\n' "$expected_triple" >&2
    return 1
  fi
  if [[ "${vendor_entries[0]##*/}" != "$expected_triple" \
    || ! -d "$tree_path/vendor/$expected_triple" \
    || -L "$tree_path/vendor/$expected_triple" ]]; then
    printf 'Provider candidate must contain only vendor/%s.\n' "$expected_triple" >&2
    return 1
  fi

  verify_public_ca_bundle "$tree_path" || return 1
  reject_unsafe_tree "$tree_path"
}

verify_pinned_platform_vendor_tree() {
  local candidate_vendor
  local extracted_vendor
  local checkout
  local actual_commit
  local lockfile
  local platform_metadata
  local resolved_url
  local lock_sri
  local platform_host
  local actual_sri
  local archive_path
  local extract_root

  for required_command in git curl tar; do
    if ! command -v "$required_command" >/dev/null 2>&1; then
      printf 'Missing required command: %s\n' "$required_command" >&2
      return 1
    fi
  done

  assert_repo_local_paths \
    "$repo_root/tmp" "$provider_build_root" "$version_dir" "$output_dir" "$resource_root" "$resource_target"
  mkdir -p "$repo_root/tmp"
  assert_repo_local_paths \
    "$repo_root/tmp" "$provider_build_root" "$version_dir" "$output_dir" "$resource_root" "$resource_target"
  cache_verify_temp="$(mktemp -d "$repo_root/tmp/codex-acp-cache-verify.XXXXXX")"
  assert_repo_local_paths "$repo_root/tmp" "$cache_verify_temp"
  trap cleanup_cache_verification EXIT

  checkout="$cache_verify_temp/source"
  assert_repo_local_paths "$cache_verify_temp" "$checkout"
  git clone --filter=blob:none --no-checkout https://github.com/agentclientprotocol/codex-acp.git "$checkout"
  assert_repo_local_paths "$cache_verify_temp" "$checkout"
  git -C "$checkout" fetch --depth=1 origin "$codex_acp_commit"
  git -C "$checkout" checkout --detach FETCH_HEAD
  actual_commit="$(git -C "$checkout" rev-parse HEAD)"
  if [[ "$actual_commit" != "$codex_acp_commit" ]]; then
    printf 'Unexpected codex-acp source commit during cache verification: %s\n' "$actual_commit" >&2
    return 1
  fi

  lockfile="$checkout/package-lock.json"
  platform_metadata="$cache_verify_temp/platform-lock-entry.json"
  assert_repo_local_paths "$cache_verify_temp" "$lockfile" "$platform_metadata"
  node - "$lockfile" "$platform_metadata" "$codex_platform_package" "$codex_platform_version" "$codex_platform_integrity" <<'NODE'
const fs = require('node:fs');
const [lockfilePath, metadataPath, packageName, packageVersion, expectedIntegrity] = process.argv.slice(2);
const lock = JSON.parse(fs.readFileSync(lockfilePath, 'utf8'));
const entry = lock.packages?.[`node_modules/${packageName}`];
if (!entry || entry.version !== packageVersion || entry.integrity !== expectedIntegrity
  || typeof entry.resolved !== 'string') {
  throw new Error('The pinned ACP lock entry does not match the Codex platform package profile.');
}
const resolved = new URL(entry.resolved);
const expectedPath = `/@openai/codex/-/codex-${packageVersion}.tgz`;
if (resolved.protocol !== 'https:' || resolved.hostname !== 'registry.npmjs.org'
  || resolved.port !== '' || resolved.username !== '' || resolved.password !== ''
  || resolved.search !== '' || resolved.hash !== '' || resolved.pathname !== expectedPath) {
  throw new Error('The pinned ACP lock entry has an unexpected or credential-bearing package URL.');
}
fs.writeFileSync(metadataPath, JSON.stringify({ resolved: resolved.href, integrity: entry.integrity, host: resolved.hostname }));
NODE

  resolved_url="$(node - "$platform_metadata" <<'NODE'
const fs = require('node:fs');
process.stdout.write(JSON.parse(fs.readFileSync(process.argv[2], 'utf8')).resolved);
NODE
  )"
  lock_sri="$(node - "$platform_metadata" <<'NODE'
const fs = require('node:fs');
process.stdout.write(JSON.parse(fs.readFileSync(process.argv[2], 'utf8')).integrity);
NODE
  )"
  platform_host="$(node - "$platform_metadata" <<'NODE'
const fs = require('node:fs');
process.stdout.write(JSON.parse(fs.readFileSync(process.argv[2], 'utf8')).host);
NODE
  )"
  archive_path="$cache_verify_temp/codex-platform.tgz"
  assert_repo_local_paths "$cache_verify_temp" "$archive_path"
  curl --disable --fail --location --silent --show-error --proto '=https' --proto-redir '=https' \
    --max-redirs 5 --output "$archive_path" "$resolved_url"
  actual_sri="$(node - "$archive_path" <<'NODE'
const crypto = require('node:crypto');
const fs = require('node:fs');
(async () => {
  const hash = crypto.createHash('sha512');
  for await (const chunk of fs.createReadStream(process.argv[2])) hash.update(chunk);
  process.stdout.write(`sha512-${hash.digest('base64')}`);
})().catch((error) => {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
});
NODE
  )"
  if [[ "$actual_sri" != "$lock_sri" || "$actual_sri" != "$codex_platform_integrity" ]]; then
    printf 'Downloaded Codex platform tarball does not match both the lockfile and pinned SRI.\n' >&2
    return 1
  fi

  printf 'Verified package-lock tarball host: %s\n' "$platform_host"
  printf 'Verified package SHA-512 SRI: %s\n' "$actual_sri"

  node - "$archive_path" <<'NODE'
const { spawnSync } = require('node:child_process');
const archivePath = process.argv[2];
const listing = spawnSync('tar', ['-tzf', archivePath], {
  encoding: 'utf8',
  maxBuffer: 64 * 1024 * 1024,
});
const details = spawnSync('tar', ['-tvzf', archivePath], {
  encoding: 'utf8',
  maxBuffer: 64 * 1024 * 1024,
});
if (listing.status !== 0 || details.status !== 0) {
  throw new Error('The pinned Codex platform archive could not be inspected.');
}
const paths = listing.stdout.split(/\r?\n/).filter(Boolean);
const entries = details.stdout.split(/\r?\n/).filter(Boolean);
if (paths.length === 0 || paths.length !== entries.length) {
  throw new Error('The pinned Codex platform archive has an invalid entry listing.');
}
const seen = new Set();
for (let index = 0; index < paths.length; index += 1) {
  const entry = entries[index];
  const archivePath = paths[index];
  const normalized = archivePath.endsWith('/') ? archivePath.slice(0, -1) : archivePath;
  const components = normalized.split('/');
  if (!['-', 'd'].includes(entry[0])
    || archivePath.startsWith('/')
    || archivePath.includes('\\')
    || components[0] !== 'package'
    || components.some((component) => component === '' || component === '.' || component === '..')
    || seen.has(normalized)) {
    throw new Error(`The pinned Codex platform archive contains an unsafe or duplicate entry: ${archivePath}`);
  }
  seen.add(normalized);
}
NODE

  extract_root="$cache_verify_temp/extracted"
  assert_repo_local_paths "$cache_verify_temp" "$extract_root"
  mkdir "$extract_root"
  assert_repo_local_paths "$cache_verify_temp" "$extract_root"
  tar -xzf "$archive_path" -C "$extract_root"
  assert_repo_local_paths "$cache_verify_temp" "$extract_root/package"
  reject_unsafe_tree "$extract_root/package"

  candidate_vendor="$output_dir/vendor/$codex_target_triple"
  extracted_vendor="$extract_root/package/vendor/$codex_target_triple"
  "$script_dir/build-codex-runtime.sh" --verify
  cp "$codex_runtime_root/codex" "$extracted_vendor/bin/codex"
  if [[ ! -d "$extracted_vendor" || -L "$extracted_vendor" ]] \
    || ! reject_unsafe_tree "$candidate_vendor" \
    || ! reject_unsafe_tree "$extracted_vendor" \
    || ! diff -qr "$candidate_vendor" "$extracted_vendor" >/dev/null; then
    printf 'Cached Codex vendor tree does not match the pinned platform package.\n' >&2
    return 1
  fi

  printf 'Verified Codex platform package SRI: %s\n' "$codex_platform_integrity"
  printf 'Verified vendor dependencies and pinned source-built Codex runtime: %s@%s (%s)\n' \
    "$codex_platform_package" "$codex_platform_version" "$codex_target_triple"
}

cleanup_cache_verification() {
  cleanup_resource_staging
  if [[ -n "${cache_verify_temp:-}" && -d "$cache_verify_temp" ]]; then
    if ! assert_repo_local_paths "$repo_root/tmp" "$cache_verify_temp"; then
      printf 'Refusing to remove provider verification data after path containment changed.\n' >&2
      return 1
    fi
    rm -rf "$cache_verify_temp"
  fi
}

verify_release_signature_for_identity() {
  local executable="$1"
  local signature_details
  local signature_identity
  local signature_team_identifier

  if ! codesign --verify --strict "$executable" >/dev/null 2>&1; then
    return 1
  fi
  if ! signature_details="$(codesign --display --verbose=4 "$executable" 2>&1)"; then
    return 1
  fi
  signature_identity="$(printf '%s\n' "$signature_details" \
    | awk '/^Authority=/ { sub(/^Authority=/, ""); print; exit }')"
  signature_team_identifier="$(printf '%s\n' "$signature_details" \
    | awk '/^TeamIdentifier=/ { sub(/^TeamIdentifier=/, ""); print; exit }')"
  [[ "$signature_identity" == "$APPLE_SIGNING_IDENTITY" \
    && "$signature_team_identifier" == "$apple_signing_team_identifier" ]]
}

verify_staged_resource_matches_candidate() {
  local resource_stage="$1"
  local relative_path
  local source_executable
  local staged_executable

  if ! diff -qr \
    -x codex-acp -x codex -x rg -x codex-acp.sha256 -x build-manifest.json \
    "$output_dir" "$resource_stage" >/dev/null; then
    printf 'Existing Tauri provider resource contains files that do not match the verified candidate.\n' >&2
    return 1
  fi
  if ! node - "$output_dir/build-manifest.json" "$resource_stage/build-manifest.json" <<'NODE'
const fs = require('node:fs');
const [candidatePath, stagedPath] = process.argv.slice(2);
const candidate = JSON.parse(fs.readFileSync(candidatePath, 'utf8'));
const staged = JSON.parse(fs.readFileSync(stagedPath, 'utf8'));
const signedDigests = new Set(['artifact_sha256', 'codex_executable_sha256', 'ripgrep_sha256']);
for (const [key, value] of Object.entries(candidate)) {
  if (!signedDigests.has(key) && staged[key] !== value) {
    throw new Error('Staged provider manifest does not match the verified candidate.');
  }
}
for (const key of Object.keys(staged)) {
  if (!Object.prototype.hasOwnProperty.call(candidate, key)) {
    throw new Error('Staged provider manifest contains an unexpected field.');
  }
}
NODE
  then
    printf 'Existing Tauri provider manifest does not match the verified candidate.\n' >&2
    return 1
  fi

  for relative_path in \
    'codex-acp' \
    "vendor/$codex_target_triple/bin/codex" \
    "vendor/$codex_target_triple/codex-path/rg"; do
    source_executable="$output_dir/$relative_path"
    staged_executable="$resource_stage/$relative_path"
    if [[ ! -f "$source_executable" || -L "$source_executable" \
      || ! -f "$staged_executable" || -L "$staged_executable" ]]; then
      printf 'Tauri provider resource is missing a required executable.\n' >&2
      return 1
    fi
    if cmp -s "$source_executable" "$staged_executable"; then
      continue
    fi
    if ! verify_release_signature_for_identity "$staged_executable"; then
      printf 'Existing Tauri provider executable is not signed by the configured release identity.\n' >&2
      return 1
    fi
  done
}

verify_published_release_resource() {
  local resource_path="$1"
  local relative_path
  local artifact_sha256
  local codex_executable_sha256
  local ripgrep_sha256

  if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]] \
    || ! verify_closed_candidate_tree "$resource_path" "$codex_target_triple" \
    || ! reject_unsafe_tree "$resource_path" \
    || ! verify_staged_resource_matches_candidate "$resource_path" >/dev/null 2>&1; then
    return 1
  fi

  for relative_path in \
    'codex-acp' \
    "vendor/$codex_target_triple/bin/codex" \
    "vendor/$codex_target_triple/codex-path/rg"; do
    if ! verify_release_signature_for_identity "$resource_path/$relative_path"; then
      return 1
    fi
  done

  artifact_sha256="$(shasum -a 256 "$resource_path/codex-acp" | awk '{ print $1 }')"
  codex_executable_sha256="$(shasum -a 256 \
    "$resource_path/vendor/$codex_target_triple/bin/codex" | awk '{ print $1 }')"
  ripgrep_sha256="$(shasum -a 256 \
    "$resource_path/vendor/$codex_target_triple/codex-path/rg" | awk '{ print $1 }')"
  node - "$resource_path/codex-acp.sha256" \
    "$resource_path/build-manifest.json" "$artifact_sha256" \
    "$codex_executable_sha256" "$ripgrep_sha256" <<'NODE'
const fs = require('node:fs');
const [checksumPath, manifestPath, artifactSha256, codexSha256, ripgrepSha256] = process.argv.slice(2);
const checksum = fs.readFileSync(checksumPath, 'utf8');
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
if (checksum !== `${artifactSha256}\n`
  || manifest.artifact_sha256 !== artifactSha256
  || manifest.codex_executable_sha256 !== codexSha256
  || manifest.ripgrep_sha256 !== ripgrepSha256) {
  throw new Error('Published ACP executable digests do not match the signed manifest.');
}
NODE
}

verify_reusable_release_resource() {
  local resource_path="$1"
  local relative_path
  local executable
  local artifact_sha256
  local codex_executable_sha256
  local ripgrep_sha256

  if [[ "$require_signature" != true || -z "${APPLE_SIGNING_IDENTITY:-}" ]] \
    || ! verify_closed_candidate_tree "$resource_path" "$codex_target_triple" \
    || ! reject_unsafe_tree "$resource_path"; then
    return 1
  fi

  for relative_path in \
    'codex-acp' \
    "vendor/$codex_target_triple/bin/codex" \
    "vendor/$codex_target_triple/codex-path/rg"; do
    executable="$resource_path/$relative_path"
    if [[ ! -f "$executable" || -L "$executable" || ! -x "$executable" ]] \
      || ! verify_release_signature_for_identity "$executable"; then
      return 1
    fi
  done

  artifact_sha256="$(shasum -a 256 "$resource_path/codex-acp" | awk '{ print $1 }')"
  codex_executable_sha256="$(shasum -a 256 \
    "$resource_path/vendor/$codex_target_triple/bin/codex" | awk '{ print $1 }')"
  ripgrep_sha256="$(shasum -a 256 \
    "$resource_path/vendor/$codex_target_triple/codex-path/rg" | awk '{ print $1 }')"
  if ! node - "$resource_path/codex-acp.sha256" \
    "$resource_path/build-manifest.json" "$target_arch" "$codex_acp_package" \
    "$codex_acp_version" "$codex_acp_integrity" "$codex_acp_commit" \
    "$codex_version" "$codex_integrity" "$codex_platform_package" \
    "$codex_platform_version" "$codex_platform_integrity" "$codex_target_triple" \
    "$codex_acp_upstream_artifact_sha256" "$codex_acp_adapter_patch_id" \
    "$artifact_sha256" "$codex_executable_sha256" "$ripgrep_sha256" <<'NODE'
const fs = require('node:fs');
const [checksumPath, manifestPath, targetArch, packageName, packageVersion,
  packageIntegrity, sourceCommit, codexVersion, codexIntegrity, platformPackage,
  platformVersion, platformIntegrity, targetTriple, upstreamArtifactSha256,
  adapterPatchId, artifactSha256, codexSha256, ripgrepSha256] = process.argv.slice(2);
const checksum = fs.readFileSync(checksumPath, 'utf8');
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const expected = {
  schema_version: 5,
  provider: 'codex-acp',
  package: packageName,
  package_version: packageVersion,
  package_integrity: packageIntegrity,
  source_commit: sourceCommit,
  bundled_codex_package: '@openai/codex',
  bundled_codex_version: codexVersion,
  bundled_codex_integrity: codexIntegrity,
  codex_platform_package: platformPackage,
  codex_platform_version: platformVersion,
  codex_platform_integrity: platformIntegrity,
  codex_target_triple: targetTriple,
  target: `darwin-${targetArch}`,
  upstream_artifact_sha256: upstreamArtifactSha256,
  adapter_patch_id: adapterPatchId,
  artifact_sha256: artifactSha256,
  codex_executable_sha256: codexSha256,
  ripgrep_sha256: ripgrepSha256,
  public_ca_source_url: 'https://curl.se/ca/cacert-2026-09-25.pem',
  public_ca_sha256: 'a41b5d356aea97a529fe27e0f7316d2f9d946d75927476cf9cf1b90637d00505',
  codex_source_commit: 'b412ff32c417f855c2b2d1581b77058eed87c84b',
  codex_source_sha256: '1ac6a92e7318b8acf3d767170c5c5e6dceeffdc074c73b1c5d422b46f0de4daf',
  codex_source_patch_id: 'codex-http-ca-preserve-backend-v1',
  codex_source_patch_sha256: 'b08f4099725b6394e5657691e10d2dc8d9696cd119623fa6155db28a70d76d54',
  codex_source_lock_sha256: 'd722f05fc760bcd1f5749ec452452d81058458b788df3b765b80500d757eba4a',
  status: 'built_unadmitted',
};
if (checksum !== `${artifactSha256}\n`
  || Object.keys(manifest).length !== Object.keys(expected).length
  || Object.entries(expected).some(([key, value]) => manifest[key] !== value)) {
  throw new Error('Signed provider resource failed its pinned source, manifest, or digest checks.');
}
NODE
  then
    return 1
  fi
}

sign_staged_provider_executables() {
  local resource_stage="$1"
  local relative_path
  local staged_executable

  if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    return 0
  fi
  if ! command -v codesign >/dev/null 2>&1; then
    printf 'Missing required command: codesign.\n' >&2
    return 1
  fi
  if ! verify_staged_resource_matches_candidate "$resource_stage"; then
    return 1
  fi

  for relative_path in \
    'codex-acp' \
    "vendor/$codex_target_triple/bin/codex" \
    "vendor/$codex_target_triple/codex-path/rg"; do
    staged_executable="$resource_stage/$relative_path"
    if ! assert_repo_local_paths "$resource_root" "$resource_stage" "$staged_executable"; then
      return 1
    fi
    if ! codesign --force --sign "$APPLE_SIGNING_IDENTITY" \
      --options runtime --timestamp "$staged_executable" >/dev/null 2>&1; then
      printf 'Could not code-sign a staged ACP executable with the configured release identity.\n' >&2
      return 1
    fi
    if ! verify_release_signature_for_identity "$staged_executable"; then
      printf 'Staged ACP executable signature verification failed.\n' >&2
      return 1
    fi
  done

  artifact_sha256="$(shasum -a 256 "$resource_stage/codex-acp" | awk '{ print $1 }')"
  codex_executable_sha256="$(shasum -a 256 \
    "$resource_stage/vendor/$codex_target_triple/bin/codex" | awk '{ print $1 }')"
  ripgrep_sha256="$(shasum -a 256 \
    "$resource_stage/vendor/$codex_target_triple/codex-path/rg" | awk '{ print $1 }')"
  printf '%s\n' "$artifact_sha256" > "$resource_stage/codex-acp.sha256"
  node - "$resource_stage/build-manifest.json" "$artifact_sha256" \
    "$codex_executable_sha256" "$ripgrep_sha256" <<'NODE'
const fs = require('node:fs');
const [manifestPath, artifactSha256, codexSha256, ripgrepSha256] = process.argv.slice(2);
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
manifest.artifact_sha256 = artifactSha256;
manifest.codex_executable_sha256 = codexSha256;
manifest.ripgrep_sha256 = ripgrepSha256;
fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
NODE

  artifact_sha256="$(shasum -a 256 "$resource_stage/codex-acp" | awk '{ print $1 }')"
  codex_executable_sha256="$(shasum -a 256 \
    "$resource_stage/vendor/$codex_target_triple/bin/codex" | awk '{ print $1 }')"
  ripgrep_sha256="$(shasum -a 256 \
    "$resource_stage/vendor/$codex_target_triple/codex-path/rg" | awk '{ print $1 }')"
  for relative_path in \
    'codex-acp' \
    "vendor/$codex_target_triple/bin/codex" \
    "vendor/$codex_target_triple/codex-path/rg"; do
    if ! verify_release_signature_for_identity "$resource_stage/$relative_path"; then
      printf 'Bundled ACP executable signature verification failed before packaging.\n' >&2
      return 1
    fi
  done
  if ! node - "$resource_stage/codex-acp.sha256" \
    "$resource_stage/build-manifest.json" "$artifact_sha256" \
    "$codex_executable_sha256" "$ripgrep_sha256" <<'NODE'
const fs = require('node:fs');
const [checksumPath, manifestPath, artifactSha256, codexSha256, ripgrepSha256] = process.argv.slice(2);
const checksum = fs.readFileSync(checksumPath, 'utf8');
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
if (checksum !== `${artifactSha256}\n`
  || manifest.artifact_sha256 !== artifactSha256
  || manifest.codex_executable_sha256 !== codexSha256
  || manifest.ripgrep_sha256 !== ripgrepSha256) {
  throw new Error('Signed ACP executable digests do not match the staged manifest.');
}
NODE
  then
    printf 'Signed ACP executable digest verification failed before packaging.\n' >&2
    return 1
  fi
}

stage_resource_candidate() {
  if [[ "$candidate_only" == true ]]; then
    return 0
  fi
  local unexpected_resource
  local previous_resource=false

  assert_repo_local_paths \
    "$provider_build_root" "$version_dir" "$output_dir" "$resource_root" \
    "$resource_target" "$resource_staging_target" "$resource_backup_target"
  verify_closed_candidate_tree "$output_dir" "$codex_target_triple"
  reject_unsafe_tree "$output_dir"

  if [[ -e "$resource_root" || -L "$resource_root" ]]; then
    if [[ ! -d "$resource_root" || -L "$resource_root" ]]; then
      printf 'Tauri provider resource staging path is not a regular directory.\n' >&2
      return 1
    fi
  else
    assert_repo_local_paths "$provider_build_root" "$resource_root"
    mkdir -p "$resource_root"
    assert_repo_local_paths "$provider_build_root" "$resource_root"
  fi

  if [[ -e "$resource_staging_target" || -L "$resource_staging_target" ]]; then
    printf 'Refusing to overwrite an existing Tauri provider resource staging directory.\n' >&2
    return 1
  fi
  unexpected_resource="$(find "$resource_root" -mindepth 1 -maxdepth 1 \
    ! -path "$resource_target" ! -path "$resource_staging_target" -print -quit)"
  if [[ -n "$unexpected_resource" ]]; then
    printf 'Tauri provider resource staging contains another target or unexpected entry.\n' >&2
    return 1
  fi

  if [[ -e "$resource_backup_target" || -L "$resource_backup_target" ]]; then
    printf 'Refusing to overwrite an existing Tauri provider recovery backup.\n' >&2
    return 1
  fi

  assert_repo_local_paths "$provider_build_root" "$resource_root" "$resource_staging_target"
  if ! mkdir "$resource_staging_target"; then
    printf 'Could not reserve the Tauri provider resource staging directory.\n' >&2
    return 1
  fi
  if ! cp -pR "$output_dir/." "$resource_staging_target/" \
    || ! verify_closed_candidate_tree "$resource_staging_target" "$codex_target_triple" \
    || ! reject_unsafe_tree "$resource_staging_target"; then
    cleanup_resource_staging
    printf 'Could not stage the verified provider candidate for Tauri.\n' >&2
    return 1
  fi
  if ! node - "$output_dir" "$resource_staging_target" "$codex_target_triple" <<'NODE'
const fs = require('node:fs');
const path = require('node:path');
const [sourceRoot, stagedRoot, triple] = process.argv.slice(2);
for (const relative of ['codex-acp', `vendor/${triple}/bin/codex`, `vendor/${triple}/codex-path/rg`]) {
  const original = fs.statSync(path.join(sourceRoot, relative), {bigint: true});
  const staged = fs.statSync(path.join(stagedRoot, relative), {bigint: true});
  if (original.dev === staged.dev && original.ino === staged.ino) {
    throw new Error('Provider publication requires a fresh executable inode.');
  }
}
NODE
  then
    cleanup_resource_staging
    printf 'Provider executable inode publication check failed.\n' >&2
    return 1
  fi
  if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
    if ! sign_staged_provider_executables "$resource_staging_target"; then
      cleanup_resource_staging
      printf 'Could not sign the staged Tauri provider resource.\n' >&2
      return 1
    fi
  elif ! diff -qr "$output_dir" "$resource_staging_target" >/dev/null; then
    cleanup_resource_staging
    printf 'Could not stage the verified provider candidate for Tauri.\n' >&2
    return 1
  fi

  if ! verify_closed_candidate_tree "$resource_staging_target" "$codex_target_triple" \
    || ! reject_unsafe_tree "$resource_staging_target"; then
    cleanup_resource_staging
    printf 'Staged Tauri provider resource failed final tree validation.\n' >&2
    return 1
  fi

  if [[ -e "$resource_target" || -L "$resource_target" ]]; then
    assert_repo_local_paths "$resource_root" "$resource_target" "$resource_backup_target"
    if [[ ! -d "$resource_target" || -L "$resource_target" ]]; then
      cleanup_resource_staging
      printf 'Existing Tauri provider resource is not a regular directory.\n' >&2
      return 1
    fi
    if ! mv "$resource_target" "$resource_backup_target"; then
      cleanup_resource_staging
      printf 'Could not preserve the previous Tauri provider resource.\n' >&2
      return 1
    fi
    previous_resource=true
    resource_backup_moved=true
  fi

  if ! mv "$resource_staging_target" "$resource_target"; then
    if [[ "$previous_resource" == true && ! -e "$resource_target" && -d "$resource_backup_target" ]]; then
      if ! mv "$resource_backup_target" "$resource_target"; then
        printf 'Provider resource publish failed; the previous resource remains in its recovery backup.\n' >&2
        return 1
      fi
      resource_backup_moved=false
    fi
    cleanup_resource_staging
    printf 'Could not publish the verified Tauri provider resource.\n' >&2
    return 1
  fi
  resource_backup_moved=false

  if [[ "$previous_resource" == true ]]; then
    printf 'Previous Tauri provider resource preserved until packaging succeeds: %s\n' \
      "${resource_backup_target##*/}"
  fi
}

if [[ "$require_signature" == true && -d "$resource_target" && ! -L "$resource_target" ]]; then
  if verify_reusable_release_resource "$resource_target"; then
    printf 'Reusing verified signed provider resource in place; skipping duplicate staging and platform extraction: %s/codex-acp\n' \
      "$resource_target"
    exit 0
  fi
  printf 'Existing provider resource failed in-place verification; continuing with pinned source, SRI, vendor, and signing checks.\n' >&2
fi

if [[ -e "$output_dir" || -L "$output_dir" ]]; then
  assert_repo_local_paths \
    "$provider_build_root" "$version_dir" "$output_dir" "$resource_root" "$resource_target"
  if [[ ! -d "$output_dir" || -L "$output_dir" ]]; then
    printf 'Existing provider candidate is not a regular directory: %s\n' "$output_dir" >&2
    exit 1
  fi
  for relative_path in \
    'codex-acp' \
    'codex-acp.sha256' \
    'build-manifest.json' \
    "vendor/$codex_target_triple/bin/codex" \
    "vendor/$codex_target_triple/codex-path/rg"; do
    candidate_path="$output_dir/$relative_path"
    if [[ ! -f "$candidate_path" || -L "$candidate_path" ]]; then
      printf 'Existing provider candidate is incomplete or contains a symlink: %s\n' "$candidate_path" >&2
      exit 1
    fi
  done
  for relative_path in 'vendor' "vendor/$codex_target_triple" "vendor/$codex_target_triple/bin" "vendor/$codex_target_triple/codex-path"; do
    candidate_path="$output_dir/$relative_path"
    if [[ ! -d "$candidate_path" || -L "$candidate_path" ]]; then
      printf 'Existing provider candidate contains an invalid directory: %s\n' "$candidate_path" >&2
      exit 1
    fi
  done
  reject_unsafe_tree "$output_dir"
  assert_repo_local_paths \
    "$provider_build_root" "$version_dir" "$output_dir" "$resource_root" "$resource_target"
  artifact_sha256="$(shasum -a 256 "$output_dir/codex-acp" | awk '{ print $1 }')"
  node - "$output_dir/codex-acp.sha256" "$artifact_sha256" <<'NODE'
const fs = require('node:fs');
const [checksumPath, expected] = process.argv.slice(2);
const checksum = fs.readFileSync(checksumPath, 'utf8');
if (!/^[a-f0-9]{64}\n$/.test(checksum) || checksum.slice(0, -1) !== expected) {
  throw new Error('Cached Codex ACP executable does not match its staged checksum.');
}
NODE
  codex_executable_sha256="$(shasum -a 256 "$output_dir/vendor/$codex_target_triple/bin/codex" | awk '{ print $1 }')"
  ripgrep_sha256="$(shasum -a 256 "$output_dir/vendor/$codex_target_triple/codex-path/rg" | awk '{ print $1 }')"
  stale_adapter_patch_id="$(node - "$output_dir/build-manifest.json" "$target_arch" "$codex_acp_package" "$codex_acp_version" "$codex_acp_integrity" "$codex_acp_commit" "$codex_version" "$codex_integrity" "$codex_platform_package" "$codex_platform_version" "$codex_platform_integrity" "$codex_target_triple" "$artifact_sha256" "$codex_executable_sha256" "$ripgrep_sha256" "$codex_acp_upstream_artifact_sha256" "$codex_acp_adapter_patch_id" "$codex_acp_previous_adapter_patch_id" <<'NODE'
const fs = require('node:fs');
const [manifestPath, targetArch, packageName, packageVersion, packageIntegrity, sourceCommit,
  codexVersion, codexIntegrity, platformPackage, platformVersion, platformIntegrity,
  targetTriple, artifactSha256, codexSha256, ripgrepSha256, upstreamArtifactSha256,
  adapterPatchId, previousAdapterPatchId] = process.argv.slice(2);
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const expected = {
  schema_version: 5,
  provider: 'codex-acp',
  package: packageName,
  package_version: packageVersion,
  package_integrity: packageIntegrity,
  source_commit: sourceCommit,
  bundled_codex_package: '@openai/codex',
  bundled_codex_version: codexVersion,
  bundled_codex_integrity: codexIntegrity,
  codex_platform_package: platformPackage,
  codex_platform_version: platformVersion,
  codex_platform_integrity: platformIntegrity,
  codex_target_triple: targetTriple,
  target: `darwin-${targetArch}`,
  upstream_artifact_sha256: upstreamArtifactSha256,
  adapter_patch_id: adapterPatchId,
  artifact_sha256: artifactSha256,
  codex_executable_sha256: codexSha256,
  ripgrep_sha256: ripgrepSha256,
  public_ca_source_url: 'https://curl.se/ca/cacert-2026-09-25.pem',
  public_ca_sha256: 'a41b5d356aea97a529fe27e0f7316d2f9d946d75927476cf9cf1b90637d00505',
  codex_source_commit: 'b412ff32c417f855c2b2d1581b77058eed87c84b',
  codex_source_sha256: '1ac6a92e7318b8acf3d767170c5c5e6dceeffdc074c73b1c5d422b46f0de4daf',
  codex_source_patch_id: 'codex-http-ca-preserve-backend-v1',
  codex_source_patch_sha256: 'b08f4099725b6394e5657691e10d2dc8d9696cd119623fa6155db28a70d76d54',
  codex_source_lock_sha256: 'd722f05fc760bcd1f5749ec452452d81058458b788df3b765b80500d757eba4a',
  status: 'built_unadmitted',
};
const hasExactFields = Object.keys(manifest).length === Object.keys(expected).length
  && Object.keys(expected).every((key) => Object.prototype.hasOwnProperty.call(manifest, key));
const matches = (patchId) => hasExactFields
  && Object.entries({ ...expected, adapter_patch_id: patchId })
    .every(([key, value]) => manifest[key] === value);
if (matches(adapterPatchId)) {
  process.stdout.write('');
} else if (manifest.adapter_patch_id === previousAdapterPatchId && matches(previousAdapterPatchId)) {
  process.stdout.write(previousAdapterPatchId);
} else {
  const mismatch = Object.entries(expected).find(([key, value]) => manifest[key] !== value)?.[0]
    ?? 'manifest_fields';
  throw new Error(`Existing provider candidate failed its pinned manifest check: ${mismatch}`);
}
NODE
  )"
  if [[ -n "$stale_adapter_patch_id" ]]; then
    stale_candidate_backup="$version_dir/darwin-$target_arch.stale-$stale_adapter_patch_id-${artifact_sha256:0:16}"
    assert_repo_local_paths "$version_dir" "$output_dir" "$stale_candidate_backup"
    if [[ -e "$stale_candidate_backup" || -L "$stale_candidate_backup" ]]; then
      printf 'Refusing to overwrite the preserved stale provider candidate.\n' >&2
      exit 1
    fi
    mv "$output_dir" "$stale_candidate_backup"
    printf 'Preserved recognized stale provider candidate; rebuilding from pinned source (%s).\n' \
      "$stale_adapter_patch_id"
  else
    # Reuse only after the pinned candidate, resource tree, checksums, and Developer ID signatures pass.
    if [[ "$require_signature" == true && -d "$resource_target" && ! -L "$resource_target" ]]; then
      if verify_published_release_resource "$resource_target"; then
        printf 'Reusing verified signed artifact resource: %s/codex-acp\n' "$resource_target"
        exit 0
      fi
      printf 'Existing signed artifact resource failed verification; staging a fresh pinned copy.\n' >&2
    fi
    verify_pinned_platform_vendor_tree
    stage_resource_candidate
    if [[ "$candidate_only" == true ]]; then
      printf 'Verified unsigned unadmitted provider candidate: %s/codex-acp\n' "$output_dir"
    else
      printf 'Verified pinned artifact resource: %s/codex-acp\n' "$resource_target"
    fi
    printf 'SHA-256: %s\n' "$artifact_sha256"
    exit 0
  fi
fi
assert_repo_local_paths \
  "$provider_build_root" "$version_dir" "$output_dir" "$staging_dir" "$resource_root" "$resource_target" "$repo_root/tmp"
if [[ -e "$staging_dir" || -L "$staging_dir" ]]; then
  printf 'Refusing to overwrite an existing provider staging directory.\n' >&2
  exit 1
fi
for required_command in git npm; do
  if ! command -v "$required_command" >/dev/null 2>&1; then
    printf 'Missing required command: %s\n' "$required_command" >&2
    exit 1
  fi
done

if [[ "$("$provider_bun" --version)" != "$bun_version" ]]; then
  printf 'The signable provider build requires Bun %s.\n' "$bun_version" >&2
  exit 1
fi

assert_repo_local_paths "$repo_root/tmp" "$version_dir" "$staging_dir"
mkdir -p "$repo_root/tmp" "$version_dir"
assert_repo_local_paths "$repo_root/tmp" "$version_dir" "$staging_dir"
temporary_dir="$(mktemp -d "$repo_root/tmp/codex-acp-build.XXXXXX")"
assert_repo_local_paths "$repo_root/tmp" "$temporary_dir"
cleanup() {
  cleanup_resource_staging
  if ! assert_repo_local_paths "$repo_root/tmp" "$temporary_dir" "$version_dir" "$staging_dir"; then
    printf 'Refusing to remove provider build data after path containment changed; preserving it.\n' >&2
    return 1
  fi
  rm -rf "$temporary_dir"
  if [[ -d "$staging_dir" ]]; then
    if ! assert_repo_local_paths "$version_dir" "$staging_dir"; then
      printf 'Refusing to remove provider staging after path containment changed; preserving it.\n' >&2
      return 1
    fi
    rm -rf "$staging_dir"
  fi
}
trap cleanup EXIT
readonly npm_cache="$temporary_dir/npm-cache"

checkout="$temporary_dir/source"
assert_repo_local_paths "$temporary_dir" "$checkout"
git clone --filter=blob:none --no-checkout https://github.com/agentclientprotocol/codex-acp.git "$checkout"
assert_repo_local_paths "$temporary_dir" "$checkout"
git -C "$checkout" fetch --depth=1 origin "$codex_acp_commit"
git -C "$checkout" checkout --detach FETCH_HEAD
actual_commit="$(git -C "$checkout" rev-parse HEAD)"
if [[ "$actual_commit" != "$codex_acp_commit" ]]; then
  printf 'Unexpected codex-acp source commit: %s\n' "$actual_commit" >&2
  exit 1
fi
assert_repo_local_paths "$temporary_dir" "$checkout/src/CodexJsonRpcConnection.ts"
apply_adapter_home_lock_patch "$checkout/src/CodexJsonRpcConnection.ts"
assert_repo_local_paths "$temporary_dir" "$checkout/src" "$checkout/src/index.ts" "$checkout/src/MagiHostAuth.ts"
apply_adapter_existing_subscription_patch "$checkout/src"
apply_adapter_auth_exit_patch "$checkout/src/index.ts"

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
assert_repo_local_paths "$temporary_dir" "$npm_cache" "$pack_json" "$checkout"
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
  assert_repo_local_paths "$temporary_dir" "$checkout" "$npm_cache"
  cd "$checkout"
  npm ci --ignore-scripts --cache "$npm_cache" --no-audit --no-fund
  npx tsc --noEmit
  "$provider_bun" build src/index.ts --minify --sourcemap --compile --target="bun-darwin-$target_arch" --outfile "dist/bin/codex-acp-$target_arch-darwin"
)

binary="$checkout/dist/bin/codex-acp-$target_arch-darwin"
if [[ ! -f "$binary" ]]; then
  printf 'Pinned build did not produce the expected macOS executable.\n' >&2
  exit 1
fi

assert_repo_local_paths "$temporary_dir" "$checkout" "$binary" "$version_dir" "$staging_dir"
mkdir -p "$staging_dir"
assert_repo_local_paths "$version_dir" "$staging_dir"
chmod 755 "$binary"
mv "$binary" "$staging_dir/codex-acp"
artifact_sha256="$(shasum -a 256 "$staging_dir/codex-acp" | awk '{ print $1 }')"
printf '%s\n' "$artifact_sha256" > "$staging_dir/codex-acp.sha256"
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
cp "$codex_runtime_root/codex" "$codex_executable"
codex_executable_sha256="$(shasum -a 256 "$codex_executable" | awk '{ print $1 }')"
ripgrep_sha256="$(shasum -a 256 "$ripgrep_executable" | awk '{ print $1 }')"
curl --disable --fail --location --silent --show-error --proto '=https' --proto-redir '=https' \
  --max-redirs 3 --max-time 60 --max-filesize 4194304 \
  --output "$staging_dir/cacert.pem" "$public_ca_source_url"
chmod 444 "$staging_dir/cacert.pem"
verify_public_ca_bundle "$staging_dir"
cat > "$staging_dir/ca-notice.txt" <<EOF
Public CA root certificates extracted from Mozilla by curl's mk-ca-bundle.
Source: $public_ca_source_url
SHA-256: $public_ca_sha256
The Mozilla certificate data is distributed under the Mozilla Public License 2.0.
A copy of the license is available at https://www.mozilla.org/MPL/2.0/.
The original certificate bundle headers and certificate data are preserved without modification.
EOF
cat "$codex_runtime_root/LICENSE" "$codex_runtime_root/NOTICE" >> "$staging_dir/ca-notice.txt"
node "$script_dir/collect-provider-licenses.mjs" "$checkout" >> "$staging_dir/ca-notice.txt"
chmod 444 "$staging_dir/ca-notice.txt"
cat > "$staging_dir/build-manifest.json" <<EOF
{
  "schema_version": 5,
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
  "upstream_artifact_sha256": "$codex_acp_upstream_artifact_sha256",
  "adapter_patch_id": "$codex_acp_adapter_patch_id",
  "artifact_sha256": "$artifact_sha256",
  "codex_executable_sha256": "$codex_executable_sha256",
  "ripgrep_sha256": "$ripgrep_sha256",
  "public_ca_source_url": "$public_ca_source_url",
  "public_ca_sha256": "$public_ca_sha256",
  "codex_source_commit": "$codex_source_commit",
  "codex_source_sha256": "$codex_source_sha256",
  "codex_source_patch_id": "$codex_source_patch_id",
  "codex_source_patch_sha256": "$codex_source_patch_sha256",
  "codex_source_lock_sha256": "$codex_source_lock_sha256",
  "status": "built_unadmitted"
}
EOF

verify_closed_candidate_tree "$staging_dir" "$codex_target_triple"

if [[ -e "$output_dir" ]]; then
  printf 'Refusing to overwrite an existing provider candidate: %s\n' "$output_dir" >&2
  exit 1
fi
assert_repo_local_paths "$version_dir" "$staging_dir" "$output_dir" "$provider_build_root"
mv "$staging_dir" "$output_dir"
stage_resource_candidate

if [[ "$candidate_only" == true ]]; then
  printf 'Built unsigned unadmitted provider candidate: %s/codex-acp\n' "$output_dir"
else
  printf 'Built pinned artifact resource: %s/codex-acp\n' "$resource_target"
fi
printf 'SHA-256: %s\n' "$artifact_sha256"
