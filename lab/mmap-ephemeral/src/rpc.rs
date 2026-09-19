//! Client call / server serve on a [`Slot`](crate::Slot).

use crate::slot::{Slot, PAYLOAD_CAP, STATE_IDLE, STATE_REQ, STATE_RESP};
use crate::wait::{require_native, wait_idle_slice, wait_state, wake_state, WaitKind};
use std::io;
use std::sync::atomic::Ordering;
use std::time::Duration;

pub const DEFAULT_SERVE_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Lab toy transform (matches historical benches / nng court).
pub fn xor_a5(buf: &mut [u8]) {
    for b in buf {
        *b ^= 0xA5;
    }
}

impl Slot {
    /// Client: publish `req`, wait for response, return response bytes.
    pub fn call(&mut self, req: &[u8], wait: WaitKind) -> io::Result<Vec<u8>> {
        self.call_timeout(req, wait, DEFAULT_CALL_TIMEOUT)
    }

    pub fn call_timeout(
        &mut self,
        req: &[u8],
        wait: WaitKind,
        timeout: Duration,
    ) -> io::Result<Vec<u8>> {
        if wait == WaitKind::Native {
            require_native(wait)?;
        }
        if req.len() > PAYLOAD_CAP {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "payload too large",
            ));
        }
        {
            let h = self.header();
            if h.state.load(Ordering::Acquire) != STATE_IDLE {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "slot busy (state != idle)",
                ));
            }
        }
        self.payload_mut()[..req.len()].copy_from_slice(req);
        {
            let h = self.header();
            h.req_len.store(req.len() as u32, Ordering::Release);
            h.state.store(STATE_REQ, Ordering::Release);
            wake_state(h, wait);
        }
        wait_state(self.header(), STATE_RESP, timeout, wait)?;
        let n = self.header().resp_len.load(Ordering::Acquire) as usize;
        let out = self.payload()[..n].to_vec();
        {
            let h = self.header();
            h.state.store(STATE_IDLE, Ordering::Release);
            wake_state(h, wait);
        }
        Ok(out)
    }

    /// Server: wait for one request, run `handler` on the payload slice, publish response.
    pub fn serve_one<F>(&mut self, wait: WaitKind, mut handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]),
    {
        self.serve_one_timeout(wait, DEFAULT_SERVE_TIMEOUT, &mut handler)
    }

    pub fn serve_one_timeout<F>(
        &mut self,
        wait: WaitKind,
        timeout: Duration,
        handler: &mut F,
    ) -> io::Result<()>
    where
        F: FnMut(&mut [u8]),
    {
        if wait == WaitKind::Native {
            require_native(wait)?;
        }
        wait_state(self.header(), STATE_REQ, timeout, wait)?;
        let n = self.header().req_len.load(Ordering::Acquire) as usize;
        if n > PAYLOAD_CAP {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "req too large"));
        }
        handler(&mut self.payload_mut()[..n]);
        let h = self.header();
        h.resp_len.store(n as u32, Ordering::Release);
        h.seq.fetch_add(1, Ordering::AcqRel);
        h.state.store(STATE_RESP, Ordering::Release);
        wake_state(h, wait);
        Ok(())
    }

    /// Resident loop until `request_shutdown`. `handler` runs per request.
    pub fn run_resident<F>(&mut self, wait: WaitKind, mut handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]),
    {
        if wait == WaitKind::Native {
            require_native(wait)?;
        }
        loop {
            if self.is_shutdown() {
                return Ok(());
            }
            if self.state() == STATE_REQ {
                self.serve_one(wait, &mut handler)?;
                continue;
            }
            wait_idle_slice(self.header(), wait)?;
        }
    }

    /// After shutdown store: nudge waiters (IDLE + wake).
    pub fn nudge_after_shutdown(&self, wait: WaitKind) {
        let h = self.header();
        h.state.store(STATE_IDLE, Ordering::Release);
        wake_state(h, wait);
    }
}
