use std::process::{Command, Stdio};

use crate::audio::{
    AudioError, AudioErrorKind, AudioObserveUnsupported, AudioOutputSettings, NativeAudioState,
};

const PULSE_IDENTITY_DOMAIN: &[u8] = b"pulse-pactl\0";
const PIPEWIRE_IDENTITY_DOMAIN: &[u8] = b"pipewire-wpctl\0";
const ALSA_IDENTITY_DOMAIN: &[u8] = b"alsa-amixer\0";

const LINUX_AUDIO_ALTERNATIVES: &[&str] = &[
    "install pulseaudio-utils or pipewire-pulse and expose pactl on PATH",
    "pactl get-default-sink && pactl get-sink-volume @DEFAULT_SINK@ && pactl get-sink-mute @DEFAULT_SINK@",
    "wpctl get-volume @DEFAULT_AUDIO_SINK@ (PipeWire/WirePlumber)",
    "amixer -D pulse sget Master (ALSA via the Pulse plugin)",
];

pub(crate) fn query() -> Result<NativeAudioState, AudioError> {
    let mut probed = Vec::new();
    if let Some(state) = probe_pactl(&mut probed) {
        return Ok(state);
    }
    if let Some(state) = probe_wpctl(&mut probed) {
        return Ok(state);
    }
    if let Some(state) = probe_amixer_pulse(&mut probed) {
        return Ok(state);
    }
    if let Some(state) = probe_amixer_default(&mut probed) {
        return Ok(state);
    }
    Err(unsupported())
}

pub(crate) fn set(_: &NativeAudioState, _: AudioOutputSettings) -> Result<(), AudioError> {
    Err(unsupported())
}

pub(crate) fn unsupported_detail() -> AudioObserveUnsupported {
    let mut probed = Vec::new();
    let _ = probe_pactl(&mut probed);
    let _ = probe_wpctl(&mut probed);
    let _ = probe_amixer_pulse(&mut probed);
    let _ = probe_amixer_default(&mut probed);
    let session_bus_available = session_bus_available();
    AudioObserveUnsupported {
        reason: if session_bus_available {
            "no PulseAudio, PipeWire or ALSA default-output mechanism answered on this host"
                .into()
        } else {
            "the desktop session bus is unavailable and no ALSA default-output mechanism answered"
                .into()
        },
        probed_mechanisms: probed,
        session_bus_available,
        required_mechanism: "linux-default-output".into(),
        alternatives: LINUX_AUDIO_ALTERNATIVES
            .iter()
            .map(|alternative| (*alternative).to_owned())
            .collect(),
    }
}

fn probe_pactl(probed: &mut Vec<String>) -> Option<NativeAudioState> {
    probed.push("pactl".into());
    let sink = run_command("pactl", &["get-default-sink"])?;
    let uid = sink.stdout.trim();
    if uid.is_empty() {
        return None;
    }
    let volume = run_command("pactl", &["get-sink-volume", "@DEFAULT_SINK@"])?;
    let mute = run_command("pactl", &["get-sink-mute", "@DEFAULT_SINK@"])?;
    Some(NativeAudioState {
        device_id: 0,
        provider: "linux-pulse-pactl",
        identity_domain: PULSE_IDENTITY_DOMAIN,
        uid: uid.to_owned(),
        name: friendly_sink_name(uid),
        manufacturer: "PulseAudio".into(),
        volume_scalar: parse_pactl_volume_percent(&volume.stdout)?,
        muted: parse_pactl_mute(&mute.stdout)?,
    })
}

fn probe_wpctl(probed: &mut Vec<String>) -> Option<NativeAudioState> {
    probed.push("wpctl".into());
    let volume = run_command("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?;
    let inspect = run_command("wpctl", &["inspect", "@DEFAULT_AUDIO_SINK@"])?;
    let uid = parse_wpctl_name(&inspect.stdout).unwrap_or_else(|| "default-audio-sink".into());
    let name = friendly_sink_name(&uid);
    Some(NativeAudioState {
        device_id: 0,
        provider: "linux-pipewire-wpctl",
        identity_domain: PIPEWIRE_IDENTITY_DOMAIN,
        uid: uid.clone(),
        name: friendly_sink_name(&uid),
        manufacturer: "PipeWire".into(),
        volume_scalar: parse_wpctl_volume(&volume.stdout)?,
        muted: parse_wpctl_mute(&inspect.stdout),
    })
}

fn probe_amixer_pulse(probed: &mut Vec<String>) -> Option<NativeAudioState> {
    probed.push("amixer-pulse".into());
    probe_amixer(&["-D", "pulse", "sget", "Master"], "pulse")
}

fn probe_amixer_default(probed: &mut Vec<String>) -> Option<NativeAudioState> {
    probed.push("amixer-default".into());
    probe_amixer(&["sget", "Master"], "default")
}

fn probe_amixer(args: &[&str], label: &str) -> Option<NativeAudioState> {
    let output = run_command("amixer", args)?;
    let (volume_scalar, muted) = parse_amixer_master(&output.stdout)?;
    Some(NativeAudioState {
        device_id: 0,
        provider: if label == "pulse" {
            "linux-alsa-amixer-pulse"
        } else {
            "linux-alsa-amixer"
        },
        identity_domain: ALSA_IDENTITY_DOMAIN,
        uid: format!("alsa-master-{label}"),
        name: "Master".into(),
        manufacturer: "ALSA".into(),
        volume_scalar,
        muted,
    })
}

fn parse_pactl_volume_percent(stdout: &str) -> Option<f32> {
    for token in stdout.split_whitespace() {
        if let Some(percent) = token.strip_suffix('%') {
            if let Ok(value) = percent.parse::<f32>() {
                return Some((value / 100.0).clamp(0.0, 1.0));
            }
        }
    }
    None
}

fn parse_pactl_mute(stdout: &str) -> Option<bool> {
    let trimmed = stdout.trim();
    if trimmed.ends_with("yes") {
        return Some(true);
    }
    if trimmed.ends_with("no") {
        return Some(false);
    }
    None
}

fn parse_wpctl_volume(stdout: &str) -> Option<f32> {
    let token = stdout.split_whitespace().next()?;
    let value = token.parse::<f32>().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some(value.clamp(0.0, 1.0))
}

fn parse_wpctl_mute(stdout: &str) -> bool {
    stdout.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("mute:") && (trimmed.contains("true") || trimmed.contains("yes"))
    })
}

fn parse_wpctl_name(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("node.name = ") {
            let value = value.trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn parse_amixer_master(stdout: &str) -> Option<(f32, bool)> {
    let mut muted = false;
    let mut volume_scalar = None;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Front Left:") || trimmed.starts_with("Mono:") {
            volume_scalar = parse_amixer_volume_line(trimmed);
        }
        if trimmed.contains("[off]") {
            muted = true;
        }
        if trimmed.contains("[on]") {
            muted = false;
        }
    }
    volume_scalar.map(|volume| (volume, muted))
}

fn parse_amixer_volume_line(line: &str) -> Option<f32> {
    let start = line.find('[')?;
    let end = line[start + 1..].find(']')? + start + 1;
    let bracket = line[start + 1..end].trim();
    let percent = bracket.strip_suffix('%')?;
    let value = percent.parse::<f32>().ok()?;
    Some((value / 100.0).clamp(0.0, 1.0))
}

fn friendly_sink_name(uid: &str) -> String {
    uid.split('.').next_back().unwrap_or(uid).replace('_', " ")
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

fn unsupported() -> AudioError {
    AudioError::new(
        AudioErrorKind::Unsupported,
        "default-output volume and mute observation are unsupported on this Linux host",
    )
}
