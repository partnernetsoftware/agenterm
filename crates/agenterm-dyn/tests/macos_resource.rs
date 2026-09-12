//! Typed ownership tests for Darwin's `mach_host_self` send right.

use agenterm_dyn::{
    ALL_CELLS, CpuCountSnapshot, DlAddressSnapshot, MachHostPort, MachTimebaseSnapshot,
    SystemProbeStatus,
};
#[cfg(not(target_os = "macos"))]
use agenterm_dyn::{
    CpuCountError, DlAddressError, DomainNameError, DomainNameSnapshot, LoginNameError,
    LoginNameSnapshot, MachHostPortError, MachTimebaseError,
};

#[test]
fn only_darwin_catalogues_mach_host_self_as_owned_live() {
    for cell in ALL_CELLS {
        let probe = cell
            .system_probes
            .iter()
            .find(|probe| probe.name == "mach_host_self")
            .unwrap_or_else(|| panic!("{} × {} catalogues mach_host_self", cell.os, cell.arch));
        if cell.os == "macos" {
            assert_eq!(
                probe.status,
                SystemProbeStatus::LiveOwned {
                    api: "MachHostPort::acquire"
                }
            );
        } else {
            assert!(matches!(probe.status, SystemProbeStatus::Placeholder));
        }
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn acquisition_is_honestly_unsupported_off_darwin() {
    assert!(matches!(
        MachHostPort::acquire(),
        Err(MachHostPortError::Unsupported)
    ));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn current_image_snapshot_is_honestly_unsupported_off_darwin() {
    assert_eq!(
        DlAddressSnapshot::current_image(),
        Err(DlAddressError::Unsupported)
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn cpu_count_snapshot_is_honestly_unsupported_off_darwin() {
    assert_eq!(CpuCountSnapshot::acquire(), Err(CpuCountError::Unsupported));
}

#[cfg(not(target_os = "macos"))]
#[test]
fn domain_name_snapshot_is_honestly_unsupported_off_darwin() {
    assert_eq!(
        DomainNameSnapshot::acquire(),
        Err(DomainNameError::Unsupported)
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn login_name_snapshot_is_honestly_unsupported_off_darwin() {
    assert_eq!(
        LoginNameSnapshot::acquire(),
        Err(LoginNameError::Unsupported)
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn mach_timebase_snapshot_is_honestly_unsupported_off_darwin() {
    assert_eq!(
        MachTimebaseSnapshot::acquire(),
        Err(MachTimebaseError::Unsupported)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn acquisition_adds_one_send_ref_and_drop_releases_exactly_that_ref() {
    let survivor = MachHostPort::acquire().expect("first owned host send right");
    let before = survivor.send_right_refs().expect("initial send-right refs");

    let second = MachHostPort::acquire().expect("second owned host send right");
    assert_eq!(
        second.send_right_refs().expect("refs after second acquire"),
        before + 1
    );

    drop(second);
    assert_eq!(
        survivor.send_right_refs().expect("refs after second drop"),
        before
    );
}

#[cfg(target_os = "macos")]
#[test]
fn current_image_snapshot_owns_native_path_bytes() {
    let snapshot = DlAddressSnapshot::current_image().expect("current image snapshot");
    assert!(!snapshot.image_path().is_empty());
    if let Some(symbol) = snapshot.symbol_name() {
        assert!(!symbol.is_empty());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn cpu_count_snapshot_is_a_live_positive_host_fact() {
    let snapshot = CpuCountSnapshot::acquire().expect("Darwin hw.ncpu snapshot");
    let mut direct = 0_u32;
    let mut direct_size = std::mem::size_of_val(&direct);
    // SAFETY: the name is NUL-terminated, both output pointers are valid, and
    // this read-only query supplies no replacement value.
    let status = unsafe {
        libc::sysctlbyname(
            c"hw.ncpu".as_ptr(),
            (&raw mut direct).cast(),
            &mut direct_size,
            std::ptr::null_mut(),
            0,
        )
    };
    assert_eq!(status, 0, "direct hw.ncpu query succeeds");
    assert_eq!(direct_size, std::mem::size_of_val(&direct));
    assert_eq!(snapshot.logical_cpus(), direct);
    assert!(snapshot.logical_cpus() > 0);
    assert!(
        snapshot.logical_cpus() as usize
            >= std::thread::available_parallelism()
                .expect("available parallelism")
                .get()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn mach_timebase_snapshot_matches_the_direct_native_ratio() {
    #[repr(C)]
    struct DirectTimebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut DirectTimebase) -> libc::c_int;
    }

    let snapshot = MachTimebaseSnapshot::acquire().expect("typed Mach timebase snapshot");
    let mut direct = DirectTimebase { numer: 0, denom: 0 };
    // SAFETY: direct is complete writable storage for the native out structure.
    assert_eq!(unsafe { mach_timebase_info(&mut direct) }, 0);
    assert_eq!(snapshot.numerator(), direct.numer);
    assert_eq!(snapshot.denominator(), direct.denom);
}
