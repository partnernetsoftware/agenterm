use std::{
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd as _, OwnedFd},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use crate::process_reference::ProcessWait;

pub struct ProcessReference {
    queue: OwnedFd,
    process_id: u32,
    audit_token: Option<AuditToken>,
    exited: AtomicBool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct AuditToken {
    value: [u32; 8],
}

const TASK_AUDIT_TOKEN: u32 = 15;
const TASK_AUDIT_TOKEN_COUNT: u32 = 8;

unsafe extern "C" {
    static mach_task_self_: u32;
    fn task_name_for_pid(target_task: u32, pid: libc::c_int, task: *mut u32) -> libc::c_int;
    fn task_info(
        task: u32,
        flavor: u32,
        information: *mut libc::c_int,
        count: *mut u32,
    ) -> libc::c_int;
    fn mach_port_deallocate(task: u32, name: u32) -> libc::c_int;
    fn proc_signal_with_audittoken(token: *mut AuditToken, signal: libc::c_int) -> libc::c_int;
}

impl ProcessReference {
    pub(crate) fn open(process_id: u32) -> io::Result<Self> {
        let queue = unsafe { libc::kqueue() };
        if queue < 0 {
            return Err(io::Error::last_os_error());
        }
        let queue = unsafe { OwnedFd::from_raw_fd(queue) };
        let change = libc::kevent {
            ident: process_id as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_CLEAR,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        if unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &raw const change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            queue,
            process_id,
            audit_token: None,
            exited: AtomicBool::new(false),
        })
    }

    pub(crate) fn open_for_termination(process_id: u32) -> io::Result<Self> {
        let mut reference = Self::open(process_id)?;
        reference.audit_token = Some(audit_token_for_pid(process_id)?);
        Ok(reference)
    }

    pub(crate) const fn id(&self) -> u32 {
        self.process_id
    }

    pub(crate) fn wait_for_exit(&self, timeout: Option<Duration>) -> io::Result<ProcessWait> {
        if self.exited.load(Ordering::Acquire) {
            return Ok(ProcessWait::Exited);
        }
        let started = Instant::now();
        loop {
            let mut event = unsafe { std::mem::zeroed::<libc::kevent>() };
            let remaining = timeout.map(|limit| limit.saturating_sub(started.elapsed()));
            let native_timeout = remaining.map(|duration| libc::timespec {
                tv_sec: duration.as_secs().min(i64::MAX as u64) as libc::time_t,
                tv_nsec: duration.subsec_nanos().into(),
            });
            let ready = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &raw mut event,
                    1,
                    native_timeout
                        .as_ref()
                        .map_or(std::ptr::null(), std::ptr::from_ref),
                )
            };
            match ready {
                0 if timeout.is_some_and(|limit| started.elapsed() >= limit) => {
                    return Ok(ProcessWait::TimedOut);
                }
                0 => {}
                1 if event.flags & libc::EV_ERROR != 0 => {
                    return Err(io::Error::from_raw_os_error(event.data as i32));
                }
                1 if event.filter == libc::EVFILT_PROC && event.fflags & libc::NOTE_EXIT != 0 => {
                    self.exited.store(true, Ordering::Release);
                    return Ok(ProcessWait::Exited);
                }
                1 => return Err(io::Error::other("unexpected kqueue process event")),
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
                value => {
                    return Err(io::Error::other(format!(
                        "unexpected kqueue result {value}"
                    )));
                }
            }
        }
    }

    pub(crate) fn terminate(
        &self,
        mode: crate::process_control::TerminationMode,
    ) -> io::Result<()> {
        let mut token = self.audit_token.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "exact-process audit token was not retained for termination",
            )
        })?;
        let signal = match mode {
            crate::process_control::TerminationMode::Graceful => libc::SIGTERM,
            crate::process_control::TerminationMode::Forceful => libc::SIGKILL,
        };
        let error = unsafe { proc_signal_with_audittoken(&raw mut token, signal) };
        if error == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(error))
        }
    }

    pub(crate) fn set_suspended(&self, suspended: bool) -> io::Result<()> {
        let mut token = self.audit_token.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "exact-process audit token was not retained for control",
            )
        })?;
        let signal = if suspended {
            libc::SIGSTOP
        } else {
            libc::SIGCONT
        };
        let error = unsafe { proc_signal_with_audittoken(&raw mut token, signal) };
        if error == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(error))
        }
    }
}

fn audit_token_for_pid(process_id: u32) -> io::Result<AuditToken> {
    let pid = libc::c_int::try_from(process_id)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "process id is out of range"))?;
    let self_task = unsafe { mach_task_self_ };
    let mut task = 0_u32;
    let result = unsafe { task_name_for_pid(self_task, pid, &raw mut task) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    let mut token = AuditToken::default();
    let mut count = TASK_AUDIT_TOKEN_COUNT;
    let info_result = unsafe {
        task_info(
            task,
            TASK_AUDIT_TOKEN,
            token.value.as_mut_ptr().cast(),
            &raw mut count,
        )
    };
    let _ = unsafe { mach_port_deallocate(self_task, task) };
    if info_result != 0 {
        return Err(io::Error::from_raw_os_error(info_result));
    }
    if count != TASK_AUDIT_TOKEN_COUNT || token.value[5] != process_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "task audit token does not identify the requested process",
        ));
    }
    Ok(token)
}

impl AsRawFd for crate::process_reference::ProcessReference {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.queue.as_raw_fd()
    }
}

impl AsFd for crate::process_reference::ProcessReference {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.queue.as_fd()
    }
}
