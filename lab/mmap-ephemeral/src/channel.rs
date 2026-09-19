//! Practical IPC shape on top of [`Slot`](crate::Slot): bind / connect / call / serve.
//!
//! One file (or named shm) is the rendezvous. No listen socket. The server
//! process owns `Server::serve`; clients `Client::connect` then `call`.
//!
//! Recommended on macOS: **file-backed** `SlotLoc::File` + [`WaitKind::Native`]
//! (`os_sync`). Prefer a stable path under an app data dir, mode 0o600.

use crate::rpc::xor_a5;
use crate::slot::{Slot, SlotLoc};
use crate::wait::WaitKind;
use std::io;
use std::path::PathBuf;

/// How a peer finds the mailbox.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub loc: SlotLoc,
    pub wait: WaitKind,
}

impl Endpoint {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            loc: SlotLoc::File(path.into()),
            wait: WaitKind::Native,
        }
    }

    pub fn with_wait(mut self, wait: WaitKind) -> Self {
        self.wait = wait;
        self
    }
}

/// Server side of a one-mailbox RPC channel.
pub struct Server {
    slot: Slot,
    wait: WaitKind,
}

impl Server {
    /// Create (truncate) the slot and become the server.
    pub fn bind(ep: &Endpoint) -> io::Result<Self> {
        let slot = Slot::create(&ep.loc)?;
        Ok(Self {
            slot,
            wait: ep.wait,
        })
    }

    /// Open an existing slot as server (e.g. after crash recovery).
    pub fn attach(ep: &Endpoint) -> io::Result<Self> {
        let slot = Slot::open(&ep.loc)?;
        slot.reset_mailbox();
        Ok(Self {
            slot,
            wait: ep.wait,
        })
    }

    pub fn serve_one<F>(&mut self, handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]),
    {
        self.slot.serve_one(self.wait, handler)
    }

    /// Block serving until [`Slot::request_shutdown`] from a peer or self.
    pub fn serve<F>(&mut self, handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]),
    {
        self.slot.run_resident(self.wait, handler)
    }

    /// Lab default: XOR `0xA5` (matches historical courts).
    pub fn serve_xor_lab(&mut self) -> io::Result<()> {
        self.serve(xor_a5)
    }

    pub fn request_shutdown(&self) {
        self.slot.request_shutdown();
        self.slot.nudge_after_shutdown(self.wait);
    }

    pub fn slot(&self) -> &Slot {
        &self.slot
    }
}

/// Client side of a one-mailbox RPC channel.
pub struct Client {
    slot: Slot,
    wait: WaitKind,
}

impl Client {
    /// Open an existing mailbox (server must have `bind`/`create` already).
    pub fn connect(ep: &Endpoint) -> io::Result<Self> {
        let slot = Slot::open(&ep.loc)?;
        Ok(Self {
            slot,
            wait: ep.wait,
        })
    }

    pub fn call(&mut self, req: &[u8]) -> io::Result<Vec<u8>> {
        self.slot.call(req, self.wait)
    }

    pub fn slot(&self) -> &Slot {
        &self.slot
    }
}
