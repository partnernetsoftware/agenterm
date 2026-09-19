//! Client call / server serve on a [`Slot`](crate::Slot).
//!
//! `seq` is the in-flight id. A client CAS-es `IDLE → CLAIM` before writing the
//! payload, then publishes `REQ`. The server may publish `RESP` only while
//! `state` is still `REQ` and `seq` still matches. A client timeout CAS-es
//! `REQ → IDLE` only for its own `seq`.

use crate::slot::{
    PAYLOAD_CAP, STATE_CLAIM, STATE_IDLE, STATE_REQ, STATE_RESP, STATUS_OK, STATUS_REJECTED,
    STATUS_TIMEOUT, Slot,
};
use crate::wait::{WaitKind, require_native, wait_idle_slice, wait_state, wake_state};
use std::io;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

pub const DEFAULT_SERVE_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Lab toy transform (matches historical benches / nng court).
pub fn xor_a5(buf: &mut [u8]) {
    for b in buf {
        *b ^= 0xA5;
    }
}

fn fail(kind: io::ErrorKind, msg: &str) -> io::Error {
    io::Error::new(kind, msg)
}

/// Clears `CLAIM` if this call panics before publishing `REQ`.
struct ClaimGuard {
    state: *const AtomicU32,
    seq: *const AtomicU64,
    id: u64,
    armed: bool,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if !self.armed || self.state.is_null() {
            return;
        }
        // SAFETY: pointers are this Slot's header; the Slot outlives the guard.
        unsafe {
            if (*self.seq).load(Ordering::Acquire) == self.id
                && (*self.state).load(Ordering::Acquire) == STATE_CLAIM
            {
                (*self.state).store(STATE_IDLE, Ordering::Release);
            }
        }
    }
}

impl Slot {
    /// Client: publish `req`, wait for response, return response bytes.
    pub fn call(&mut self, req: &[u8], wait: WaitKind) -> io::Result<Vec<u8>> {
        self.call_observed(req, wait, DEFAULT_CALL_TIMEOUT, None)
    }

    pub fn call_timeout(
        &mut self,
        req: &[u8],
        wait: WaitKind,
        timeout: Duration,
    ) -> io::Result<Vec<u8>> {
        self.call_observed(req, wait, timeout, None)
    }

    pub(crate) fn call_observed(
        &mut self,
        req: &[u8],
        wait: WaitKind,
        timeout: Duration,
        expect_gen: Option<u32>,
    ) -> io::Result<Vec<u8>> {
        if wait == WaitKind::Native {
            require_native(wait)?;
        }
        if req.len() > PAYLOAD_CAP {
            return Err(fail(io::ErrorKind::InvalidInput, "payload too large"));
        }
        let id = self.publish_req(req, wait, expect_gen)?;
        self.wait_reply(id, wait, timeout, expect_gen, false)
    }

    pub(crate) fn publish_req(
        &mut self,
        req: &[u8],
        wait: WaitKind,
        expect_gen: Option<u32>,
    ) -> io::Result<u64> {
        if wait == WaitKind::Native {
            require_native(wait)?;
        }
        if req.len() > PAYLOAD_CAP {
            return Err(fail(io::ErrorKind::InvalidInput, "payload too large"));
        }
        self.reclaim_dead_flight(wait);
        self.ensure_generation(expect_gen)?;
        let id = {
            let h = self.header();
            h.state
                .compare_exchange(STATE_IDLE, STATE_CLAIM, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| fail(io::ErrorKind::WouldBlock, "busy"))?;
            h.seq.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
        };
        let mut guard = {
            let h = self.header();
            ClaimGuard {
                state: &h.state,
                seq: &h.seq,
                id,
                armed: true,
            }
        };
        self.stamp_client();
        self.payload_mut()[..req.len()].copy_from_slice(req);
        {
            let h = self.header();
            h.req_len.store(req.len() as u32, Ordering::Release);
            h.status.store(STATUS_OK, Ordering::Release);
            h.state.store(STATE_REQ, Ordering::Release);
            wake_state(h, wait);
        }
        guard.armed = false;
        Ok(id)
    }

    pub(crate) fn wait_reply(
        &mut self,
        id: u64,
        wait: WaitKind,
        timeout: Duration,
        expect_gen: Option<u32>,
        nonblock: bool,
    ) -> io::Result<Vec<u8>> {
        if nonblock {
            self.ensure_generation(expect_gen)?;
            if self.header().state.load(Ordering::Acquire) == STATE_RESP {
                return self.take_reply(wait, id);
            }
            return Err(fail(io::ErrorKind::WouldBlock, "busy"));
        }
        match wait_state(self.header(), STATE_RESP, timeout, wait) {
            Ok(()) => self.take_reply(wait, id),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                self.ensure_generation(expect_gen)?;
                self.abandon_flight(wait, id)
            }
            Err(e) => Err(e),
        }
    }

    /// Copy one request out. The slot stays `REQ` until [`reply_req`](Self::reply_req).
    pub(crate) fn accept_req(
        &mut self,
        wait: WaitKind,
        timeout: Duration,
        nonblock: bool,
    ) -> io::Result<(u64, Vec<u8>)> {
        if wait == WaitKind::Native {
            require_native(wait)?;
        }
        if nonblock {
            self.reclaim_dead_flight(wait);
            if self.state() != STATE_REQ {
                return Err(fail(io::ErrorKind::WouldBlock, "busy"));
            }
        } else {
            loop {
                wait_state(self.header(), STATE_REQ, timeout, wait)?;
                if self.reclaim_dead_flight(wait) {
                    continue;
                }
                break;
            }
        }
        let flight = self.header().seq.load(Ordering::Acquire);
        let n = self.header().req_len.load(Ordering::Acquire) as usize;
        if n > PAYLOAD_CAP {
            return Err(fail(io::ErrorKind::InvalidData, "req too large"));
        }
        let bytes = self.payload()[..n].to_vec();
        if self.header().seq.load(Ordering::Acquire) != flight
            || self.header().state.load(Ordering::Acquire) != STATE_REQ
        {
            return Err(fail(io::ErrorKind::TimedOut, "timeout"));
        }
        Ok((flight, bytes))
    }

    /// Publish the reply for `id`. A lost CAS means the caller already timed out.
    pub(crate) fn reply_req(&mut self, id: u64, body: &[u8], wait: WaitKind) -> io::Result<()> {
        if body.len() > PAYLOAD_CAP {
            return Err(fail(io::ErrorKind::InvalidInput, "payload too large"));
        }
        if self.header().seq.load(Ordering::Acquire) != id
            || self.header().state.load(Ordering::Acquire) != STATE_REQ
        {
            return Err(fail(io::ErrorKind::TimedOut, "timeout"));
        }
        self.payload_mut()[..body.len()].copy_from_slice(body);
        let h = self.header();
        if h.seq.load(Ordering::Acquire) != id {
            return Err(fail(io::ErrorKind::TimedOut, "timeout"));
        }
        h.resp_len.store(body.len() as u32, Ordering::Release);
        h.status.store(STATUS_OK, Ordering::Release);
        if h.seq.load(Ordering::Acquire) == id
            && h.state
                .compare_exchange(STATE_REQ, STATE_RESP, Ordering::Release, Ordering::Acquire)
                .is_ok()
        {
            wake_state(h, wait);
            Ok(())
        } else {
            Err(fail(io::ErrorKind::TimedOut, "timeout"))
        }
    }

    pub(crate) fn abandon_flight(&mut self, wait: WaitKind, id: u64) -> io::Result<Vec<u8>> {
        if self.header().seq.load(Ordering::Acquire) != id {
            return Err(fail(io::ErrorKind::TimedOut, "timeout"));
        }
        if self.header().state.load(Ordering::Acquire) == STATE_RESP {
            return self.take_reply(wait, id);
        }
        match self.header().state.compare_exchange(
            STATE_REQ,
            STATE_IDLE,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                let h = self.header();
                h.status.store(STATUS_TIMEOUT, Ordering::Release);
                wake_state(h, wait);
                Err(fail(io::ErrorKind::TimedOut, "timeout"))
            }
            Err(_) => {
                if self.header().seq.load(Ordering::Acquire) == id
                    && self.header().state.load(Ordering::Acquire) == STATE_RESP
                {
                    self.take_reply(wait, id)
                } else {
                    Err(fail(io::ErrorKind::TimedOut, "timeout"))
                }
            }
        }
    }

    fn ensure_generation(&self, expect_gen: Option<u32>) -> io::Result<()> {
        if let Some(g) = expect_gen {
            if self.generation() != g {
                return Err(fail(io::ErrorKind::BrokenPipe, "dead"));
            }
        }
        Ok(())
    }

    fn take_reply(&mut self, wait: WaitKind, id: u64) -> io::Result<Vec<u8>> {
        if self.header().seq.load(Ordering::Acquire) != id
            || self.header().state.load(Ordering::Acquire) != STATE_RESP
        {
            return Err(fail(io::ErrorKind::InvalidData, "stale reply"));
        }
        let status = self.header().status.load(Ordering::Acquire);
        let n = self.header().resp_len.load(Ordering::Acquire) as usize;
        let n = n.min(PAYLOAD_CAP);
        let out = self.payload()[..n].to_vec();
        {
            let h = self.header();
            h.state.store(STATE_IDLE, Ordering::Release);
            wake_state(h, wait);
        }
        if status == STATUS_OK {
            Ok(out)
        } else if status == STATUS_REJECTED {
            Err(fail(io::ErrorKind::InvalidData, "rejected"))
        } else {
            Err(fail(io::ErrorKind::InvalidData, "bad status"))
        }
    }

    /// Server: wait for one request, run `handler`, publish `Ok`.
    pub fn serve_one<F>(&mut self, wait: WaitKind, mut handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]),
    {
        self.serve_reply(wait, DEFAULT_SERVE_TIMEOUT, |buf| {
            handler(buf);
            Ok(())
        })
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
        self.serve_reply(wait, timeout, |buf| {
            handler(buf);
            Ok(())
        })
    }

    /// Like [`serve_one`](Self::serve_one), but `Err` becomes status `Rejected`
    /// (still a reply). A lost CAS (client already timed out) is not an error.
    pub fn serve_reply<F>(
        &mut self,
        wait: WaitKind,
        timeout: Duration,
        mut handler: F,
    ) -> io::Result<()>
    where
        F: FnMut(&mut [u8]) -> io::Result<()>,
    {
        if wait == WaitKind::Native {
            require_native(wait)?;
        }
        loop {
            wait_state(self.header(), STATE_REQ, timeout, wait)?;
            if self.reclaim_dead_flight(wait) {
                continue;
            }
            break;
        }
        let flight = self.header().seq.load(Ordering::Acquire);
        let n = self.header().req_len.load(Ordering::Acquire) as usize;
        if n > PAYLOAD_CAP {
            return Err(fail(io::ErrorKind::InvalidData, "req too large"));
        }
        // Own the bytes for the handler so a late reply cannot clobber the next request.
        let mut local = self.payload()[..n].to_vec();
        let reply = handler(&mut local);
        if self.header().seq.load(Ordering::Acquire) != flight
            || self.header().state.load(Ordering::Acquire) != STATE_REQ
        {
            return Ok(());
        }
        let (status, resp_n) = match reply {
            Ok(()) => {
                self.payload_mut()[..n].copy_from_slice(&local);
                (STATUS_OK, n)
            }
            Err(_) => (STATUS_REJECTED, 0usize),
        };
        let h = self.header();
        if h.seq.load(Ordering::Acquire) != flight {
            return Ok(());
        }
        h.resp_len.store(resp_n as u32, Ordering::Release);
        h.status.store(status, Ordering::Release);
        if h.seq.load(Ordering::Acquire) == flight
            && h.state
                .compare_exchange(STATE_REQ, STATE_RESP, Ordering::Release, Ordering::Acquire)
                .is_ok()
        {
            wake_state(h, wait);
        }
        Ok(())
    }

    /// Resident loop until `request_shutdown`. `handler` runs per request.
    pub fn run_resident<F>(&mut self, wait: WaitKind, mut handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]),
    {
        self.run_resident_gen(wait, None, |buf| {
            handler(buf);
            Ok(())
        })
    }

    pub(crate) fn run_resident_gen<F>(
        &mut self,
        wait: WaitKind,
        generation: Option<u32>,
        mut handler: F,
    ) -> io::Result<()>
    where
        F: FnMut(&mut [u8]) -> io::Result<()>,
    {
        if wait == WaitKind::Native {
            require_native(wait)?;
        }
        loop {
            if self.is_shutdown() {
                return Ok(());
            }
            if let Some(g) = generation {
                if self.generation() != g {
                    return Err(fail(io::ErrorKind::BrokenPipe, "dead"));
                }
            }
            if self.state() == STATE_REQ {
                self.serve_reply(wait, DEFAULT_SERVE_TIMEOUT, &mut handler)?;
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
