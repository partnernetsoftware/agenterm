//! Public contract tests for pointer-free Unix resource snapshots.

use agenterm_dyn::{
    ALL_CELLS, ClockId, ClockSnapshot, InterfaceAddresses, StatVfsError, StatVfsSnapshot,
    SystemProbeStatus,
};
#[cfg(windows)]
use agenterm_dyn::{ClockSnapshotError, InterfaceAddressesError};
use std::path::Path;

#[test]
fn unix_cells_catalogue_getifaddrs_as_owned_and_windows_stays_placeholder() {
    for cell in ALL_CELLS {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "getifaddrs")
            .unwrap_or_else(|| panic!("{} × {} catalogues getifaddrs", cell.os, cell.arch));
        if matches!(cell.os, "linux" | "macos") {
            assert_eq!(
                probe.status,
                SystemProbeStatus::LiveOwned {
                    api: "InterfaceAddresses::acquire"
                }
            );
        } else {
            assert_eq!(probe.status, SystemProbeStatus::Placeholder);
        }
    }
}

#[cfg(unix)]
#[test]
fn live_snapshot_is_nonempty_and_contains_structurally_valid_names() {
    let owner = InterfaceAddresses::acquire().expect("getifaddrs succeeds on the live Unix host");
    let snapshot = owner.snapshot().expect("bounded pointer-free snapshot");
    assert!(!snapshot.is_empty());
    assert!(snapshot.iter().all(|entry| !entry.name.is_empty()));
    assert!(snapshot.iter().all(|entry| !entry.name.contains(&0)));
}

#[cfg(unix)]
fn direct_clock(clock: libc::clockid_t) -> ClockSnapshot {
    let mut native = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: native is writable storage and the caller supplies a libc clock id.
    assert_eq!(
        unsafe { libc::clock_gettime(clock, native.as_mut_ptr()) },
        0
    );
    // SAFETY: a successful call initialized the complete timespec.
    let native = unsafe { native.assume_init() };
    ClockSnapshot {
        seconds: native.tv_sec,
        nanoseconds: u32::try_from(native.tv_nsec).expect("native nanoseconds are nonnegative"),
    }
}

#[cfg(unix)]
#[test]
fn controlled_clocks_return_bounded_pointer_free_snapshots() {
    for (clock, native_id) in [
        (ClockId::Realtime, libc::CLOCK_REALTIME),
        (ClockId::Monotonic, libc::CLOCK_MONOTONIC),
    ] {
        let before = direct_clock(native_id);
        let snapshot = ClockSnapshot::acquire(clock).expect("controlled native clock succeeds");
        let after = direct_clock(native_id);
        assert!(snapshot.nanoseconds < 1_000_000_000);
        assert!(before <= snapshot, "snapshot precedes the first oracle");
        assert!(snapshot <= after, "snapshot follows the second oracle");
    }
}

#[cfg(unix)]
#[test]
fn statvfs_snapshot_copies_stable_root_filesystem_facts() {
    let snapshot =
        StatVfsSnapshot::acquire(Path::new("/")).expect("statvfs snapshots the root filesystem");
    assert!(snapshot.block_size > 0);
    assert!(snapshot.fragment_size > 0);
    assert!(snapshot.maximum_name_bytes > 0);

    let mut direct = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the path is a static NUL-terminated byte string and `direct`
    // provides writable storage for one complete statvfs result.
    let status = unsafe { libc::statvfs(c"/".as_ptr(), direct.as_mut_ptr()) };
    assert_eq!(status, 0);
    // SAFETY: the successful call initialized the complete result.
    let direct = unsafe { direct.assume_init() };
    assert_eq!(snapshot.block_size, direct.f_bsize);
    assert_eq!(snapshot.fragment_size, direct.f_frsize);
    assert_eq!(snapshot.blocks, u64::from(direct.f_blocks));
    assert_eq!(snapshot.blocks_free, u64::from(direct.f_bfree));
    assert_eq!(snapshot.files, u64::from(direct.f_files));
    assert_eq!(snapshot.maximum_name_bytes, direct.f_namemax);
}

#[cfg(unix)]
#[test]
fn statvfs_rejects_an_interior_nul_before_calling_the_os() {
    use std::os::unix::ffi::OsStrExt;

    let path = Path::new(std::ffi::OsStr::from_bytes(b"bad\0path"));
    assert_eq!(
        StatVfsSnapshot::acquire(path),
        Err(StatVfsError::InputContainsNul)
    );
}

#[cfg(unix)]
#[test]
fn statvfs_preserves_the_os_error_for_a_missing_path() {
    let error = StatVfsSnapshot::acquire(Path::new(
        "target/agenterm-dyn-statvfs-definitely-missing/path",
    ))
    .expect_err("missing path must fail");
    assert!(matches!(error, StatVfsError::Os(code) if code > 0));
}

#[cfg(windows)]
#[test]
fn acquisition_is_honestly_unsupported_on_windows() {
    assert!(matches!(
        InterfaceAddresses::acquire(),
        Err(InterfaceAddressesError::Unsupported)
    ));
    assert!(matches!(
        StatVfsSnapshot::acquire(Path::new(".")),
        Err(StatVfsError::Unsupported)
    ));
    assert_eq!(
        ClockSnapshot::acquire(ClockId::Realtime),
        Err(ClockSnapshotError::Unsupported)
    );
}
