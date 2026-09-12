//! Public contract tests for bounded supplementary-group snapshots.

use agenterm_dyn::SupplementaryGroups;
#[cfg(not(unix))]
use agenterm_dyn::SupplementaryGroupsError;

#[cfg(unix)]
#[test]
fn live_snapshot_matches_an_independent_process_group_oracle() {
    let snapshot = SupplementaryGroups::acquire().expect("getgroups snapshot succeeds");
    let mut actual = snapshot.into_vec();
    actual.sort_unstable();

    let output = std::process::Command::new("id")
        .arg("-G")
        .output()
        .expect("the host id utility is available");
    assert!(output.status.success(), "id -G succeeds");
    let stdout = String::from_utf8(output.stdout).expect("id -G emits UTF-8 digits");
    let mut expected = stdout
        .split_whitespace()
        .map(|value| value.parse::<u32>().expect("id -G emits numeric gids"))
        .collect::<Vec<_>>();
    expected.sort_unstable();

    // POSIX leaves it unspecified whether `getgroups` includes the effective
    // group ID, while `id -G` reports it. Accept exactly those two portable
    // representations rather than baking one host's choice into the court.
    let primary_output = std::process::Command::new("id")
        .arg("-g")
        .output()
        .expect("the host id utility reports the effective group");
    assert!(primary_output.status.success(), "id -g succeeds");
    let primary = String::from_utf8(primary_output.stdout)
        .expect("id -g emits UTF-8 digits")
        .trim()
        .parse::<u32>()
        .expect("id -g emits one numeric gid");
    let mut without_primary = expected.clone();
    without_primary.retain(|group| *group != primary);

    assert!(
        actual == expected || actual == without_primary,
        "getgroups must equal id -G, optionally excluding the effective gid"
    );
}

#[cfg(not(unix))]
#[test]
fn acquisition_is_honestly_unsupported_off_unix() {
    assert!(matches!(
        SupplementaryGroups::acquire(),
        Err(SupplementaryGroupsError::Unsupported)
    ));
}
