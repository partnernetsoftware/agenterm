use std::{process::Stdio, sync::OnceLock, time::Duration};

use crate::host_notification::{
    HostNotificationAction, HostNotificationError, HostNotificationErrorKind,
    HostNotificationOptions, HostNotificationReceipt,
};

const NOTIFY_SEND_PATHS: &[&str] = &["/usr/bin/notify-send", "/bin/notify-send"];

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
    if let Some(dispatcher) = NOTIFY_SEND_PATHS
        .iter()
        .find(|path| std::path::Path::new(**path).is_file())
    {
        return notify_via_send(dispatcher, title, body, options.actions);
    }
    notify_via_fdo(title, body, options.actions)
}

fn notify_via_send(
    dispatcher: &str,
    title: &str,
    body: &str,
    actions: &[HostNotificationAction<'_>],
) -> Result<HostNotificationReceipt, HostNotificationError> {
    let mut command = std::process::Command::new(dispatcher);
    for action in actions {
        command.arg(format!("--action={}", action.key));
        command.arg(action.label);
    }
    let mut child = command
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
    wait_send(&mut child)
}

fn wait_send(
    child: &mut std::process::Child,
) -> Result<HostNotificationReceipt, HostNotificationError> {
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

fn notify_via_fdo(
    title: &str,
    body: &str,
    actions: &[HostNotificationAction<'_>],
) -> Result<HostNotificationReceipt, HostNotificationError> {
    #[cfg(feature = "service")]
    {
        return notify_via_fdo_session_bus(title, body, actions);
    }
    #[cfg(not(feature = "service"))]
    {
        let _ = (title, body, actions);
        Err(HostNotificationError::new(
            HostNotificationErrorKind::DispatcherUnavailable,
            "notify-send is not installed and session-bus notification dispatch is unavailable",
        ))
    }
}

#[cfg(feature = "service")]
fn notify_via_fdo_session_bus(
    title: &str,
    body: &str,
    actions: &[HostNotificationAction<'_>],
) -> Result<HostNotificationReceipt, HostNotificationError> {
    use tokio::time::{Duration as TokioDuration, timeout};

    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    fn runtime() -> &'static tokio::runtime::Runtime {
        RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime for freedesktop notifications")
        })
    }

    runtime().block_on(async {
        timeout(
            TokioDuration::from_secs(10),
            notify_via_fdo_async(title, body, actions),
        )
        .await
        .map_err(|_| {
            HostNotificationError::new(
                HostNotificationErrorKind::TimedOut,
                "org.freedesktop.Notifications.Notify did not finish within 10 seconds",
            )
        })?
    })
}

#[cfg(feature = "service")]
async fn notify_via_fdo_async(
    title: &str,
    body: &str,
    actions: &[HostNotificationAction<'_>],
) -> Result<HostNotificationReceipt, HostNotificationError> {
    use std::collections::HashMap;

    use zbus::{Connection, Proxy, zvariant::OwnedValue};

    let connection = Connection::session().await.map_err(|error| {
        HostNotificationError::new(
            HostNotificationErrorKind::DispatcherUnavailable,
            format!("session D-Bus is unavailable: {error}"),
        )
    })?;
    let proxy = Proxy::new(
        &connection,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
    .await
    .map_err(|error| {
        HostNotificationError::new(
            HostNotificationErrorKind::Native,
            format!("org.freedesktop.Notifications proxy failed: {error}"),
        )
    })?;
    let mut flat_actions: Vec<String> = Vec::with_capacity(actions.len() * 2);
    for action in actions {
        flat_actions.push(action.key.to_owned());
        flat_actions.push(action.label.to_owned());
    }
    let hints: HashMap<String, OwnedValue> = HashMap::new();
    let _: u32 = proxy
        .call(
            "Notify",
            &(
                "agenterm",
                0u32,
                "",
                title,
                body,
                flat_actions,
                hints,
                -1i32,
            ),
        )
        .await
        .map_err(map_fdo_notify_error)?;
    Ok(HostNotificationReceipt {
        provider: "linux-fdo-notifications",
        accepted: true,
    })
}

#[cfg(feature = "service")]
fn map_fdo_notify_error(error: zbus::Error) -> HostNotificationError {
    if let zbus::Error::MethodError(name, _, _) = &error {
        let name = name.as_str();
        if name == "org.freedesktop.DBus.Error.ServiceUnknown"
            || name == "org.freedesktop.DBus.Error.NameHasNoOwner"
        {
            return HostNotificationError::new(
                HostNotificationErrorKind::DispatcherUnavailable,
                "org.freedesktop.Notifications is not available on the session bus",
            );
        }
    }
    HostNotificationError::new(
        HostNotificationErrorKind::Rejected,
        format!("org.freedesktop.Notifications.Notify rejected the request: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_notification::HostNotificationOptions;

    #[test]
    fn linux_rejects_subtitle_and_sound_before_dispatch() {
        assert_eq!(
            notify(
                "title",
                "body",
                HostNotificationOptions {
                    subtitle: Some("sub"),
                    sound: false,
                    actions: &[],
                }
            )
            .unwrap_err()
            .kind(),
            HostNotificationErrorKind::Unsupported
        );
        assert_eq!(
            notify(
                "title",
                "body",
                HostNotificationOptions {
                    subtitle: None,
                    sound: true,
                    actions: &[],
                }
            )
            .unwrap_err()
            .kind(),
            HostNotificationErrorKind::Unsupported
        );
    }
}
