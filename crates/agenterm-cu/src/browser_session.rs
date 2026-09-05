//! Pure contracts for an ACU-owned Chromium session. The resident owner and
//! durable registry consume these helpers; no user profile is opened or copied
//! here.

use std::path::{Path, PathBuf};

pub const ACTIVE_PORT_FILE: &str = "DevToolsActivePort";
pub const OWNER_MARKER_FILE: &str = ".agenterm-cu-browser-session";
pub const ACTIVE_PORT_MAX_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevToolsEndpoint {
    pub port: u16,
    pub browser_path: String,
}

impl DevToolsEndpoint {
    pub fn websocket_url(&self) -> String {
        format!("ws://127.0.0.1:{}{}", self.port, self.browser_path)
    }
}

pub fn validate_session_name(name: &str) -> Result<(), &'static str> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 32
        || !bytes[0].is_ascii_lowercase()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err("browser_session_name_invalid");
    }
    Ok(())
}

pub fn parse_active_port(bytes: &[u8]) -> Result<DevToolsEndpoint, &'static str> {
    if bytes.is_empty() || bytes.len() > ACTIVE_PORT_MAX_BYTES {
        return Err("browser_debug_endpoint_invalid");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "browser_debug_endpoint_invalid")?;
    let mut lines = text.lines();
    let port = lines
        .next()
        .and_then(|line| line.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .ok_or("browser_debug_endpoint_invalid")?;
    let browser_path = lines
        .next()
        .filter(|line| line.starts_with("/devtools/browser/"))
        .filter(|line| {
            line.len() <= 1024
                && line
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() && byte != b'#')
        })
        .ok_or("browser_debug_endpoint_invalid")?;
    if lines.any(|line| !line.is_empty()) {
        return Err("browser_debug_endpoint_invalid");
    }
    Ok(DevToolsEndpoint {
        port,
        browser_path: browser_path.to_owned(),
    })
}

pub fn active_port_path(profile_root: &Path) -> PathBuf {
    profile_root.join(ACTIVE_PORT_FILE)
}

pub fn owned_launch_args(profile_root: &Path) -> Vec<String> {
    vec![
        format!("--user-data-dir={}", profile_root.display()),
        "--remote-debugging-port=0".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--no-startup-window".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_name_is_a_closed_portable_component() {
        for valid in ["a", "browser-1", "work"] {
            validate_session_name(valid).unwrap();
        }
        for invalid in [
            "",
            "A",
            "1work",
            "work_space",
            "work/space",
            &"x".repeat(33),
        ] {
            assert_eq!(
                validate_session_name(invalid).unwrap_err(),
                "browser_session_name_invalid"
            );
        }
    }

    #[test]
    fn parses_chromium_active_port_without_guessing() {
        let endpoint = parse_active_port(b"43127\n/devtools/browser/abc-123\n").unwrap();
        assert_eq!(endpoint.port, 43127);
        assert_eq!(
            endpoint.websocket_url(),
            "ws://127.0.0.1:43127/devtools/browser/abc-123"
        );
    }

    #[test]
    fn rejects_bad_or_ambiguous_active_port_records() {
        for bytes in [
            b"0\n/devtools/browser/id\n".as_slice(),
            b"9222\n/devtools/page/id\n".as_slice(),
            b"9222\n/devtools/browser/id#fragment\n".as_slice(),
            b"9222\n/devtools/browser/id\nextra\n".as_slice(),
            b"not-a-port\n/devtools/browser/id\n".as_slice(),
        ] {
            assert_eq!(
                parse_active_port(bytes).unwrap_err(),
                "browser_debug_endpoint_invalid"
            );
        }
    }

    #[test]
    fn owned_launch_uses_random_port_file_contract() {
        let root = Path::new("browser-session-fixture");
        let args = owned_launch_args(root);
        assert!(args.contains(&"--remote-debugging-port=0".to_owned()));
        assert!(args.contains(&"--no-startup-window".to_owned()));
        assert!(
            args.iter()
                .any(|arg| arg == "--user-data-dir=browser-session-fixture")
        );
        assert_eq!(active_port_path(root), root.join(ACTIVE_PORT_FILE));
    }
}
