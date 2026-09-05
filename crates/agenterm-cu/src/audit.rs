//! Machine-readable audit records for authorized actuation (PRD_02_31).

use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::{auth::Grant, command::Command, reply::CuError, target::TargetRef};
use agenterm_platform::locking::{LockErrorKind, PathLock};

const MAX_AUDIT_RECORD_BYTES: usize = 256 * 1024;

#[derive(Serialize)]
struct AuditRecord<'a> {
    schema_version: u32,
    ts_ms: u128,
    target: &'a str,
    verb: &'a str,
    grant: &'a str,
    decision: &'a str,
    authority_scope: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    grant_id: Option<&'a str>,
    outcome: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<serde_json::Value>,
}

pub struct AuditLog {
    path: PathBuf,
    file: File,
    #[cfg(test)]
    injected_failure: Option<InjectedAuditFailure>,
    #[cfg(test)]
    successful_records: usize,
}

pub const DEFAULT_QUERY_MAX: usize = 200;
pub const MAX_QUERY_MAX: usize = 5_000;
pub const DEFAULT_QUERY_SCAN_MAX: usize = 10_000;
pub const MAX_QUERY_SCAN_MAX: usize = 100_000;
pub const DEFAULT_QUERY_BYTE_MAX: usize = 4 * 1024 * 1024;
pub const MAX_QUERY_BYTE_MAX: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct AuditQuery<'a> {
    pub verb: Option<&'a str>,
    pub outcome: Option<&'a str>,
    pub since_ms: Option<u128>,
    pub offset: Option<usize>,
    pub max: Option<usize>,
    pub scan_max: Option<usize>,
    pub byte_max: Option<usize>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum InjectedAuditFailure {
    AppendAfter(usize),
    FlushAfter(usize),
}

impl AuditLog {
    pub fn open() -> Result<Self, CuError> {
        let path =
            resolved_audit_path().map_err(|error| CuError::new("audit_unavailable", error))?;
        Self::open_at(path)
    }

    pub(crate) fn open_at(path: impl AsRef<Path>) -> Result<Self, CuError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                CuError::new(
                    "audit_unavailable",
                    format!(
                        "could not create audit directory {}: {error}",
                        parent.display()
                    ),
                )
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                CuError::new(
                    "audit_unavailable",
                    format!("could not open audit log {}: {error}", path.display()),
                )
            })?;
        Ok(Self {
            path,
            file,
            #[cfg(test)]
            injected_failure: None,
            #[cfg(test)]
            successful_records: 0,
        })
    }

    #[cfg(test)]
    pub(crate) fn inject_failure(&mut self, failure: InjectedAuditFailure) {
        self.injected_failure = Some(failure);
    }

    pub fn record_actuation(
        &mut self,
        target: TargetRef,
        command: &Command,
        grant: Grant,
        outcome: &str,
        detail: Option<serde_json::Value>,
    ) -> Result<(), CuError> {
        let record = AuditRecord {
            schema_version: 1,
            ts_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0),
            target: target.as_str(),
            verb: &command.verb(),
            grant: match grant {
                Grant::Observe => "observe",
                Grant::Actuate => "actuate",
            },
            decision: "authorized",
            authority_scope: "process",
            decision_id: None,
            target_id: None,
            grant_id: None,
            outcome,
            detail,
        };
        self.write_record(&record)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_persisted(
        &mut self,
        target: TargetRef,
        command: &Command,
        grant: Grant,
        decision_id: &str,
        target_id: &str,
        grant_id: &str,
        decision: &str,
        outcome: &str,
        detail: Option<serde_json::Value>,
    ) -> Result<(), CuError> {
        let record = AuditRecord {
            schema_version: 1,
            ts_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis())
                .unwrap_or(0),
            target: target.as_str(),
            verb: &command.verb(),
            grant: match grant {
                Grant::Observe => "observe",
                Grant::Actuate => "actuate",
            },
            decision,
            authority_scope: "stored_bounded",
            decision_id: Some(decision_id),
            target_id: Some(target_id),
            grant_id: Some(grant_id),
            outcome,
            detail,
        };
        self.write_record(&record)
    }

    fn write_record(&mut self, record: &AuditRecord<'_>) -> Result<(), CuError> {
        let line = serde_json::to_string(&record).map_err(|error| {
            CuError::new(
                "audit_unavailable",
                format!("audit serialization failed: {error}"),
            )
        })?;
        if line.len() > MAX_AUDIT_RECORD_BYTES {
            return Err(CuError::new(
                "audit_record_too_large",
                format!(
                    "audit record is {} bytes; limit is {MAX_AUDIT_RECORD_BYTES}",
                    line.len()
                ),
            ));
        }
        let lock_path = self.path.with_extension("jsonl.lock");
        let _lock = acquire_audit_lock(&lock_path)?;
        #[cfg(test)]
        if matches!(
            self.injected_failure,
            Some(InjectedAuditFailure::AppendAfter(records))
                if records == self.successful_records
        ) {
            return Err(CuError::new(
                "audit_unavailable",
                format!(
                    "could not append audit log {}: injected failure",
                    self.path.display()
                ),
            ));
        }
        writeln!(self.file, "{line}").map_err(|error| {
            CuError::new(
                "audit_unavailable",
                format!(
                    "could not append audit log {}: {error}",
                    self.path.display()
                ),
            )
        })?;
        #[cfg(test)]
        if matches!(
            self.injected_failure,
            Some(InjectedAuditFailure::FlushAfter(records))
                if records == self.successful_records
        ) {
            return Err(CuError::new(
                "audit_unavailable",
                format!(
                    "could not flush audit log {}: injected failure",
                    self.path.display()
                ),
            ));
        }
        self.file
            .flush()
            .and_then(|_| self.file.sync_data())
            .map_err(|error| {
                CuError::new(
                    "audit_unavailable",
                    format!(
                        "could not durably flush audit log {}: {error}",
                        self.path.display()
                    ),
                )
            })?;
        #[cfg(test)]
        {
            self.successful_records += 1;
        }
        Ok(())
    }
}

fn acquire_audit_lock(path: &Path) -> Result<PathLock, CuError> {
    // Durable fsync can exceed a scheduler quantum under concurrent writers.
    // Keep admission bounded, but do not turn normal serialization into a
    // spurious audit failure on a busy disk.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match PathLock::try_acquire(path) {
            Ok(lock) => return Ok(lock),
            Err(error) if error.kind() == LockErrorKind::Contended && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                let code = if error.kind() == LockErrorKind::Contended {
                    "audit_busy"
                } else {
                    "audit_unavailable"
                };
                return Err(
                    CuError::new(code, "could not acquire the audit append lock")
                        .with_detail(serde_json::json!({ "kind": format!("{:?}", error.kind()) })),
                );
            }
        }
    }
}

/// Read the newest matching audit records under independent scan, result and
/// byte budgets. A torn/malformed record is counted and skipped: audit query
/// must expose evidence loss without making every older valid record
/// unreachable.
pub fn query(query: AuditQuery<'_>) -> Result<serde_json::Value, CuError> {
    let path = resolved_audit_path().map_err(|error| CuError::new("audit_unavailable", error))?;
    query_at(&path, query)
}

pub(crate) fn query_at(path: &Path, query: AuditQuery<'_>) -> Result<serde_json::Value, CuError> {
    let offset = bounded("--offset", query.offset.unwrap_or(0), 0, 100_000)?;
    let max = bounded(
        "--max",
        query.max.unwrap_or(DEFAULT_QUERY_MAX),
        1,
        MAX_QUERY_MAX,
    )?;
    let scan_max = bounded(
        "--scan-max",
        query.scan_max.unwrap_or(DEFAULT_QUERY_SCAN_MAX),
        1,
        MAX_QUERY_SCAN_MAX,
    )?;
    let byte_max = bounded(
        "--byte-max",
        query.byte_max.unwrap_or(DEFAULT_QUERY_BYTE_MAX),
        1_024,
        MAX_QUERY_BYTE_MAX,
    )?;
    if let Some(outcome) = query.outcome
        && !matches!(outcome, "attempt" | "ok" | "failed" | "refused")
    {
        return Err(CuError::new(
            "invalid_input",
            "--outcome must be attempt|ok|failed|refused",
        ));
    }

    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(query_reply(
                path,
                query,
                offset,
                max,
                scan_max,
                byte_max,
                Vec::new(),
                0,
                0,
                0,
                0,
                false,
                false,
            ));
        }
        Err(error) => {
            return Err(CuError::new(
                "audit_unavailable",
                format!("could not open audit log {}: {error}", path.display()),
            ));
        }
    };
    let size = file
        .metadata()
        .map_err(|error| {
            CuError::new(
                "audit_unavailable",
                format!("could not stat audit log {}: {error}", path.display()),
            )
        })?
        .len();
    let read_len = size.min(byte_max as u64) as usize;
    let start = size.saturating_sub(read_len as u64);
    file.seek(SeekFrom::Start(start)).map_err(|error| {
        CuError::new(
            "audit_unavailable",
            format!("could not seek audit log {}: {error}", path.display()),
        )
    })?;
    let mut bytes = vec![0; read_len];
    file.read_exact(&mut bytes).map_err(|error| {
        CuError::new(
            "audit_unavailable",
            format!("could not read audit log {}: {error}", path.display()),
        )
    })?;
    let truncated_bytes = start > 0;
    if truncated_bytes {
        bytes = match bytes.iter().position(|byte| *byte == b'\n') {
            Some(boundary) => bytes.split_off(boundary + 1),
            None => Vec::new(),
        };
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut scanned = 0usize;
    let mut matched = 0usize;
    let mut malformed = 0usize;
    let mut records = Vec::new();
    let mut truncated_scan = false;
    for line in text.lines().rev().filter(|line| !line.trim().is_empty()) {
        if scanned == scan_max {
            truncated_scan = true;
            break;
        }
        scanned += 1;
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(serde_json::Value::Object(object)) => serde_json::Value::Object(object),
            _ => {
                malformed += 1;
                continue;
            }
        };
        if query.verb.is_some_and(|needle| {
            !value["verb"]
                .as_str()
                .is_some_and(|verb| verb.contains(needle))
        }) || query
            .outcome
            .is_some_and(|outcome| value["outcome"].as_str() != Some(outcome))
            || query.since_ms.is_some_and(|since| {
                value["ts_ms"]
                    .as_u64()
                    .map(u128::from)
                    .is_none_or(|ts| ts < since)
            })
        {
            continue;
        }
        let index = matched;
        matched += 1;
        if index >= offset && records.len() < max {
            records.push(value);
        }
    }
    Ok(query_reply(
        path,
        query,
        offset,
        max,
        scan_max,
        byte_max,
        records,
        scanned,
        matched,
        malformed,
        bytes.len(),
        truncated_scan,
        truncated_bytes,
    ))
}

fn bounded(name: &str, value: usize, min: usize, max: usize) -> Result<usize, CuError> {
    if value < min || value > max {
        return Err(CuError::new(
            "invalid_input",
            format!("{name} must be in {min}..={max}, got {value}"),
        ));
    }
    Ok(value)
}

#[allow(clippy::too_many_arguments)]
fn query_reply(
    path: &Path,
    query: AuditQuery<'_>,
    offset: usize,
    max: usize,
    scan_max: usize,
    byte_max: usize,
    records: Vec<serde_json::Value>,
    scanned: usize,
    matched: usize,
    malformed: usize,
    scanned_bytes: usize,
    truncated_scan: bool,
    truncated_bytes: bool,
) -> serde_json::Value {
    let truncated_results = matched > offset.saturating_add(records.len());
    let complete = !(truncated_results || truncated_scan || truncated_bytes);
    // An offset can continue only within the same scanned byte window. Byte or
    // scan truncation needs a future cursor contract; do not fabricate one.
    let next_offset = truncated_results.then(|| offset.saturating_add(records.len()));
    serde_json::json!({
        "addressing": "append-only-audit-jsonl",
        "path": path,
        "filter": { "verb": query.verb, "outcome": query.outcome, "since_ms": query.since_ms },
        "offset": offset,
        "max": max,
        "scan_max": scan_max,
        "byte_max": byte_max,
        "scanned": scanned,
        "scanned_bytes": scanned_bytes,
        "matched": matched,
        "returned": records.len(),
        "malformed": malformed,
        "truncated_results": truncated_results,
        "truncated_scan": truncated_scan,
        "truncated_bytes": truncated_bytes,
        "complete": complete,
        "next_offset": next_offset,
        "truncated": !complete,
        "records": records,
    })
}

/// The audit log path this process would write: `AGENTERM_CU_AUDIT_PATH`
/// or the platform default. The receipt file (`receipt.rs`) lives beside
/// it, so one variable relocates both.
pub(crate) fn resolved_audit_path() -> Result<PathBuf, String> {
    std::env::var("AGENTERM_CU_AUDIT_PATH")
        .map(PathBuf::from)
        .or_else(|_| default_audit_path())
}

fn default_audit_path() -> Result<PathBuf, String> {
    // Resolution order: AGENTERM_CU_AUDIT_PATH is handled by AuditLog::open();
    // here we fall back HOME -> USERPROFILE (the latter covers Windows, which
    // does not set HOME by default).
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| {
            if cfg!(windows) {
                "neither HOME nor USERPROFILE is set".to_owned()
            } else {
                "HOME is not set".to_owned()
            }
        })?;
    if cfg!(windows) {
        Ok(PathBuf::from(home)
            .join("AppData")
            .join("Local")
            .join("agenterm")
            .join("cu-audit.jsonl"))
    } else {
        Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("agenterm")
            .join("cu-audit.jsonl"))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::OpenOptions,
        io::Write,
        path::{Path, PathBuf},
        sync::{
            Arc, Barrier,
            atomic::{AtomicU64, Ordering},
        },
    };

    use super::{AuditLog, AuditQuery, InjectedAuditFailure, default_audit_path, query_at};
    use crate::{auth::Grant, command::Command, target::TargetRef};

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    fn scratch_path(label: &str) -> PathBuf {
        let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "agenterm-cu-audit-{label}-{}-{sequence}",
                std::process::id()
            ))
            .join("audit.jsonl")
    }

    fn command() -> Command {
        Command::WindowPlace {
            target: TargetRef::Current,
            action: "left-half".into(),
            window: None,
            frame: None,
        }
    }

    fn remove_scratch(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn default_audit_path_resolves_on_current_platform() {
        let path =
            default_audit_path().expect("default audit path must resolve on the current platform");
        assert!(!path.as_os_str().is_empty(), "path must not be empty");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("cu-audit.jsonl")
        );
    }

    #[test]
    fn one_open_log_appends_and_flushes_both_records() {
        let path = scratch_path("same-handle");
        let mut audit = AuditLog::open_at(&path).expect("open isolated audit");
        audit
            .record_actuation(
                TargetRef::Current,
                &command(),
                Grant::Actuate,
                "attempt",
                None,
            )
            .expect("attempt record");
        audit
            .record_actuation(
                TargetRef::Current,
                &command(),
                Grant::Actuate,
                "ok",
                Some(serde_json::json!({"result": "placed"})),
            )
            .expect("outcome record");
        let records: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .expect("read audit")
            .lines()
            .map(|line| serde_json::from_str(line).expect("record JSON"))
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["outcome"], "attempt");
        assert_eq!(records[0]["decision"], "authorized");
        assert_eq!(records[0]["authority_scope"], "process");
        assert_eq!(records[1]["outcome"], "ok");
        drop(audit);
        remove_scratch(&path);
    }

    #[test]
    fn append_failure_is_typed() {
        let path = scratch_path("append-failure");
        let mut audit = AuditLog::open_at(&path).expect("open isolated audit");
        audit.inject_failure(InjectedAuditFailure::AppendAfter(0));
        let error = audit
            .record_actuation(
                TargetRef::Current,
                &command(),
                Grant::Actuate,
                "attempt",
                None,
            )
            .expect_err("append failure must fail closed");
        assert_eq!(error.code, "audit_unavailable");
        assert!(error.message.contains("append"));
        drop(audit);
        remove_scratch(&path);
    }

    #[test]
    fn flush_failure_is_typed() {
        let path = scratch_path("flush-failure");
        let mut audit = AuditLog::open_at(&path).expect("open isolated audit");
        audit.inject_failure(InjectedAuditFailure::FlushAfter(0));
        let error = audit
            .record_actuation(
                TargetRef::Current,
                &command(),
                Grant::Actuate,
                "attempt",
                None,
            )
            .expect_err("flush failure must fail closed");
        assert_eq!(error.code, "audit_unavailable");
        assert!(error.message.contains("flush"));
        drop(audit);
        remove_scratch(&path);
    }

    #[test]
    fn query_is_newest_first_filtered_bounded_and_malformed_visible() {
        let path = scratch_path("query");
        let mut audit = AuditLog::open_at(&path).expect("open isolated audit");
        for (outcome, marker) in [("attempt", 1), ("ok", 2), ("failed", 3)] {
            audit
                .record_actuation(
                    TargetRef::Current,
                    &command(),
                    Grant::Actuate,
                    outcome,
                    Some(serde_json::json!({"marker": marker})),
                )
                .expect("audit record");
        }
        drop(audit);
        let mut append = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append malformed fixture");
        writeln!(append, "{{torn").expect("append malformed line");
        append.flush().expect("flush malformed line");

        let reply = query_at(
            &path,
            AuditQuery {
                verb: Some("window"),
                max: Some(2),
                ..AuditQuery::default()
            },
        )
        .expect("query");
        assert_eq!(reply["scanned"], 4);
        assert_eq!(reply["matched"], 3);
        assert_eq!(reply["returned"], 2);
        assert_eq!(reply["malformed"], 1);
        assert_eq!(reply["truncated"], true);
        assert_eq!(reply["records"][0]["outcome"], "failed");
        assert_eq!(reply["records"][1]["outcome"], "ok");

        let filtered = query_at(
            &path,
            AuditQuery {
                outcome: Some("ok"),
                ..AuditQuery::default()
            },
        )
        .expect("filtered query");
        assert_eq!(filtered["matched"], 1);
        assert_eq!(filtered["records"][0]["detail"]["marker"], 2);
        remove_scratch(&path);
    }

    #[test]
    fn query_missing_file_is_empty_and_limits_fail_typed() {
        let path = scratch_path("missing-query");
        let empty = query_at(&path, AuditQuery::default()).expect("missing is empty");
        assert_eq!(empty["records"], serde_json::json!([]));
        let error = query_at(
            &path,
            AuditQuery {
                max: Some(0),
                ..AuditQuery::default()
            },
        )
        .expect_err("zero max");
        assert_eq!(error.code, "invalid_input");
    }

    #[test]
    fn concurrent_writers_keep_one_json_object_per_line() {
        let path = scratch_path("concurrent");
        let barrier = Arc::new(Barrier::new(4));
        let mut threads = Vec::new();
        for worker in 0..4 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                let mut audit = AuditLog::open_at(path).expect("open writer");
                barrier.wait();
                for sequence in 0..20 {
                    audit
                        .record_actuation(
                            TargetRef::Current,
                            &command(),
                            Grant::Actuate,
                            "ok",
                            Some(serde_json::json!({ "worker": worker, "sequence": sequence })),
                        )
                        .expect("serialized append");
                }
            }));
        }
        for thread in threads {
            thread.join().expect("writer thread");
        }
        let lines: Vec<_> = std::fs::read_to_string(&path)
            .expect("audit bytes")
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("whole JSON line"))
            .collect();
        assert_eq!(lines.len(), 80);
        remove_scratch(&path);
    }
}
