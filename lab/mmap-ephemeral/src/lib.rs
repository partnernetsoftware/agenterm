//! # shmbox
//!
//! Shared-memory mailbox RPC without listen sockets.
//!
//! Wait policy is busy-yield or **native** address-wait:
//!
//! | OS | `WaitKind::Native` |
//! |----|--------------------|
//! | macOS | `os_sync_*_SHARED` |
//! | Linux | `futex` WAIT/WAKE |
//! | Windows | `WaitOnAddress` / `WakeByAddressSingle` |
//!
//! Address forms: [`Address`] / [`Endpoint::parse`] —
//! `shmbox:file:/path`, `shmbox:/path`, `shmbox:shm:name`.
//! Peers: [`Server`] / [`Client`] (`call`/`serve`, or split `ask`/`accept`).
//!
//! ```ignore
//! use shmbox::{xor_a5, Client, Endpoint, Server, WaitKind};
//! let ep = Endpoint::parse("shmbox:file:bench.slot")?.with_wait(WaitKind::Native);
//! // A: let mut s = Server::bind(&ep)?; let mut b = s.accept()?; xor_a5(&mut b); s.reply(&b)?;
//! // B: let mut c = Client::connect(&ep)?; c.ask(b"hello")?; let out = c.await_reply()?;
//! ```

mod address;
mod channel;
mod error;
mod ffi;
mod rpc;
mod slot;
mod wait;

#[cfg(test)]
mod court;

pub use address::Address;
pub use channel::{Client, Endpoint, Server};
pub use error::Error;
pub use rpc::{DEFAULT_CALL_TIMEOUT, DEFAULT_SERVE_TIMEOUT, xor_a5};
pub use slot::{HEADER_BYTES, MAGIC, PAYLOAD_CAP, SLOT_BYTES, Slot, SlotLoc, probe_shm};
pub use wait::{WaitKind, probe_native, probe_os_sync};
