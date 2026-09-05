use std::{process::Stdio, time::Duration};

use crate::host_notification::{
    HostNotificationError, HostNotificationErrorKind, HostNotificationOptions,
    HostNotificationReceipt,
};

pub(crate) fn notify(
    title: &str,
    body: &str,
    options: HostNotificationOptions<'_>,
) -> Result<HostNotificationReceipt, HostNotificationError> {
    if options.subtitle.is_some() || options.sound {
        return Err(HostNotificationError::new(
            HostNotificationErrorKind::Unsupported,
            "Linux host notification does not claim subtitle or sound semantics",
        ));
    }
    let dispatcher = ["/usr/bin/notify-send", "/bin/notify-send"]
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
        .ok_or_else(|| {
            HostNotificationError::new(
                HostNotificationErrorKind::DispatcherUnavailable,
                "notify-send is not installed at a system path",
            )
        })?;
    let mut child = std::process::Command::new(dispatcher)
        .args([title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            HostNotificationError::new(
                HostNotificationErrorKind::Native,
                format!("notify-send could not start: {error}"),
            )
        })?;
    wait(&mut child)
}

fn wait(child: &mut std::process::Child) -> Result<HostNotificationReceipt, HostNotificationError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return Ok(HostNotificationReceipt {
                    provider: "linux-notify-send",
                    accepted: true,
                });
            }
            Ok(Some(status)) => {
                return Err(HostNotificationError::new(
                    HostNotificationErrorKind::Rejected,
                    format!("notify-send rejected the request with {status}"),
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
                    "notify-send did not finish within 10 seconds",
                ));
            }
            Err(error) => {
                return Err(HostNotificationError::new(
                    HostNotificationErrorKind::Native,
                    format!("notify-send status failed: {error}"),
                ));
            }
        }
    }
}
