//! Typed ownership tests for Darwin's `mach_host_self` send right.

#[cfg(not(target_os = "macos"))]
use agenterm_dyn::MachHostPortError;
use agenterm_dyn::{ALL_CELLS, MachHostPort, SystemProbeStatus};

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
