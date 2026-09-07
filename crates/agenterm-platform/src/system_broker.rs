//! Typed boundary for the Linux system-activated privilege broker.
//!
//! Production callers cannot choose an endpoint or an activation descriptor.
//! The fixed socket and kernel-derived peer identity are part of the authority
//! contract, not configuration.

use std::{
    fmt,
    io::{self, Read, Write},
    time::Duration,
};

#[path = "adapters/linux/system_broker.rs"]
mod native;

/// The sole production endpoint accepted by the system broker carrier.
pub const SYSTEM_BROKER_SOCKET: &str = "/run/agenterm/cu-privilege.sock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SystemBrokerErrorCode {
    InvalidActivation,
    UnsafeEndpoint,
    UnsafePeer,
    PeerIdentityUnavailable,
    StalePeer,
    ConnectTimeout,
    AcceptTimeout,
    Io,
}

#[derive(Debug)]
pub struct SystemBrokerError {
    code: SystemBrokerErrorCode,
    message: String,
    source: Option<io::Error>,
}

impl SystemBrokerError {
    pub(crate) fn new(code: SystemBrokerErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn io(
        code: SystemBrokerErrorCode,
        message: impl Into<String>,
        source: io::Error,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            source: Some(source),
        }
    }

    #[must_use]
    pub const fn code(&self) -> SystemBrokerErrorCode {
        self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SystemBrokerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SystemBrokerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

pub type SystemBrokerResult<T> = Result<T, SystemBrokerError>;

/// Kernel-derived identity for the process on the other end of a connection.
///
/// `start_ticks` is Linux `/proc/<pid>/stat` field 22 and is bracketed around
/// opening the retained pidfd. The pidfd itself remains private to the stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemBrokerPeerFacts {
    pub process_id: u32,
    pub effective_user_id: u32,
    pub effective_group_id: u32,
    pub start_ticks: u64,
}

/// Listener adopted from systemd's sole activation descriptor.
pub struct SystemBrokerListener(native::SystemBrokerListener);

impl SystemBrokerListener {
    /// Adopt and verify systemd descriptor 3 for the fixed production socket.
    pub fn from_systemd_activation() -> SystemBrokerResult<Self> {
        native::SystemBrokerListener::from_systemd_activation().map(Self)
    }

    /// Accept one ordinary-user peer and retain its exact process identity.
    pub fn accept(&self, timeout: Duration) -> SystemBrokerResult<SystemBrokerStream> {
        self.0.accept(timeout).map(SystemBrokerStream)
    }
}

/// One authenticated broker stream. The retained pidfd prevents liveness
/// checks from accidentally following later PID reuse.
pub struct SystemBrokerStream(native::SystemBrokerStream);

impl SystemBrokerStream {
    /// Connect to the fixed root-owned production socket.
    pub fn connect(timeout: Duration) -> SystemBrokerResult<Self> {
        native::SystemBrokerStream::connect(timeout).map(Self)
    }

    #[must_use]
    pub fn peer(&self) -> &SystemBrokerPeerFacts {
        self.0.peer()
    }

    /// `true` only while the retained peer process is still live.
    pub fn peer_is_alive(&self) -> SystemBrokerResult<bool> {
        self.0.peer_is_alive()
    }

    pub fn set_io_timeout(&self, timeout: Duration) -> SystemBrokerResult<()> {
        self.0.set_io_timeout(timeout)
    }

    /// Half-close the outgoing direction after one complete request or reply.
    /// The peer can then require EOF and reject a trailing second frame.
    pub fn shutdown_write(&self) -> SystemBrokerResult<()> {
        self.0.shutdown_write()
    }
}

impl Read for SystemBrokerStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Write for SystemBrokerStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
