//! VNC (RFB) client sessions as a reusable mechanism.
//!
//! This crate owns protocol and pixels only. It knows nothing about windows,
//! webviews, or any product shell: a caller supplies a host and credentials
//! and receives composited RGBA frames plus a handle for input. The Tauri
//! product surface lives in `agenterm-vnc-app`.
//!
//! ```no_run
//! # async fn demo() -> Result<(), agenterm_vnc::VncError> {
//! use agenterm_vnc::{ConnectOptions, MouseButtons};
//!
//! let (session, mut frames) =
//!     agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", 5900, Some("secret".into()))).await?;
//! while let Some(frame) = frames.recv().await {
//!     // frame.rgba is `frame.width * frame.height * 4` bytes.
//!     let _ = frame.rgba.len();
//!     session.send_mouse(10, 10, MouseButtons::LEFT)?;
//! }
//! # Ok(()) }
//! ```

mod ard;
mod des;
mod framebuffer;
mod preflight;
mod resolve;
mod session;

pub use framebuffer::{BYTES_PER_PIXEL, Framebuffer, Rect};
pub use session::{ColourDepth, ConnectOptions, SessionHandle, connect};

/// One changed rectangle within a [`Frame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    /// Where this tile belongs in the framebuffer.
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Byte offset of this tile's pixels within the frame's `rgba`.
    pub offset: usize,
}

/// One update to the screen, ready for a canvas or texture upload.
///
/// Only the region that changed is carried. A full screen is a legitimate
/// value of that region, but the common case -- a cursor moving, a character
/// appearing in a terminal -- is a few thousand bytes rather than the whole
/// surface, which is the difference between a responsive session and one that
/// spends all its time copying pixels nobody looked at.
#[derive(Debug, Clone)]
pub struct Frame {
    /// The full framebuffer size, so a consumer can size its canvas.
    pub width: u16,
    pub height: u16,
    /// The rectangles this frame carries, in draw order.
    ///
    /// A server tiles one update into many small rects -- macOS sends 64x64 --
    /// and they arrive together. Carrying them in a single frame keeps one
    /// update to one handoff: sending a frame each turned a repaint into
    /// thousands of round trips, which no consumer could drain in time.
    pub tiles: Vec<Tile>,
    /// The tiles' pixels, concatenated in `tiles` order. Each tile's bytes
    /// start at its `offset` and run `width * height * 4`.
    pub rgba: Vec<u8>,
}

/// The RFB pointer button mask.
///
/// RFB packs buttons into one byte where bit N is button N+1; wheel scrolling
/// is reported as momentary presses of buttons 4 through 7 rather than as a
/// separate axis, which is why the scroll directions are members here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseButtons(u8);

impl MouseButtons {
    pub const NONE: Self = Self(0);
    pub const LEFT: Self = Self(1 << 0);
    pub const MIDDLE: Self = Self(1 << 1);
    pub const RIGHT: Self = Self(1 << 2);
    pub const SCROLL_UP: Self = Self(1 << 3);
    pub const SCROLL_DOWN: Self = Self(1 << 4);
    pub const SCROLL_LEFT: Self = Self(1 << 5);
    pub const SCROLL_RIGHT: Self = Self(1 << 6);

    /// Build a mask from a raw RFB button byte.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// The raw RFB button byte.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Combine two masks.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit in `other` is set here.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Everything that can go wrong opening or driving a session.
#[derive(Debug)]
pub enum VncError {
    /// The TCP connection itself failed; `address` is host:port as dialed.
    Connect { address: String, source: std::io::Error },
    /// The RFB handshake, authentication, or stream failed.
    Protocol(vnc::VncError),
    /// Something answered on that port, but it does not speak RFB.
    NotRfbServer { address: String },
    /// The handshake could not be read to completion.
    Handshake(String),
    /// The server refused the session and gave this reason.
    Rejected(String),
    /// The server offers no security type this client can perform.
    ///
    UnsupportedSecurity(Vec<u8>),
    /// A password was supplied, but the server does not accept one.
    PasswordNotAccepted,
    /// The server requires a password and none was supplied.
    PasswordRequired,
    /// The server rejected the supplied credentials.
    WrongPassword,
    /// A username was supplied but no offered auth type uses one.
    UsernameNotAccepted,
    /// The server wants Apple Remote Management, which needs a username.
    UsernameRequired,
    /// A username or password exceeds the 63 bytes the ARD block allows.
    CredentialTooLong,
    /// The session task is gone, so the command could not be delivered.
    Disconnected,
}

impl std::fmt::Display for VncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect { address, source } => {
                write!(f, "could not reach VNC server at {address}: {source}")
            }
            Self::Protocol(source) => write!(f, "VNC protocol error: {source}"),
            Self::NotRfbServer { address } => {
                write!(f, "{address} answered, but it is not a VNC (RFB) server")
            }
            Self::Handshake(reason) => write!(f, "the VNC handshake failed: {reason}"),
            Self::Rejected(reason) => write!(f, "the VNC server refused the connection: {reason}"),
            Self::UnsupportedSecurity(types) => {
                let offered = types
                    .iter()
                    .map(|value| format!("{} ({value})", preflight::security_type_name(*value)))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "the server offers no supported authentication; it accepts only: {offered}. \
                     This client supports None, VNC Auth, and \
                     Apple Remote Management."
                )
            }
            Self::PasswordNotAccepted => {
                f.write_str("this server does not use a password; leave the password blank")
            }
            Self::PasswordRequired => f.write_str("this server requires a password"),
            Self::WrongPassword => f.write_str("the server rejected those credentials"),
            Self::UsernameNotAccepted => {
                f.write_str("this server does not use a username; leave it blank")
            }
            Self::UsernameRequired => f.write_str(
                "this server uses Apple Remote Management, which needs a username and password",
            ),
            Self::CredentialTooLong => {
                f.write_str("the username and password must each be 63 bytes or fewer")
            }
            Self::Disconnected => f.write_str("the VNC session is no longer connected"),
        }
    }
}

impl std::error::Error for VncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connect { source, .. } => Some(source),
            Self::Protocol(source) => Some(source),
            _ => None,
        }
    }
}

/// Internals exposed only so the integration tests can assert on them.
///
/// This is not a supported API: the DES here exists to serve [`preflight`],
/// and conformance vectors are the only reason it is reachable at all.
#[doc(hidden)]
pub mod testing {
    pub use crate::ard::{ArdError, ArdResponse, respond};
    pub use crate::des::{encrypt_challenge, key_from_password};
}
