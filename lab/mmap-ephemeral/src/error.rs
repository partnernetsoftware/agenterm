//! Library errors for [`Server`](crate::Server) / [`Client`](crate::Client).

use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    /// Slot already has an in-flight request.
    Busy,
    /// `call` / `recv` deadline elapsed.
    Timeout,
    /// Server published a rejection.
    Rejected,
    /// Owner generation changed, or the peer was closed.
    Dead,
    /// `listen` saw a live owner.
    AlreadyBound,
    /// No server has claimed the slot.
    NoOwner,
    /// Wrong flight phase (`ask` without `await_reply`, `reply` without `accept`, …).
    State,
    /// [`Client::close`] / [`Server::close`] already ran.
    Closed,
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Busy => write!(f, "busy"),
            Error::Timeout => write!(f, "timeout"),
            Error::Rejected => write!(f, "rejected"),
            Error::Dead => write!(f, "dead"),
            Error::AlreadyBound => write!(f, "already bound"),
            Error::NoOwner => write!(f, "no owner"),
            Error::State => write!(f, "state"),
            Error::Closed => write!(f, "closed"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::WouldBlock => Error::Busy,
            io::ErrorKind::TimedOut => Error::Timeout,
            io::ErrorKind::BrokenPipe => Error::Dead,
            io::ErrorKind::AlreadyExists => Error::AlreadyBound,
            io::ErrorKind::NotFound => Error::NoOwner,
            io::ErrorKind::InvalidData if err.to_string().contains("rejected") => Error::Rejected,
            _ => Error::Io(err),
        }
    }
}
