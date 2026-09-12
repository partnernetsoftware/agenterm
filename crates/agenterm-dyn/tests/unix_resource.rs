//! Public contract tests for the owned Unix `getifaddrs` list.

#[cfg(windows)]
use agenterm_dyn::InterfaceAddressesError;
use agenterm_dyn::{ALL_CELLS, InterfaceAddresses, SystemProbeStatus};

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

#[cfg(windows)]
#[test]
fn acquisition_is_honestly_unsupported_on_windows() {
    assert!(matches!(
        InterfaceAddresses::acquire(),
        Err(InterfaceAddressesError::Unsupported)
    ));
}
