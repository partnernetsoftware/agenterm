//! # shmbox
//!
//! Lab prototype: **shared-memory mailbox RPC** without listen sockets.
//!
//! Wait policy is busy-yield or **native** address-wait:
//!
//! | OS | `WaitKind::Native` |
//! |----|--------------------|
//! | macOS | `os_sync_*_SHARED` |
//! | Linux | `futex` WAIT/WAKE |
//! | Windows | `WaitOnAddress` / `WakeByAddressSingle` |
//!
//! Practical shape: [`Endpoint`] + [`Server`] / [`Client`] (no listen socket).
//!
//! ```ignore
//! use shmbox::{xor_a5, Client, Endpoint, Server, WaitKind};
//! let ep = Endpoint::file("bench.slot").with_wait(WaitKind::Native);
//! // A: Server::bind(&ep)?.serve(xor_a5)?;
//! // B: Client::connect(&ep)?.call(b"hello-mmap-ephemeral")?;
//! ```

mod channel;
mod ffi;
mod rpc;
mod slot;
mod wait;

pub use channel::{Client, Endpoint, Server};
pub use rpc::{xor_a5, DEFAULT_CALL_TIMEOUT, DEFAULT_SERVE_TIMEOUT};
pub use slot::{probe_shm, Slot, SlotLoc, HEADER_BYTES, MAGIC, PAYLOAD_CAP, SLOT_BYTES};
pub use wait::{probe_native, probe_os_sync, WaitKind};
