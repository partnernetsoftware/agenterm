use std::{process::Stdio, time::Duration};

use crate::host_notification::{
    HostNotificationError, HostNotificationErrorKind, HostNotificationOptions,
    HostNotificationReceipt,
};

const SCRIPT: &str = r#"on run argv
set t to item 1 of argv
set b to item 2 of argv
set s to item 3 of argv
set audible to item 4 of argv
if audible is "1" and s is not "" then
  display notification b with title t subtitle s sound name "Submarine"
else if audible is "1" then
  display notification b with title t sound name "Submarine"
else if s is not "" then
  display notification b with title t subtitle s
else
  display notification b with title t
end if
end run"#;

pub(crate) fn notify(
    title: &str,
    body: &str,
    options: HostNotificationOptions<'_>,
) -> Result<HostNotificationReceipt, HostNotificationError> {
    let mut child = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            SCRIPT,
            "--",
            title,
            body,
            options.subtitle.unwrap_or(""),
            if options.sound { "1" } else { "0" },
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            HostNotificationError::new(
                HostNotificationErrorKind::Native,
                format!("notification dispatcher could not start: {error}"),
            )
        })?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return Ok(HostNotificationReceipt {
                    provider: "macos-user-notification",
                    accepted: true,
                });
            }
            Ok(Some(status)) => {
                return Err(HostNotificationError::new(
                    HostNotificationErrorKind::Rejected,
                    format!("notification dispatcher rejected the request with {status}"),
                ));
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HostNotificationError::new(
                    HostNotificationErrorKind::TimedOut,
                    "notification dispatcher did not finish within 10 seconds",
                ));
            }
            Err(error) => {
                return Err(HostNotificationError::new(
                    HostNotificationErrorKind::Native,
                    format!("notification dispatcher status failed: {error}"),
                ));
            }
        }
    }
}
