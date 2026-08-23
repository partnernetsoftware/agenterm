//! Handshake validation performed before `vnc-rs` touches the socket.
//!
//! Why this exists: `vnc-rs` 0.5.3 decodes the RFB `SecurityResult` word with
//! `std::mem::transmute::<u32, AuthResult>` over a two-variant enum
//! (`client/auth.rs:102`). Any value other than 0 or 1 is undefined behaviour,
//! and in practice aborts the process with a non-unwinding panic that no
//! `catch_unwind` can contain — a malformed or non-RFB server would take the
//! whole app down with it.
//!
//! So the session dials twice: once here, to read the handshake far enough to
//! prove the server is well formed, and once for real. The cost is one extra
//! TCP round trip at connect time; the benefit is that a bad server yields a
//! typed error instead of killing the process.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;


/// What the caller is willing to authenticate with.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Credentials<'a> {
    /// Only Apple Remote Management uses a username.
    pub(crate) username: Option<&'a str>,
    pub(crate) password: Option<&'a str>,
}

/// What the preflight learned about a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Handshake {
    /// Whether the server offered the `VncAuth` security type.
    pub(crate) supports_vnc_auth: bool,
    /// Whether the server offered the `None` security type.
    pub(crate) supports_none: bool,
    /// Whether the server offered Apple Remote Management (type 30).
    pub(crate) supports_ard: bool,
}

/// Why a server was rejected before the real connection was made.
#[derive(Debug, Clone)]
pub(crate) enum PreflightError {
    /// The greeting was not a `RFB xxx.yyy\n` banner.
    NotRfb,
    /// The server refused the session outright, with its stated reason.
    Rejected(String),
    /// The server offers only security types this client cannot perform.
    ///
    /// Carries the raw type numbers so the message can name the obstacle —
    /// notably type 30, Apple Remote Management, which `vnc-rs` cannot do.
    UnsupportedSecurity(Vec<u8>),
    /// A password was offered but the server does not accept one.
    PasswordNotAccepted,
    /// A username was offered but the server has no auth type that uses one.
    UsernameNotAccepted,
    /// The server needs a username (Apple Remote Management) and got none.
    UsernameRequired,

    /// The server requires a password and none was supplied.
    PasswordRequired,
    Io(String),
}

/// Read a server's greeting and security list, without authenticating.
///
/// This exists to turn "the connection failed" into a specific, actionable
/// message: not an RFB server at all, no auth type in common, or a credential
/// shape the server cannot use. Authentication itself happens once, on the
/// real connection, so a server that rate-limits attempts sees only one.
pub(crate) async fn probe(
    address: &str,
    credentials: Credentials<'_>,
) -> Result<Handshake, PreflightError> {
    let password = credentials.password;
    // Resolution goes through `resolve` so a `.local` name behaves the same
    // here as it does on the real connection.
    let (host, port) = address.rsplit_once(':').unwrap_or((address, "5900"));
    let targets = crate::resolve::resolve(host, port.parse().unwrap_or(5900))
        .map_err(|error| PreflightError::Io(error.to_string()))?;
    let mut stream = TcpStream::connect(&targets[..])
        .await
        .map_err(|error| PreflightError::Io(error.to_string()))?;

    // ProtocolVersion: exactly 12 bytes, "RFB 003.008\n" and friends.
    let mut banner = [0u8; 12];
    stream
        .read_exact(&mut banner)
        .await
        .map_err(|error| PreflightError::Io(error.to_string()))?;
    if !banner.starts_with(b"RFB ") || banner[11] != b'\n' {
        return Err(PreflightError::NotRfb);
    }

    // Echo a version back so the server will send its security list. Claiming
    // 3.8 matches what `vnc-rs` negotiates, so the list seen here is the list
    // the real connection will get.
    let minor = parse_minor(&banner);
    let version: &[u8] = if minor >= 8 { b"RFB 003.008\n" } else { b"RFB 003.003\n" };
    stream
        .write_all(version)
        .await
        .map_err(|error| PreflightError::Io(error.to_string()))?;

    let types = read_security_types(&mut stream, minor).await?;
    if types.is_empty() {
        return Err(PreflightError::Rejected("the server offered no security types".into()));
    }

    let handshake = Handshake {
        supports_vnc_auth: types.contains(&2),
        supports_none: types.contains(&1),
        supports_ard: types.contains(&30),
    };
    if !handshake.supports_vnc_auth && !handshake.supports_none && !handshake.supports_ard {
        return Err(PreflightError::UnsupportedSecurity(types));
    }
    // Decide credential mismatches from the offered list alone; rehearsing an
    // exchange the server cannot perform would only yield a murkier error.
    // A username is meaningful only to ARD, so it selects that path.
    let use_ard = handshake.supports_ard && credentials.username.is_some();
    if credentials.username.is_some() && !handshake.supports_ard {
        return Err(PreflightError::UsernameNotAccepted);
    }
    if !use_ard {
        if password.is_some() && !handshake.supports_vnc_auth {
            return Err(PreflightError::PasswordNotAccepted);
        }
        if password.is_none() && !handshake.supports_none {
            // An ARD-only server needs a username as well as a password.
            return Err(if handshake.supports_ard {
                PreflightError::UsernameRequired
            } else {
                PreflightError::PasswordRequired
            });
        }
    }

    Ok(handshake)
}

/// The effective minor version of a `RFB 003.008\n` banner.
///
/// Apple's Screen Sharing announces `RFB 003.889`, which is not a real RFB
/// minor version: it means 3.8 plus Apple's extensions. Parsing it as a `u8`
/// overflows, and treating the failure as 3.3 sends the client down the branch
/// where the *server* dictates the security type, so it never selects one and
/// the handshake stalls. Anything at or above 8 is therefore clamped to 8.
fn parse_minor(banner: &[u8; 12]) -> u8 {
    let Some(text) = std::str::from_utf8(&banner[8..11]).ok() else {
        return 3;
    };
    match text.trim().parse::<u16>() {
        Ok(minor) if minor >= 8 => 8,
        Ok(minor) => minor as u8,
        Err(_) => 3,
    }
}

/// Read the security-type list, honouring the 3.3 and 3.7+ encodings.
async fn read_security_types(
    stream: &mut TcpStream,
    minor: u8,
) -> Result<Vec<u8>, PreflightError> {
    if minor < 7 {
        // RFB 3.3: the server dictates one type as a u32.
        let value = stream
            .read_u32()
            .await
            .map_err(|error| PreflightError::Io(error.to_string()))?;
        if value == 0 {
            return Err(PreflightError::Rejected(read_reason(stream).await));
        }
        // A u32 that does not fit a security type byte is not RFB.
        return u8::try_from(value).map(|byte| vec![byte]).map_err(|_| PreflightError::NotRfb);
    }

    let count = stream
        .read_u8()
        .await
        .map_err(|error| PreflightError::Io(error.to_string()))?;
    if count == 0 {
        // A zero count is the server's way of refusing, followed by a reason.
        return Err(PreflightError::Rejected(read_reason(stream).await));
    }
    let mut types = vec![0u8; count as usize];
    stream
        .read_exact(&mut types)
        .await
        .map_err(|error| PreflightError::Io(error.to_string()))?;
    Ok(types)
}

/// Read the length-prefixed failure string that follows a refusal.
async fn read_reason(stream: &mut TcpStream) -> String {
    let Ok(length) = stream.read_u32().await else {
        return "the server closed the connection".into();
    };
    // Cap the read so a bogus length cannot make the client allocate wildly.
    let capped = length.min(4096) as usize;
    let mut reason = vec![0u8; capped];
    if stream.read_exact(&mut reason).await.is_err() {
        return "the server closed the connection".into();
    }
    String::from_utf8_lossy(&reason).trim().to_string()
}

/// Human-readable name for a security type number, for error messages.
pub(crate) fn security_type_name(value: u8) -> &'static str {
    match value {
        0 => "Invalid",
        1 => "None",
        2 => "VNC Auth",
        5 => "RA2",
        6 => "RA2ne",
        16 => "Tight",
        17 => "Ultra",
        18 => "TLS",
        19 => "VeNCrypt",
        20 => "GTK-VNC SASL",
        21 => "MD5 hash",
        22 => "Colin Dean xvp",
        30 => "Apple Remote Management",
        35 => "Apple Remote Management (variant)",
        _ => "unknown",
    }
}
