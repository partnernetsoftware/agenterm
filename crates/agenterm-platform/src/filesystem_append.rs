//! Durable append to an existing host file.
//!
//! The singleton-safety control log must be appended without following any link
//! and without creating a missing file, and each write must be durable before the
//! call returns. This facade opens the existing regular file component-wise
//! without following a link-like final or ancestor component (reusing
//! [`crate::filesystem_open::open_existing_path`]), appends the exact bytes
//! through that one opened object, and flushes the same object to stable storage.
//!
//! It is a data-integrity door, not a permission or allowlist: it grants no
//! authority the caller did not already have. It never creates, deletes, truncates
//! or replaces a target, but it is **error-after-effect**: `write_all` may append a
//! partial prefix and `sync_all` may fail after the bytes were already appended, so
//! a returned error does not prove the target is unchanged and does not prove any
//! appended bytes are durable. A caller must fail closed and must not replay the
//! same record.

use std::{
    io::{self, Write as _},
    path::Path,
};

use crate::filesystem_open::{self, ExistingEntryAccess, ExistingEntryType};

/// Append `bytes` to an existing regular file and make the write durable.
///
/// Refuses a missing file, a directory, a FIFO or any other non-regular object,
/// and refuses a link-like final component or a link-like ancestor. An open-time
/// refusal happens before any write, so those failures leave the target untouched.
/// A write-time or sync-time failure is **error-after-effect**: nothing is created,
/// deleted, truncated or replaced, but the append may have partially or fully
/// happened and its durability is unproven. The caller must fail closed and must
/// not replay the same record.
///
/// The append is a single-writer contract: the caller must serialize its writers.
/// No guarantee beyond the local filesystem's own `O_APPEND`/append-right
/// semantics is claimed for network filesystems.
pub fn append_existing_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // Component-wise no-follow open of an existing file, opened for append.
    let mut file = filesystem_open::open_existing_path_with_access(
        path,
        ExistingEntryType::File,
        ExistingEntryAccess::Append,
    )?;
    file.write_all(bytes)?;
    // Durable: flush the same opened object to stable storage. On Unix this maps
    // to fsync, on Windows to FlushFileBuffers.
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    fn fixture(label: &str) -> PathBuf {
        // Componentwise opening refuses symlinked ancestors, so resolve the temp
        // root first (macOS points TMPDIR under a symlinked /var).
        let base = std::env::temp_dir();
        let base = std::fs::canonicalize(&base).unwrap_or(base);
        base.join(format!(
            "agenterm-platform-append-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn appends_twice_and_preserves_the_prefix() {
        let root = fixture("twice");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let path = root.join("control-log");
        fs::write(&path, b"seed\n").expect("write seed");

        append_existing_durable(&path, b"one\n").expect("first append");
        append_existing_durable(&path, b"two\n").expect("second append");
        assert_eq!(fs::read(&path).unwrap(), b"seed\none\ntwo\n");

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn missing_file_is_refused_and_not_created() {
        let root = fixture("missing");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let path = root.join("absent");

        let error = append_existing_durable(&path, b"x").expect_err("missing must fail");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!path.exists(), "missing target must not be created");

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn directory_is_refused() {
        let root = fixture("dir");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        assert!(
            append_existing_durable(&root, b"x").is_err(),
            "directory must be refused"
        );
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_is_refused_and_target_unchanged() {
        use std::os::unix::fs::symlink;
        let root = fixture("final-link");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let outside = root.join("outside");
        fs::write(&outside, b"canary").expect("write canary");
        symlink(&outside, root.join("link")).expect("create symlink");

        assert!(
            append_existing_durable(&root.join("link"), b"x").is_err(),
            "final symlink must be refused"
        );
        assert_eq!(
            fs::read(&outside).unwrap(),
            b"canary",
            "target bytes unchanged"
        );

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlink_is_refused_and_target_unchanged() {
        use std::os::unix::fs::symlink;
        let root = fixture("ancestor-link");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let real = root.join("real");
        fs::create_dir(&real).expect("create real dir");
        fs::write(real.join("log"), b"canary").expect("write canary");
        symlink(&real, root.join("alias")).expect("create ancestor symlink");

        assert!(
            append_existing_durable(&root.join("alias").join("log"), b"x").is_err(),
            "ancestor symlink must be refused"
        );
        assert_eq!(
            fs::read(real.join("log")).unwrap(),
            b"canary",
            "target bytes unchanged"
        );

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_refused_and_no_bytes_are_written() {
        use std::os::{
            fd::{FromRawFd as _, IntoRawFd as _},
            unix::ffi::OsStrExt as _,
        };

        let root = fixture("fifo");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let fifo = root.join("pipe");
        let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        let created = unsafe { libc::mkfifo(c.as_ptr(), 0o600) };
        // Creation failure must fail the test, never silently skip it.
        assert_eq!(created, 0, "mkfifo failed: {}", io::Error::last_os_error());

        // Hold a non-blocking reader so the writer-side open succeeds (a FIFO open
        // for write blocks until a reader exists). The refusal must then come from
        // the same-handle regular-type check, not from a missing reader.
        let reader_fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        assert!(
            reader_fd >= 0,
            "open fifo reader: {}",
            io::Error::last_os_error()
        );
        let reader = unsafe { std::fs::File::from_raw_fd(reader_fd) };
        // Keep the raw fd alive independent of the File's drop order.
        let reader_fd = reader.into_raw_fd();
        let reader = unsafe { std::fs::File::from_raw_fd(reader_fd) };

        let error = append_existing_durable(&fifo, b"x").expect_err("fifo must be refused");
        assert_eq!(
            error.kind(),
            io::ErrorKind::InvalidData,
            "regular-type check"
        );
        drop(reader);

        // No byte was delivered to the pipe: a non-blocking read returns WouldBlock.
        let rf = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        assert!(rf >= 0, "reopen fifo reader");
        let mut byte = [0u8; 1];
        let n = unsafe { libc::read(rf, byte.as_mut_ptr().cast(), 1) };
        // No byte was appended: either the fifo is empty (WouldBlock) or the short
        // writer already closed its end (EOF, 0). A positive read would mean the
        // refused append still delivered bytes.
        assert!(n <= 0, "no byte must be written to the fifo (read {n})");
        unsafe { libc::close(rf) };

        fs::remove_file(&fifo).expect("remove fifo");
        fs::remove_dir_all(root).expect("remove fixture");
    }
    #[cfg(windows)]
    #[test]
    fn windows_appends_regular_and_syncs() {
        let root = fixture("win-regular");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let path = root.join("control-log");
        fs::write(&path, b"seed\n").expect("seed file");
        append_existing_durable(&path, b"one\n").expect("append");
        append_existing_durable(&path, b"two\n").expect("append");
        assert_eq!(fs::read(&path).unwrap(), b"seed\none\ntwo\n");
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(windows)]
    #[test]
    fn windows_final_reparse_is_refused_and_target_unchanged() {
        use std::os::windows::fs::symlink_file;
        let root = fixture("win-final");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let outside = root.join("outside");
        fs::write(&outside, b"canary").expect("write canary");
        let link = root.join("link");
        // A symlink_file creation failure must fail the test, never silent-skip.
        symlink_file(&outside, &link).expect("create file symlink");
        assert!(
            append_existing_durable(&link, b"x").is_err(),
            "reparse refused"
        );
        assert_eq!(fs::read(&outside).unwrap(), b"canary");
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(windows)]
    #[test]
    fn windows_ancestor_junction_is_refused_and_target_unchanged() {
        let root = fixture("win-ancestor");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("create fixture");
        let real = root.join("real");
        fs::create_dir(&real).expect("create real");
        fs::write(real.join("log"), b"canary").expect("write canary");
        let junction = root.join("junc");
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&real)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run mklink junction fixture");
        assert!(status.success(), "mklink /J failed: {status}");
        assert!(
            append_existing_durable(&junction.join("log"), b"x").is_err(),
            "ancestor junction must be refused"
        );
        assert_eq!(fs::read(real.join("log")).unwrap(), b"canary");
        let _ = fs::remove_dir(&junction);
        fs::remove_dir_all(root).expect("remove fixture");
    }
}
