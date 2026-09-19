//! Practical IPC shape: one mailbox, [`Server`] binds, [`Client`] connects.
//!
//! Addresses use [`crate::Address`] (`shmbox:file:…` / `shmbox:shm:…`).
//! No listen socket. One in-flight call. A busy slot is [`Error::Busy`].
//!
//! Split flight (optional): client [`ask`](Client::ask) then
//! [`await_reply`](Client::await_reply); server [`accept`](Server::accept)
//! then [`reply`](Server::reply). [`call`](Client::call) / [`serve_one`](Server::serve_one)
//! stay the one-shot path.

use crate::address::Address;
use crate::error::Error;
use crate::rpc::xor_a5;
use crate::slot::{Slot, SlotLoc};
use crate::wait::WaitKind;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

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

    pub fn shm(name: impl Into<String>) -> Self {
        Self {
            loc: SlotLoc::Shm(name.into()),
            wait: WaitKind::Native,
        }
    }

    pub fn parse(addr: &str) -> Result<Self, Error> {
        Ok(Self {
            loc: Address::parse(addr)?.to_loc(),
            wait: WaitKind::Native,
        })
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
    generation: u32,
    pending: Option<u64>,
    accept_timeout: Duration,
    closed: bool,
}

fn tighten_file_mode(loc: &SlotLoc) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let SlotLoc::File(path) = loc else {
            return;
        };
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if meta.permissions().mode() & 0o077 != 0 {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = loc;
    }
}

fn corrupt_slot(err: &io::Error) -> bool {
    if err.kind() != io::ErrorKind::InvalidData {
        return false;
    }
    let msg = err.to_string();
    msg.contains("bad slot magic") || msg.contains("short slot")
}

fn recreate_slot(loc: &SlotLoc) -> io::Result<(Slot, u32)> {
    if let SlotLoc::File(path) = loc {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    let slot = Slot::create(loc)?;
    slot.claim_owner();
    let generation = slot.generation();
    Ok((slot, generation))
}

fn bind_slot(loc: &SlotLoc) -> io::Result<(Slot, u32)> {
    tighten_file_mode(loc);
    match Slot::open(loc) {
        Ok(slot) => {
            if slot.owner_blocks_bind() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "already bound",
                ));
            }
            slot.reclaim_owner();
            let generation = slot.generation();
            Ok((slot, generation))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let slot = Slot::create(loc)?;
            slot.claim_owner();
            let generation = slot.generation();
            Ok((slot, generation))
        }
        Err(e) if corrupt_slot(&e) => recreate_slot(loc),
        Err(e) => Err(e),
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if !self.closed {
            self.slot.release_owner(self.generation, self.wait);
        }
    }
}

impl Server {
    /// Create or reclaim the slot and become the server.
    pub fn bind(ep: &Endpoint) -> Result<Self, Error> {
        let (slot, generation) = bind_slot(&ep.loc)?;
        Ok(Self {
            slot,
            wait: ep.wait,
            generation,
            pending: None,
            accept_timeout: crate::rpc::DEFAULT_SERVE_TIMEOUT,
            closed: false,
        })
    }

    /// Bind from a `shmbox:…` address string.
    pub fn bind_addr(addr: &str) -> Result<Self, Error> {
        Self::bind(&Endpoint::parse(addr)?)
    }

    /// Reset our own slot. A live foreign owner is [`Error::AlreadyBound`].
    pub fn attach(ep: &Endpoint) -> Result<Self, Error> {
        let slot = Slot::open(&ep.loc)?;
        let pid = slot.owner_pid();
        if pid != std::process::id() {
            return Err(if pid == 0 {
                Error::NoOwner
            } else if slot.owner_blocks_bind() {
                Error::AlreadyBound
            } else {
                Error::Dead
            });
        }
        let generation = slot.generation();
        slot.reset_mailbox();
        Ok(Self {
            slot,
            wait: ep.wait,
            generation,
            pending: None,
            accept_timeout: crate::rpc::DEFAULT_SERVE_TIMEOUT,
            closed: false,
        })
    }

    pub fn set_accept_timeout(&mut self, timeout: Duration) -> Result<(), Error> {
        self.ensure_open()?;
        self.accept_timeout = timeout;
        Ok(())
    }

    /// Wait for one request. Pair with [`reply`](Self::reply).
    pub fn accept(&mut self) -> Result<Vec<u8>, Error> {
        self.accept_inner(false)
    }

    /// Non-blocking accept. Empty slot → [`Error::Busy`].
    pub fn try_accept(&mut self) -> Result<Vec<u8>, Error> {
        self.accept_inner(true)
    }

    fn accept_inner(&mut self, nonblock: bool) -> Result<Vec<u8>, Error> {
        self.ensure_open()?;
        if self.pending.is_some() {
            return Err(Error::State);
        }
        let (id, bytes) = self
            .slot
            .accept_req(self.wait, self.accept_timeout, nonblock)?;
        self.pending = Some(id);
        Ok(bytes)
    }

    /// Publish the reply for the last [`accept`](Self::accept).
    pub fn reply(&mut self, body: &[u8]) -> Result<(), Error> {
        self.ensure_open()?;
        let Some(id) = self.pending.take() else {
            return Err(Error::State);
        };
        match self.slot.reply_req(id, body, self.wait) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.pending = None;
                Err(e.into())
            }
        }
    }

    pub fn serve_one<F>(&mut self, mut handler: F) -> Result<(), Error>
    where
        F: FnMut(&mut [u8]),
    {
        self.ensure_open()?;
        if self.pending.is_some() {
            return Err(Error::State);
        }
        Ok(self.slot.serve_one(self.wait, |buf| handler(buf))?)
    }

    pub fn serve_reply<F>(&mut self, handler: F) -> Result<(), Error>
    where
        F: FnMut(&mut [u8]) -> io::Result<()>,
    {
        self.ensure_open()?;
        if self.pending.is_some() {
            return Err(Error::State);
        }
        Ok(self
            .slot
            .serve_reply(self.wait, self.accept_timeout, handler)?)
    }

    /// Block serving until shutdown, or until another `bind` steals `generation`.
    pub fn serve<F>(&mut self, mut handler: F) -> Result<(), Error>
    where
        F: FnMut(&mut [u8]),
    {
        self.ensure_open()?;
        if self.pending.is_some() {
            return Err(Error::State);
        }
        Ok(self
            .slot
            .run_resident_gen(self.wait, Some(self.generation), |buf| {
                handler(buf);
                Ok(())
            })?)
    }

    pub fn serve_xor_lab(&mut self) -> Result<(), Error> {
        self.serve(xor_a5)
    }

    pub fn request_shutdown(&self) {
        self.slot.request_shutdown();
        self.slot.nudge_after_shutdown(self.wait);
    }

    /// Release ownership. Further use returns [`Error::Closed`].
    pub fn close(&mut self) -> Result<(), Error> {
        self.ensure_open()?;
        self.pending = None;
        self.slot.release_owner(self.generation, self.wait);
        self.closed = true;
        Ok(())
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn slot(&self) -> &Slot {
        &self.slot
    }

    fn ensure_open(&self) -> Result<(), Error> {
        if self.closed {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }
}

/// Client side of a one-mailbox RPC channel.
pub struct Client {
    slot: Slot,
    wait: WaitKind,
    generation: u32,
    pending: Option<u64>,
    call_timeout: Duration,
    closed: bool,
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(id) = self.pending.take() {
            let _ = self.slot.abandon_flight(self.wait, id);
        }
    }
}

impl Client {
    /// Open an existing mailbox (server must have `bind` already).
    pub fn connect(ep: &Endpoint) -> Result<Self, Error> {
        let slot = Slot::open(&ep.loc)?;
        let generation = slot.require_live_owner()?;
        Ok(Self {
            slot,
            wait: ep.wait,
            generation,
            pending: None,
            call_timeout: crate::rpc::DEFAULT_CALL_TIMEOUT,
            closed: false,
        })
    }

    pub fn connect_addr(addr: &str) -> Result<Self, Error> {
        Self::connect(&Endpoint::parse(addr)?)
    }

    pub fn set_call_timeout(&mut self, timeout: Duration) -> Result<(), Error> {
        self.ensure_open()?;
        self.call_timeout = timeout;
        Ok(())
    }

    /// Publish a request. Pair with [`await_reply`](Self::await_reply).
    pub fn ask(&mut self, req: &[u8]) -> Result<(), Error> {
        self.ensure_open()?;
        if self.pending.is_some() {
            return Err(Error::State);
        }
        let id = self
            .slot
            .publish_req(req, self.wait, Some(self.generation))?;
        self.pending = Some(id);
        Ok(())
    }

    /// Wait for the reply of the last [`ask`](Self::ask).
    pub fn await_reply(&mut self) -> Result<Vec<u8>, Error> {
        self.await_reply_inner(false)
    }

    /// Non-blocking take of the last ask. Not ready → [`Error::Busy`].
    pub fn try_reply(&mut self) -> Result<Vec<u8>, Error> {
        self.await_reply_inner(true)
    }

    fn await_reply_inner(&mut self, nonblock: bool) -> Result<Vec<u8>, Error> {
        self.ensure_open()?;
        let Some(id) = self.pending else {
            return Err(Error::State);
        };
        let result = self.slot.wait_reply(
            id,
            self.wait,
            self.call_timeout,
            Some(self.generation),
            nonblock,
        );
        match result {
            Ok(bytes) => {
                self.pending = None;
                Ok(bytes)
            }
            Err(e) if nonblock && e.kind() == io::ErrorKind::WouldBlock => Err(Error::Busy),
            Err(e) => {
                let err = Error::from(e);
                if matches!(err, Error::Dead) {
                    self.pending = None;
                    self.closed = true;
                } else if !nonblock {
                    self.pending = None;
                }
                Err(err)
            }
        }
    }

    pub fn call(&mut self, req: &[u8]) -> Result<Vec<u8>, Error> {
        self.call_timeout(req, self.call_timeout)
    }

    pub fn call_timeout(&mut self, req: &[u8], timeout: Duration) -> Result<Vec<u8>, Error> {
        self.ensure_open()?;
        if self.pending.is_some() {
            return Err(Error::State);
        }
        Ok(self
            .slot
            .call_observed(req, self.wait, timeout, Some(self.generation))?)
    }

    pub fn close(&mut self) -> Result<(), Error> {
        self.ensure_open()?;
        if let Some(id) = self.pending.take() {
            let _ = self.slot.abandon_flight(self.wait, id);
        }
        self.closed = true;
        Ok(())
    }

    pub fn slot(&self) -> &Slot {
        &self.slot
    }

    fn ensure_open(&self) -> Result<(), Error> {
        if self.closed {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }
}
