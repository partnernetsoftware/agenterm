//! Crash-persistent effect receipts (PRD_02_31, shape absorbed from
//! `moltbaby/skills/mcu`, slice 4 of `plan/design-mcu-absorption.md`).
//!
//! Every actuation that goes through the a11y / window mechanisms appends
//! two lines to a per-target JSONL file beside the audit log, each flushed
//! before the next thing happens:
//!
//! 1. `reserved` — written **before** the mechanism is touched: target,
//!    window, node, action, value, the `before` state and (for a
//!    destructive verb) the prior snapshot;
//! 2. `completed` / `failed` — written after the read-back: `after`,
//!    `verified`, the verification method and reason, or the typed error.
//!
//! A receipt that has a `reserved` line and no second line is the crash
//! signature: the process died between reserving and reading back, so the
//! effect is *uncertain* — never "did not happen". Failure to reserve is
//! failure to act (`receipt_unavailable`); the mechanism is not called.
//!
//! The file is `<audit dir>/cu-receipts/<target>.jsonl`, so
//! `AGENTERM_CU_AUDIT_PATH` relocates audit and receipts together; `receipts
//! --window H --max N` (`list`) reads it back in order.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{reply::CuError, target::TargetRef};

/// Default and ceiling for `receipts --max`.
pub const DEFAULT_LIST_MAX: usize = 50;
pub const MAX_LIST_MAX: usize = 1_000;

static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct ReceiptLog {
    path: PathBuf,
    file: File,
    target: TargetRef,
}

/// The identity of one reserved effect; `complete` closes it.
#[derive(Clone, Debug)]
pub struct ReceiptTicket {
    pub id: String,
    pub path: PathBuf,
}

impl ReceiptTicket {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({ "id": self.id, "path": self.path })
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// `<audit dir>/cu-receipts` for the audit log at `audit_path`.
pub(crate) fn receipt_dir_beside(audit_path: &Path) -> PathBuf {
    audit_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cu-receipts")
}

impl ReceiptLog {
    /// The production path: beside the audit log this process would write.
    pub fn open(target: TargetRef) -> Result<Self, CuError> {
        let audit_path = crate::audit::resolved_audit_path()
            .map_err(|error| CuError::new("receipt_unavailable", error))?;
        Self::open_in(&receipt_dir_beside(&audit_path), target)
    }

    /// Open (append) the target's receipt file under `dir`.
    pub fn open_in(dir: &Path, target: TargetRef) -> Result<Self, CuError> {
        std::fs::create_dir_all(dir).map_err(|error| {
            CuError::new(
                "receipt_unavailable",
                format!(
                    "could not create receipt directory {}: {error}",
                    dir.display()
                ),
            )
        })?;
        let path = dir.join(format!("{}.jsonl", target.as_str()));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                CuError::new(
                    "receipt_unavailable",
                    format!("could not open receipt file {}: {error}", path.display()),
                )
            })?;
        Ok(Self { path, file, target })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append the `reserved` line and flush it. `body` carries the
    /// verb-specific evidence (node, action, value, before, snapshot).
    pub fn reserve(
        &mut self,
        verb: &str,
        window: isize,
        body: serde_json::Value,
    ) -> Result<ReceiptTicket, CuError> {
        let ts_ms = now_ms();
        let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let id = format!("{ts_ms}-{}-{sequence}", std::process::id());
        let line = merged(
            serde_json::json!({
                "receipt_id": id,
                "phase": "reserved",
                "ts_ms": ts_ms,
                "pid": std::process::id(),
                "target": self.target.as_str(),
                "verb": verb,
                "window": window,
            }),
            body,
        );
        self.append(&line)?;
        Ok(ReceiptTicket {
            id,
            path: self.path.clone(),
        })
    }

    /// Append the closing line (`completed` when the effect was read back
    /// as intended, `failed` otherwise) and flush it.
    pub fn complete(
        &mut self,
        ticket: &ReceiptTicket,
        verb: &str,
        window: isize,
        verified: bool,
        body: serde_json::Value,
    ) -> Result<(), CuError> {
        let line = merged(
            serde_json::json!({
                "receipt_id": ticket.id,
                "phase": if verified { "completed" } else { "failed" },
                "ts_ms": now_ms(),
                "pid": std::process::id(),
                "target": self.target.as_str(),
                "verb": verb,
                "window": window,
                "verified": verified,
            }),
            body,
        );
        self.append(&line)
    }

    fn append(&mut self, line: &serde_json::Value) -> Result<(), CuError> {
        let text = serde_json::to_string(line).map_err(|error| {
            CuError::new(
                "receipt_unavailable",
                format!("receipt serialization failed: {error}"),
            )
        })?;
        writeln!(self.file, "{text}")
            .and_then(|_| self.file.flush())
            .map_err(|error| {
                CuError::new(
                    "receipt_unavailable",
                    format!(
                        "could not append receipt file {}: {error}",
                        self.path.display()
                    ),
                )
            })
    }

    /// The receipt lines in file order (oldest first), filtered by
    /// `window`, keeping the **last** `max`. Returns `(lines, total)` where
    /// `total` counts every matching line before the cut.
    pub fn list(
        &self,
        window: Option<isize>,
        max: usize,
    ) -> Result<(Vec<serde_json::Value>, usize), CuError> {
        list_file(&self.path, window, max)
    }
}

/// `list` on a path: a missing file is an empty receipt set, a malformed
/// line is a typed failure (the file is evidence; a torn line must be
/// visible, not skipped).
pub fn list_file(
    path: &Path,
    window: Option<isize>,
    max: usize,
) -> Result<(Vec<serde_json::Value>, usize), CuError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(error) => {
            return Err(CuError::new(
                "receipt_unavailable",
                format!("could not read receipt file {}: {error}", path.display()),
            ));
        }
    };
    let mut matching = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
            CuError::new(
                "receipt_corrupt",
                format!(
                    "receipt file {} line {} is not JSON: {error}",
                    path.display(),
                    index + 1
                ),
            )
        })?;
        if let Some(window) = window
            && value.get("window").and_then(serde_json::Value::as_i64) != Some(window as i64)
        {
            continue;
        }
        matching.push(value);
    }
    let total = matching.len();
    if total > max {
        matching.drain(..total - max);
    }
    Ok((matching, total))
}

/// `receipts --max` validation: `None` is the default, `0` and anything
/// above the ceiling are `invalid_input`.
pub fn validate_list_max(max: Option<usize>) -> Result<usize, String> {
    match max {
        None => Ok(DEFAULT_LIST_MAX),
        Some(0) => Err("--max must be at least 1".to_owned()),
        Some(value) if value > MAX_LIST_MAX => {
            Err(format!("--max must be at most {MAX_LIST_MAX}, got {value}"))
        }
        Some(value) => Ok(value),
    }
}

fn merged(mut head: serde_json::Value, body: serde_json::Value) -> serde_json::Value {
    if let (Some(head_map), serde_json::Value::Object(body_map)) = (head.as_object_mut(), body) {
        for (key, value) in body_map {
            head_map.entry(key).or_insert(value);
        }
    }
    head
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::atomic::AtomicU64, sync::atomic::Ordering};

    use super::{ReceiptLog, list_file, validate_list_max};
    use crate::target::TargetRef;

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    fn scratch_dir(label: &str) -> PathBuf {
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "agenterm-cu-receipt-{label}-{}-{sequence}",
            std::process::id()
        ))
    }

    #[test]
    fn reserve_then_complete_writes_two_ordered_lines_that_share_an_id() {
        let dir = scratch_dir("pair");
        let mut log = ReceiptLog::open_in(&dir, TargetRef::Current).expect("open");
        let ticket = log
            .reserve(
                "invoke",
                7,
                serde_json::json!({ "action": "press", "before": { "text": "a" } }),
            )
            .expect("reserve");
        // The reserved line is on disk before anything else happens.
        let (lines, total) = list_file(log.path(), Some(7), 10).expect("list");
        assert_eq!(total, 1);
        assert_eq!(lines[0]["phase"], "reserved");
        assert_eq!(lines[0]["verb"], "invoke");
        assert_eq!(lines[0]["action"], "press");
        assert_eq!(lines[0]["before"]["text"], "a");
        log.complete(
            &ticket,
            "invoke",
            7,
            true,
            serde_json::json!({ "after": { "text": "b" } }),
        )
        .expect("complete");
        let other = log
            .reserve("close", 9, serde_json::json!({}))
            .expect("reserve other");
        log.complete(
            &other,
            "close",
            9,
            false,
            serde_json::json!({ "error": "x" }),
        )
        .expect("complete other");
        let (lines, total) = log.list(Some(7), 10).expect("list");
        assert_eq!(total, 2);
        assert_eq!(lines[0]["receipt_id"], lines[1]["receipt_id"]);
        assert_eq!(lines[1]["phase"], "completed");
        assert_eq!(lines[1]["verified"], true);
        let (all, total) = log.list(None, 1).expect("list last");
        assert_eq!(total, 4);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0]["phase"], "failed");
        assert_eq!(all[0]["window"], 9);
        assert_eq!(
            log.path().file_name().and_then(|name| name.to_str()),
            Some("current.jsonl")
        );
        drop(log);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_empty_and_a_torn_line_is_typed() {
        let dir = scratch_dir("torn");
        let (lines, total) = list_file(&dir.join("nope.jsonl"), None, 5).expect("missing");
        assert!(lines.is_empty() && total == 0);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("current.jsonl");
        std::fs::write(&path, "{\"window\":1}\n{\"win").expect("write");
        let error = list_file(&path, None, 5).expect_err("torn line");
        assert_eq!(error.code, "receipt_corrupt");
        assert!(error.message.contains("line 2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_failure_is_typed_receipt_unavailable() {
        let dir = scratch_dir("blocked");
        std::fs::create_dir_all(&dir).expect("dir");
        // A regular file where the receipt directory must be.
        let blocked = dir.join("cu-receipts");
        std::fs::write(&blocked, "not a directory").expect("write");
        let error = match ReceiptLog::open_in(&blocked, TargetRef::Current) {
            Ok(_) => panic!("a file where the directory must be cannot open"),
            Err(error) => error,
        };
        assert_eq!(error.code, "receipt_unavailable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_max_is_bounded() {
        assert_eq!(validate_list_max(None), Ok(50));
        assert_eq!(validate_list_max(Some(3)), Ok(3));
        assert!(validate_list_max(Some(0)).is_err());
        assert!(validate_list_max(Some(1_001)).is_err());
    }
}
