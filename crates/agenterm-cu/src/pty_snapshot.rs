//! Persisted, identity-bound screen baselines for durable PTY jobs.
//!
//! These records are deliberately separate from accessibility-tree snapshots:
//! a PTY baseline belongs to one job name, server scope, epoch and stable tab.
//! A same-name restarted server is a different authority and cannot consume it.

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{reply::CuError, snapshot::parse_snapshot_id};

pub const KEEP_TOTAL: usize = 128;
pub const DEFAULT_DIFF_MAX: usize = 200;
pub const MAX_DIFF_MAX: usize = 512;
const MAX_RECORD_BYTES: u64 = 2 * 1024 * 1024;

static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

pub fn pty_snapshot_dir_beside(audit_path: &Path) -> PathBuf {
    audit_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cu-pty-snapshots")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredPtySnapshot {
    pub snapshot_id: String,
    pub ts_ms: u128,
    pub pid: u32,
    pub name: String,
    pub server_scope_id: String,
    pub server_epoch: String,
    pub tab_id: String,
    pub cursor_sequence: u64,
    pub screen: Value,
}

impl StoredPtySnapshot {
    pub fn meta_json(&self) -> Value {
        json!({
            "snapshot_id": self.snapshot_id,
            "ts_ms": self.ts_ms,
            "name": self.name,
            "server_scope_id": self.server_scope_id,
            "server_epoch": self.server_epoch,
            "tab_id": self.tab_id,
            "cursor_sequence": self.cursor_sequence,
            "rows": self.screen["rows"],
            "columns": self.screen["columns"],
        })
    }
}

pub struct PtySnapshotStore {
    root: PathBuf,
}

impl PtySnapshotStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write(&self, name: &str, snapshot: &Value) -> Result<StoredPtySnapshot, CuError> {
        validate_name(name)?;
        let server_scope_id = required_string(snapshot, "server_scope_id")?;
        let server_epoch = required_string(snapshot, "server_epoch")?;
        let tab_id = snapshot["tab"]["id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid_record("snapshot tab omitted its stable id"))?;
        let cursor_sequence = snapshot["cursor"]["sequence"]
            .as_u64()
            .ok_or_else(|| invalid_record("snapshot omitted its event cursor sequence"))?;
        let screen = snapshot["tab"]["screen"].clone();
        validate_screen(&screen)?;
        let ts_ms = now_ms();
        let pid = std::process::id();
        let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let stored = StoredPtySnapshot {
            snapshot_id: format!("{ts_ms}-{pid}-{sequence}"),
            ts_ms,
            pid,
            name: name.to_owned(),
            server_scope_id: server_scope_id.to_owned(),
            server_epoch: server_epoch.to_owned(),
            tab_id: tab_id.to_owned(),
            cursor_sequence,
            screen,
        };
        prepare_directory(&self.root)?;
        let final_path = self.root.join(format!("{}.json", stored.snapshot_id));
        let temporary = self.root.join(format!(".{}.tmp", stored.snapshot_id));
        let bytes = serde_json::to_vec(&stored)
            .map_err(|error| CuError::new("pty_snapshot_unavailable", error.to_string()))?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(CuError::new(
                "pty_snapshot_too_large",
                "PTY screen baseline exceeds the 2 MiB persisted-record budget",
            ));
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| unavailable(&temporary, error))?;
        let publish = (|| -> std::io::Result<()> {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, &final_path)
        })();
        if let Err(error) = publish {
            let _ = std::fs::remove_file(&temporary);
            return Err(unavailable(&final_path, error));
        }
        #[cfg(unix)]
        std::fs::File::open(&self.root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| unavailable(&self.root, error))?;
        let _ = prune(&self.root, KEEP_TOTAL);
        Ok(stored)
    }

    pub fn load(&self, name: &str, snapshot_id: &str) -> Result<StoredPtySnapshot, CuError> {
        validate_name(name)?;
        parse_snapshot_id(snapshot_id)
            .map_err(|message| CuError::new("pty_snapshot_id_invalid", message))?;
        let path = self.root.join(format!("{snapshot_id}.json"));
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CuError::new(
                    "pty_snapshot_not_found",
                    "the named PTY screen baseline does not exist or was pruned",
                )
                .with_detail(json!({ "name": name, "snapshot_id": snapshot_id }))
            } else {
                unavailable(&path, error)
            }
        })?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_RECORD_BYTES {
            return Err(CuError::new(
                "pty_snapshot_corrupt",
                "PTY screen baseline is not a bounded regular file",
            ));
        }
        let text = std::fs::read_to_string(&path).map_err(|error| unavailable(&path, error))?;
        let stored: StoredPtySnapshot = serde_json::from_str(&text).map_err(|error| {
            CuError::new(
                "pty_snapshot_corrupt",
                format!("PTY screen baseline is not a valid record: {error}"),
            )
        })?;
        if stored.snapshot_id != snapshot_id || stored.name != name {
            return Err(CuError::new(
                "pty_snapshot_identity_mismatch",
                "PTY screen baseline does not belong to the requested job",
            ));
        }
        validate_screen(&stored.screen)?;
        Ok(stored)
    }
}

fn validate_name(name: &str) -> Result<(), CuError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(CuError::new(
            "pty_snapshot_name_invalid",
            "PTY snapshot job name must be 1..=64 ASCII letters, digits, dot, underscore or hyphen",
        ));
    }
    Ok(())
}

fn prepare_directory(path: &Path) -> Result<(), CuError> {
    std::fs::create_dir_all(path).map_err(|error| unavailable(path, error))?;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| unavailable(path, error))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(CuError::new(
            "pty_snapshot_unavailable",
            "PTY snapshot store is not a real directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| unavailable(path, error))?;
    }
    Ok(())
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, CuError> {
    value[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_record(&format!("snapshot omitted {key}")))
}

fn validate_screen(screen: &Value) -> Result<(), CuError> {
    let rows = screen["rows"].as_u64();
    let columns = screen["columns"].as_u64();
    let runs = screen["runs"].as_array();
    if !matches!(rows, Some(1..=512))
        || !matches!(columns, Some(1..=512))
        || runs.is_none_or(|runs| runs.len() > 16_384)
    {
        return Err(invalid_record(
            "snapshot screen shape is outside its bounds",
        ));
    }
    Ok(())
}

fn invalid_record(message: &str) -> CuError {
    CuError::new("pty_snapshot_invalid", message)
}

fn unavailable(path: &Path, error: std::io::Error) -> CuError {
    CuError::new(
        "pty_snapshot_unavailable",
        format!(
            "could not access PTY screen baseline {}: {error}",
            path.display()
        ),
    )
}

fn ids_newest_first(directory: &Path) -> Result<Vec<(u128, u64, u64, String)>, CuError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(unavailable(directory, error)),
    };
    let mut ids = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        if let Ok((ts, pid, sequence)) = parse_snapshot_id(stem) {
            ids.push((ts, pid, sequence, stem.to_owned()));
        }
    }
    ids.sort_by_key(|row| std::cmp::Reverse((row.0, row.2, row.1)));
    Ok(ids)
}

fn prune(directory: &Path, keep: usize) -> Result<(), CuError> {
    let stale_before = SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(15 * 60))
        .unwrap_or(UNIX_EPOCH);
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(id) = name
                .strip_prefix('.')
                .and_then(|name| name.strip_suffix(".tmp"))
            else {
                continue;
            };
            if parse_snapshot_id(id).is_err() {
                continue;
            }
            let Ok(metadata) = entry.path().symlink_metadata() else {
                continue;
            };
            if metadata.file_type().is_file()
                && !metadata.file_type().is_symlink()
                && metadata
                    .modified()
                    .is_ok_and(|modified| modified < stale_before)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    for (_, _, _, id) in ids_newest_first(directory)?.into_iter().skip(keep) {
        let _ = std::fs::remove_file(directory.join(format!("{id}.json")));
    }
    Ok(())
}

pub fn validate_diff_max(max: Option<usize>) -> Result<usize, CuError> {
    match max {
        None => Ok(DEFAULT_DIFF_MAX),
        Some(0) => Err(CuError::new(
            "pty_diff_limit_invalid",
            "pty-diff --max must be at least 1",
        )),
        Some(value) if value > MAX_DIFF_MAX => Err(CuError::new(
            "pty_diff_limit_invalid",
            format!("pty-diff --max must be at most {MAX_DIFF_MAX}"),
        )),
        Some(value) => Ok(value),
    }
}

fn rows(screen: &Value) -> Result<Vec<Vec<Value>>, CuError> {
    validate_screen(screen)?;
    let count = screen["rows"].as_u64().expect("validated") as usize;
    let mut rows = vec![Vec::new(); count];
    for run in screen["runs"].as_array().expect("validated") {
        let row = run["row"]
            .as_u64()
            .and_then(|row| usize::try_from(row).ok())
            .filter(|row| *row < count)
            .ok_or_else(|| invalid_record("screen run names an out-of-range row"))?;
        rows[row].push(run.clone());
    }
    Ok(rows)
}

pub fn diff_screens(before: &Value, after: &Value, max: usize) -> Result<Value, CuError> {
    let before_rows = rows(before)?;
    let after_rows = rows(after)?;
    let total = before_rows.len().max(after_rows.len());
    let mut changed_count = 0usize;
    let mut changed = Vec::new();
    for row in 0..total {
        let prior = before_rows.get(row);
        let current = after_rows.get(row);
        if prior == current {
            continue;
        }
        changed_count += 1;
        if changed.len() < max {
            changed.push(json!({
                "row": row,
                "before": prior,
                "after": current,
            }));
        }
    }
    let keys = [
        "rows",
        "columns",
        "cursor",
        "alternate_screen",
        "application_cursor",
        "bracketed_paste",
        "mouse_protocol_mode",
        "mouse_protocol_encoding",
        "scrollback_offset",
        "complete",
        "truncated",
    ];
    let metadata_changed: BTreeMap<&str, Value> = keys
        .into_iter()
        .filter(|key| before[*key] != after[*key])
        .map(|key| (key, json!({ "before": before[key], "after": after[key] })))
        .collect();
    Ok(json!({
        "changed_rows": changed,
        "changed_count": changed_count,
        "truncated": changed_count > max,
        "metadata_changed": metadata_changed,
        "metadata_changed_count": metadata_changed.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(text: &str, cursor: u64) -> Value {
        json!({
            "rows": 2,
            "columns": 8,
            "cursor": { "row": cursor, "column": 0, "visible": true },
            "runs": [
                { "row": 0, "column": 0, "columns": 8, "text": text },
                { "row": 1, "column": 0, "columns": 8, "text": "        " },
            ],
            "complete": true,
            "truncated": false,
        })
    }

    fn snapshot(name: &str) -> Value {
        json!({
            "server_scope_id": "scope-a",
            "server_epoch": "epoch-a",
            "cursor": { "server_epoch": "epoch-a", "sequence": 7 },
            "tab": { "id": "@1", "screen": screen(name, 0) },
        })
    }

    #[test]
    fn diff_reports_rows_and_metadata_under_separate_bounds() {
        let diff = diff_screens(&screen("before  ", 0), &screen("after   ", 1), 1).unwrap();
        assert_eq!(diff["changed_count"], 1);
        assert_eq!(diff["changed_rows"].as_array().unwrap().len(), 1);
        assert!(diff["metadata_changed"]["cursor"].is_object());
        assert_eq!(diff["truncated"], false);
    }

    #[test]
    fn limits_are_closed() {
        assert_eq!(validate_diff_max(None).unwrap(), DEFAULT_DIFF_MAX);
        assert!(validate_diff_max(Some(0)).is_err());
        assert!(validate_diff_max(Some(MAX_DIFF_MAX + 1)).is_err());
    }

    #[test]
    fn store_round_trip_is_atomic_bounded_and_job_bound() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-pty-snapshot-{}-{}",
            std::process::id(),
            NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let store = PtySnapshotStore::new(&root);
        let written = store.write("job-a", &snapshot("alpha   ")).unwrap();
        let loaded = store.load("job-a", &written.snapshot_id).unwrap();
        assert_eq!(loaded.server_epoch, "epoch-a");
        assert_eq!(loaded.screen["runs"][0]["text"], "alpha   ");
        assert_eq!(
            store.load("job-b", &written.snapshot_id).unwrap_err().code,
            "pty_snapshot_identity_mismatch"
        );
        assert!(std::fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
        std::fs::remove_dir_all(root).unwrap();
    }
}
