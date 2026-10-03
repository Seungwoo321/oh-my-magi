#!/usr/bin/env bash
set -euo pipefail

readonly source_commit='b412ff32c417f855c2b2d1581b77058eed87c84b'
readonly source_sha256='1ac6a92e7318b8acf3d767170c5c5e6dceeffdc074c73b1c5d422b46f0de4daf'
readonly patch_id='codex-http-ca-preserve-backend-v1'
readonly patch_sha256='2ed2741afc0cecc14da03cf05a4474176bbc9a81f6e7657078c069f2d584ec5b'
readonly lock_sha256='d722f05fc760bcd1f5749ec452452d81058458b788df3b765b80500d757eba4a'
readonly source_url="https://codeload.github.com/openai/codex/tar.gz/$source_commit"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "$script_dir/.." && pwd -P)"
output_root="$repo_root/.local/provider-build/$patch_id"
root="$output_root"
patch_file="$script_dir/patches/codex-http-ca-backend.patch"

if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then
  printf 'The pinned Codex runtime build requires Apple Silicon macOS.\n' >&2
  exit 1
fi
for command in curl shasum tar patch cargo rustup python3; do
  command -v "$command" >/dev/null || { printf 'Missing runtime build dependency.\n' >&2; exit 1; }
done
node "$script_dir/assert-repo-local-paths.cjs" "$repo_root" "$repo_root/.local" "$root" "$patch_file"
if [[ ! -f "$patch_file" || -L "$patch_file" || "$(shasum -a 256 "$patch_file" | awk '{print $1}')" != "$patch_sha256" ]]; then
  printf 'The runtime source patch does not match its pin.\n' >&2
  exit 1
fi
if [[ "${1:-}" == --verify ]]; then
  python3 - "$root" "$source_commit" "$source_sha256" "$patch_id" "$patch_sha256" "$lock_sha256" <<'PY'
import hashlib,json,pathlib,stat,sys
root=pathlib.Path(sys.argv[1]); expected=dict(zip(('source_commit','source_sha256','patch_id','patch_sha256','lock_sha256'),sys.argv[2:]))
manifest=json.loads((root/'runtime-build.json').read_text())
assert set(manifest)==set(expected)|{'schema_version','target','executable_sha256','rust_toolchain'}
assert manifest['schema_version']==1 and manifest['target']=='aarch64-apple-darwin' and manifest['rust_toolchain']=='1.95.0'
assert all(manifest[key]==value for key,value in expected.items())
for name in ('codex','runtime-build.json','LICENSE','NOTICE'):
    metadata=(root/name).lstat();assert stat.S_ISREG(metadata.st_mode) and metadata.st_nlink==1 and not metadata.st_mode&0o022
assert hashlib.sha256((root/'codex').read_bytes()).hexdigest()==manifest['executable_sha256']
PY
  exit 0
fi
if [[ "$#" -ne 0 || -e "$output_root" || -L "$output_root" ]]; then
  printf 'Runtime builds require a fresh output generation; existing outputs may only be verified.\n' >&2
  exit 1
fi
root="$repo_root/.local/provider-build/.$patch_id-build-$$"
node "$script_dir/assert-repo-local-paths.cjs" "$repo_root" "$root"
mkdir -p "$root"
chmod 700 "$root"
node "$script_dir/assert-repo-local-paths.cjs" "$repo_root" "$root"
archive="$root/source.tar.gz"
curl --disable --fail --location --silent --show-error --proto '=https' --proto-redir '=https' \
  --max-redirs 2 --max-time 90 --max-filesize 104857600 --output "$archive" "$source_url"
if [[ "$(shasum -a 256 "$archive" | awk '{print $1}')" != "$source_sha256" ]]; then
  printf 'The official Codex source archive does not match its pin.\n' >&2
  exit 1
fi
python3 - "$archive" "$source_commit" <<'PY'
import pathlib,sys,tarfile
prefix='codex-'+sys.argv[2]
with tarfile.open(sys.argv[1]) as archive:
    for member in archive.getmembers():
        path=pathlib.PurePosixPath(member.name)
        assert not path.is_absolute() and path.parts[0]==prefix and '..' not in path.parts
        safe_license_link=member.issym() and member.name==prefix+'/codex-rs/vendor/bubblewrap/LICENSE' and member.linkname=='COPYING'
        assert member.isfile() or member.isdir() or safe_license_link, 'Nonregular source archive entry rejected'
PY
mkdir "$root/source"
tar -xzf "$archive" -C "$root/source" --strip-components=1
cp "$root/source/codex-rs/Cargo.lock" "$root/original-Cargo.lock"
patch --batch --fuzz=0 -p1 -d "$root/source" < "$patch_file"
python3 - "$root" "$lock_sha256" <<'PY'
import hashlib,pathlib,sys,tomllib
root=pathlib.Path(sys.argv[1]);source=root/'source/codex-rs'
workspace=tomllib.loads((source/'Cargo.toml').read_text())['workspace']
names=set()
for manifest in source.rglob('Cargo.toml'):
    package=tomllib.loads(manifest.read_text()).get('package',{})
    if package.get('version')=={'workspace':True}:names.add(package['name'])
before=tomllib.loads((root/'original-Cargo.lock').read_text());after=tomllib.loads((source/'Cargo.lock').read_text())
assert set(before)==set(after) and before['version']==after['version']
assert len(before['package'])==len(after['package'])
changed=0
for old,new in zip(before['package'],after['package']):
    normalized=dict(new)
    if old['name'] in names and 'source' not in old:
        assert old['version']=='0.0.0' and new['version']==workspace['package']['version']=='0.156.1'
        normalized['version']=old['version'];changed+=1
    assert old==normalized, 'Dependency authority changed'
assert changed==155
assert hashlib.sha256((source/'Cargo.lock').read_bytes()).hexdigest()==sys.argv[2]
PY
python3 - "$root/source/codex-rs/http-client/src/custom_ca.rs" <<'PY'
import pathlib,sys
source=pathlib.Path(sys.argv[1]).read_text()
start=source.index('fn build_reqwest_client_with_env(');end=source.index('\nfn ',start+3)
http=source[start:end]
assert 'builder.use_rustls_tls()' not in http
assert 'builder.add_root_certificate(certificate)' in http
assert 'rustls_native_certs::load_native_certs()' in source
assert 'configured_ca_bundle' in source
PY
rustup toolchain install 1.95.0 --profile minimal
export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="$root/target"
export CARGO_BUILD_JOBS=2
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_PROFILE_RELEASE_DEBUG=0
export CARGO_PROFILE_RELEASE_STRIP=symbols
export CARGO_PROFILE_RELEASE_LTO=false
export RUSTFLAGS="--remap-path-prefix=$root=/codex-runtime-build"
(
  cd "$root/source/codex-rs"
  cargo +1.95.0 test --locked -p codex-http-client custom_ca --lib
  python3 - "$root" <<'PY'
import json,os,pathlib,shutil,stat,sys
root=pathlib.Path(sys.argv[1]);target=root/'target/debug'
assert root.resolve()==root and root.stat().st_uid==os.getuid()
assert target.resolve()==target and stat.S_ISDIR(target.lstat().st_mode)
entries=list(target.rglob('*'));assert all(not entry.is_symlink() for entry in entries)
size=sum(entry.stat().st_size for entry in entries if entry.is_file())
shutil.rmtree(target)
(root/'source-test-target-reclamation.json').write_text(json.dumps({'removedBytes':size,'entries':len(entries),'purpose':'Discarded isolated source-test objects after successful tests; release objects and source retained.'},indent=2)+'\n')
assert shutil.disk_usage(root).free>=4*1024**3, 'Insufficient storage for isolated release build'
PY
  cargo +1.95.0 build --locked --release -p codex-cli --bin codex
)
cp "$root/target/release/codex" "$root/codex"
cp "$root/source/LICENSE" "$root/LICENSE"
cp "$root/source/NOTICE" "$root/NOTICE"
chmod 555 "$root/codex"
chmod 444 "$root/LICENSE" "$root/NOTICE"
python3 - "$root" "$source_commit" "$source_sha256" "$patch_id" "$patch_sha256" "$lock_sha256" <<'PY'
import hashlib,json,pathlib,sys
root=pathlib.Path(sys.argv[1]);manifest=dict(zip(('source_commit','source_sha256','patch_id','patch_sha256','lock_sha256'),sys.argv[2:]))
manifest.update(schema_version=1,target='aarch64-apple-darwin',rust_toolchain='1.95.0',executable_sha256=hashlib.sha256((root/'codex').read_bytes()).hexdigest())
(root/'runtime-build.json').write_text(json.dumps(manifest,indent=2)+'\n')
PY
chmod 444 "$root/runtime-build.json"
if [[ -e "$output_root" || -L "$output_root" ]]; then
  printf 'Refusing to overwrite an existing runtime generation.\n' >&2
  exit 1
fi
mv "$root" "$output_root"
"$0" --verify
