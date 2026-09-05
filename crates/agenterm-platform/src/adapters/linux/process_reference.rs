use std::{
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd as _, OwnedFd},
    time::{Duration, Instant},
};

use crate::process_reference::ProcessWait;

pub struct ProcessReference {
    descriptor: OwnedFd,
    process_id: u32,
}

impl ProcessReference {
    pub(crate) fn open(process_id: u32) -> io::Result<Self> {
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, process_id, 0) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            descriptor: unsafe { OwnedFd::from_raw_fd(descriptor as i32) },
            process_id,
        })
    }

    pub(crate) fn open_for_termination(process_id: u32) -> io::Result<Self> {
        Self::open(process_id)
    }

    pub(crate) const fn id(&self) -> u32 {
        self.process_id
    }

    pub(crate) fn wait_for_exit(&self, timeout: Option<Duration>) -> io::Result<ProcessWait> {
        let started = Instant::now();
        loop {
            let timeout_ms = match timeout {
                None => -1,
                Some(limit) => {
                    let remaining = limit.saturating_sub(started.elapsed());
                    if remaining.is_zero() {
                        0
                    } else {
                        remaining
                            .as_millis()
                            .saturating_add(1)
                            .min(i32::MAX as u128) as i32
                    }
                }
            };
            let mut descriptor = libc::pollfd {
                fd: self.descriptor.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&raw mut descriptor, 1, timeout_ms) };
            match ready {
                0 if timeout.is_some_and(|limit| started.elapsed() >= limit) => {
                    return Ok(ProcessWait::TimedOut);
                }
                0 => {}
                1 if descriptor.revents & libc::POLLNVAL != 0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "pidfd is invalid",
                    ));
                }
                1 if descriptor.revents & libc::POLLIN != 0 => return Ok(ProcessWait::Exited),
                1 => return Err(io::Error::other("unexpected pidfd poll event")),
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
                value => {
                    return Err(io::Error::other(format!(
                        "unexpected pidfd poll result {value}"
                    )));
                }
            }
        }
    }

    pub(crate) fn terminate(
        &self,
        mode: crate::process_control::TerminationMode,
    ) -> io::Result<()> {
        let signal = match mode {
            crate::process_control::TerminationMode::Graceful => libc::SIGTERM,
            crate::process_control::TerminationMode::Forceful => libc::SIGKILL,
        };
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.descriptor.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(crate) fn set_suspended(&self, suspended: bool) -> io::Result<()> {
        self.send_signal(if suspended {
            libc::SIGSTOP
        } else {
            libc::SIGCONT
        })
    }

    fn send_signal(&self, signal: libc::c_int) -> io::Result<()> {
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.descriptor.as_raw_fd(),
                signal,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

impl AsRawFd for crate::process_reference::ProcessReference {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.descriptor.as_raw_fd()
    }
}

impl AsFd for crate::process_reference::ProcessReference {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.descriptor.as_fd()
    }
}
