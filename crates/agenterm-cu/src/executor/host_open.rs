//! Typed host application dispatch. Acceptance is evidence of dispatch only,
//! never proof that a handler rendered or consumed the target.

use agenterm_platform::host_open::{HostOpenErrorKind, HostOpenOptions};
use serde_json::json;

use super::*;

pub(super) fn host_open_payload(
    target: &str,
    application: Option<&str>,
    background: bool,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let target_evidence = json!({
        "byte_length": target.len(),
        "sha256": clipboard_sha256_hex(target.as_bytes()),
    });
    let application_evidence = application.map(|value| {
        json!({
            "byte_length": value.len(),
            "sha256": clipboard_sha256_hex(value.as_bytes()),
        })
    });
    let ticket = receipts.reserve(
        "host-open",
        0,
        json!({
            "target_evidence": target_evidence,
            "application_evidence": application_evidence,
            "background_requested": background,
        }),
    )?;
    let native = match agenterm_platform::host_open::open(
        target,
        HostOpenOptions {
            application,
            background,
        },
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            let (code, effect) = match error.kind() {
                HostOpenErrorKind::InvalidInput => ("host_open_invalid_input", "not_performed"),
                HostOpenErrorKind::Unsupported => ("host_open_unsupported", "not_performed"),
                HostOpenErrorKind::LauncherUnavailable => {
                    ("host_open_launcher_unavailable", "not_performed")
                }
                HostOpenErrorKind::Rejected => ("host_open_rejected", "not_performed"),
                HostOpenErrorKind::TimedOut => ("host_open_outcome_unknown", "unknown"),
                HostOpenErrorKind::Native => ("host_open_failed", "unknown"),
                _ => ("host_open_failed", "unknown"),
            };
            let typed = CuError::new(code, error.to_string());
            receipts.complete(
                &ticket,
                "host-open",
                0,
                false,
                json!({
                    "performed": effect,
                    "accepted": false,
                    "verified": false,
                    "error": error_payload(&typed),
                }),
            )?;
            return Err(typed.with_detail(json!({
                "effect": effect,
                "receipt": ticket.json(),
            })));
        }
    };
    receipts.complete(
        &ticket,
        "host-open",
        0,
        true,
        json!({
            "performed": true,
            "accepted": native.accepted,
            "verified": false,
            "provider": native.provider,
            "verification": "dispatcher-accepted-only",
        }),
    )?;
    Ok(json!({
        "performed": true,
        "accepted": native.accepted,
        "verified": false,
        "provider": native.provider,
        "verification": "dispatcher-accepted-only",
        "background_requested": background,
        "target_evidence": target_evidence,
        "application_evidence": application_evidence,
        "receipt": ticket.json(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_target_fails_typed_and_closes_the_reserved_receipt() {
        let directory = std::env::temp_dir().join(format!(
            "agenterm-cu-host-open-test-{}-{}",
            std::process::id(),
            crate::executor::persisted::now_utc_ms().unwrap_or_default()
        ));
        let mut receipts =
            ReceiptLog::open_in(&directory, TargetRef::Current).expect("open receipt log");
        let error = host_open_payload("-option", None, false, &mut receipts)
            .expect_err("option-like target must fail before native dispatch");
        assert_eq!(error.code, "host_open_invalid_input");
        assert_eq!(error.detail.as_ref().unwrap()["effect"], "not_performed");
        let (lines, total) = receipts.list(None, 10).expect("read receipts");
        assert_eq!(total, 2);
        assert_eq!(lines[0]["phase"], "reserved");
        assert_eq!(lines[1]["phase"], "failed");
        assert_eq!(lines[1]["performed"], "not_performed");
        assert_eq!(lines[1]["accepted"], false);
        assert_eq!(lines[1]["verified"], false);
        std::fs::remove_dir_all(directory).expect("remove receipt fixture");
    }
}
