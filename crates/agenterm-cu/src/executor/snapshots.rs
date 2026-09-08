//! Incremental observation: `snapshot` (name a baseline) and `diff`
//! (what changed since one).
//!
//! An agent loop that re-reads a whole window tree on every poll spends
//! most of its budget re-reading what it already knows. `snapshot` writes
//! one bounded walk to the store beside the receipts and hands back an id;
//! `diff` walks the window again and returns only the added, removed and
//! changed nodes, in the node shape `query` returns. `diff --advance`
//! stores the walk it just made as the next baseline in the same call, so
//! a poll loop is one verb per tick with no bookkeeping on the caller.
//!
//! Both are observation: nothing here presses, focuses, raises or writes
//! anything to the desktop. The only side effect is the baseline file, and
//! the store prunes itself per window.

use super::*;

use crate::pty_snapshot::{self, PtySnapshotStore};
use crate::snapshot::{self, SnapshotStore};

const MAX_SNAPSHOT_IDENTITY_WINDOWS: usize = 512;

impl Executor {
    /// `<audit dir>/cu-snapshots`, resolved exactly like the receipt
    /// directory so one audit path relocates audit, receipts and baselines
    /// together.
    pub(super) fn snapshot_store(&self) -> Result<SnapshotStore, CuError> {
        #[cfg(test)]
        if let Some(path) = self.audit_path.as_ref() {
            return Ok(SnapshotStore::new(snapshot::snapshot_dir_beside(path)));
        }
        let audit_path = crate::audit::resolved_audit_path()
            .map_err(|error| CuError::new("snapshot_unavailable", error))?;
        Ok(SnapshotStore::new(snapshot::snapshot_dir_beside(
            &audit_path,
        )))
    }

    /// Persisted PTY screen baselines share the audit root but use a separate
    /// identity-aware store from accessibility-tree snapshots.
    pub(super) fn pty_snapshot_store(&self) -> Result<PtySnapshotStore, CuError> {
        #[cfg(test)]
        if let Some(path) = self.audit_path.as_ref() {
            return Ok(PtySnapshotStore::new(
                pty_snapshot::pty_snapshot_dir_beside(path),
            ));
        }
        let audit_path = crate::audit::resolved_audit_path()
            .map_err(|error| CuError::new("pty_snapshot_unavailable", error))?;
        Ok(PtySnapshotStore::new(
            pty_snapshot::pty_snapshot_dir_beside(&audit_path),
        ))
    }
}

fn require_window(verb: &str, window: isize) -> Result<(), CuError> {
    if window == 0 {
        return Err(invalid_input(format!(
            "{verb} requires --window <handle> (a non-zero handle from `windows`)"
        )));
    }
    Ok(())
}

/// `snapshot --window H [--depth N] [--max-nodes N] [--out PATH] [--shot]`.
pub(super) fn snapshot_payload(
    store: &SnapshotStore,
    target: TargetRef,
    window: isize,
    depth: Option<u32>,
    max_nodes: Option<usize>,
    out: Option<&str>,
    shot: bool,
) -> Result<serde_json::Value, CuError> {
    require_window("snapshot", window)?;
    if out.is_some_and(|path| path.trim().is_empty() || path.contains('\0')) {
        return Err(invalid_input("snapshot --out needs a writable path".into()));
    }
    let budget = tree_budget(depth, max_nodes)?;
    let identity = if shot {
        Some(snapshot_window_identity(window)?)
    } else {
        None
    };
    let tree =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let stored = if let Some(identity) = identity.as_ref() {
        store.write_with_shot(target, window, (depth, max_nodes), &tree, |staged_path| {
            let captured = mechanism::screenshot::capture_native_window_png(window, staged_path)
                .map_err(map_mechanism_err)?;
            if captured.output_width == 0 || captured.output_height == 0 {
                return Err(CuError::new(
                    "snapshot_shot_invalid",
                    "snapshot --shot capture did not produce a readable PNG header",
                ));
            }
            revalidate_snapshot_window(identity)?;
            Ok(snapshot::SnapshotShotCapture {
                output_width: captured.output_width,
                output_height: captured.output_height,
                output_pixels: captured.output_pixels,
            })
        })?
    } else {
        store.write(target, window, (depth, max_nodes), &tree)?
    };
    // `--out` is a convenience copy for a caller that wants the tree
    // itself; the baseline `diff` reads is the store's, always.
    if let Some(path) = out {
        let text = match serde_json::to_string_pretty(&stored) {
            Ok(text) => text,
            Err(error) => {
                if shot {
                    store.remove_pair(target, window, &stored.snapshot_id);
                }
                return Err(CuError::new("serialize", error.to_string()));
            }
        };
        if let Err(error) = std::fs::write(path, text) {
            if shot {
                store.remove_pair(target, window, &stored.snapshot_id);
            }
            return Err(CuError::new(
                "snapshot_unavailable",
                format!("could not write snapshot --out {path}: {error}"),
            ));
        }
    }
    let mut payload = stored.meta_json();
    if let Some(object) = payload.as_object_mut() {
        object.insert("addressing".into(), serde_json::json!("accessibility-tree"));
        object.insert("mechanism".into(), serde_json::json!("libagenterm"));
        object.insert("store".into(), serde_json::json!(store.root()));
        object.insert("out".into(), serde_json::json!(out));
        object.insert("nodes".into(), serde_json::json!(stored.returned));
        if shot {
            object.insert(
                "observation".into(),
                serde_json::json!({
                    "atomic": false,
                    "order": ["window-identity-before", "bounded-tree", "window-png", "window-identity-after"],
                    "identity_clamped": true,
                }),
            );
            object.insert(
                "window_identity".into(),
                identity
                    .as_ref()
                    .map(observe::window_row_json)
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        object.insert(
            "next_actions".into(),
            serde_json::json!([format!(
                "diff --window {window} --base {} (or --advance to keep polling)",
                stored.snapshot_id
            )]),
        );
    }
    Ok(payload)
}

fn snapshot_window_identity(
    window: isize,
) -> Result<mechanism::window_enumerate::WindowInfo, CuError> {
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    if windows.len() > MAX_SNAPSHOT_IDENTITY_WINDOWS {
        return Err(CuError::new(
            "snapshot_window_limit",
            format!(
                "snapshot --shot observed {} windows; identity inventory limit is {MAX_SNAPSHOT_IDENTITY_WINDOWS}",
                windows.len()
            ),
        ));
    }
    windows
        .into_iter()
        .find(|candidate| candidate.handle == window)
        .ok_or_else(|| {
            CuError::new(
                "snapshot_window_not_found",
                format!("snapshot --shot window {window} is absent from the bounded inventory"),
            )
        })
}

fn revalidate_snapshot_window(
    identity: &mechanism::window_enumerate::WindowInfo,
) -> Result<(), CuError> {
    let after = snapshot_window_identity(identity.handle)?;
    validate_snapshot_window_identity(identity, &after)
}

fn validate_snapshot_window_identity(
    before: &mechanism::window_enumerate::WindowInfo,
    after: &mechanism::window_enumerate::WindowInfo,
) -> Result<(), CuError> {
    if after == before {
        return Ok(());
    }
    Err(CuError::new(
        "snapshot_window_changed",
        "snapshot --shot window identity changed during the sequential tree/PNG observation",
    )
    .with_detail(serde_json::json!({
        "before": observe::window_row_json(before),
        "after": observe::window_row_json(after),
        "atomic": false,
    })))
}

/// One bucket of the diff, in the node shape `query` returns, bounded.
fn bucket(
    flat: &[observe::FlatNode<'_>],
    indices: &[usize],
    max: usize,
) -> Result<(serde_json::Value, bool), CuError> {
    let truncated = indices.len() > max;
    let rows: Vec<serde_json::Value> = indices
        .iter()
        .take(max)
        .filter_map(|index| flat.get(*index))
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()
        .map_err(|error| CuError::new("serialize", error.to_string()))?;
    Ok((serde_json::Value::Array(rows), truncated))
}

/// `diff --window H [--base ID] [--advance] [--max N]`.
pub(super) fn diff_payload(
    store: &SnapshotStore,
    target: TargetRef,
    window: isize,
    base: Option<&str>,
    advance: bool,
    max: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    require_window("diff", window)?;
    let max = snapshot::validate_diff_max(max).map_err(invalid_input)?;
    let (baseline, selected_by) = match base {
        Some(id) => (store.load(target, window, id)?, "explicit"),
        None => {
            let Some(latest) = store.latest(target, window)? else {
                return Err(CuError::new(
                    "snapshot_not_found",
                    format!(
                        "window {window} has no baseline yet; take one with `snapshot --window {window}` first"
                    ),
                )
                .with_detail(serde_json::json!({
                    "window": window,
                    "store": store.root(),
                    "next_actions": [format!("snapshot --window {window}")],
                })));
            };
            (latest, "most-recent")
        }
    };
    // The comparison must be like for like: the current walk reuses the
    // baseline's own budget, so a difference is a difference in the
    // window, never a difference in how far each side looked.
    let budget = tree_budget(baseline.depth, baseline.max_nodes)?;
    let tree =
        mechanism::tree_for_window_bounded(Some(window), budget).map_err(map_mechanism_err)?;
    let diff = snapshot::diff_nodes(&baseline.nodes, &tree.nodes);
    let after_flat = observe::flatten(&tree);
    let baseline_tree = mechanism::A11yTree {
        backend: baseline.backend.clone(),
        window_handle: Some(window),
        root_id: baseline.root_id.clone(),
        nodes: baseline.nodes.clone(),
        truncated: baseline.truncated,
        visited: baseline.visited,
        returned: baseline.returned,
    };
    let before_flat = observe::flatten(&baseline_tree);
    let (added, added_truncated) = bucket(&after_flat, &diff.added, max)?;
    let (removed, removed_truncated) = bucket(&before_flat, &diff.removed, max)?;
    let changed_indices: Vec<usize> = diff.changed.iter().map(|(index, _)| *index).collect();
    let (changed_nodes, changed_truncated) = bucket(&after_flat, &changed_indices, max)?;
    let changed: Vec<serde_json::Value> = changed_nodes
        .as_array()
        .map(|rows| rows.to_vec())
        .unwrap_or_default()
        .into_iter()
        .zip(diff.changed.iter())
        .map(|(mut node, (_, fields))| {
            if let Some(object) = node.as_object_mut() {
                object.insert("changed".into(), serde_json::json!(fields));
            }
            node
        })
        .collect();
    let truncated = added_truncated || removed_truncated || changed_truncated;
    // `--advance` is what makes a poll loop one call per tick: the walk
    // just compared becomes the next baseline, so nothing is missed
    // between this reply and the next request.
    let advanced = if advance {
        Some(store.write(target, window, (baseline.depth, baseline.max_nodes), &tree)?)
    } else {
        None
    };
    Ok(serde_json::json!({
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": tree.backend,
        "window": window,
        "base": baseline.meta_json(),
        "base_selected_by": selected_by,
        "budget": budget_json(baseline.depth, baseline.max_nodes),
        "current": {
            "root_id": tree.root_id,
            "visited": tree.visited,
            "returned": tree.returned,
            "truncated": tree.truncated,
        },
        "changes": diff.total(),
        "max": max,
        "truncated": truncated,
        "walk_truncated": tree.truncated || baseline.truncated,
        "counts": {
            "added": diff.added.len(),
            "removed": diff.removed.len(),
            "changed": diff.changed.len(),
        },
        "added": added,
        "removed": removed,
        "changed": changed,
        "advanced": advanced.as_ref().map(|stored| stored.meta_json()),
        "next_base": advanced
            .as_ref()
            .map(|stored| stored.snapshot_id.clone())
            .unwrap_or_else(|| baseline.snapshot_id.clone()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_identity(handle: isize, title: &str) -> mechanism::window_enumerate::WindowInfo {
        mechanism::window_enumerate::WindowInfo {
            handle,
            title: title.into(),
            process_id: 7,
            app_name: "Fixture".into(),
            bounds: mechanism::window_enumerate::WindowBounds {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            },
            focused: false,
            minimized: false,
            maximized: false,
            fullscreen: false,
            above: false,
        }
    }

    #[test]
    fn shot_identity_clamp_compares_the_complete_window_row() {
        let before = window_identity(7, "before");
        assert!(validate_snapshot_window_identity(&before, &before).is_ok());
        let after = window_identity(7, "after");
        let error = validate_snapshot_window_identity(&before, &after).expect_err("drift");
        assert_eq!(error.code, "snapshot_window_changed");
        assert_eq!(error.detail.as_ref().expect("detail")["atomic"], false);
    }

    #[test]
    fn both_verbs_need_a_window_and_a_bounded_page() {
        let path = audit_scratch("snapshot-usage");
        let executor = observe_executor().with_audit_path(path.clone());
        let no_window = executor.execute(&Command::Snapshot {
            target: TargetRef::Current,
            window: 0,
            depth: None,
            max_nodes: None,
            out: None,
            shot: false,
        });
        assert_eq!(no_window.command, "snapshot");
        assert_eq!(
            no_window.error.as_ref().expect("typed").code,
            "invalid_input"
        );
        let deep = executor.execute(&Command::Snapshot {
            target: TargetRef::Current,
            window: 1,
            depth: Some(65),
            max_nodes: None,
            out: None,
            shot: false,
        });
        assert_eq!(deep.error.as_ref().expect("typed").code, "invalid_input");
        let no_diff_window = executor.execute(&Command::Diff {
            target: TargetRef::Current,
            window: 0,
            base: None,
            advance: false,
            max: None,
        });
        assert_eq!(no_diff_window.command, "diff");
        assert_eq!(
            no_diff_window.error.as_ref().expect("typed").code,
            "invalid_input"
        );
        let bad_max = executor.execute(&Command::Diff {
            target: TargetRef::Current,
            window: 1,
            base: None,
            advance: false,
            max: Some(0),
        });
        assert_eq!(bad_max.error.as_ref().expect("typed").code, "invalid_input");
        remove_audit_scratch(&path);
    }

    /// `diff` without a baseline must say so and name the repair, never
    /// treat "no baseline" as an empty tree in which everything is new.
    #[test]
    fn diff_without_a_baseline_is_typed_and_names_the_repair() {
        let path = audit_scratch("snapshot-missing");
        let executor = observe_executor().with_audit_path(path.clone());
        let reply = executor.execute(&Command::Diff {
            target: TargetRef::Current,
            window: 987_654,
            base: None,
            advance: false,
            max: None,
        });
        assert!(!reply.ok);
        let error = reply.error.as_ref().expect("typed");
        assert_eq!(error.code, "snapshot_not_found");
        let detail = error.detail.as_ref().expect("detail");
        assert!(
            detail["next_actions"][0]
                .as_str()
                .expect("next action")
                .contains("snapshot --window"),
            "{detail}"
        );
        // An id that is not an id is refused before any path is opened.
        let bad_base = executor.execute(&Command::Diff {
            target: TargetRef::Current,
            window: 987_654,
            base: Some("../escape".into()),
            advance: false,
            max: None,
        });
        assert_eq!(
            bad_base.error.as_ref().expect("typed").code,
            "invalid_input"
        );
        remove_audit_scratch(&path);
    }

    /// The store the executor resolves is the one beside the receipts, so
    /// one audit path relocates both.
    #[test]
    fn the_snapshot_store_sits_beside_the_receipt_directory() {
        let path = audit_scratch("snapshot-store");
        let executor = observe_executor().with_audit_path(path.clone());
        let store = executor.snapshot_store().expect("store");
        let receipts = executor.receipt_dir().expect("receipt dir");
        assert_eq!(store.root().parent(), receipts.parent());
        assert_eq!(
            store.root().file_name().and_then(|name| name.to_str()),
            Some("cu-snapshots")
        );
        remove_audit_scratch(&path);
    }

    /// Both verbs are observation: neither may require the actuate grant,
    /// and neither may reach the actuation mechanism.
    #[test]
    fn snapshot_and_diff_are_observe_only() {
        for command in [
            Command::Snapshot {
                target: TargetRef::Current,
                window: 1,
                depth: None,
                max_nodes: None,
                out: None,
                shot: false,
            },
            Command::Diff {
                target: TargetRef::Current,
                window: 1,
                base: None,
                advance: true,
                max: None,
            },
        ] {
            assert_eq!(command.required_grant(), crate::auth::Grant::Observe);
            let before = mechanism::write_ledger::attempts();
            let reply = observe_executor().execute(&command);
            assert_ne!(
                reply.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
                "refused",
                "{}",
                command.verb()
            );
            assert_eq!(
                mechanism::write_ledger::attempts(),
                before,
                "{} must not actuate",
                command.verb()
            );
        }
    }
}
