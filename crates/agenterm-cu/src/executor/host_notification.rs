//! Typed host notification dispatch with content-redacted durable evidence.

use agenterm_platform::host_notification::{HostNotificationErrorKind, HostNotificationOptions};
use serde_json::json;

use super::*;

fn evidence(value: &str) -> serde_json::Value {
    json!({ "byte_length": value.len(), "sha256": clipboard_sha256_hex(value.as_bytes()) })
}

pub(super) fn host_notify_payload(
    title: &str,
    body: &str,
    subtitle: Option<&str>,
    sound: bool,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    let title_evidence = evidence(title);
    let body_evidence = evidence(body);
    let subtitle_evidence = subtitle.map(evidence);
    let ticket = receipts.reserve(
        "host-notify",
        0,
        json!({
            "title_evidence": title_evidence,
            "body_evidence": body_evidence,
            "subtitle_evidence": subtitle_evidence,
            "sound_requested": sound,
        }),
    )?;
    let native = match agenterm_platform::host_notification::notify(
        title,
        body,
        HostNotificationOptions { subtitle, sound },
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            let (code, effect) = match error.kind() {
                HostNotificationErrorKind::InvalidInput => {
                    ("host_notification_invalid_input", "not_performed")
                }
                HostNotificationErrorKind::Unsupported => {
                    ("host_notification_unsupported", "not_performed")
                }
                HostNotificationErrorKind::DispatcherUnavailable => {
                    ("host_notification_dispatcher_unavailable", "not_performed")
                }
                HostNotificationErrorKind::Rejected => {
                    ("host_notification_rejected", "not_performed")
                }
                HostNotificationErrorKind::TimedOut => {
                    ("host_notification_outcome_unknown", "unknown")
                }
                HostNotificationErrorKind::Native => ("host_notification_failed", "unknown"),
                _ => ("host_notification_failed", "unknown"),
            };
            let typed = CuError::new(code, error.to_string());
            receipts.complete(
                &ticket,
                "host-notify",
                0,
                false,
                json!({
                    "performed": effect, "accepted": false, "verified": false,
                    "error": error_payload(&typed),
                }),
            )?;
            return Err(typed.with_detail(json!({ "effect": effect, "receipt": ticket.json() })));
        }
    };
    receipts.complete(
        &ticket,
        "host-notify",
        0,
        true,
        json!({
            "performed": true, "accepted": native.accepted, "verified": false,
            "provider": native.provider, "verification": "dispatcher-accepted-only",
        }),
    )?;
    Ok(json!({
        "performed": true, "accepted": native.accepted, "verified": false,
        "provider": native.provider, "verification": "dispatcher-accepted-only",
        "title_evidence": title_evidence, "body_evidence": body_evidence,
        "subtitle_evidence": subtitle_evidence, "sound_requested": sound,
        "receipt": ticket.json(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_title_fails_typed_and_closes_receipt_without_plaintext() {
        let directory = std::env::temp_dir().join(format!(
            "agenterm-cu-host-notify-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        let mut receipts = ReceiptLog::open_in(&directory, TargetRef::Current).unwrap();
        let error = host_notify_payload("", "secret body", None, false, &mut receipts).unwrap_err();
        assert_eq!(error.code, "host_notification_invalid_input");
        let (lines, total) = receipts.list(None, 10).unwrap();
        assert_eq!(total, 2);
        let encoded = serde_json::to_string(&lines).unwrap();
        assert!(!encoded.contains("secret body"));
        assert_eq!(lines[1]["performed"], "not_performed");
        std::fs::remove_dir_all(directory).unwrap();
    }
}
