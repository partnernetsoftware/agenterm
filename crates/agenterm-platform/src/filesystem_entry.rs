//! Lightweight host filesystem entry classification and identity-safe metadata.

use std::{
    fs::{File, Metadata},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// Product-neutral facts for one already-resolved filesystem object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilesystemEntryFacts {
    directory: bool,
    regular_file: bool,
    link_like: bool,
}

/// Bounded, product-neutral metadata for one named filesystem entry.
///
/// `inspect_path` uses `symlink_metadata`, so the final link/reparse object is
/// described rather than silently following it to a different authority
/// target. Wide counters remain integers here; JSON-facing callers should
/// publish them as decimal strings when their consumer may be JavaScript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemEntryMetadata {
    pub facts: FilesystemEntryFacts,
    pub identity: Option<String>,
    pub length: u64,
    pub readonly: bool,
    pub created_unix_ns: Option<i128>,
    pub modified_unix_ns: Option<i128>,
    pub accessed_unix_ns: Option<i128>,
    pub unix_mode: Option<u32>,
    pub unix_uid: Option<u32>,
    pub unix_gid: Option<u32>,
    pub windows_attributes: Option<u32>,
}

struct NativeEntryDetails {
    identity: Option<String>,
    unix_mode: Option<u32>,
    unix_uid: Option<u32>,
    unix_gid: Option<u32>,
    windows_attributes: Option<u32>,
}

impl FilesystemEntryFacts {
    #[must_use]
    pub const fn is_directory(self) -> bool {
        self.directory
    }

    #[must_use]
    pub const fn is_file(self) -> bool {
        self.regular_file
    }

    #[must_use]
    pub const fn is_link_like(self) -> bool {
        self.link_like
    }

    #[must_use]
    pub const fn is_real_directory(self) -> bool {
        self.directory && !self.link_like
    }

    #[must_use]
    pub const fn is_real_file(self) -> bool {
        self.regular_file && !self.link_like
    }
}

/// Classify metadata without resolving another path.
#[must_use]
pub fn metadata_entry_facts(metadata: &Metadata) -> FilesystemEntryFacts {
    FilesystemEntryFacts {
        directory: metadata.is_dir(),
        regular_file: metadata.is_file(),
        link_like: crate::selected::filesystem_entry::metadata_is_link_like(metadata),
    }
}

/// Classify the object referenced by an already-open file handle.
///
/// The caller chooses whether the native open followed the final link. For
/// component-wise traversal, open the link object itself (`O_NOFOLLOW`/an
/// equivalent reparse-point option) before calling this function. No path is
/// reopened here, so classification cannot race a second name resolution.
pub fn opened_file_entry_facts(file: &File) -> std::io::Result<FilesystemEntryFacts> {
    file.metadata()
        .map(|metadata| metadata_entry_facts(&metadata))
}

/// Whether metadata obtained without following the final component describes
/// a host link-like entry.
///
/// Unix classifies symbolic links. Windows classifies every reparse point,
/// including junctions and symbolic links, because generic recursive callers
/// must not assume that any of them is an ordinary directory.
#[must_use]
pub fn metadata_is_link_like(metadata: &Metadata) -> bool {
    metadata_entry_facts(metadata).is_link_like()
}

/// Whether metadata describes an ordinary directory that is safe to treat as
/// a directory entry rather than as a host link-like object.
#[must_use]
pub fn metadata_is_real_directory(metadata: &Metadata) -> bool {
    metadata_entry_facts(metadata).is_real_directory()
}

/// Inspect the final directory entry without following a link-like object.
pub fn inspect_path(path: &Path) -> std::io::Result<FilesystemEntryMetadata> {
    let metadata = std::fs::symlink_metadata(path)?;
    Ok(metadata_details(&metadata))
}

fn timestamp_ns(value: std::io::Result<SystemTime>) -> Option<i128> {
    let value = value.ok()?;
    Some(match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok()?,
        Err(error) => -i128::try_from(error.duration().as_nanos()).ok()?,
    })
}

#[cfg(unix)]
fn native_details(metadata: &Metadata) -> NativeEntryDetails {
    use std::os::unix::fs::MetadataExt as _;
    NativeEntryDetails {
        identity: Some(format!("unix:{:x}:{:x}", metadata.dev(), metadata.ino())),
        unix_mode: Some(metadata.mode()),
        unix_uid: Some(metadata.uid()),
        unix_gid: Some(metadata.gid()),
        windows_attributes: None,
    }
}

#[cfg(windows)]
fn native_details(metadata: &Metadata) -> NativeEntryDetails {
    use std::os::windows::fs::MetadataExt as _;
    NativeEntryDetails {
        // Stable Windows object identity requires an opened handle and the
        // separate `file-identity` facade. MetadataExt's by-handle identity
        // accessors are unstable; do not replace them with a path spelling.
        identity: None,
        unix_mode: None,
        unix_uid: None,
        unix_gid: None,
        windows_attributes: Some(metadata.file_attributes()),
    }
}

#[cfg(not(any(unix, windows)))]
fn native_details(_metadata: &Metadata) -> NativeEntryDetails {
    NativeEntryDetails {
        identity: None,
        unix_mode: None,
        unix_uid: None,
        unix_gid: None,
        windows_attributes: None,
    }
}

fn metadata_details(metadata: &Metadata) -> FilesystemEntryMetadata {
    let native = native_details(metadata);
    FilesystemEntryMetadata {
        facts: metadata_entry_facts(metadata),
        identity: native.identity,
        length: metadata.len(),
        readonly: metadata.permissions().readonly(),
        created_unix_ns: timestamp_ns(metadata.created()),
        modified_unix_ns: timestamp_ns(metadata.modified()),
        accessed_unix_ns: timestamp_ns(metadata.accessed()),
        unix_mode: native.unix_mode,
        unix_uid: native.unix_uid,
        unix_gid: native.unix_gid,
        windows_attributes: native.windows_attributes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn fixture(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agenterm-platform-entry-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn distinguishes_ordinary_files_and_directories() {
        let root = fixture("ordinary");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create entry fixture");
        let file = root.join("file");
        fs::write(&file, b"value").expect("write entry fixture");

        let directory = fs::symlink_metadata(&root).expect("directory metadata");
        let file = fs::symlink_metadata(&file).expect("file metadata");
        assert!(metadata_is_real_directory(&directory));
        assert!(!metadata_is_link_like(&directory));
        assert!(!metadata_is_real_directory(&file));
        assert!(!metadata_is_link_like(&file));
        let opened = fs::File::open(root.join("file")).expect("open entry fixture");
        assert!(
            opened_file_entry_facts(&opened)
                .expect("classify opened file")
                .is_real_file()
        );
        let details = inspect_path(&root.join("file")).expect("inspect entry");
        assert!(details.facts.is_real_file());
        assert_eq!(details.length, 5);
        assert!(details.identity.is_some());
        assert!(details.modified_unix_ns.is_some());
        fs::remove_dir_all(root).expect("remove entry fixture");
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_is_link_like_not_a_real_directory() {
        use std::os::unix::fs::symlink;

        let root = fixture("unix-link");
        let outside = fixture("unix-link-outside");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir(&root).expect("create link fixture");
        fs::create_dir(&outside).expect("create link target");
        let link = root.join("link");
        symlink(&outside, &link).expect("create directory symlink");

        let metadata = fs::symlink_metadata(&link).expect("link metadata");
        assert!(metadata_is_link_like(&metadata));
        assert!(!metadata_is_real_directory(&metadata));
        let details = inspect_path(&link).expect("inspect link itself");
        assert!(details.facts.is_link_like());
        assert_ne!(details.identity, inspect_path(&outside).unwrap().identity);
        fs::remove_dir_all(root).expect("remove link fixture");
        fs::remove_dir_all(outside).expect("remove link target");
    }

    #[cfg(windows)]
    #[test]
    fn directory_junction_is_link_like_not_a_real_directory() {
        let root = fixture("windows-junction");
        let outside = fixture("windows-junction-outside");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir(&root).expect("create junction fixture");
        fs::create_dir(&outside).expect("create junction target");
        let junction = root.join("junction");
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run mklink junction fixture");
        assert!(status.success(), "mklink /J fixture failed: {status}");

        let metadata = fs::symlink_metadata(&junction).expect("junction metadata");
        assert!(metadata_is_link_like(&metadata));
        assert!(!metadata_is_real_directory(&metadata));

        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let opened = fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(&junction)
            .expect("open junction itself");
        let opened = opened_file_entry_facts(&opened).expect("classify opened junction");
        assert!(opened.is_link_like());
        assert!(!opened.is_real_directory());
        assert!(!opened.is_real_file());
        fs::remove_dir(&junction).expect("remove junction");
        fs::remove_dir(&root).expect("remove junction fixture");
        fs::remove_dir_all(outside).expect("remove junction target");
    }
}
