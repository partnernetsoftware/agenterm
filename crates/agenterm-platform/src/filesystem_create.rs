//! Durable exclusive creation of a new host regular file.
//!
//! The singleton-safety control log must be created exactly once by the runner,
//! without following any link and without ever opening, truncating or replacing an
//! existing object. This facade resolves the parent directory component-wise
//! without following a link-like ancestor (reusing
//! [`crate::filesystem_open::open_existing_path`]), then exclusively creates the
//! final entry relative to that retained parent, writes the exact bytes through
//! that one opened object, and flushes the same object to stable storage.
//!
//! It is a data-integrity door, not a permission or allowlist: it grants no
//! authority the caller did not already have. An open-time refusal leaves the
//! target untouched. It is **error-after-effect**: once the exclusive create
//! succeeds, a later `write_all`/`sync_all` failure can leave a partial or empty
//! file that this caller itself just created, and a returned error proves neither
//! the file's absence nor its durability. The caller must fail closed and must not
//! blindly replay the call.
//!
//! # Durability boundary
//!
//! Success proves the exact bytes were written to, and the **same opened file
//! object's** content and metadata were flushed to stable storage. It does **not**
//! prove that the parent directory entry naming this new file is crash-durable: no
//! parent-directory `fsync` is performed. A caller that needs the directory entry
//! itself durable must arrange that separately; this door never adds one.
//!
//! # Access semantics
//!
//! The privacy of the destination is the **caller's precondition**, not something
//! this door measures or enforces after the fact: the door never inspects or
//! re-asserts the parent directory's access control. On Unix the new file is
//! created with mode `0600`. On Windows the door only *inherits* the parent
//! directory's ACL: it does not itself guarantee a current-user-only or otherwise
//! private ACL. A caller that needs owner-only containment must create and protect
//! its owned parent directory before calling this door.

use std::{
    io::{self, Write as _},
    path::Path,
};

use crate::filesystem_open::{self, ExistingEntryType};

/// Create `path` as a new regular file with `bytes` and make the write durable.
///
/// Refuses an existing file, directory, FIFO, socket or any other object at the
/// final component (never truncating or replacing it), refuses a link-like final
/// component, and refuses a link-like ancestor or a missing ancestor directory.
/// The new file is owner-only on Unix (`0600`); on Windows the door only inherits
/// the parent directory's ACL and claims no private ACL of its own. The final
/// basename must be exactly one ordinary component: empty, `.`, `..` and any name
/// containing a separator are refused before the native create.
///
/// Success proves the exact bytes were written and the same opened file object's
/// content and metadata were flushed to stable storage. It does **not** prove the
/// parent directory entry is crash-durable; no parent `fsync` is performed.
///
/// A write-time, sync-time or post-create type-verification failure is
/// **error-after-effect**: nothing existing is modified, but the call may have left
/// a partial or empty file that it just created, and its durability is unproven.
/// The door never removes that file. The caller must fail closed and must not
/// blindly replay the same call.
///
/// No guarantee beyond the local filesystem's own create/sync semantics is claimed
/// for network filesystems.
pub fn create_new_regular_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // The final component is taken LEXICALLY from the raw path, never through
    // `Path::file_name`, which folds a trailing separator or `.`/`..` away and
    // would let `new/` or `new/.` silently mean `new`. A raw lexical split keeps
    // the anchor (root or prefix) plus the ordinary components the caller spelled.
    let (parent_path, file_name) = split_lexical_target(path)?;
    // Walk to the parent component-wise without following a link-like ancestor,
    // then create the final entry relative to that retained parent object.
    let parent = filesystem_open::open_existing_path(&parent_path, ExistingEntryType::Directory)?;
    let mut file = filesystem_open::create_new_regular_child(&parent, &file_name)?;
    file.write_all(bytes)?;
    // Durable: flush the same created object to stable storage. On Unix this maps
    // to fsync, on Windows to FlushFileBuffers. The parent directory entry is NOT
    // synced.
    file.sync_all()?;
    Ok(())
}

/// Split a raw path into its lexical parent path and its exact final component.
///
/// The raw encoded bytes are first checked so a trailing separator or a trailing
/// `.`/`..` cannot be folded away and turn `new/` or `new/.` into `new`. Only after
/// that shape check does the platform's own [`std::path::Path::parent`] and
/// [`std::path::Path::file_name`] produce the parent and the final component: at
/// that point they cannot fold an illegal tail, and they get the anchor right
/// (e.g. the parent of `C:\file` is `C:\`, never the drive-relative `C:`). No parent
/// path is rebuilt by hand, so no byte-level unsafe is involved.
///
/// Refused before any native call: an empty path, a path that is only a root (or a
/// prefix plus root), a trailing separator, a trailing `.` or `..`, and a path whose
/// final component is not an ordinary file name.
fn split_lexical_target(path: &Path) -> io::Result<(std::path::PathBuf, std::ffi::OsString)> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "create target must end in exactly one ordinary file-name component",
        )
    };

    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.is_empty() {
        return Err(invalid());
    }
    let is_separator = |byte: u8| byte == b'/' || (cfg!(windows) && byte == b'\\');
    // A trailing separator means the caller named a directory, not a file. On
    // Windows both `\` and `/` separate; on Unix only `/`.
    if is_separator(bytes[bytes.len() - 1]) {
        return Err(invalid());
    }
    // The raw last component is the bytes after the last separator; `.` and `..`
    // are not ordinary file names. This is exactly the folding the standard
    // accessors would hide.
    let split_at = bytes.iter().rposition(|byte| is_separator(*byte));
    let last = match split_at {
        Some(at) => &bytes[at + 1..],
        None => bytes,
    };
    if last.is_empty() || last == b"." || last == b".." {
        return Err(invalid());
    }

    // A prefix without a root is drive-relative (`C:new-file`) or otherwise has no
    // anchor directory to retain: the retained-parent walk would bind a
    // drive-relative current directory, which no caller can mean. Refuse it before
    // the standard accessors.
    #[cfg(windows)]
    {
        let mut components = path.components();
        if matches!(components.next(), Some(std::path::Component::Prefix(_)))
            && !matches!(components.next(), Some(std::path::Component::RootDir))
        {
            return Err(invalid());
        }
    }

    // The shape is valid, so the standard accessors give the platform-correct
    // parent and final component without folding an illegal tail. `parent()` is
    // `None` only for a bare anchor and an empty path, both refused above; for a
    // bare root `file_name()` is `None`, also refused.
    let parent = path.parent().ok_or_else(invalid)?;
    let parent = if parent.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        parent
    };
    let file_name = path.file_name().ok_or_else(invalid)?.to_os_string();
    Ok((parent.to_path_buf(), file_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    /// Drop a Windows verbatim `\\?\` prefix so a fixture base behaves like an
    /// ordinary caller path. `Path::join` on a verbatim base strips a trailing
    /// separator from the appended component, which would erase the `new/`
    /// spelling before the door could refuse it.
    #[cfg(windows)]
    fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
        path
    }

    #[cfg(not(windows))]
    fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
        path
    }

    fn fixture(label: &str) -> PathBuf {
        // The component-wise opener refuses symlinked ancestors, and macOS points
        // TMPDIR at /var/... where /var is a symlink to private/var. Resolve the
        // temporary root first so fixtures exercise the walk itself.
        let base = std::env::temp_dir();
        let base = std::fs::canonicalize(&base).unwrap_or(base);
        let base = strip_verbatim_prefix(base);
        base.join(format!(
            "agenterm-platform-create-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn creates_exact_bytes_and_reads_back() {
        let root = fixture("happy");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let target = root.join("control-log.jsonl");

        create_new_regular_durable(&target, b"record-1\n").expect("create new file");
        assert_eq!(fs::read(&target).unwrap(), b"record-1\n");

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn split_resolves_a_posix_root_and_relative_parent() {
        // Pure split only: no disk is touched.
        let (parent, name) = split_lexical_target(std::path::Path::new("/new-file")).unwrap();
        assert_eq!(parent, std::path::Path::new("/"));
        assert_eq!(name, std::ffi::OsString::from("new-file"));

        let (parent, name) = split_lexical_target(std::path::Path::new("new-file")).unwrap();
        assert_eq!(parent, std::path::Path::new("."));
        assert_eq!(name, std::ffi::OsString::from("new-file"));

        let (parent, name) = split_lexical_target(std::path::Path::new("../dir/new-file")).unwrap();
        assert_eq!(parent, std::path::Path::new("../dir"));
        assert_eq!(name, std::ffi::OsString::from("new-file"));
    }

    #[cfg(windows)]
    #[test]
    fn split_resolves_a_drive_root_not_a_drive_relative_directory() {
        // Pure split only: no disk is touched. The parent of `C:\file` is the root
        // `C:\`, never the drive-relative current directory `C:`.
        let (parent, name) = split_lexical_target(std::path::Path::new("C:\\new-file")).unwrap();
        assert_eq!(parent, std::path::Path::new("C:\\"));
        assert_eq!(name, std::ffi::OsString::from("new-file"));

        // A drive-relative spelling (`C:new-file`, no separator) has no anchor
        // directory to retain and must be refused rather than silently reaching a
        // drive-relative parent.
        assert!(split_lexical_target(std::path::Path::new("C:new-file")).is_err());

        // A UNC root keeps the server and share as the parent.
        let (parent, name) =
            split_lexical_target(std::path::Path::new("\\\\server\\share\\new-file")).unwrap();
        assert_eq!(parent, std::path::Path::new("\\\\server\\share\\"));
        assert_eq!(name, std::ffi::OsString::from("new-file"));

        // A verbatim (extended-length) path keeps its prefix as the parent anchor.
        let (parent, name) =
            split_lexical_target(std::path::Path::new("\\\\?\\C:\\new-file")).unwrap();
        assert_eq!(parent, std::path::Path::new("\\\\?\\C:\\"));
        assert_eq!(name, std::ffi::OsString::from("new-file"));
    }

    #[test]
    fn refuses_a_non_component_basename_before_native_create() {
        let root = fixture("basename");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");

        // The target does not exist yet: `new/` and `new/.` must NOT fold to a
        // creation of `new`. An existing-root collision would be a false green, so
        // the check is that the un-created name is still absent afterwards.
        assert!(create_new_regular_durable(&root.join("new"), b"x").is_ok());
        fs::remove_file(root.join("new")).expect("remove first");

        for candidate in ["new/", "new/.", "new/..", ".", "..", ""] {
            let target = root.join(candidate);
            assert!(
                create_new_regular_durable(&target, b"x").is_err(),
                "must refuse {candidate:?}"
            );
        }
        // The raw path itself (a bare anchor and the root path) is refused too.
        assert!(create_new_regular_durable(std::path::Path::new("/"), b"x").is_err());
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            0,
            "no `new` may appear from a folded trailing separator or dot"
        );

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn refuses_an_existing_regular_file_and_leaves_it_unchanged() {
        let root = fixture("existing");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let target = root.join("control-log.jsonl");
        fs::write(&target, b"canary").expect("seed existing file");

        // The exclusive create may not truncate the existing bytes, but a
        // userland read-back alone cannot prove that, so the check is the exact
        // continuation below.
        assert!(create_new_regular_durable(&target, b"record").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"canary");

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn refuses_a_missing_ancestor_directory() {
        let root = fixture("missing-ancestor");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let target = root.join("absent").join("control-log.jsonl");

        let error = create_new_regular_durable(&target, b"x").expect_err("missing ancestor");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!root.join("absent").exists());

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn refuses_a_directory_collision_and_leaves_it() {
        let root = fixture("directory");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let target = root.join("occupied");
        fs::create_dir(&target).expect("seed directory");

        assert!(create_new_regular_durable(&target, b"x").is_err());
        assert!(target.is_dir());

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_final_symlink_and_leaves_its_target_unchanged() {
        use std::os::unix::fs::symlink;
        let root = fixture("final-link");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let outside = root.join("outside");
        fs::write(&outside, b"canary").expect("write canary");
        let link = root.join("link");
        symlink(&outside, &link).expect("create symlink");

        assert!(create_new_regular_durable(&link, b"x").is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"canary");

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_an_ancestor_symlink_and_leaves_its_target_absent() {
        use std::os::unix::fs::symlink;
        let root = fixture("ancestor-link");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let real = root.join("real");
        fs::create_dir(&real).expect("create real dir");
        let link = root.join("junc");
        symlink(&real, &link).expect("create dir symlink");

        assert!(create_new_regular_durable(&link.join("log"), b"x").is_err());
        assert!(!real.join("log").exists());

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_fifo_collision_without_writing() {
        use std::os::unix::ffi::OsStrExt as _;
        let root = fixture("fifo");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let fifo = root.join("pipe");
        let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        let created = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
        assert_eq!(created, 0, "mkfifo failed: {}", io::Error::last_os_error());

        // O_EXCL refuses the existing FIFO regardless of a reader, so the create
        // never opens or writes it.
        assert!(create_new_regular_durable(&fifo, b"x").is_err());

        // Survives as a FIFO holding no byte: a non-blocking reader returns
        // WouldBlock (or EOF when no writer ever opened it).
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        assert!(fd >= 0, "open fifo reader");
        let mut byte = [0u8; 1];
        let n = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), 1) };
        assert!(n <= 0, "fifo must hold no byte (read {n})");
        unsafe { libc::close(fd) };

        fs::remove_file(&fifo).expect("remove fifo");
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn two_creators_race_and_exactly_one_wins_without_overwrite() {
        use std::sync::{Arc, Barrier};

        let root = fixture("race");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let target = root.join("control-log.jsonl");

        // Two threads race through a barrier on the SAME path with different
        // bytes. Exactly one exclusive create may succeed and the final bytes must
        // be exactly that winner's payload -- never a splice or an overwrite.
        let barrier = Arc::new(Barrier::new(2));
        let first_path = target.clone();
        let second_path = target.clone();
        let first_barrier = Arc::clone(&barrier);
        let second_barrier = Arc::clone(&barrier);
        let first = std::thread::spawn(move || {
            first_barrier.wait();
            create_new_regular_durable(&first_path, b"first-payload").is_ok()
        });
        let second = std::thread::spawn(move || {
            second_barrier.wait();
            create_new_regular_durable(&second_path, b"second-payload").is_ok()
        });
        let first_won = first.join().expect("first thread");
        let second_won = second.join().expect("second thread");

        assert!(
            first_won ^ second_won,
            "exactly one creator must win (first={first_won}, second={second_won})"
        );
        let bytes = fs::read(&target).expect("read winner payload");
        let expected: &[u8] = if first_won {
            b"first-payload"
        } else {
            b"second-payload"
        };
        assert_eq!(bytes, expected, "final bytes must be exactly the winner's");

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn creates_with_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = fixture("mode");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let target = root.join("private");
        create_new_regular_durable(&target, b"x").expect("create new file");
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "new file must be owner-only");
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn retained_parent_identity_confines_the_create() {
        // Retain the parent handle, move the original directory elsewhere and put a
        // same-named impostor at the old path. The create must land only under the
        // renamed ORIGINAL, proving the retained object -- not the path -- chooses
        // the directory.
        let base = fixture("identity");
        let original = base.join("root");
        let retained = base.join("retained");
        let impostor = base.join("impostor");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&original).expect("create original");
        fs::create_dir(&impostor).expect("create impostor");

        let parent = filesystem_open::open_existing_path(&original, ExistingEntryType::Directory)
            .expect("retain original parent");
        fs::rename(&original, &retained).expect("rename original");
        fs::rename(&impostor, &original).expect("install impostor");

        let mut file =
            filesystem_open::create_new_regular_child(&parent, std::ffi::OsStr::new("log"))
                .expect("create through retained parent");
        file.write_all(b"owned").expect("write owned");
        file.sync_all().expect("sync owned");
        drop(file);

        assert_eq!(fs::read(retained.join("log")).unwrap(), b"owned");
        assert!(
            !original.join("log").exists(),
            "the impostor at the old path must gain no file"
        );

        fs::remove_dir_all(base).expect("remove fixture");
    }

    #[cfg(windows)]
    #[test]
    fn windows_creates_regular_and_syncs() {
        let root = fixture("win-happy");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let target = root.join("control-log");
        create_new_regular_durable(&target, b"seed\n").expect("create new file");
        assert_eq!(fs::read(&target).unwrap(), b"seed\n");
        assert!(create_new_regular_durable(&target, b"more").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"seed\n");
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(windows)]
    #[test]
    fn windows_refuses_an_alternate_data_stream_and_leaves_the_base_file() {
        let root = fixture("win-ads");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let base = root.join("canary");
        fs::write(&base, b"base-bytes").expect("seed canary");
        let before = fs::metadata(&base).expect("canary metadata").len();

        // `canary:stream` would add an alternate data stream to the existing base
        // file if it reached the kernel; the door must refuse it first.
        assert!(
            create_new_regular_durable(&root.join("canary:stream"), b"x").is_err(),
            "an ADS name must be refused"
        );
        assert!(
            create_new_regular_durable(&root.join("canary::$DATA"), b"x").is_err(),
            "the unnamed-stream spelling must be refused"
        );
        assert_eq!(
            fs::read(&base).unwrap(),
            b"base-bytes",
            "base bytes unchanged"
        );
        assert_eq!(
            fs::metadata(&base).expect("canary metadata").len(),
            before,
            "base metadata unchanged"
        );

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(windows)]
    #[test]
    fn windows_refuses_a_final_reparse_and_leaves_its_target() {
        use std::os::windows::fs::symlink_file;
        let root = fixture("win-final-link");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let outside = root.join("outside");
        fs::write(&outside, b"canary").expect("write canary");
        let link = root.join("link");
        // Creating a symlink needs a privilege some hosts lack; a failure is a
        // typed refusal to exercise the case, never a silent skip of the test.
        match symlink_file(&outside, &link) {
            Ok(()) => {
                assert!(create_new_regular_durable(&link, b"x").is_err());
                assert_eq!(fs::read(&outside).unwrap(), b"canary");
            }
            Err(error) => {
                eprintln!("windows final-reparse case not exercisable here: {error}");
                assert!(
                    matches!(error.kind(), std::io::ErrorKind::PermissionDenied),
                    "an un-creatable symlink must be a privilege refusal, not another error: {error}"
                );
            }
        }
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(windows)]
    #[test]
    fn windows_refuses_an_ancestor_junction_and_leaves_its_target_absent() {
        let root = fixture("win-ancestor");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let real = root.join("real");
        fs::create_dir(&real).expect("create real dir");
        let junction = root.join("junc");
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&real)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run mklink junction fixture");
        assert!(status.success(), "mklink /J fixture failed: {status}");

        assert!(create_new_regular_durable(&junction.join("log"), b"x").is_err());
        assert!(
            !real.join("log").exists(),
            "junction target must gain no file"
        );

        let _ = fs::remove_dir(&junction);
        fs::remove_dir_all(root).expect("remove fixture");
    }
}
