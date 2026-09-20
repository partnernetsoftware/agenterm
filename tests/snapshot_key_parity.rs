//! Cross-host `ui-snapshot` key-vocabulary parity gate.
//!
//! The two snapshot producers — `ui_snapshot_json` in
//! `src/platform/adapters/windows/remote_frontend.rs` and
//! `build_ui_snapshot_json` in `src/platform/adapters/unix/frontend/mod.rs`
//! — are the largest remaining frontend duplication
//! (`plan/design-frontend-shared-core.md` §1 #1, ~600 dual-written lines).
//! Until that assembly is extracted behind a shared builder, nothing
//! structural stops one host from adding a JSON key the other never emits;
//! that drift IS the parity defect class automation trips over (F7 in
//! `plan/agent-human-parity-audit.md`: unix emits `caret`/`anchor`/
//! `draft_length`, windows does not).
//!
//! Technique follows `src/frontend/ui_action_catalog.rs`: `include_str!`
//! both surfaces, scan string literals, compare sets against explicit
//! per-host allowlists. Each host's haystack is its snapshot-assembly
//! slice PLUS the shared `src/ui_snapshot.rs` builders both hosts call, so
//! keys emitted via the shared module never read as host-only. Source
//! scanning proves vocabulary, not call paths — a key in the haystack is
//! "this host's snapshot code can spell it", which is exactly the cheap
//! invariant worth pinning before the real extraction lands.
//!
//! When extraction candidate #1 lands, the allowlists below should shrink
//! toward empty; deleting an entry requires the key to genuinely appear on
//! both hosts (or disappear from both).

use std::collections::BTreeSet;

/// Keys today emitted only by the Windows remote client. Mostly
/// remote-protocol and native-control machinery (control bounds/visibility
/// reconciliation, render activity, parent paints), plus the sidebar clock
/// and selection-highlight publication detail.
const WINDOWS_ONLY_SNAPSHOT_KEYS: &[&str] = &[
    "capture_owned",
    "classification",
    "control_bounds_skips",
    "control_bounds_updates",
    "control_visibility_skips",
    "control_visibility_updates",
    "copyable",
    "date",
    "desired_cols",
    "desired_rows",
    "endpoint",
    "highlight",
    "instance_label",
    "open_instance",
    "parent_paints",
    "placeholder",
    "redraw_requests",
    "render_activity",
    "rendered",
    "resize_pending",
    "selected",
    "time",
];

/// Keys today emitted only by the Unix embedded frontend:
/// embedded-window/session facts the remote client has no analog for yet.
/// (`caret`/`anchor`/`draft_length`/`focused` left this list when the
/// windows host gained the top-level `composer` object — F7 closed.)
const UNIX_ONLY_SNAPSHOT_KEYS: &[&str] = &[
    "active_window_id",
    "add",
    "as_window",
    "menu",
    "session",
    "tab_count",
];

/// Slice `source` from the line containing `start_marker` up to
/// `end_marker`. Panics loudly if either marker vanishes so a refactor
/// that renames the functions fails this gate visibly instead of silently
/// scanning nothing.
fn slice_between<'a>(source: &'a str, start_marker: &str, end_marker: &str) -> &'a str {
    let start = source.find(start_marker).unwrap_or_else(|| {
        panic!("marker `{start_marker}` not found — update snapshot_key_parity")
    });
    let end = source[start..]
        .find(end_marker)
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("marker `{end_marker}` not found — update snapshot_key_parity"));
    &source[start..end]
}

/// Extract every `"key":`-shaped string literal — the JSON object keys
/// spelled by `json!`/`Value` construction. Values, evidence labels, and
/// non-key literals are excluded by requiring the trailing colon.
fn json_keys(haystack: &str) -> BTreeSet<String> {
    let bytes = haystack.as_bytes();
    let mut keys = BTreeSet::new();
    let mut index = 0;
    while let Some(open) = haystack[index..].find('"').map(|o| index + o) {
        let Some(close) = haystack[open + 1..].find('"').map(|o| open + 1 + o) else {
            break;
        };
        let key = &haystack[open + 1..close];
        let mut after = close + 1;
        while after < bytes.len()
            && (bytes[after] == b' ' || bytes[after] == b'\n' || bytes[after] == b'\r')
        {
            after += 1;
        }
        let is_key_shape = !key.is_empty()
            && key.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.' || c == '-'
            });
        if is_key_shape && after < bytes.len() && bytes[after] == b':' {
            keys.insert(key.to_owned());
        }
        index = close + 1;
    }
    keys
}

fn windows_snapshot_keys() -> BTreeSet<String> {
    let remote = include_str!("../src/platform/adapters/windows/remote_frontend.rs");
    let shared = include_str!("../src/ui_snapshot.rs");
    let mut keys = json_keys(slice_between(
        remote,
        "fn ui_snapshot_json",
        "fn publish_ui_snapshot",
    ));
    keys.extend(json_keys(shared));
    keys
}

fn unix_snapshot_keys() -> BTreeSet<String> {
    let unix = include_str!("../src/platform/adapters/unix/frontend/mod.rs");
    let shared = include_str!("../src/ui_snapshot.rs");
    let mut keys = json_keys(slice_between(
        unix,
        "fn build_ui_snapshot_json",
        "\n    fn ",
    ));
    keys.extend(json_keys(shared));
    keys
}

#[test]
fn snapshot_key_vocabulary_matches_across_hosts_modulo_allowlists() {
    let windows = windows_snapshot_keys();
    let unix = unix_snapshot_keys();

    let windows_shared: BTreeSet<_> = windows
        .iter()
        .filter(|key| !WINDOWS_ONLY_SNAPSHOT_KEYS.contains(&key.as_str()))
        .cloned()
        .collect();
    let unix_shared: BTreeSet<_> = unix
        .iter()
        .filter(|key| !UNIX_ONLY_SNAPSHOT_KEYS.contains(&key.as_str()))
        .cloned()
        .collect();

    let missing_on_unix: Vec<_> = windows_shared.difference(&unix_shared).collect();
    let missing_on_windows: Vec<_> = unix_shared.difference(&windows_shared).collect();
    assert!(
        missing_on_unix.is_empty() && missing_on_windows.is_empty(),
        "ui-snapshot key vocabulary drifted between hosts.\n\
         keys only in windows (add to unix, or to WINDOWS_ONLY_SNAPSHOT_KEYS with a reason): {missing_on_unix:?}\n\
         keys only in unix (add to windows, or to UNIX_ONLY_SNAPSHOT_KEYS with a reason): {missing_on_windows:?}\n\
         see plan/design-frontend-shared-core.md §1 #1"
    );
}

#[test]
fn allowlists_are_live_and_disjoint() {
    let windows = windows_snapshot_keys();
    let unix = unix_snapshot_keys();
    for key in WINDOWS_ONLY_SNAPSHOT_KEYS {
        assert!(
            windows.contains(*key),
            "stale allowlist entry: `{key}` is no longer in the windows snapshot vocabulary — delete it"
        );
        assert!(
            !unix.contains(*key),
            "`{key}` is allowlisted windows-only but unix now emits it — parity improved, delete the entry"
        );
    }
    for key in UNIX_ONLY_SNAPSHOT_KEYS {
        assert!(
            unix.contains(*key),
            "stale allowlist entry: `{key}` is no longer in the unix snapshot vocabulary — delete it"
        );
        assert!(
            !windows.contains(*key),
            "`{key}` is allowlisted unix-only but windows now emits it — parity improved, delete the entry"
        );
    }
}

/// Every relayed UI command one host serves, the other must serve too.
///
/// The same duplication this file exists for produced a second drift, in
/// dispatch rather than in keys. `77df1f84c` moved Windows `screenshot-pane` /
/// `screenshot-tab` handling out of the relay's command table into an earlier
/// branch, but gated that branch on `--json` as well as on the command name.
/// The handler already decides its own reply shape from the arguments, so the
/// extra condition did nothing except route the plain form back to the command
/// table -- which only knows `screenshot`. A relayed `screenshot-pane` without
/// `--json` then failed as `unsupported relayed UI command`, and four smoke
/// gates (cli, remote-ui, fleet, script) failed with it, for seventeen days.
///
/// The gate is parity, not a hard-coded list: whatever screenshot spellings the
/// unix frontend accepts, the Windows relay must dispatch, and it must not
/// condition that dispatch on an optional output flag.
#[test]
fn every_screenshot_spelling_the_unix_frontend_accepts_the_windows_relay_dispatches() {
    const UNIX: &str = include_str!("../src/platform/adapters/unix/frontend/mod.rs");
    const WINDOWS: &str = include_str!("../src/platform/adapters/windows/remote_frontend.rs");

    let unix_arm = UNIX
        .split_once("Some(\"screenshot\")")
        .expect("the unix frontend must still accept screenshot commands")
        .1
        .split_once(')')
        .expect("that pattern must be closed")
        .0;
    let mut expected = vec!["screenshot"];
    for spelling in ["screenshot-pane", "screenshot-tab"] {
        if unix_arm.contains(spelling) {
            expected.push(spelling);
        }
    }
    assert!(
        expected.len() > 1,
        "the unix pattern shape changed; re-read it before trusting this gate"
    );

    for spelling in &expected {
        assert!(
            WINDOWS.contains(&format!("\"{spelling}\"")),
            "the Windows relay never names {spelling}"
        );
    }

    // The aliases are dispatched by name alone. A guard that also requires an
    // optional flag silently sends the other form to the command table, which
    // is exactly how this regression shipped.
    let guard = WINDOWS
        .split_once("matches!(command_name, Some(\"screenshot-pane\" | \"screenshot-tab\"))")
        .expect("the relay must dispatch both aliases by name")
        .1
        .split_once('{')
        .expect("that branch must open a block")
        .0;
    assert!(
        !guard.contains("--json"),
        "relayed screenshot dispatch must not depend on --json: {guard:?}"
    );
}
