//! Linux X11 keyboard layout observation through `_XKB_RULES_NAMES`.

use x11rb::{
    connection::Connection,
    protocol::xproto::{AtomEnum, ConnectionExt as _},
    rust_connection::RustConnection,
};

use crate::keyboard_layout::{
    KeyboardLayoutObservation, KeyboardLayoutObserveResult, KeyboardLayoutObserveUnsupported,
};

const LINUX_KEYBOARD_LAYOUT_ALTERNATIVES: &[&str] = &[
    "export DISPLAY to the active X11 session before calling keyboard-layout",
    "setxkbmap -query (verify the X server publishes rules/model/layout)",
    "xprop -root _XKB_RULES_NAMES (verify the root window exposes XKB rule names)",
];

pub(crate) fn observe() -> KeyboardLayoutObserveResult {
    if std::env::var_os("DISPLAY").is_none() {
        return KeyboardLayoutObserveResult::Unsupported(unsupported(
            "keyboard layout observation requires a graphical desktop session",
            "x11-display",
        ));
    }

    match read_xkb_rules_names() {
        Ok(observation) => KeyboardLayoutObserveResult::Ok(observation),
        Err(reason) => KeyboardLayoutObserveResult::Unsupported(unsupported(
            reason,
            "x11-xkb-rules-names",
        )),
    }
}

fn unsupported(
    reason: impl Into<String>,
    required_mechanism: impl Into<String>,
) -> KeyboardLayoutObserveUnsupported {
    KeyboardLayoutObserveUnsupported {
        reason: reason.into(),
        required_mechanism: required_mechanism.into(),
        alternatives: LINUX_KEYBOARD_LAYOUT_ALTERNATIVES
            .iter()
            .map(|alternative| (*alternative).to_owned())
            .collect(),
    }
}

fn read_xkb_rules_names() -> Result<KeyboardLayoutObservation, String> {
    let (connection, screen_index) =
        x11rb::connect(None).map_err(|error| format!("X11 display could not be opened: {error}"))?;
    let root = connection
        .setup()
        .roots
        .get(screen_index)
        .ok_or_else(|| "configured X11 screen does not exist".to_owned())?
        .root;
    let atom = intern_atom(&connection, "_XKB_RULES_NAMES")?;
    let reply = connection
        .get_property(false, root, atom, AtomEnum::STRING, 0, 16)
        .map_err(|error| format!("XKB rules property request could not be sent: {error}"))?
        .reply()
        .map_err(|error| format!("XKB rules property request failed: {error}"))?;
    let (rules, model, layout, variant) = parse_xkb_rules_names(&reply.value)?;
    if layout.is_empty() {
        return Err("the X server published an empty keyboard layout".into());
    }
    let id = layout_id(&layout, &variant);
    let name = layout_name(&layout, &variant);
    Ok(KeyboardLayoutObservation {
        provider: "x11-xkb-rules-names".into(),
        rules,
        model,
        layout,
        variant,
        id,
        name,
    })
}

fn intern_atom(connection: &RustConnection, name: &str) -> Result<u32, String> {
    connection
        .intern_atom(false, name.as_bytes())
        .map_err(|error| format!("X11 atom {name} could not be interned: {error}"))?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|error| format!("X11 atom {name} reply failed: {error}"))
}

fn parse_xkb_rules_names(bytes: &[u8]) -> Result<(String, String, String, String), String> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == 0 {
            parts.push(String::from_utf8_lossy(&bytes[start..index]).into_owned());
            start = index + 1;
        }
    }
    if start < bytes.len() {
        parts.push(String::from_utf8_lossy(&bytes[start..]).into_owned());
    }
    if parts.len() < 3 {
        return Err("the X server published an incomplete _XKB_RULES_NAMES property".into());
    }
    let rules = parts[0].clone();
    let model = parts[1].clone();
    let layout = parts[2].clone();
    let variant = parts.get(3).cloned().unwrap_or_default();
    Ok((rules, model, layout, variant))
}

fn layout_id(layout: &str, variant: &str) -> String {
    if variant.is_empty() {
        layout.to_owned()
    } else {
        format!("{layout}+{variant}")
    }
}

fn layout_name(layout: &str, variant: &str) -> String {
    if variant.is_empty() {
        layout.to_owned()
    } else {
        format!("{layout} ({variant})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xkb_rules_names_parser_reads_four_fields() {
        let bytes = b"evdev\0pc105\0us\0\0";
        let (rules, model, layout, variant) =
            parse_xkb_rules_names(bytes).expect("rules names should parse");
        assert_eq!(rules, "evdev");
        assert_eq!(model, "pc105");
        assert_eq!(layout, "us");
        assert_eq!(variant, "");
        assert_eq!(layout_id(&layout, &variant), "us");
        assert_eq!(layout_name(&layout, &variant), "us");
    }

    #[test]
    fn layout_id_and_name_include_variant_when_present() {
        assert_eq!(layout_id("de", "nodeadkeys"), "de+nodeadkeys");
        assert_eq!(layout_name("de", "nodeadkeys"), "de (nodeadkeys)");
    }
}
