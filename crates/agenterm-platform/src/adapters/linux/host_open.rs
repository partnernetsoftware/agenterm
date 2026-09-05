use std::{process::Stdio, time::Duration};

use crate::host_open::{HostOpenError, HostOpenErrorKind, HostOpenOptions, HostOpenReceipt};

pub(crate) fn open(
    target: &str,
    options: HostOpenOptions<'_>,
) -> Result<HostOpenReceipt, HostOpenError> {
    if options.application.is_some() || options.background {
        return Err(HostOpenError::new(
            HostOpenErrorKind::Unsupported,
            "Linux host-open does not claim application selection or background activation semantics",
        ));
    }
    let launcher = ["/usr/bin/xdg-open", "/bin/xdg-open"]
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
        .ok_or_else(|| {
            HostOpenError::new(
                HostOpenErrorKind::LauncherUnavailable,
                "xdg-open is not installed at a system path",
            )
        })?;
    let mut child = std::process::Command::new(launcher)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            HostOpenError::new(
                HostOpenErrorKind::Native,
                format!("xdg-open could not start: {error}"),
            )
        })?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return Ok(HostOpenReceipt {
                    provider: "linux-xdg-open",
                    accepted: true,
                });
            }
            Ok(Some(status)) => {
                return Err(HostOpenError::new(
                    HostOpenErrorKind::Rejected,
                    format!("xdg-open rejected the request with {status}"),
                ));
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HostOpenError::new(
                    HostOpenErrorKind::TimedOut,
                    "xdg-open did not finish within 10 seconds",
                ));
            }
            Err(error) => {
                return Err(HostOpenError::new(
                    HostOpenErrorKind::Native,
                    format!("xdg-open status failed: {error}"),
                ));
            }
        }
    }
}
