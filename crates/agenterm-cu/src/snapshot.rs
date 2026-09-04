//! Persisted tree baselines for incremental observation (`snapshot` /
//! `diff`).
//!
//! An agent watching a window does not want the whole tree on every poll;
//! it wants "what changed since I last looked". `snapshot` writes one
//! bounded walk as a named baseline and `diff` compares the current walk
//! against it, so only the difference crosses the wire.
//!
//! The baselines live **beside the receipts**, under the same audit
//! directory the rest of cu already writes to (`<audit dir>/cu-snapshots`,
//! next to `<audit dir>/cu-receipts`), so `AGENTERM_CU_AUDIT_PATH`
//! relocates all three together and there is exactly one store to find,
//! back up or delete. Layout:
//!
//! ```text
//! <audit dir>/cu-snapshots/<target>/w<window>/<snapshot_id>.json
//! ```
//!
//! One directory per window keeps `diff --base` a direct file open and
//! "the most recent baseline for this window" a single small listing, and
//! it bounds retention per window rather than globally: writing a baseline
//! prunes that window's directory to [`KEEP_PER_WINDOW`] newest files, so
//! an agent polling one window forever cannot grow the store without end.
//!
//! `snapshot_id` is `<ts_ms>-<pid>-<seq>`, the same shape a receipt id
//! has, and it is ordered by those three numbers rather than by string, so
//! ids stay comparable across a millisecond digit rollover.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{mechanism::A11yNode, reply::CuError, target::TargetRef};

/// Baselines kept per window before the oldest are dropped.
pub const KEEP_PER_WINDOW: usize = 32;

/// Default and ceiling for `diff --max` (per bucket).
pub const DEFAULT_DIFF_MAX: usize = 200;
pub const MAX_DIFF_MAX: usize = 2_000;

static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// `<audit dir>/cu-snapshots` for the audit log at `audit_path` — the same
/// resolution [`crate::receipt::receipt_dir_beside`] does for receipts.
pub fn snapshot_dir_beside(audit_path: &Path) -> PathBuf {
    audit_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cu-snapshots")
}

/// One persisted bounded walk.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredSnapshot {
    pub snapshot_id: String,
    pub ts_ms: u128,
    pub pid: u32,
    pub target: String,
    pub window: isize,
    pub backend: String,
    pub root_id: String,
    #[serde(default)]
    pub depth: Option<u32>,
    #[serde(default)]
    pub max_nodes: Option<usize>,
    pub truncated: bool,
    pub visited: usize,
    pub returned: usize,
    pub nodes: Vec<A11yNode>,
}

impl StoredSnapshot {
    /// The reply's identity block: everything about the baseline except
    /// its nodes.
    pub fn meta_json(&self) -> serde_json::Value {
        serde_json::json!({
            "snapshot_id": self.snapshot_id,
            "ts_ms": self.ts_ms,
            "window": self.window,
            "backend": self.backend,
            "root_id": self.root_id,
            "budget": { "depth": self.depth, "max_nodes": self.max_nodes },
            "truncated": self.truncated,
            "visited": self.visited,
            "returned": self.returned,
        })
    }
}

/// A snapshot id is a file name component: three unsigned decimal fields.
/// Anything else is refused before it is ever joined onto a path, so
/// `--base` cannot address a file outside the store.
pub fn parse_snapshot_id(raw: &str) -> Result<(u128, u64, u64), String> {
    let mut parts = raw.split('-');
    let (Some(ts), Some(pid), Some(seq), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(format!(
            "snapshot id {raw:?} is not <ts_ms>-<pid>-<sequence>; use the snapshot_id `snapshot` returned"
        ));
    };
    let bad = |field: &str| {
        format!(
            "snapshot id {raw:?} has a non-numeric {field}; use the snapshot_id `snapshot` returned"
        )
    };
    Ok((
        ts.parse().map_err(|_| bad("timestamp"))?,
        pid.parse().map_err(|_| bad("pid"))?,
        seq.parse().map_err(|_| bad("sequence"))?,
    ))
}

/// The per-window baseline directory, created on demand.
pub struct SnapshotStore {
    dir: PathBuf,
}

impl SnapshotStore {
    /// `dir` is the store root (`<audit dir>/cu-snapshots`); nothing is
    /// created until a write happens, so an observe-only `diff` against a
    /// store that was never written answers `snapshot_not_found` instead
    /// of leaving an empty tree of directories behind.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn root(&self) -> &Path {
        &self.dir
    }

    fn window_dir(&self, target: TargetRef, window: isize) -> PathBuf {
        self.dir.join(target.as_str()).join(format!("w{window}"))
    }

    /// Persist one walk as a new baseline and prune the window's older
    /// ones. The id is assigned here, so two concurrent writers cannot
    /// collide on a file name.
    /// `budget` is the walk's own `(depth, max_nodes)`: it is stored with
    /// the baseline because `diff` must re-walk with exactly the budget
    /// the baseline was taken under, or a difference in how far each side
    /// looked would read as a difference in the window.
    pub fn write(
        &self,
        target: TargetRef,
        window: isize,
        budget: (Option<u32>, Option<usize>),
        tree: &crate::mechanism::A11yTree,
    ) -> Result<StoredSnapshot, CuError> {
        let ts_ms = now_ms();
        let pid = std::process::id();
        let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let snapshot = StoredSnapshot {
            snapshot_id: format!("{ts_ms}-{pid}-{sequence}"),
            ts_ms,
            pid,
            target: target.as_str().to_owned(),
            window,
            backend: tree.backend.clone(),
            root_id: tree.root_id.clone(),
            depth: budget.0,
            max_nodes: budget.1,
            truncated: tree.truncated,
            visited: tree.visited,
            returned: tree.nodes.len(),
            nodes: tree.nodes.clone(),
        };
        let dir = self.window_dir(target, window);
        std::fs::create_dir_all(&dir).map_err(|error| {
            CuError::new(
                "snapshot_unavailable",
                format!(
                    "could not create snapshot directory {}: {error}",
                    dir.display()
                ),
            )
        })?;
        let path = dir.join(format!("{}.json", snapshot.snapshot_id));
        let text = serde_json::to_string(&snapshot).map_err(|error| {
            CuError::new(
                "snapshot_unavailable",
                format!("snapshot serialization failed: {error}"),
            )
        })?;
        std::fs::write(&path, text).map_err(|error| {
            CuError::new(
                "snapshot_unavailable",
                format!("could not write snapshot {}: {error}", path.display()),
            )
        })?;
        // Best effort: a baseline that could not be pruned is not a reason
        // to fail the observation that just succeeded.
        let _ = prune(&dir, KEEP_PER_WINDOW);
        Ok(snapshot)
    }

    /// One baseline by id. A missing file is the typed
    /// `snapshot_not_found`, never an empty baseline that would make every
    /// node look added.
    pub fn load(
        &self,
        target: TargetRef,
        window: isize,
        snapshot_id: &str,
    ) -> Result<StoredSnapshot, CuError> {
        parse_snapshot_id(snapshot_id).map_err(|message| CuError::new("invalid_input", message))?;
        let path = self
            .window_dir(target, window)
            .join(format!("{snapshot_id}.json"));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(CuError::new(
                    "snapshot_not_found",
                    format!(
                        "no snapshot {snapshot_id} for window {window}; take one with `snapshot --window {window}`"
                    ),
                )
                .with_detail(serde_json::json!({
                    "snapshot_id": snapshot_id,
                    "window": window,
                    "store": self.dir,
                })));
            }
            Err(error) => {
                return Err(CuError::new(
                    "snapshot_unavailable",
                    format!("could not read snapshot {}: {error}", path.display()),
                ));
            }
        };
        parse_stored(&path, &text)
    }

    /// The newest baseline for this window, or `None` when the window has
    /// none. Ordering is on the id's three numbers, not on the string.
    pub fn latest(
        &self,
        target: TargetRef,
        window: isize,
    ) -> Result<Option<StoredSnapshot>, CuError> {
        let dir = self.window_dir(target, window);
        let Some(newest) = newest_id(&dir)? else {
            return Ok(None);
        };
        self.load(target, window, &newest).map(Some)
    }
}

fn parse_stored(path: &Path, text: &str) -> Result<StoredSnapshot, CuError> {
    serde_json::from_str(text).map_err(|error| {
        CuError::new(
            "snapshot_corrupt",
            format!(
                "snapshot {} is not a snapshot record: {error}",
                path.display()
            ),
        )
    })
}

/// Every `<id>.json` in `dir` whose stem is a well-formed snapshot id,
/// newest first. A file with any other name is ignored rather than
/// failing the listing: the directory is a store, not a manifest.
fn ids_newest_first(dir: &Path) -> Result<Vec<(u128, u64, u64, String)>, CuError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CuError::new(
                "snapshot_unavailable",
                format!("could not list snapshots in {}: {error}", dir.display()),
            ));
        }
    };
    let mut ids = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        if let Ok((ts, pid, seq)) = parse_snapshot_id(stem) {
            ids.push((ts, pid, seq, stem.to_owned()));
        }
    }
    ids.sort_by_key(|id| std::cmp::Reverse((id.0, id.2, id.1)));
    Ok(ids)
}

fn newest_id(dir: &Path) -> Result<Option<String>, CuError> {
    Ok(ids_newest_first(dir)?.into_iter().next().map(|id| id.3))
}

/// Keep the `keep` newest baselines in `dir`; delete the rest.
pub fn prune(dir: &Path, keep: usize) -> Result<usize, CuError> {
    let ids = ids_newest_first(dir)?;
    let mut removed = 0usize;
    for (_, _, _, id) in ids.into_iter().skip(keep) {
        if std::fs::remove_file(dir.join(format!("{id}.json"))).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// `diff --max` validation: `None` is the default, `0` and anything above
/// the ceiling are `invalid_input`.
pub fn validate_diff_max(max: Option<usize>) -> Result<usize, String> {
    match max {
        None => Ok(DEFAULT_DIFF_MAX),
        Some(0) => Err("--max must be at least 1".to_owned()),
        Some(value) if value > MAX_DIFF_MAX => {
            Err(format!("--max must be at most {MAX_DIFF_MAX}, got {value}"))
        }
        Some(value) => Ok(value),
    }
}

// ---------------------------------------------------------------------------
// The pure diff.
// ---------------------------------------------------------------------------

/// Which observable fields of one node differ between two walks, in a
/// fixed order so a reply never depends on map iteration.
pub fn changed_fields(before: &A11yNode, after: &A11yNode) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if before.role != after.role {
        fields.push("role");
    }
    if before.name != after.name {
        fields.push("name");
    }
    if before.parent_id != after.parent_id {
        fields.push("parent_id");
    }
    if before.states != after.states {
        fields.push("states");
    }
    if before.bounds != after.bounds {
        fields.push("bounds");
    }
    if before.actions != after.actions {
        fields.push("actions");
    }
    if before.text != after.text {
        fields.push("text");
    }
    if before.identifier != after.identifier {
        fields.push("identifier");
    }
    fields
}

/// The difference between two bounded walks of one window, as positions
/// rather than borrows so the caller owns the rendering.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeDiff {
    /// Indices into the **after** walk.
    pub added: Vec<usize>,
    /// Indices into the **before** walk.
    pub removed: Vec<usize>,
    /// Index into the **after** walk plus the fields that differ.
    pub changed: Vec<(usize, Vec<&'static str>)>,
}

impl NodeDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }

    pub fn total(&self) -> usize {
        self.added.len() + self.removed.len() + self.changed.len()
    }
}

/// Compare two walks by node id.
///
/// Node ids are **positional paths** (`/0/3/1`), which is what makes them
/// directly usable with `invoke --node`; the cost is that inserting a
/// sibling renumbers the ones after it, so one real insertion can read as
/// one `added` plus several `changed`. That is a true report of what the
/// addressing space did, and it is why `diff` reports field names: a
/// renumbered node changes `name`/`role`, a moved one changes only
/// `bounds`.
pub fn diff_nodes(before: &[A11yNode], after: &[A11yNode]) -> NodeDiff {
    let mut diff = NodeDiff::default();
    let mut previous: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::with_capacity(before.len());
    for (index, node) in before.iter().enumerate() {
        previous.insert(node.id.as_str(), index);
    }
    let mut seen = vec![false; before.len()];
    for (index, node) in after.iter().enumerate() {
        match previous.get(node.id.as_str()) {
            Some(&was) => {
                seen[was] = true;
                let fields = changed_fields(&before[was], node);
                if !fields.is_empty() {
                    diff.changed.push((index, fields));
                }
            }
            None => diff.added.push(index),
        }
    }
    for (index, was) in seen.iter().enumerate() {
        if !was {
            diff.removed.push(index);
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mechanism::A11yBounds;

    fn node(id: &str, role: &str, name: &str) -> A11yNode {
        A11yNode {
            id: id.into(),
            parent_id: None,
            role: role.into(),
            name: name.into(),
            states: vec!["showing".into()],
            bounds: A11yBounds {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            actions: Vec::new(),
            text: None,
            identifier: None,
        }
    }

    fn walked(nodes: Vec<A11yNode>) -> crate::mechanism::A11yTree {
        let returned = nodes.len();
        crate::mechanism::A11yTree {
            backend: "ax".into(),
            window_handle: Some(7),
            root_id: "/0".into(),
            nodes,
            truncated: false,
            visited: returned,
            returned,
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "agenterm-cu-snapshot-{label}-{}-{sequence}",
            std::process::id()
        ))
    }

    #[test]
    fn diff_reports_added_removed_and_the_changed_field_names() {
        let before = [
            node("/0", "AXWindow", "W"),
            node("/0/1", "AXButton", "Save"),
        ];
        let mut renamed = node("/0/1", "AXButton", "Saved");
        renamed.bounds.x = 4;
        let after = [
            node("/0", "AXWindow", "W"),
            renamed,
            node("/0/2", "AXButton", "New"),
        ];
        let diff = diff_nodes(&before, &after);
        assert_eq!(diff.added, vec![2]);
        assert!(diff.removed.is_empty());
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].0, 1);
        assert_eq!(diff.changed[0].1, vec!["name", "bounds"]);
        assert_eq!(diff.total(), 2);
        assert!(!diff.is_empty());
    }

    #[test]
    fn a_removed_node_is_reported_against_the_baseline_positions() {
        let before = [
            node("/0", "AXWindow", "W"),
            node("/0/1", "AXButton", "Save"),
            node("/0/2", "AXButton", "New"),
        ];
        let after = [node("/0", "AXWindow", "W"), node("/0/2", "AXButton", "New")];
        let diff = diff_nodes(&before, &after);
        assert_eq!(diff.removed, vec![1]);
        assert!(diff.added.is_empty() && diff.changed.is_empty());
    }

    #[test]
    fn two_identical_walks_diff_to_nothing() {
        let nodes = [
            node("/0", "AXWindow", "W"),
            node("/0/1", "AXButton", "Save"),
        ];
        let diff = diff_nodes(&nodes, &nodes);
        assert!(diff.is_empty(), "{diff:?}");
    }

    #[test]
    fn every_observable_field_is_named_when_it_changes() {
        let before = node("/0/1", "AXButton", "Save");
        let mut after = before.clone();
        after.role = "AXLink".into();
        after.name = "Saved".into();
        after.parent_id = Some("/0".into());
        after.states = vec!["disabled".into()];
        after.bounds.height = 20;
        after.actions = vec!["AXPress".into()];
        after.text = Some("t".into());
        after.identifier = Some("save".into());
        assert_eq!(
            changed_fields(&before, &after),
            vec![
                "role",
                "name",
                "parent_id",
                "states",
                "bounds",
                "actions",
                "text",
                "identifier"
            ]
        );
        assert!(changed_fields(&before, &before).is_empty());
    }

    #[test]
    fn store_round_trips_and_latest_is_the_newest_write() {
        let dir = scratch("round-trip");
        let store = SnapshotStore::new(&dir);
        assert!(
            store
                .latest(TargetRef::Current, 7)
                .expect("empty store lists")
                .is_none(),
            "a store that was never written has no baseline"
        );
        let first = store
            .write(
                TargetRef::Current,
                7,
                (Some(4), Some(100)),
                &walked(vec![node("/0", "AXWindow", "W")]),
            )
            .expect("write");
        let second = store
            .write(
                TargetRef::Current,
                7,
                (Some(4), Some(100)),
                &walked(vec![node("/0", "AXWindow", "W2")]),
            )
            .expect("write");
        assert_ne!(first.snapshot_id, second.snapshot_id);
        let loaded = store
            .load(TargetRef::Current, 7, &first.snapshot_id)
            .expect("load by id");
        assert_eq!(loaded.nodes[0].name, "W");
        assert_eq!(loaded.returned, 1);
        assert_eq!(loaded.visited, 1);
        assert_eq!(loaded.depth, Some(4));
        let latest = store
            .latest(TargetRef::Current, 7)
            .expect("latest")
            .expect("some");
        assert_eq!(latest.snapshot_id, second.snapshot_id);
        // Another window's store is separate.
        assert!(
            store
                .latest(TargetRef::Current, 8)
                .expect("other window")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_baseline_is_typed_and_never_an_empty_tree() {
        let dir = scratch("missing");
        let store = SnapshotStore::new(&dir);
        let error = store
            .load(TargetRef::Current, 7, "1-2-3")
            .expect_err("missing baseline");
        assert_eq!(error.code, "snapshot_not_found");
        // A traversal attempt is refused before any path is joined.
        let error = store
            .load(TargetRef::Current, 7, "../../etc/passwd")
            .expect_err("traversal");
        assert_eq!(error.code, "invalid_input");
        let error = store
            .load(TargetRef::Current, 7, "not-an-id")
            .expect_err("not an id");
        assert_eq!(error.code, "invalid_input");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_prunes_the_window_to_the_retention_bound() {
        let dir = scratch("prune");
        let store = SnapshotStore::new(&dir);
        let mut ids = Vec::new();
        for _ in 0..(KEEP_PER_WINDOW + 3) {
            ids.push(
                store
                    .write(
                        TargetRef::Current,
                        9,
                        (None, None),
                        &walked(vec![node("/0", "AXWindow", "W")]),
                    )
                    .expect("write")
                    .snapshot_id,
            );
        }
        let kept = ids_newest_first(&dir.join("current").join("w9")).expect("list");
        assert_eq!(kept.len(), KEEP_PER_WINDOW);
        // The newest survive and the oldest are gone.
        assert_eq!(kept[0].3, *ids.last().expect("last"));
        assert!(
            store
                .load(TargetRef::Current, 9, &ids[0])
                .is_err_and(|error| error.code == "snapshot_not_found")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_order_by_number_not_by_string() {
        // 999 ms and 1000 ms: string order puts "1000..." first, which
        // would make the older baseline look newest.
        assert!(
            parse_snapshot_id("999-1-0").expect("parse")
                < parse_snapshot_id("1000-1-0").expect("parse")
        );
        assert!(parse_snapshot_id("7-1").is_err());
        assert!(parse_snapshot_id("7-1-0-2").is_err());
        assert!(parse_snapshot_id("a-1-0").is_err());
    }

    #[test]
    fn diff_max_is_bounded() {
        assert_eq!(validate_diff_max(None), Ok(DEFAULT_DIFF_MAX));
        assert_eq!(validate_diff_max(Some(5)), Ok(5));
        assert!(validate_diff_max(Some(0)).is_err());
        assert!(validate_diff_max(Some(MAX_DIFF_MAX + 1)).is_err());
    }

    #[test]
    fn snapshots_sit_beside_the_receipts_under_one_audit_directory() {
        let audit = Path::new("/scratch/audit/cu-audit.jsonl");
        assert_eq!(
            snapshot_dir_beside(audit),
            PathBuf::from("/scratch/audit/cu-snapshots")
        );
        assert_eq!(
            crate::receipt::receipt_dir_beside(audit).parent(),
            snapshot_dir_beside(audit).parent(),
            "receipts and snapshots must share one store root"
        );
    }
}
