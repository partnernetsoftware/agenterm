//! Crash-persistent receipt file plumbing (`ReceiptLog`) and the
//! `receipts` read-back verb.

use super::*;

impl Executor {
    /// `<audit dir>/cu-receipts`: beside the audit log this executor writes
    /// (the injected test path, or the production resolution).
    pub(super) fn receipt_dir(&self) -> Result<PathBuf, CuError> {
        #[cfg(test)]
        if let Some(path) = self.audit_path.as_ref() {
            return Ok(receipt::receipt_dir_beside(path));
        }
        let audit_path = crate::audit::resolved_audit_path()
            .map_err(|error| CuError::new("receipt_unavailable", error))?;
        Ok(receipt::receipt_dir_beside(&audit_path))
    }

    /// The crash-persistent receipt file for `target`, opened before the
    /// mechanism is touched: failure to open it is failure to act.
    pub(super) fn open_receipts(&self, target: TargetRef) -> Result<ReceiptLog, CuError> {
        ReceiptLog::open_in(&self.receipt_dir()?, target)
    }
}

/// `receipts --window H --max N`: the target's receipt file read back in
/// order. Observation only — the file is not created here.
pub(super) fn receipts_payload(
    dir: &std::path::Path,
    target: TargetRef,
    window: Option<isize>,
    max: Option<usize>,
) -> Result<serde_json::Value, CuError> {
    let max = receipt::validate_list_max(max).map_err(invalid_input)?;
    let path = dir.join(format!("{}.jsonl", target.as_str()));
    let (lines, total) = receipt::list_file(&path, window, max)?;
    Ok(serde_json::json!({
        "addressing": "receipt-file",
        "path": path,
        "target": target.as_str(),
        "window": window,
        "max": max,
        "total": total,
        "returned": lines.len(),
        "truncated": total > lines.len(),
        "receipts": lines,
    }))
}
