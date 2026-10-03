#!/usr/bin/env python3
"""Verify and restore the pinned native-consumer inputs of a runtime generation."""
import hashlib, json, os, pathlib, signal, stat, subprocess, sys, tarfile, tempfile

LIMIT = 100 * 1024 * 1024
LOCK_LIMIT = 4 * 1024 * 1024

def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()

def owned(path, directory=False):
    m = path.lstat()
    assert m.st_uid == os.getuid() and not m.st_mode & 0o022
    assert stat.S_ISDIR(m.st_mode) if directory else stat.S_ISREG(m.st_mode) and m.st_nlink == 1
    return (m.st_dev, m.st_ino)

def verify(root, expected, complete=True):
    identity = owned(root, True)
    names = {"codex", "runtime-build.json", "LICENSE", "NOTICE"}
    assert {p.name for p in root.iterdir()} == names | ({"source", "source.tar.gz"} if complete else set())
    for name in names:
        owned(root / name)
    manifest = json.loads((root / "runtime-build.json").read_text())
    assert set(manifest) == set(expected) | {"schema_version", "target", "executable_sha256", "rust_toolchain"}
    assert manifest["schema_version"] == 1 and manifest["target"] == "aarch64-apple-darwin" and manifest["rust_toolchain"] == "1.95.0"
    assert all(manifest[k] == v for k, v in expected.items())
    assert digest(root / "codex") == manifest["executable_sha256"]
    if complete:
        archive = root / "source.tar.gz"
        owned(archive)
        assert archive.stat().st_size <= LIMIT and digest(archive) == expected["source_sha256"]
        source = root / "source"
        owned(source, True)
        assert {p.name for p in source.iterdir()} == {"codex-rs"}
        code = source / "codex-rs"
        owned(code, True)
        assert {p.name for p in code.iterdir()} == {"Cargo.lock"}
        lock = code / "Cargo.lock"
        owned(lock)
        assert lock.stat().st_size <= LOCK_LIMIT and digest(lock) == expected["lock_sha256"]
    assert owned(root, True) == identity
    return identity

def restore(root, expected, patch):
    identity = verify(root, expected, False)
    owned(patch)
    assert digest(patch) == expected["patch_sha256"]
    lockdir = root / ".provenance-publication"
    lockdir.mkdir(mode=0o700)
    lock_identity = owned(lockdir, True)
    published = []
    def interrupted(signum, frame):
        raise InterruptedError("Runtime provenance restoration interrupted")
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)
    try:
        archive = lockdir / "source.tar.gz"
        subprocess.run(["curl", "--disable", "--fail", "--location", "--silent", "--show-error", "--proto", "=https", "--proto-redir", "=https", "--max-redirs", "2", "--max-time", "90", "--max-filesize", str(LIMIT), "--output", str(archive), "https://codeload.github.com/openai/codex/tar.gz/" + expected["source_commit"]], check=True)
        assert archive.stat().st_size <= LIMIT and digest(archive) == expected["source_sha256"]
        source = lockdir / "source"
        code = source / "codex-rs"
        code.mkdir(parents=True, mode=0o700)
        lock = code / "Cargo.lock"
        wanted = "codex-" + expected["source_commit"] + "/codex-rs/Cargo.lock"
        matches = 0
        expanded = 0
        entries = 0
        prefix = "codex-" + expected["source_commit"]
        with tarfile.open(archive) as tar:
            for member in tar:
                entries += 1
                assert entries <= 100000 and member.size >= 0
                expanded += member.size
                assert expanded <= 1024 * 1024 * 1024
                path = pathlib.PurePosixPath(member.name)
                assert not path.is_absolute() and path.parts[0] == prefix and ".." not in path.parts
                license_link = member.issym() and member.name == prefix + "/codex-rs/vendor/bubblewrap/LICENSE" and member.linkname == "COPYING"
                assert member.isfile() or member.isdir() or license_link
                if member.name != wanted:
                    continue
                matches += 1
                assert matches == 1 and member.isfile() and member.size <= LOCK_LIMIT
                with tar.extractfile(member) as src, lock.open("xb") as dst:
                    data = src.read(LOCK_LIMIT + 1)
                    assert len(data) == member.size
                    dst.write(data)
        assert matches == 1
        text = patch.read_text()
        start = text.index("--- a/codex-rs/Cargo.lock\n")
        end = text.find("\n--- a/", start + 1)
        hunk = text[start:] if end == -1 else text[start:end + 1]
        subprocess.run(["patch", "--batch", "--fuzz=0", "--forward", str(lock)], input=hunk.encode(), check=True, stdout=subprocess.DEVNULL)
        assert digest(lock) == expected["lock_sha256"]
        assert set(code.iterdir()) == {lock}
        for file in (archive, lock):
            file.chmod(0o444)
            with file.open("rb") as stream:
                os.fsync(stream.fileno())
        source.chmod(0o700)
        code.chmod(0o700)
        assert owned(root, True) == identity
        for name in ("source.tar.gz", "source"):
            target = root / name
            assert not target.exists() and not target.is_symlink()
            os.rename(lockdir / name, target)
            published.append((target, target.lstat().st_ino))
        lockdir.rmdir()
        fd = os.open(root, os.O_RDONLY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)
        verify(root, expected)
        published.clear()
    finally:
        import shutil
        assert owned(root, True) == identity
        for target, inode in reversed(published):
            assert target.lstat().st_ino == inode
            if target.is_dir():
                shutil.rmtree(target)
            else:
                target.unlink()
        if lockdir.exists():
            assert owned(lockdir, True) == lock_identity
            shutil.rmtree(lockdir)

def main():
    mode, root, patch, *pins = sys.argv[1:]
    expected = dict(zip(("source_commit", "source_sha256", "patch_id", "patch_sha256", "lock_sha256"), pins, strict=True))
    root = pathlib.Path(root)
    assert root.resolve() == root
    if mode == "--verify":
        verify(root, expected)
    elif mode == "--restore-provenance":
        restore(root, expected, pathlib.Path(patch))
    else:
        raise ValueError("Unknown provenance command")

if __name__ == "__main__":
    if not __debug__:
        raise RuntimeError("Runtime provenance verification requires enabled security checks")
    main()
