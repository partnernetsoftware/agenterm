use std::{process::Stdio, time::Duration};

use crate::host_open::{HostOpenError, HostOpenErrorKind, HostOpenOptions, HostOpenReceipt};

pub(crate) fn open(
    target: &str,
    options: HostOpenOptions<'_>,
) -> Result<HostOpenReceipt, HostOpenError> {
    let mut command = std::process::Command::new("/usr/bin/open");
    if options.background {
        command.arg("-g");
    }
    if let Some(application) = options.application {
        command.args(["-a", application]);
    }
    command
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    dispatch(command, "macos-open")
}

fn dispatch(
    mut command: std::process::Command,
    provider: &'static str,
) -> Result<HostOpenReceipt, HostOpenError> {
    let mut child = command.spawn().map_err(|error| {
        HostOpenError::new(
            if error.kind() == std::io::ErrorKind::NotFound {
                HostOpenErrorKind::LauncherUnavailable
            } else {
                HostOpenErrorKind::Native
            },
            format!("host open launcher could not start: {error}"),
        )
    })?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return Ok(HostOpenReceipt {
                    provider,
                    accepted: true,
                });
            }
            Ok(Some(status)) => {
                return Err(HostOpenError::new(
                    HostOpenErrorKind::Rejected,
                    format!("host open launcher rejected the request with {status}"),
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
                    "host open launcher did not finish within 10 seconds",
                ));
            }
            Err(error) => {
                return Err(HostOpenError::new(
                    HostOpenErrorKind::Native,
                    format!("host open launcher status failed: {error}"),
                ));
            }
        }
    }
}
