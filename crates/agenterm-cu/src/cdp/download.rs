//! Browser-level download control and receipts for one background page.
//!
//! Chromium's download policy belongs to the browser websocket, while the
//! click belongs to a page websocket.  The executor serializes that global
//! policy per listener; this module correlates events back to the selected
//! page's frame tree and never reads downloaded bytes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{CdpError, Session, Transport};

pub const DEFAULT_WAIT: Duration = Duration::from_secs(30);
pub const MAX_WAIT: Duration = Duration::from_secs(300);
const MAX_FRAMES: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct DownloadReceipt {
    pub guid: String,
    pub suggested_filename: String,
    pub final_path: PathBuf,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub file_size: u64,
}

impl DownloadReceipt {
    pub fn json(&self) -> Value {
        json!({
            "guid": self.guid,
            "suggested_filename": self.suggested_filename,
            "final_path": self.final_path.to_string_lossy(),
            "state": "completed",
            "received_bytes": self.received_bytes.to_string(),
            "total_bytes": self.total_bytes.to_string(),
            "file_size": self.file_size.to_string(),
            "verified": true,
            "content_read": false,
            "focus_changed": false,
        })
    }
}

pub fn frame_ids<T: Transport>(page: &mut Session<T>) -> Result<HashSet<String>, CdpError> {
    let result = page.call("Page.getFrameTree", json!({}))?;
    let mut ids = HashSet::new();
    collect_frame_ids(&result["frameTree"], &mut ids)?;
    if ids.is_empty() {
        return Err(CdpError::typed(
            "cdp_download_frame_missing",
            "selected page exposed no frame identity for download correlation",
        ));
    }
    Ok(ids)
}

fn collect_frame_ids(node: &Value, ids: &mut HashSet<String>) -> Result<(), CdpError> {
    if ids.len() >= MAX_FRAMES {
        return Err(CdpError::typed(
            "cdp_download_frame_limit",
            format!("page frame tree exceeds the {MAX_FRAMES}-frame download limit"),
        ));
    }
    if let Some(id) = node["frame"]["id"].as_str().filter(|id| !id.is_empty()) {
        ids.insert(id.to_owned());
    }
    if let Some(children) = node["childFrames"].as_array() {
        for child in children {
            collect_frame_ids(child, ids)?;
        }
    }
    Ok(())
}

pub fn enable<T: Transport>(browser: &mut Session<T>, directory: &Path) -> Result<(), CdpError> {
    browser
        .call(
            "Browser.setDownloadBehavior",
            json!({
                "behavior": "allowAndName",
                "downloadPath": directory.to_string_lossy(),
                "eventsEnabled": true,
            }),
        )
        .map(|_| ())
        .map_err(|error| {
            error.recode(
                "cdp_download_blocked",
                "Chromium refused the explicit download policy",
            )
        })
}

pub fn disable<T: Transport>(browser: &mut Session<T>) -> Result<(), CdpError> {
    browser
        .call(
            "Browser.setDownloadBehavior",
            json!({ "behavior": "default", "eventsEnabled": false }),
        )
        .map(|_| ())
        .map_err(|error| {
            error.recode(
                "cdp_download_cleanup_failed",
                "could not restore Chromium's default download policy",
            )
        })
}

pub fn wait<T: Transport>(
    browser: &mut Session<T>,
    frames: &HashSet<String>,
    directory: &Path,
    timeout: Duration,
) -> Result<DownloadReceipt, CdpError> {
    let deadline = Instant::now() + timeout.min(MAX_WAIT);
    let started = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(CdpError::typed(
                "cdp_download_not_started",
                "no download started from the selected page before the deadline",
            ));
        }
        let Some(event) = browser.wait_event("Browser.downloadWillBegin", remaining)? else {
            return Err(CdpError::typed(
                "cdp_download_not_started",
                "no download started from the selected page before the deadline",
            ));
        };
        if event["params"]["frameId"]
            .as_str()
            .is_some_and(|id| frames.contains(id))
        {
            break event;
        }
    };
    let guid = required_text(&started["params"], "guid")?;
    let suggested_filename = required_text(&started["params"], "suggestedFilename")?;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(CdpError::typed(
                "cdp_download_timeout",
                "download did not complete before the deadline",
            )
            .with_detail(json!({ "guid": guid })));
        }
        let Some(event) = browser.wait_event("Browser.downloadProgress", remaining)? else {
            return Err(CdpError::typed(
                "cdp_download_timeout",
                "download did not complete before the deadline",
            )
            .with_detail(json!({ "guid": guid })));
        };
        if event["params"]["guid"].as_str() != Some(guid.as_str()) {
            continue;
        }
        let state = event["params"]["state"].as_str().unwrap_or("inProgress");
        if state == "canceled" {
            return Err(
                CdpError::typed("cdp_download_canceled", "Chromium canceled the download")
                    .with_detail(json!({ "guid": guid })),
            );
        }
        if state != "completed" {
            continue;
        }
        let received_bytes = json_u64(&event["params"]["receivedBytes"]);
        let total_bytes = json_u64(&event["params"]["totalBytes"]);
        let candidate = event["params"]["filePath"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| directory.join(&guid));
        let canonical = candidate.canonicalize().map_err(|_| {
            CdpError::typed(
                "cdp_download_file_missing",
                "Chromium reported completion but the downloaded file is absent",
            )
            .with_detail(json!({ "guid": guid }))
        })?;
        if canonical.parent() != Some(directory) {
            return Err(CdpError::typed(
                "cdp_download_path_mismatch",
                "Chromium completed outside the requested download directory",
            )
            .with_detail(json!({ "guid": guid })));
        }
        let metadata = std::fs::symlink_metadata(&canonical).map_err(|_| {
            CdpError::typed(
                "cdp_download_file_missing",
                "downloaded file cannot be stated",
            )
            .with_detail(json!({ "guid": guid }))
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(CdpError::typed(
                "cdp_download_file_invalid",
                "download result is not a regular non-symlink file",
            )
            .with_detail(json!({ "guid": guid })));
        }
        return Ok(DownloadReceipt {
            guid,
            suggested_filename,
            final_path: canonical,
            received_bytes,
            total_bytes,
            file_size: metadata.len(),
        });
    }
}

fn required_text(value: &Value, key: &str) -> Result<String, CdpError> {
    value[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            CdpError::typed(
                "cdp_download_protocol_error",
                format!("download event has no {key}"),
            )
        })
}

fn json_u64(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| {
            value
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .map(|v| v as u64)
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdp::ws::fake::{FakeTransport, event, result};

    fn fixture_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agenterm-cu-download-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).expect("create download fixture");
        path.canonicalize().expect("canonical fixture")
    }

    #[test]
    fn completed_event_returns_a_stat_only_guid_receipt_and_restores_policy() {
        let directory = fixture_dir("completed");
        let guid = "11111111-2222-3333-4444-555555555555";
        std::fs::write(directory.join(guid), b"smoke").expect("write browser result fixture");
        let mut browser = Session::new(FakeTransport::new(move |id, method, params| {
            if method == "Browser.setDownloadBehavior" && params["behavior"] == "allowAndName" {
                return vec![
                    result(id, json!({})),
                    event(
                        "Browser.downloadWillBegin",
                        json!({
                            "frameId": "frame-1",
                            "guid": guid,
                            "url": "blob:smoke",
                            "suggestedFilename": "smoke.txt",
                        }),
                    ),
                    event(
                        "Browser.downloadProgress",
                        json!({
                            "guid": guid,
                            "totalBytes": 5,
                            "receivedBytes": 5,
                            "state": "completed",
                        }),
                    ),
                ];
            }
            vec![result(id, json!({}))]
        }));
        enable(&mut browser, &directory).expect("enable");
        let receipt = wait(
            &mut browser,
            &HashSet::from(["frame-1".to_owned()]),
            &directory,
            Duration::from_secs(1),
        )
        .expect("completed download");
        disable(&mut browser).expect("restore");
        assert_eq!(receipt.guid, guid);
        assert_eq!(receipt.suggested_filename, "smoke.txt");
        assert_eq!(receipt.file_size, 5);
        assert_eq!(receipt.json()["content_read"], false);
        let sent = &browser.transport_ref().sent;
        assert_eq!(sent[0]["params"]["behavior"], "allowAndName");
        assert_eq!(sent[0]["params"]["eventsEnabled"], true);
        assert_eq!(sent[1]["params"]["behavior"], "default");
        assert_eq!(sent[1]["params"]["eventsEnabled"], false);
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn canceled_and_absent_start_have_distinct_typed_failures() {
        let directory = fixture_dir("failure");
        let mut canceled = Session::new(FakeTransport::new(|id, method, _| {
            if method == "Browser.setDownloadBehavior" {
                vec![
                    result(id, json!({})),
                    event(
                        "Browser.downloadWillBegin",
                        json!({
                            "frameId": "frame-1",
                            "guid": "gone",
                            "suggestedFilename": "gone.txt",
                        }),
                    ),
                    event(
                        "Browser.downloadProgress",
                        json!({ "guid": "gone", "state": "canceled" }),
                    ),
                ]
            } else {
                vec![result(id, json!({}))]
            }
        }));
        enable(&mut canceled, &directory).expect("enable");
        let error = wait(
            &mut canceled,
            &HashSet::from(["frame-1".to_owned()]),
            &directory,
            Duration::from_secs(1),
        )
        .expect_err("canceled");
        assert_eq!(error.code, "cdp_download_canceled");

        let mut silent = Session::new(FakeTransport::new(|id, _, _| vec![result(id, json!({}))]));
        let error = wait(
            &mut silent,
            &HashSet::from(["frame-1".to_owned()]),
            &directory,
            Duration::from_millis(1),
        )
        .expect_err("no start");
        assert_eq!(error.code, "cdp_download_not_started");
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn frame_tree_is_bounded_and_recursive() {
        let mut page = crate::cdp::ws::fake::session(|method, _| {
            assert_eq!(method, "Page.getFrameTree");
            Ok(json!({
                "frameTree": {
                    "frame": { "id": "root" },
                    "childFrames": [{ "frame": { "id": "child" } }],
                }
            }))
        });
        assert_eq!(
            frame_ids(&mut page).expect("frames"),
            HashSet::from(["root".to_owned(), "child".to_owned()])
        );
    }
}
