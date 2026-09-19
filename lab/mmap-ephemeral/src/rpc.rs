//! Client call / server serve on a [`Slot`](crate::Slot).
//!
//! `seq` is the in-flight id. The server may publish `RESP` only while `state`
//! is still `REQ` and `seq` still matches. A client timeout CAS-es `REQ → IDLE`.

use crate::slot::{
    Slot, PAYLOAD_CAP, STATE_IDLE, STATE_REQ, STATE_RESP, STATUS_OK, STATUS_REJECTED,
    STATUS_TIMEOUT,
};
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

fn fail(kind: io::ErrorKind, msg: &str) -> io::Error {
    io::Error::new(kind, msg)
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
        self.ensure_generation(expect_gen)?;
        let _id = {
            let h = self.header();
            if h.state.load(Ordering::Acquire) != STATE_IDLE {
                return Err(fail(io::ErrorKind::WouldBlock, "busy"));
            }
            h.seq.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
        };
        self.payload_mut()[..req.len()].copy_from_slice(req);
        {
            let h = self.header();
            h.req_len.store(req.len() as u32, Ordering::Release);
            h.status.store(STATUS_OK, Ordering::Release);
            h.state.store(STATE_REQ, Ordering::Release);
            wake_state(h, wait);
        }

        match wait_state(self.header(), STATE_RESP, timeout, wait) {
            Ok(()) => self.take_reply(wait),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                self.ensure_generation(expect_gen)?;
                if self.header().state.load(Ordering::Acquire) == STATE_RESP {
                    return self.take_reply(wait);
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
                        if self.header().state.load(Ordering::Acquire) == STATE_RESP {
                            self.take_reply(wait)
                        } else {
                            Err(fail(io::ErrorKind::TimedOut, "timeout"))
                        }
                    }
                }
            }
            Err(e) => Err(e),
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

    fn take_reply(&mut self, wait: WaitKind) -> io::Result<Vec<u8>> {
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
            Err(fail(
                io::ErrorKind::InvalidData,
                "bad status",
            ))
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
    pub fn serve_reply<F>(&mut self, wait: WaitKind, timeout: Duration, mut handler: F) -> io::Result<()>
    where
        F: FnMut(&mut [u8]) -> io::Result<()>,
    {
        if wait == WaitKind::Native {
            require_native(wait)?;
        }
        wait_state(self.header(), STATE_REQ, timeout, wait)?;
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
            && h
                .state
                .compare_exchange(
                    STATE_REQ,
                    STATE_RESP,
                    Ordering::Release,
                    Ordering::Acquire,
                )
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
