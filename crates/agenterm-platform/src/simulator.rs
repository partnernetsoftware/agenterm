//! Bounded CoreSimulator inventory and exact-device boot operations.

use std::fmt;
use std::time::Duration;

mod selected;

pub const MAX_SIMULATOR_DEVICES: usize = 200;
const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_STRING_BYTES: usize = 4_096;
const MAX_VISITED_DEVICES: usize = 5_000;
const MAX_RUNTIME_BYTES: usize = 512;
const MAX_DEVICE_TYPE_BYTES: usize = 512;
const MAX_STATE_BYTES: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorDevice {
    pub udid: String,
    pub runtime: String,
    pub device_type: String,
    pub state: String,
}

impl SimulatorDevice {
    #[must_use]
    pub fn is_booted(&self) -> bool {
        self.state == "Booted"
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorDeviceList {
    pub devices: Vec<SimulatorDevice>,
    pub visited: usize,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorBootReceipt {
    pub udid: String,
    pub before_state: String,
    pub after_state: String,
    pub already_booted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SimulatorErrorKind {
    Unsupported,
    InvalidLimit,
    InvalidUdid,
    InvalidTimeout,
    Unavailable,
    NotFound,
    Timeout,
    OutputLimit,
    InvalidJson,
    Changed,
    Io,
}

impl SimulatorErrorKind {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "simulator_unsupported",
            Self::InvalidLimit => "simulator_limit_invalid",
            Self::InvalidUdid => "simulator_udid_invalid",
            Self::InvalidTimeout => "simulator_timeout_invalid",
            Self::Unavailable => "simulator_unavailable",
            Self::NotFound => "simulator_not_found",
            Self::Timeout => "simulator_timeout",
            Self::OutputLimit => "simulator_output_limit",
            Self::InvalidJson => "simulator_invalid_json",
            Self::Changed => "simulator_device_changed",
            Self::Io => "simulator_io_failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorError {
    pub kind: SimulatorErrorKind,
    message: String,
}

impl SimulatorError {
    pub(crate) fn new(kind: SimulatorErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.kind.code()
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SimulatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for SimulatorError {}

/// List at most `max` CoreSimulator devices while reporting the full bounded
/// number visited by the native inventory.
pub fn list_devices(max: usize) -> Result<SimulatorDeviceList, SimulatorError> {
    validate_limit(max)?;
    selected::list_devices(max)
}

/// Boot exactly one CoreSimulator UDID and verify it reaches `Booted` without
/// launching or activating Simulator.app.
pub fn boot_exact(udid: &str, timeout: Duration) -> Result<SimulatorBootReceipt, SimulatorError> {
    validate_udid(udid)?;
    if timeout.is_zero() || timeout > Duration::from_secs(600) {
        return Err(SimulatorError::new(
            SimulatorErrorKind::InvalidTimeout,
            "timeout must be within 1ns..=600s",
        ));
    }
    selected::boot_exact(udid, timeout)
}

fn validate_limit(max: usize) -> Result<(), SimulatorError> {
    if !(1..=MAX_SIMULATOR_DEVICES).contains(&max) {
        return Err(SimulatorError::new(
            SimulatorErrorKind::InvalidLimit,
            "device limit must be within 1..=200",
        ));
    }
    Ok(())
}

fn validate_udid(udid: &str) -> Result<(), SimulatorError> {
    let bytes = udid.as_bytes();
    let valid = bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if !valid {
        return Err(SimulatorError::new(
            SimulatorErrorKind::InvalidUdid,
            "UDID must be a 36-byte hyphenated hexadecimal identifier",
        ));
    }
    Ok(())
}

pub(crate) fn parse_device_list(
    bytes: &[u8],
    max: usize,
) -> Result<SimulatorDeviceList, SimulatorError> {
    validate_limit(max)?;
    let mut parser = JsonParser::new(bytes);
    let result = parser.parse_root(max)?;
    parser.whitespace();
    if parser.position != bytes.len() {
        return Err(invalid_json("trailing bytes after root object"));
    }
    Ok(result)
}

fn invalid_json(message: &'static str) -> SimulatorError {
    SimulatorError::new(SimulatorErrorKind::InvalidJson, message)
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn parse_root(&mut self, max: usize) -> Result<SimulatorDeviceList, SimulatorError> {
        self.whitespace();
        self.expect(b'{')?;
        let mut devices = None;
        self.object_fields(|parser, key| {
            if key == "devices" {
                if devices.is_some() {
                    return Err(invalid_json("duplicate devices object"));
                }
                devices = Some(parser.parse_runtime_map(max)?);
                Ok(())
            } else {
                parser.skip_value(1)
            }
        })?;
        devices.ok_or_else(|| invalid_json("missing devices object"))
    }

    fn parse_runtime_map(&mut self, max: usize) -> Result<SimulatorDeviceList, SimulatorError> {
        self.whitespace();
        self.expect(b'{')?;
        let mut rows = Vec::new();
        let mut visited = 0usize;
        self.object_fields(|parser, runtime| {
            if !bounded_field(&runtime, MAX_RUNTIME_BYTES) {
                return Err(invalid_json("runtime identifier is invalid"));
            }
            parser.whitespace();
            parser.expect(b'[')?;
            parser.array_values(|parser| {
                visited = visited
                    .checked_add(1)
                    .ok_or_else(|| invalid_json("device count overflow"))?;
                if visited > MAX_VISITED_DEVICES {
                    return Err(invalid_json("device inventory exceeds native scan bound"));
                }
                let row = parser.parse_device(&runtime)?;
                if rows.len() < max {
                    rows.push(row);
                }
                Ok(())
            })
        })?;
        Ok(SimulatorDeviceList {
            truncated: visited > rows.len(),
            devices: rows,
            visited,
        })
    }

    fn parse_device(&mut self, runtime: &str) -> Result<SimulatorDevice, SimulatorError> {
        self.whitespace();
        self.expect(b'{')?;
        let mut udid = None;
        let mut device_type = None;
        let mut state = None;
        self.object_fields(|parser, key| match key.as_str() {
            "udid" => parse_unique_string(parser, &mut udid, "duplicate device UDID"),
            "deviceTypeIdentifier" => {
                parse_unique_string(parser, &mut device_type, "duplicate device type")
            }
            "state" => parse_unique_string(parser, &mut state, "duplicate device state"),
            _ => parser.skip_value(2),
        })?;
        let udid = udid.ok_or_else(|| invalid_json("device is missing UDID"))?;
        validate_udid(&udid).map_err(|_| invalid_json("device UDID has invalid grammar"))?;
        let device_type = device_type.ok_or_else(|| invalid_json("device is missing type"))?;
        let state = state.ok_or_else(|| invalid_json("device is missing state"))?;
        if !bounded_field(&device_type, MAX_DEVICE_TYPE_BYTES)
            || !bounded_field(&state, MAX_STATE_BYTES)
        {
            return Err(invalid_json("device type or state is invalid"));
        }
        Ok(SimulatorDevice {
            udid,
            runtime: runtime.to_owned(),
            device_type,
            state,
        })
    }

    fn object_fields<F>(&mut self, mut field: F) -> Result<(), SimulatorError>
    where
        F: FnMut(&mut Self, String) -> Result<(), SimulatorError>,
    {
        self.whitespace();
        if self.take(b'}') {
            return Ok(());
        }
        loop {
            self.whitespace();
            let key = self.string()?;
            self.whitespace();
            self.expect(b':')?;
            field(self, key)?;
            self.whitespace();
            if self.take(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn array_values<F>(&mut self, mut value: F) -> Result<(), SimulatorError>
    where
        F: FnMut(&mut Self) -> Result<(), SimulatorError>,
    {
        self.whitespace();
        if self.take(b']') {
            return Ok(());
        }
        loop {
            value(self)?;
            self.whitespace();
            if self.take(b']') {
                return Ok(());
            }
            self.expect(b',')?;
        }
    }

    fn skip_value(&mut self, depth: usize) -> Result<(), SimulatorError> {
        if depth > MAX_JSON_DEPTH {
            return Err(invalid_json("JSON nesting exceeds limit"));
        }
        self.whitespace();
        match self.peek() {
            Some(b'"') => self.string().map(|_| ()),
            Some(b'{') => {
                self.position += 1;
                self.object_fields(|parser, _| parser.skip_value(depth + 1))
            }
            Some(b'[') => {
                self.position += 1;
                self.array_values(|parser| parser.skip_value(depth + 1))
            }
            Some(b't') => self.literal(b"true"),
            Some(b'f') => self.literal(b"false"),
            Some(b'n') => self.literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(invalid_json("invalid JSON value")),
        }
    }

    fn string(&mut self) -> Result<String, SimulatorError> {
        self.expect(b'"')?;
        let mut output = String::new();
        loop {
            let byte = self
                .next()
                .ok_or_else(|| invalid_json("unterminated string"))?;
            match byte {
                b'"' => return Ok(output),
                b'\\' => self.escape(&mut output)?,
                0..=0x1f => return Err(invalid_json("control byte in string")),
                _ => {
                    let start = self.position - 1;
                    while self
                        .peek()
                        .is_some_and(|next| next != b'"' && next != b'\\')
                    {
                        if self.peek().is_some_and(|next| next <= 0x1f) {
                            return Err(invalid_json("control byte in string"));
                        }
                        self.position += 1;
                    }
                    let text = std::str::from_utf8(&self.bytes[start..self.position])
                        .map_err(|_| invalid_json("string is not UTF-8"))?;
                    output.push_str(text);
                }
            }
            if output.len() > MAX_JSON_STRING_BYTES {
                return Err(invalid_json("JSON string exceeds limit"));
            }
        }
    }

    fn escape(&mut self, output: &mut String) -> Result<(), SimulatorError> {
        match self
            .next()
            .ok_or_else(|| invalid_json("truncated escape"))?
        {
            b'"' => output.push('"'),
            b'\\' => output.push('\\'),
            b'/' => output.push('/'),
            b'b' => output.push('\u{0008}'),
            b'f' => output.push('\u{000c}'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' => {
                let first = self.hex_quad()?;
                let scalar = if (0xd800..=0xdbff).contains(&first) {
                    if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                        return Err(invalid_json("unpaired high surrogate"));
                    }
                    let second = self.hex_quad()?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(invalid_json("invalid low surrogate"));
                    }
                    0x1_0000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
                } else if (0xdc00..=0xdfff).contains(&first) {
                    return Err(invalid_json("unpaired low surrogate"));
                } else {
                    u32::from(first)
                };
                output.push(char::from_u32(scalar).ok_or_else(|| invalid_json("invalid escape"))?);
            }
            _ => return Err(invalid_json("unknown string escape")),
        }
        Ok(())
    }

    fn hex_quad(&mut self) -> Result<u16, SimulatorError> {
        let mut value = 0u16;
        for _ in 0..4 {
            let digit = self.next().and_then(|byte| (byte as char).to_digit(16));
            value = value
                .checked_mul(16)
                .and_then(|value| digit.and_then(|digit| value.checked_add(digit as u16)))
                .ok_or_else(|| invalid_json("invalid unicode escape"))?;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<(), SimulatorError> {
        let start = self.position;
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.position += 1;
        }
        let value = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| invalid_json("number is not UTF-8"))?;
        if valid_json_number(value) {
            Ok(())
        } else {
            Err(invalid_json("invalid JSON number"))
        }
    }

    fn literal(&mut self, expected: &[u8]) -> Result<(), SimulatorError> {
        if self
            .bytes
            .get(self.position..self.position + expected.len())
            == Some(expected)
        {
            self.position += expected.len();
            Ok(())
        } else {
            Err(invalid_json("invalid JSON literal"))
        }
    }

    fn whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), SimulatorError> {
        if self.take(expected) {
            Ok(())
        } else {
            Err(invalid_json("unexpected JSON token"))
        }
    }

    fn take(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.position += 1;
        Some(byte)
    }
}

fn parse_unique_string(
    parser: &mut JsonParser<'_>,
    slot: &mut Option<String>,
    duplicate_message: &'static str,
) -> Result<(), SimulatorError> {
    if slot.is_some() {
        return Err(invalid_json(duplicate_message));
    }
    parser.whitespace();
    *slot = Some(parser.string()?);
    Ok(())
}

fn bounded_field(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn valid_json_number(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut position = usize::from(bytes.first() == Some(&b'-'));
    if position >= bytes.len() {
        return false;
    }
    if bytes[position] == b'0' {
        position += 1;
    } else if matches!(bytes[position], b'1'..=b'9') {
        position += 1;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
    } else {
        return false;
    }
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        let start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if position == start {
            return false;
        }
    }
    if matches!(bytes.get(position), Some(b'e' | b'E')) {
        position += 1;
        if matches!(bytes.get(position), Some(b'+' | b'-')) {
            position += 1;
        }
        let start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if position == start {
            return false;
        }
    }
    position == bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const UDID_1: &str = "12345678-1234-1234-1234-123456789ABC";
    const UDID_2: &str = "ABCDEF12-3456-7890-ABCD-EF1234567890";

    fn fixture() -> Vec<u8> {
        format!(
            r#"{{"devices":{{"com.apple.CoreSimulator.SimRuntime.iOS-18-0":[{{"udid":"{UDID_1}","deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-16","state":"Shutdown","name":"Phone"}},{{"udid":"{UDID_2}","deviceTypeIdentifier":"com.apple.CoreSimulator.SimDeviceType.iPad","state":"Booted","metadata":{{"n":1}}}}]}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn parses_bounded_simctl_fixture() {
        let list = parse_device_list(&fixture(), 1).unwrap();
        assert_eq!(list.visited, 2);
        assert!(list.truncated);
        assert_eq!(list.devices.len(), 1);
        assert_eq!(list.devices[0].udid, UDID_1);
        assert_eq!(
            list.devices[0].runtime,
            "com.apple.CoreSimulator.SimRuntime.iOS-18-0"
        );
        assert_eq!(
            list.devices[0].device_type,
            "com.apple.CoreSimulator.SimDeviceType.iPhone-16"
        );
        assert_eq!(list.devices[0].state, "Shutdown");
    }

    #[test]
    fn rejects_malformed_or_ambiguous_devices() {
        for fixture in [
            br#"{"devices":[]}"#.as_slice(),
            br#"{"devices":{"runtime":[{"udid":"bad","deviceTypeIdentifier":"type","state":"Booted"}]}}"#,
            br#"{"devices":{"runtime":[{"udid":"12345678-1234-1234-1234-123456789ABC","udid":"12345678-1234-1234-1234-123456789ABC","deviceTypeIdentifier":"type","state":"Booted"}]}}"#,
            br#"{"devices":{"runtime":[{"udid":"12345678-1234-1234-1234-123456789ABC","deviceTypeIdentifier":"type"}]}}"#,
            br#"{"devices":{}} trailing"#,
        ] {
            assert_eq!(
                parse_device_list(fixture, 10).unwrap_err().kind,
                SimulatorErrorKind::InvalidJson
            );
        }
    }

    #[test]
    fn validates_limits_udids_and_timeouts_before_platform_dispatch() {
        assert_eq!(
            list_devices(0).unwrap_err().kind,
            SimulatorErrorKind::InvalidLimit
        );
        assert_eq!(
            boot_exact("not-a-udid", Duration::from_secs(1))
                .unwrap_err()
                .kind,
            SimulatorErrorKind::InvalidUdid
        );
        assert_eq!(
            boot_exact(UDID_1, Duration::ZERO).unwrap_err().kind,
            SimulatorErrorKind::InvalidTimeout
        );
    }

    #[test]
    fn json_string_parser_handles_surrogate_pairs_and_rejects_broken_ones() {
        let mut parser = JsonParser::new(br#""\uD83D\uDE80""#);
        assert_eq!(parser.string().unwrap(), "🚀");
        let mut parser = JsonParser::new(br#""\uD83Dx""#);
        assert_eq!(
            parser.string().unwrap_err().kind,
            SimulatorErrorKind::InvalidJson
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_is_typed_unsupported() {
        assert_eq!(
            list_devices(1).unwrap_err().kind,
            SimulatorErrorKind::Unsupported
        );
        assert_eq!(
            boot_exact(UDID_1, Duration::from_secs(1)).unwrap_err().kind,
            SimulatorErrorKind::Unsupported
        );
    }

    #[cfg(all(target_os = "macos", feature = "simulator"))]
    #[test]
    #[ignore = "read-only local CoreSimulator inventory probe"]
    fn live_list_is_bounded() {
        let list = list_devices(3).expect("read local CoreSimulator inventory");
        assert!(list.devices.len() <= 3);
        assert!(list.visited >= list.devices.len());
        assert_eq!(list.truncated, list.visited > list.devices.len());
    }
}
