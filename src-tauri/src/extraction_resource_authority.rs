use magi_domain::Digest;
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
const UNAVAILABLE: &str = "The verified native extraction helper is unavailable.";
pub(crate) struct VerifiedExtractionResource {
    path: PathBuf,
    bytes: Vec<u8>,
    held: Vec<(PathBuf, fs::File, Identity, bool)>,
}
impl VerifiedExtractionResource {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
    pub(crate) fn check(&self) -> Result<(), String> {
        for (path, file, expected, contents) in &self.held {
            let opened = identity(&file.metadata().map_err(|_| UNAVAILABLE)?);
            let current = identity(&fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?);
            if if *contents {
                opened != *expected || current != *expected
            } else {
                (opened.0, opened.1, opened.7, opened.8)
                    != (expected.0, expected.1, expected.7, expected.8)
                    || (current.0, current.1, current.7, current.8)
                        != (expected.0, expected.1, expected.7, expected.8)
            } {
                return Err(UNAVAILABLE.into());
            }
        }
        Ok(())
    }
}
impl AsRef<[u8]> for VerifiedExtractionResource {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
pub(crate) fn validate_source_binding(
    resource: &VerifiedExtractionResource,
    source_bytes: &[u8],
) -> Result<(), String> {
    resource.check()?;
    if source_section(&resource.bytes)? != Digest::from_bytes(source_bytes).as_str() {
        return Err(UNAVAILABLE.into());
    }
    resource.check()
}
fn source_section(bytes: &[u8]) -> Result<&str, String> {
    fn word(bytes: &[u8], at: usize) -> Result<u32, String> {
        bytes
            .get(at..at + 4)
            .and_then(|v| v.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| UNAVAILABLE.into())
    }
    let count = word(bytes, 16)?;
    let command_bytes = word(bytes, 20)? as usize;
    let end = 32usize
        .checked_add(command_bytes)
        .filter(|e| *e <= bytes.len())
        .ok_or(UNAVAILABLE)?;
    let mut cursor = 32usize;
    let mut found = None;
    if count > 4096 {
        return Err(UNAVAILABLE.into());
    }
    for _ in 0..count {
        let command = word(bytes, cursor)?;
        let size = word(bytes, cursor + 4)? as usize;
        let next = cursor
            .checked_add(size)
            .filter(|n| size >= 8 && *n <= end)
            .ok_or(UNAVAILABLE)?;
        if command == 0x19 {
            let sections = word(bytes, cursor + 64)? as usize;
            if size < 72 || sections > (size - 72) / 80 {
                return Err(UNAVAILABLE.into());
            }
            for index in 0..sections {
                let at = cursor + 72 + index * 80;
                if bytes
                    .get(at..at + 16)
                    .is_some_and(|v| v == b"__magi_source\0\0\0")
                {
                    if bytes.get(at + 16..at + 32) != Some(b"__TEXT\0\0\0\0\0\0\0\0\0\0".as_slice())
                        || found.is_some()
                    {
                        return Err(UNAVAILABLE.into());
                    }
                    let length = bytes
                        .get(at + 40..at + 48)
                        .and_then(|v| v.try_into().ok())
                        .map(u64::from_le_bytes)
                        .ok_or(UNAVAILABLE)?;
                    let offset = word(bytes, at + 48)? as usize;
                    if length != 64 {
                        return Err(UNAVAILABLE.into());
                    }
                    let text =
                        std::str::from_utf8(bytes.get(offset..offset + 64).ok_or(UNAVAILABLE)?)
                            .map_err(|_| UNAVAILABLE)?;
                    if !text
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    {
                        return Err(UNAVAILABLE.into());
                    }
                    found = Some(text);
                }
            }
        }
        cursor = next;
    }
    found.ok_or_else(|| UNAVAILABLE.into())
}
pub(crate) fn verify_resource(
    resource_root: &Path,
    signature: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<VerifiedExtractionResource, String> {
    if !resource_root.is_absolute() {
        return Err(UNAVAILABLE.into());
    }
    for ancestor in resource_root.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| UNAVAILABLE)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(UNAVAILABLE.into());
        }
    }
    let root = resource_root.canonicalize().map_err(|_| UNAVAILABLE)?;
    let directory = root.join("extraction");
    let metadata = fs::symlink_metadata(&directory).map_err(|_| UNAVAILABLE)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o222 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(UNAVAILABLE.into());
    }
    let entries = fs::read_dir(&directory)
        .map_err(|_| UNAVAILABLE)?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name())
                .map_err(|_| UNAVAILABLE.to_owned())
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    if entries
        != [
            std::ffi::OsString::from("magi-extract"),
            std::ffi::OsString::from("magi-extract.sha256"),
        ]
        .into_iter()
        .collect()
    {
        return Err(UNAVAILABLE.into());
    }
    let helper = directory.join("magi-extract");
    let checksum = directory.join("magi-extract.sha256");
    let (bytes, helper_file, helper_identity) = read_immutable(&helper, 32 * 1024 * 1024, true)?;
    let (expected, checksum_file, checksum_identity) = read_immutable(&checksum, 65, false)?;
    let expected = std::str::from_utf8(&expected)
        .map_err(|_| UNAVAILABLE)?
        .trim_end_matches('\n');
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || Digest::from_bytes(&bytes).as_str() != expected
        || bytes.get(..4) != Some(&[0xcf, 0xfa, 0xed, 0xfe])
    {
        return Err(UNAVAILABLE.into());
    }
    let cpu = bytes
        .get(4..8)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes);
    let expected_cpu = match std::env::consts::ARCH {
        "aarch64" => 0x0100_000c,
        "x86_64" => 0x0100_0007,
        _ => return Err(UNAVAILABLE.into()),
    };
    if cpu != Some(expected_cpu) {
        return Err(UNAVAILABLE.into());
    }
    signature(&helper)?;
    if identity(&fs::symlink_metadata(&helper).map_err(|_| UNAVAILABLE)?) != helper_identity
        || identity(&fs::symlink_metadata(&checksum).map_err(|_| UNAVAILABLE)?) != checksum_identity
    {
        return Err(UNAVAILABLE.into());
    }
    let directory_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(&directory)
        .map_err(|_| UNAVAILABLE)?;
    if identity(&directory_file.metadata().map_err(|_| UNAVAILABLE)?) != identity(&metadata) {
        return Err(UNAVAILABLE.into());
    }
    let mut held = vec![
        (helper.clone(), helper_file, helper_identity, true),
        (checksum, checksum_file, checksum_identity, true),
        (directory, directory_file, identity(&metadata), true),
    ];
    for ancestor in root.ancestors() {
        let (file, expected) = hold_resource_ancestor(ancestor, || {})?;
        held.push((ancestor.to_owned(), file, expected, false));
    }
    let verified = VerifiedExtractionResource {
        path: helper,
        bytes,
        held,
    };
    verified.check()?;
    Ok(verified)
}

fn hold_resource_ancestor(
    path: &Path,
    between_metadata_and_open: impl FnOnce(),
) -> Result<(fs::File, Identity), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(UNAVAILABLE.into());
    }
    let expected = identity(&metadata);
    between_metadata_and_open();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| UNAVAILABLE)?;
    let opened = identity(&file.metadata().map_err(|_| UNAVAILABLE)?);
    if (opened.0, opened.1, opened.7, opened.8) != (expected.0, expected.1, expected.7, expected.8)
    {
        return Err(UNAVAILABLE.into());
    }
    Ok((file, expected))
}

pub(crate) type Identity = (u64, u64, u64, i64, i64, i64, i64, u32, u32, u64);
pub(crate) fn identity(metadata: &fs::Metadata) -> Identity {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.mode(),
        metadata.uid(),
        metadata.nlink(),
    )
}
fn read_immutable(
    path: &Path,
    limit: u64,
    executable: bool,
) -> Result<(Vec<u8>, fs::File, Identity), String> {
    let before = fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?;
    if !before.is_file()
        || before.file_type().is_symlink()
        || before.uid() != unsafe { libc::geteuid() }
        || before.nlink() != 1
        || before.mode() & 0o222 != 0
        || before.len() == 0
        || before.len() > limit
        || (executable && before.mode() & 0o111 == 0)
    {
        return Err(UNAVAILABLE.into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| UNAVAILABLE)?;
    if identity(&file.metadata().map_err(|_| UNAVAILABLE)?) != identity(&before) {
        return Err(UNAVAILABLE.into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| UNAVAILABLE)?;
    if bytes.len() as u64 > limit
        || identity(&file.metadata().map_err(|_| UNAVAILABLE)?) != identity(&before)
        || identity(&fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?) != identity(&before)
    {
        return Err(UNAVAILABLE.into());
    }
    Ok((bytes, file, identity(&before)))
}
#[cfg(test)]
fn verify_signature(path: &Path) -> Result<(), String> {
    verify_signature_with_request(
        path,
        magi_provider::VerificationRequest::until(
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        ),
    )
}
pub(crate) fn verify_signature_with_request(
    path: &Path,
    request: magi_provider::VerificationRequest,
) -> Result<(), String> {
    magi_provider::verify_codesign_identity(path, request).map_err(|_| UNAVAILABLE.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("extraction-resource-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(root.join("extraction")).unwrap();
            let cpu: u32 = if std::env::consts::ARCH == "aarch64" {
                0x0100_000c
            } else {
                0x0100_0007
            };
            let mut bytes = vec![0xcf, 0xfa, 0xed, 0xfe];
            bytes.extend(cpu.to_le_bytes());
            bytes.extend(b"public fixture");
            fs::write(root.join("extraction/magi-extract"), &bytes).unwrap();
            fs::set_permissions(
                root.join("extraction/magi-extract"),
                fs::Permissions::from_mode(0o555),
            )
            .unwrap();
            fs::write(
                root.join("extraction/magi-extract.sha256"),
                format!("{}\n", Digest::from_bytes(&bytes)),
            )
            .unwrap();
            fs::set_permissions(
                root.join("extraction/magi-extract.sha256"),
                fs::Permissions::from_mode(0o444),
            )
            .unwrap();
            fs::set_permissions(root.join("extraction"), fs::Permissions::from_mode(0o555))
                .unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ =
                fs::set_permissions(self.0.join("extraction"), fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn mutable_ancestor_sibling_churn_preserves_custody() {
        let fixture = Fixture::new();
        let (file, expected) = hold_resource_ancestor(&fixture.0, || {
            fs::create_dir(fixture.0.join("sibling")).unwrap();
        })
        .unwrap();
        let opened = identity(&file.metadata().unwrap());
        assert_eq!(
            (opened.0, opened.1, opened.7, opened.8),
            (expected.0, expected.1, expected.7, expected.8)
        );
    }

    #[test]
    fn mutable_ancestor_replacement_is_rejected() {
        let fixture = Fixture::new();
        let ancestor = fixture.0.join("ancestor");
        fs::create_dir(&ancestor).unwrap();
        assert!(
            hold_resource_ancestor(&ancestor, || {
                fs::rename(&ancestor, fixture.0.join("retained-original")).unwrap();
                fs::create_dir(&ancestor).unwrap();
            })
            .is_err()
        );
    }

    #[test]
    fn mutable_ancestor_symlink_replacement_is_rejected() {
        let fixture = Fixture::new();
        let ancestor = fixture.0.join("ancestor");
        let retained = fixture.0.join("retained-original");
        fs::create_dir(&ancestor).unwrap();
        assert!(
            hold_resource_ancestor(&ancestor, || {
                fs::rename(&ancestor, &retained).unwrap();
                symlink(&retained, &ancestor).unwrap();
            })
            .is_err()
        );
    }

    #[test]
    fn source_section_requires_exact_unique_bounded_digest() {
        let digest = Digest::from_bytes(b"approved source");
        let mut bytes = vec![0u8; 248];
        bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&152u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&0x19u32.to_le_bytes());
        bytes[36..40].copy_from_slice(&152u32.to_le_bytes());
        bytes[96..100].copy_from_slice(&1u32.to_le_bytes());
        bytes[104..120].copy_from_slice(b"__magi_source\0\0\0");
        bytes[120..136].copy_from_slice(b"__TEXT\0\0\0\0\0\0\0\0\0\0");
        bytes[144..152].copy_from_slice(&64u64.to_le_bytes());
        bytes[152..156].copy_from_slice(&184u32.to_le_bytes());
        bytes[184..248].copy_from_slice(digest.as_str().as_bytes());
        assert_eq!(source_section(&bytes).unwrap(), digest.as_str());
        bytes[144..152].copy_from_slice(&65u64.to_le_bytes());
        assert!(source_section(&bytes).is_err());
        bytes[144..152].copy_from_slice(&64u64.to_le_bytes());
        bytes[184] = b'G';
        assert!(source_section(&bytes).is_err());
        assert!(source_section(b"unsigned source binding").is_err());
    }

    #[test]
    fn exact_helper_digest_architecture_and_signature_are_required() {
        let fixture = Fixture::new();
        let resource = verify_resource(&fixture.0, |_| Ok(())).unwrap();
        assert_eq!(resource.path(), fixture.0.join("extraction/magi-extract"));
        assert!(validate_source_binding(&resource, b"approved source").is_err());
        assert!(verify_resource(&fixture.0, |_| Err(UNAVAILABLE.into())).is_err());
        assert!(verify_resource(&fixture.0, verify_signature).is_err());
        let helper = fixture.0.join("extraction/magi-extract");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            verify_resource(&fixture.0, |_| panic!(
                "writable helper must reject before signing"
            ))
            .is_err()
        );
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o555)).unwrap();
        let checksum = fixture.0.join("extraction/magi-extract.sha256");
        fs::set_permissions(
            fixture.0.join("extraction"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::remove_file(&checksum).unwrap();
        fs::write(&checksum, "a".repeat(64)).unwrap();
        fs::set_permissions(&checksum, fs::Permissions::from_mode(0o444)).unwrap();
        fs::set_permissions(
            fixture.0.join("extraction"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        assert!(
            verify_resource(&fixture.0, |_| panic!("digest mismatch must reject first")).is_err()
        );
    }
    #[test]
    fn symlinked_and_hardlinked_resources_cannot_expand_authority() {
        let fixture = Fixture::new();
        let helper = fixture.0.join("extraction/magi-extract");
        let outside = fixture.0.join("outside");
        fs::set_permissions(
            fixture.0.join("extraction"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::rename(&helper, &outside).unwrap();
        symlink(&outside, &helper).unwrap();
        fs::set_permissions(
            fixture.0.join("extraction"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        assert!(verify_resource(&fixture.0, |_| panic!("symlink rejected first")).is_err());
        fs::set_permissions(
            fixture.0.join("extraction"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::remove_file(&helper).unwrap();
        fs::hard_link(&outside, &helper).unwrap();
        fs::set_permissions(
            fixture.0.join("extraction"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        assert!(verify_resource(&fixture.0, |_| panic!("hardlink rejected first")).is_err());
        assert_eq!(
            fs::read(&outside).unwrap().get(..4),
            Some([0xcf, 0xfa, 0xed, 0xfe].as_slice())
        );
    }
    #[test]
    #[ignore = "Requires an explicit locally signed packaged extraction resource; no document or network input."]
    fn signed_packaged_helper_passes_exact_runtime_verification() {
        let root = PathBuf::from(
            std::env::var_os("MAGI_TEST_EXTRACTION_RESOURCE_ROOT").expect("explicit resource root"),
        );
        verify_resource(&root, verify_signature).unwrap();
    }
}
