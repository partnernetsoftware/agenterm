use std::borrow::Cow;
use std::process::{Command, Stdio};

use crate::{
    CapabilityStatus,
    contract::ime::{ImeComposition, ImeStatus},
    ime::{ImeEnvSnapshot, ImeHostObservation, ImeObserveResult, ImeObserveUnsupported},
};

const IBUS_SERVICE: &str = "org.freedesktop.IBus";
const FCITX5_SERVICE: &str = "org.fcitx.Fcitx5";
const FCITX_SERVICE: &str = "org.fcitx.Fcitx";

const LINUX_IME_ALTERNATIVES: &[&str] = &[
    "enable IBus or Fcitx5 in the desktop session and set GTK_IM_MODULE, QT_IM_MODULE and XMODIFIERS",
    "dbus-send --session --dest=org.freedesktop.IBus /org/freedesktop/IBus org.freedesktop.IBus.GetGlobalEngine (verify the session bus answers)",
    "GUI preedit still arrives through winit IME events; ime-status observes the session framework only",
];

pub(crate) fn status() -> Option<ImeStatus> {
    match observe() {
        ImeObserveResult::Ok(observation) => Some(ImeStatus {
            name: observation.name.clone(),
            available: observation.available,
            open: observation.open,
            native_mode: observation.native_mode,
            full_shape: observation.full_shape,
        }),
        ImeObserveResult::Unsupported(_) => None,
    }
}

pub(crate) fn composition() -> Option<ImeComposition> {
    None
}

pub(crate) fn set_anchor_position(_x: i32, _y: i32) {}

pub(crate) fn capability_status(display_available: bool) -> CapabilityStatus {
    if display_available {
        CapabilityStatus::Available
    } else {
        CapabilityStatus::Unsupported {
            reason: Cow::Borrowed("headless-display"),
        }
    }
}

pub(crate) fn observe() -> ImeObserveResult {
    let env = read_env_snapshot();
    let session_bus_available = session_bus_available();
    let mut probed_frameworks = Vec::new();

    for framework in framework_probe_order(&env) {
        if probed_frameworks.iter().any(|seen| seen == &framework) {
            continue;
        }
        probed_frameworks.push(framework.clone());
        if let Some(observation) = probe_framework(&framework, &env, session_bus_available) {
            return ImeObserveResult::Ok(observation);
        }
    }

    ImeObserveResult::Unsupported(ImeObserveUnsupported {
        reason: if session_bus_available {
            "no IBus or Fcitx input-method framework answered on the session bus".into()
        } else {
            "the desktop session bus is unavailable".into()
        },
        env,
        probed_frameworks,
        session_bus_available,
        required_mechanism: "linux-session-input-method".into(),
        alternatives: LINUX_IME_ALTERNATIVES
            .iter()
            .map(|alternative| (*alternative).to_owned())
            .collect(),
    })
}

fn read_env_snapshot() -> ImeEnvSnapshot {
    ImeEnvSnapshot {
        gtk_im_module: env_var("GTK_IM_MODULE"),
        qt_im_module: env_var("QT_IM_MODULE"),
        xmodifiers: env_var("XMODIFIERS"),
    }
}

fn env_var(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn framework_probe_order(env: &ImeEnvSnapshot) -> Vec<String> {
    let mut order = Vec::new();
    let gtk = env
        .gtk_im_module
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let qt = env
        .qt_im_module
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let xmod = env
        .xmodifiers
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();

    for hint in [gtk.as_str(), qt.as_str()] {
        push_framework_hint(&mut order, hint);
    }
    if xmod.contains("@im=ibus") {
        push_unique(&mut order, "ibus");
    } else if xmod.contains("@im=fcitx") {
        push_unique(&mut order, "fcitx5");
        push_unique(&mut order, "fcitx");
    }
    for framework in ["ibus", "fcitx5", "fcitx"] {
        push_unique(&mut order, framework);
    }
    order
}

fn push_framework_hint(order: &mut Vec<String>, hint: &str) {
    match hint {
        "ibus" => push_unique(order, "ibus"),
        "fcitx5" => push_unique(order, "fcitx5"),
        "fcitx" => {
            push_unique(order, "fcitx5");
            push_unique(order, "fcitx");
        }
        _ => {}
    }
}

fn push_unique(order: &mut Vec<String>, framework: &str) {
    if !order.iter().any(|seen| seen == framework) {
        order.push(framework.to_owned());
    }
}

fn session_bus_available() -> bool {
    std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|value| !value.is_empty())
        && run_command(
            "dbus-send",
            &[
                "--session",
                "--dest=org.freedesktop.DBus",
                "--type=method_call",
                "--print-reply",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus.GetId",
            ],
        )
        .is_some()
}

fn probe_framework(
    framework: &str,
    env: &ImeEnvSnapshot,
    session_bus_available: bool,
) -> Option<ImeHostObservation> {
    match framework {
        "ibus" => probe_ibus(env, session_bus_available),
        "fcitx5" => probe_fcitx5(env, session_bus_available),
        "fcitx" => probe_fcitx(env, session_bus_available),
        _ => None,
    }
}

fn probe_ibus(env: &ImeEnvSnapshot, session_bus_available: bool) -> Option<ImeHostObservation> {
    if let Some(engine) = read_ibus_engine_cli() {
        return Some(ibus_observation(
            engine,
            env,
            session_bus_available,
            "ibus-cli",
        ));
    }
    if !session_bus_available || !dbus_name_has_owner(IBUS_SERVICE) {
        return None;
    }
    let engine = read_ibus_engine_dbus()?;
    Some(ibus_observation(
        engine,
        env,
        session_bus_available,
        "ibus-dbus",
    ))
}

fn probe_fcitx5(env: &ImeEnvSnapshot, session_bus_available: bool) -> Option<ImeHostObservation> {
    if let Some((name, open)) = read_fcitx5_cli() {
        return Some(fcitx_observation(
            "fcitx5",
            "fcitx5-cli",
            name,
            open,
            env,
            session_bus_available,
        ));
    }
    if !session_bus_available || !dbus_name_has_owner(FCITX5_SERVICE) {
        return None;
    }
    None
}

fn probe_fcitx(env: &ImeEnvSnapshot, session_bus_available: bool) -> Option<ImeHostObservation> {
    if let Some((name, open)) = read_fcitx_cli() {
        return Some(fcitx_observation(
            "fcitx",
            "fcitx-cli",
            name,
            open,
            env,
            session_bus_available,
        ));
    }
    if !session_bus_available || !dbus_name_has_owner(FCITX_SERVICE) {
        return None;
    }
    None
}

fn ibus_observation(
    engine: String,
    env: &ImeEnvSnapshot,
    session_bus_available: bool,
    provider: &str,
) -> ImeHostObservation {
    let plain_keyboard = engine.starts_with("xkb:");
    let native_mode = !plain_keyboard && looks_like_cjk_engine(&engine);
    let status = ImeStatus {
        name: engine,
        available: !plain_keyboard,
        open: !plain_keyboard,
        native_mode,
        full_shape: false,
    };
    ImeHostObservation::from_status(provider, "ibus", status, env.clone(), session_bus_available)
}

fn fcitx_observation(
    framework: &str,
    provider: &str,
    name: String,
    open: bool,
    env: &ImeEnvSnapshot,
    session_bus_available: bool,
) -> ImeHostObservation {
    let native_mode = open && looks_like_cjk_engine(&name);
    let status = ImeStatus {
        name,
        available: true,
        open,
        native_mode,
        full_shape: false,
    };
    ImeHostObservation::from_status(
        provider,
        framework,
        status,
        env.clone(),
        session_bus_available,
    )
}

fn looks_like_cjk_engine(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "pinyin",
        "zhuyin",
        "wubi",
        "cangjie",
        "hangul",
        "kana",
        "mozc",
        "anthy",
        "chewing",
        "rime",
        "sunpinyin",
        "libpinyin",
        "googlepinyin",
        "chinese",
        "japanese",
        "korean",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn read_ibus_engine_cli() -> Option<String> {
    let output = run_command("ibus", &["engine"])?;
    let engine = output.stdout.trim();
    (!engine.is_empty()).then(|| engine.to_owned())
}

fn read_ibus_engine_dbus() -> Option<String> {
    let output = run_command(
        "dbus-send",
        &[
            "--session",
            "--dest=org.freedesktop.IBus",
            "--type=method_call",
            "--print-reply",
            "/org/freedesktop/IBus",
            "org.freedesktop.IBus.GetGlobalEngine",
        ],
    )?;
    parse_dbus_object_path_engine(&output.stdout)
}

fn dbus_name_has_owner(service: &str) -> bool {
    run_command(
        "dbus-send",
        &[
            "--session",
            "--dest=org.freedesktop.DBus",
            "--type=method_call",
            "--print-reply",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.NameHasOwner",
            &format!("string:{service}"),
        ],
    )
    .is_some_and(|output| output.stdout.contains("boolean true"))
}

fn read_fcitx5_cli() -> Option<(String, bool)> {
    let name = run_command("fcitx5-remote", &["-n"])?;
    let open = run_command("fcitx5-remote", &["-o"])?;
    let name = name.stdout.trim();
    if name.is_empty() {
        return None;
    }
    Some((name.to_owned(), fcitx_open_flag(&open.stdout)))
}

fn read_fcitx_cli() -> Option<(String, bool)> {
    let name = run_command("fcitx-remote", &["-n"])?;
    let open = run_command("fcitx-remote", &["-o"])?;
    let name = name.stdout.trim();
    if name.is_empty() {
        return None;
    }
    Some((name.to_owned(), fcitx_open_flag(&open.stdout)))
}

fn fcitx_open_flag(stdout: &str) -> bool {
    stdout.trim() == "1"
}

fn parse_dbus_object_path_engine(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(path) = trimmed.strip_prefix("object path ") {
            let path = path.trim_matches('"');
            if let Some(segment) = path.rsplit('/').next() {
                let engine = segment.trim_end_matches(';');
                if !engine.is_empty() {
                    return Some(engine.replace(';', ""));
                }
            }
        }
    }
    None
}

fn run_command(program: &str, args: &[&str]) -> Option<CommandOutput> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

struct CommandOutput {
    stdout: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_snapshot_collects_module_hints() {
        unsafe {
            std::env::set_var("GTK_IM_MODULE", "ibus");
            std::env::set_var("QT_IM_MODULE", "fcitx5");
            std::env::set_var("XMODIFIERS", "@im=ibus");
        }
        let env = read_env_snapshot();
        assert_eq!(env.gtk_im_module.as_deref(), Some("ibus"));
        assert_eq!(env.qt_im_module.as_deref(), Some("fcitx5"));
        assert_eq!(env.xmodifiers.as_deref(), Some("@im=ibus"));
        unsafe {
            std::env::remove_var("GTK_IM_MODULE");
            std::env::remove_var("QT_IM_MODULE");
            std::env::remove_var("XMODIFIERS");
        }
    }

    #[test]
    fn framework_order_prefers_env_hints() {
        let env = ImeEnvSnapshot {
            gtk_im_module: Some("fcitx5".into()),
            qt_im_module: None,
            xmodifiers: Some("@im=ibus".into()),
        };
        let order = framework_probe_order(&env);
        assert_eq!(order.first().map(String::as_str), Some("fcitx5"));
        assert!(order.contains(&"ibus".to_owned()));
    }

    #[test]
    fn ibus_xkb_engine_is_plain_keyboard() {
        let env = ImeEnvSnapshot::default();
        let observation = ibus_observation("xkb:us".into(), &env, true, "test");
        assert!(!observation.available);
        assert_eq!(observation.label, "IME: off");
    }

    #[test]
    fn ibus_pinyin_engine_is_available_and_native() {
        let env = ImeEnvSnapshot::default();
        let observation = ibus_observation("libpinyin".into(), &env, true, "test");
        assert!(observation.available);
        assert!(observation.native_mode);
        assert!(observation.label.contains("native"));
    }

    #[test]
    fn dbus_global_engine_path_parses() {
        let stdout = r#"method return time=123 sender=:1.23 -> dest=:1.45 serial=2 reply_serial=2
   object path "/org/freedesktop/IBus/Engine/pinyin;libpinyin;"
"#;
        assert_eq!(
            parse_dbus_object_path_engine(stdout).as_deref(),
            Some("libpinyin")
        );
    }

    #[test]
    fn unsupported_carries_alternatives() {
        let result = observe();
        if matches!(result, ImeObserveResult::Ok(_)) {
            return;
        }
        let ImeObserveResult::Unsupported(unsupported) = result else {
            unreachable!();
        };
        assert!(!unsupported.alternatives.is_empty());
        assert_eq!(unsupported.required_mechanism, "linux-session-input-method");
    }
}
