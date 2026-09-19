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
}

fn bind_slot(loc: &SlotLoc) -> io::Result<(Slot, u32)> {
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
        Err(e) => Err(e),
    }
}

impl Server {
    /// Create or reclaim the slot and become the server.
    ///
    /// Fails with `AlreadyExists` / `"already bound"` when `owner_pid` is still alive.
    /// A dead owner is reclaimed (generation bumps).
    pub fn bind(ep: &Endpoint) -> io::Result<Self> {
        let (slot, generation) = bind_slot(&ep.loc)?;
        Ok(Self {
            slot,
            wait: ep.wait,
            generation,
        })
    }

    /// Open an existing slot as server without reclaiming ownership.
    pub fn attach(ep: &Endpoint) -> io::Result<Self> {
        let slot = Slot::open(&ep.loc)?;
        let generation = slot.generation();
        slot.reset_mailbox();
        Ok(Self {
            slot,
            wait: ep.wait,
            generation,
        })
    }

    pub fn serve_one<F>(&mut self, handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]),
    {
        self.slot.serve_one(self.wait, handler)
    }

    pub fn serve_reply<F>(&mut self, handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]) -> io::Result<()>,
    {
        self.slot
            .serve_reply(self.wait, crate::rpc::DEFAULT_SERVE_TIMEOUT, handler)
    }

    /// Block serving until shutdown, or until another `bind` steals `generation`.
    pub fn serve<F>(&mut self, mut handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]),
    {
        self.slot
            .run_resident_gen(self.wait, Some(self.generation), |buf| {
                handler(buf);
                Ok(())
            })
    }

    /// Lab default: XOR `0xA5` (matches historical courts).
    pub fn serve_xor_lab(&mut self) -> io::Result<()> {
        self.serve(xor_a5)
    }

    pub fn request_shutdown(&self) {
        self.slot.request_shutdown();
        self.slot.nudge_after_shutdown(self.wait);
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn slot(&self) -> &Slot {
        &self.slot
    }
}

/// Client side of a one-mailbox RPC channel.
pub struct Client {
    slot: Slot,
    wait: WaitKind,
    generation: u32,
}

impl Client {
    /// Open an existing mailbox (server must have `bind` already).
    pub fn connect(ep: &Endpoint) -> io::Result<Self> {
        let slot = Slot::open(&ep.loc)?;
        let generation = slot.generation();
        if generation == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no owner generation",
            ));
        }
        Ok(Self {
            slot,
            wait: ep.wait,
            generation,
        })
    }

    pub fn call(&mut self, req: &[u8]) -> io::Result<Vec<u8>> {
        self.call_timeout(req, crate::rpc::DEFAULT_CALL_TIMEOUT)
    }

    pub fn call_timeout(&mut self, req: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
        self.slot
            .call_observed(req, self.wait, timeout, Some(self.generation))
    }

    pub fn slot(&self) -> &Slot {
        &self.slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;
    use std::time::Duration;

    fn ep(name: &str) -> (Endpoint, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("shmbox-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&path);
        (Endpoint::file(&path).with_wait(WaitKind::Yield), path)
    }

    #[test]
    fn roundtrip_xor_and_mode() {
        let (ep, path) = ep("xor");
        let mut server = Server::bind(&ep).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let h = thread::spawn(move || server.serve_one(xor_a5).unwrap());
        thread::sleep(Duration::from_millis(20));
        let mut client = Client::connect(&ep).unwrap();
        let out = client.call(b"hello-mmap-ephemeral").unwrap();
        assert_eq!(out.len(), 20);
        assert_eq!(out[0], b'h' ^ 0xA5);
        h.join().unwrap();
        let _ = fs::remove_file(path);
    }

    #[test]
    fn timeout_then_next_call_works() {
        let (ep, path) = ep("timeout");
        let mut server = Server::bind(&ep).unwrap();
        let h = thread::spawn(move || {
            server
                .serve_one(|buf| {
                    thread::sleep(Duration::from_millis(400));
                    xor_a5(buf);
                })
                .unwrap();
            server.serve_one(xor_a5).unwrap();
        });
        thread::sleep(Duration::from_millis(20));
        let mut client = Client::connect(&ep).unwrap();
        let err = client
            .call_timeout(b"hello-mmap-ephemeral", Duration::from_millis(50))
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
        let out = client
            .call_timeout(b"hello-mmap-ephemeral", Duration::from_secs(2))
            .unwrap();
        assert_eq!(out[0], b'h' ^ 0xA5);
        h.join().unwrap();
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejected_is_a_reply() {
        let (ep, path) = ep("rej");
        let mut server = Server::bind(&ep).unwrap();
        let h = thread::spawn(move || {
            server
                .serve_reply(|_| Err(std::io::Error::other("no")))
                .unwrap();
        });
        thread::sleep(Duration::from_millis(20));
        let mut client = Client::connect(&ep).unwrap();
        let err = client.call(b"x").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{err}");
        assert!(err.to_string().contains("rejected"), "{err}");
        h.join().unwrap();
        let _ = fs::remove_file(path);
    }

    #[test]
    fn live_owner_blocks_bind_dead_owner_reclaimed() {
        let (ep, path) = ep("own");
        let server = Server::bind(&ep).unwrap();
        server.slot().force_owner_pid(1);
        match Server::bind(&ep) {
            Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists, "{err}"),
            Ok(_) => panic!("expected already bound"),
        }
        server.slot().force_owner_pid(u32::MAX - 7);
        let again = Server::bind(&ep).unwrap();
        assert_ne!(again.generation(), server.generation());
        drop(again);
        drop(server);
        let _ = fs::remove_file(path);
    }
}
