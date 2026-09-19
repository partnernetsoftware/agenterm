//! Shared slot mapping (file or POSIX/Windows named shared memory).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Production mailbox (`MMAPEPH\x04`). Layout still 64 bytes.
/// The old pad is `owner_birth` + in-flight `client_pid` / `client_birth`.
pub const MAGIC: u64 = 0x4d4d_4150_4550_4804; // "MMAPEPH\x04"
pub const SLOT_BYTES: usize = 64 * 1024;
pub const HEADER_BYTES: usize = 64;
pub const PAYLOAD_CAP: usize = SLOT_BYTES - HEADER_BYTES;

pub(crate) const STATE_IDLE: u32 = 0;
pub(crate) const STATE_REQ: u32 = 1;
pub(crate) const STATE_RESP: u32 = 2;
/// Exclusive claim. Payload is published only after this CAS wins; servers ignore it.
pub(crate) const STATE_CLAIM: u32 = 3;

pub const STATUS_OK: u32 = 0;
/// Wire value. Callers see `WouldBlock` / `"busy"` without overwriting a live slot.
#[allow(dead_code)]
pub const STATUS_BUSY: u32 = 1;
pub const STATUS_TIMEOUT: u32 = 2;
pub const STATUS_REJECTED: u32 = 3;
/// Wire value. Callers see `BrokenPipe` / `"dead"` when `generation` changes.
#[allow(dead_code)]
pub const STATUS_DEAD: u32 = 4;

#[derive(Clone, Debug)]
pub enum SlotLoc {
    File(PathBuf),
    /// POSIX `shm_open` name, or Windows named file-mapping (`Local\…`).
    Shm(String),
}

impl SlotLoc {
    pub fn parse(raw: &str) -> Self {
        if let Some(name) = raw.strip_prefix("shm:") {
            Self::Shm(name.to_string())
        } else {
            Self::File(PathBuf::from(raw))
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::File(p) => p.display().to_string(),
            Self::Shm(n) => format!("shm:{n}"),
        }
    }
}

#[repr(C)]
pub(crate) struct Header {
    pub magic: AtomicU64,
    pub seq: AtomicU64,
    pub state: AtomicU32,
    pub shutdown: AtomicU32,
    pub req_len: AtomicU32,
    pub resp_len: AtomicU32,
    pub owner_pid: AtomicU32,
    pub generation: AtomicU32,
    pub status: AtomicU32,
    /// Process start-time of `owner_pid`, split so the header stays 4-aligned here.
    pub owner_birth_lo: AtomicU32,
    pub owner_birth_hi: AtomicU32,
    pub client_pid: AtomicU32,
    pub client_birth_lo: AtomicU32,
    pub client_birth_hi: AtomicU32,
}

const _: () = assert!(std::mem::size_of::<Header>() == HEADER_BYTES);

/// One mapped mailbox. Single-threaded use per process side (lab contract).
pub struct Slot {
    ptr: *mut u8,
    len: usize,
    #[cfg(unix)]
    shm_unlink_name: Option<std::ffi::CString>,
    #[cfg(unix)]
    shm_fd: Option<std::os::fd::OwnedFd>,
    #[cfg(windows)]
    file: Option<crate::ffi::HANDLE>,
    #[cfg(windows)]
    mapping: Option<crate::ffi::HANDLE>,
}

// SAFETY: lab contract is one owner thread per Slot; cross-process via shared map.
unsafe impl Send for Slot {}

impl Drop for Slot {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: ptr from mmap with this len.
            unsafe {
                let _ = crate::ffi::munmap(self.ptr as *mut _, self.len);
            }
            drop(self.shm_fd.take());
            if let Some(name) = self.shm_unlink_name.take() {
                // SAFETY: name is a CString we created for shm_open.
                unsafe {
                    let _ = crate::ffi::shm_unlink(name.as_ptr());
                }
            }
        }
        #[cfg(windows)]
        {
            use crate::ffi::{CloseHandle, INVALID_HANDLE_VALUE, UnmapViewOfFile};
            if !self.ptr.is_null() {
                // SAFETY: ptr from MapViewOfFile.
                unsafe {
                    let _ = UnmapViewOfFile(self.ptr.cast());
                }
                self.ptr = std::ptr::null_mut();
            }
            if let Some(h) = self.mapping.take() {
                unsafe {
                    let _ = CloseHandle(h);
                }
            }
            if let Some(h) = self.file.take() {
                if h != INVALID_HANDLE_VALUE {
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                }
            }
        }
    }
}

impl Slot {
    pub fn create(loc: &SlotLoc) -> io::Result<Self> {
        open_slot(loc, true)
    }

    pub fn open(loc: &SlotLoc) -> io::Result<Self> {
        open_slot(loc, false)
    }

    pub(crate) fn header(&self) -> &Header {
        assert!(self.len >= HEADER_BYTES);
        // SAFETY: mapping is SLOT_BYTES; Header fits in HEADER_BYTES.
        unsafe { &*(self.ptr as *const Header) }
    }

    pub(crate) fn payload_mut(&mut self) -> &mut [u8] {
        // SAFETY: exclusive &mut self; mapping live.
        let slice = unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) };
        &mut slice[HEADER_BYTES..HEADER_BYTES + PAYLOAD_CAP]
    }

    pub(crate) fn payload(&self) -> &[u8] {
        // SAFETY: mapping live for Slot lifetime.
        let slice = unsafe { std::slice::from_raw_parts(self.ptr, self.len) };
        &slice[HEADER_BYTES..HEADER_BYTES + PAYLOAD_CAP]
    }

    /// Clear shutdown and force IDLE (call before spawning a resident).
    pub fn reset_mailbox(&self) {
        let h = self.header();
        h.shutdown.store(0, Ordering::Release);
        h.client_pid.store(0, Ordering::Release);
        h.state.store(STATE_IDLE, Ordering::Release);
    }

    pub fn request_shutdown(&self) {
        self.header().shutdown.store(1, Ordering::Release);
    }

    pub fn is_shutdown(&self) -> bool {
        self.header().shutdown.load(Ordering::Acquire) != 0
    }

    pub fn state(&self) -> u32 {
        self.header().state.load(Ordering::Acquire)
    }

    pub fn seq(&self) -> u64 {
        self.header().seq.load(Ordering::Acquire)
    }

    pub fn generation(&self) -> u32 {
        self.header().generation.load(Ordering::Acquire)
    }

    pub fn owner_pid(&self) -> u32 {
        self.header().owner_pid.load(Ordering::Acquire)
    }

    /// First owner of a freshly created slot. Generation becomes 1.
    pub fn claim_owner(&self) {
        let birth = process_birth(std::process::id()).unwrap_or(0);
        self.store_owner_birth(birth);
        let h = self.header();
        h.status.store(STATUS_OK, Ordering::Release);
        h.client_pid.store(0, Ordering::Release);
        h.generation.store(1, Ordering::Release);
        h.owner_pid.store(std::process::id(), Ordering::Release);
    }

    /// Dead-owner takeover: bump generation, force IDLE, record this pid.
    pub fn reclaim_owner(&self) {
        let birth = process_birth(std::process::id()).unwrap_or(0);
        self.store_owner_birth(birth);
        let h = self.header();
        h.shutdown.store(0, Ordering::Release);
        h.status.store(STATUS_OK, Ordering::Release);
        h.client_pid.store(0, Ordering::Release);
        h.state.store(STATE_IDLE, Ordering::Release);
        let next = h.generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        if next == 0 {
            h.generation.store(1, Ordering::Release);
        }
        h.owner_pid.store(std::process::id(), Ordering::Release);
    }

    /// Another live process already owns this slot.
    ///
    /// A live pid whose start-time does not match `owner_birth` is a recycled
    /// pid, not the owner.
    pub fn owner_blocks_bind(&self) -> bool {
        let pid = self.owner_pid();
        pid != 0 && pid != std::process::id() && same_instance(pid, self.owner_birth())
    }

    /// `connect` gate: a live owner, or `NotFound` / `BrokenPipe`.
    pub(crate) fn require_live_owner(&self) -> io::Result<u32> {
        let pid = self.owner_pid();
        let owner_gen = self.generation();
        if owner_gen == 0 || pid == 0 {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no owner"));
        }
        if !same_instance(pid, self.owner_birth()) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "dead"));
        }
        Ok(owner_gen)
    }

    pub(crate) fn stamp_client(&self) {
        let pid = std::process::id();
        let birth = process_birth(pid).unwrap_or(0);
        self.store_client(pid, birth);
    }

    /// `CLAIM` / `REQ` / `RESP` left by a dead caller becomes `IDLE`.
    /// Returns whether this call cleared one. `client_pid == 0` is left alone.
    pub(crate) fn reclaim_dead_flight(&self, wait: crate::wait::WaitKind) -> bool {
        let h = self.header();
        let state = h.state.load(Ordering::Acquire);
        if state == STATE_IDLE {
            return false;
        }
        let pid = h.client_pid.load(Ordering::Acquire);
        if pid == 0 || same_instance(pid, self.client_birth()) {
            return false;
        }
        if h.client_pid.load(Ordering::Acquire) != pid {
            return false;
        }
        if h.state
            .compare_exchange(state, STATE_IDLE, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        h.client_pid.store(0, Ordering::Release);
        crate::wait::wake_state(h, wait);
        true
    }

    fn owner_birth(&self) -> u64 {
        let h = self.header();
        load_pair(&h.owner_birth_lo, &h.owner_birth_hi)
    }

    fn client_birth(&self) -> u64 {
        let h = self.header();
        load_pair(&h.client_birth_lo, &h.client_birth_hi)
    }

    fn store_owner_birth(&self, birth: u64) {
        let h = self.header();
        store_pair(&h.owner_birth_lo, &h.owner_birth_hi, birth);
    }

    fn store_client(&self, pid: u32, birth: u64) {
        let h = self.header();
        store_pair(&h.client_birth_lo, &h.client_birth_hi, birth);
        h.client_pid.store(pid, Ordering::Release);
    }

    /// Drop ownership if this server still holds `generation`.
    /// Bumps generation so connected clients observe [`crate::Error::Dead`],
    /// and clears `owner_pid` so a recycled pid cannot block the next `bind`.
    pub(crate) fn release_owner(&self, generation: u32, wait: crate::wait::WaitKind) {
        {
            let h = self.header();
            if h.generation.load(Ordering::Acquire) != generation {
                return;
            }
            if h.owner_pid.load(Ordering::Acquire) != std::process::id() {
                return;
            }
            let next = h.generation.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
            if next == 0 {
                h.generation.store(1, Ordering::Release);
            }
            h.owner_pid.store(0, Ordering::Release);
        }
        self.store_owner_birth(0);
        let h = self.header();
        // Leave a published reply in place so the client can still take it.
        if h.state.load(Ordering::Acquire) != STATE_RESP {
            h.client_pid.store(0, Ordering::Release);
            h.state.store(STATE_IDLE, Ordering::Release);
        }
        crate::wait::wake_state(h, wait);
    }

    #[cfg(test)]
    pub(crate) fn force_owner_pid(&self, pid: u32) {
        let birth = process_birth(pid).unwrap_or(0);
        self.store_owner_birth(birth);
        self.header().owner_pid.store(pid, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn force_owner_birth(&self, birth: u64) {
        self.store_owner_birth(birth);
    }

    /// Leave `REQ` owned by a pid that is not alive.
    #[cfg(test)]
    pub(crate) fn force_dead_client_flight(&self) {
        self.store_client(u32::MAX - 7, 1);
        self.header().state.store(STATE_REQ, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn current_birth() -> Option<u64> {
        process_birth(std::process::id())
    }
}

fn ensure_mapped_len(len: u64) -> io::Result<()> {
    if len != SLOT_BYTES as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short slot"));
    }
    Ok(())
}

fn finish_slot(map: Slot, create: bool) -> io::Result<Slot> {
    if create {
        // SAFETY: exclusive mapping before publish.
        let slice = unsafe { std::slice::from_raw_parts_mut(map.ptr, map.len) };
        slice.fill(0);
        let h = map.header();
        h.magic.store(MAGIC, Ordering::Release);
        h.state.store(STATE_IDLE, Ordering::Release);
    } else {
        let magic = map.header().magic.load(Ordering::Acquire);
        if magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bad slot magic: {magic:#x} (want {MAGIC:#x})"),
            ));
        }
    }
    Ok(map)
}

fn open_slot(loc: &SlotLoc, create: bool) -> io::Result<Slot> {
    match loc {
        SlotLoc::File(path) => open_slot_file(path, create),
        SlotLoc::Shm(name) => open_slot_shm(name, create),
    }
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only probes existence; does not deliver.
        let rc = unsafe { crate::ffi::kill(pid as i32, 0) };
        if rc == 0 {
            return true;
        }
        // ESRCH = 3 on Darwin and Linux. EPERM means the pid exists.
        io::Error::last_os_error().raw_os_error() != Some(3)
    }
    #[cfg(windows)]
    {
        use crate::ffi::{CloseHandle, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if !h.is_null() {
            unsafe {
                let _ = CloseHandle(h);
            }
            return true;
        }
        // ERROR_ACCESS_DENIED = 5 → process exists.
        io::Error::last_os_error().raw_os_error() == Some(5)
    }
}

fn load_pair(lo: &AtomicU32, hi: &AtomicU32) -> u64 {
    let hi = u64::from(hi.load(Ordering::Acquire));
    let lo = u64::from(lo.load(Ordering::Acquire));
    (hi << 32) | lo
}

fn store_pair(lo: &AtomicU32, hi: &AtomicU32, value: u64) {
    lo.store(value as u32, Ordering::Release);
    hi.store((value >> 32) as u32, Ordering::Release);
}

/// `birth == 0` means "unknown": a live pid is treated as the same instance.
/// A non-zero birth must match the process start-time, so a recycled pid does not.
fn same_instance(pid: u32, birth: u64) -> bool {
    if pid == 0 || !pid_alive(pid) {
        return false;
    }
    match process_birth(pid) {
        Some(live) => birth == 0 || birth == live,
        None => true,
    }
}

#[cfg(target_os = "macos")]
fn process_birth(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    // Offsets checked against `struct proc_bsdinfo`: size 136, `pbi_pid` 12,
    // `pbi_start_tvsec` 120, `pbi_start_tvusec` 128.
    let mut buf = [0u8; 256];
    // SAFETY: buffer is writable and large enough for PROC_PIDTBSDINFO.
    let n = unsafe {
        crate::ffi::proc_pidinfo(
            pid as i32,
            crate::ffi::PROC_PIDTBSDINFO,
            0,
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if n < 136 {
        return None;
    }
    let got = u32::from_ne_bytes(buf[12..16].try_into().ok()?);
    if got != pid {
        return None;
    }
    let sec = u64::from_ne_bytes(buf[120..128].try_into().ok()?);
    let usec = u64::from_ne_bytes(buf[128..136].try_into().ok()?);
    if sec == 0 && usec == 0 {
        return None;
    }
    Some(sec.saturating_mul(1_000_000).saturating_add(usec))
}

#[cfg(target_os = "linux")]
fn process_birth(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = text.rsplit_once(')')?.1;
    // Field 22 (starttime) is the 20th token after the comm `)`.
    rest.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(windows)]
fn process_birth(pid: u32) -> Option<u64> {
    use crate::ffi::{
        CloseHandle, FileTime, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return None;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut created = FileTime { lo: 0, hi: 0 };
    let mut exited = FileTime { lo: 0, hi: 0 };
    let mut kernel = FileTime { lo: 0, hi: 0 };
    let mut user = FileTime { lo: 0, hi: 0 };
    // SAFETY: handle from OpenProcess; FileTime is the Win32 FILETIME layout.
    let ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    unsafe {
        let _ = CloseHandle(handle);
    }
    if ok == 0 {
        return None;
    }
    Some((u64::from(created.hi) << 32) | u64::from(created.lo))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn process_birth(_pid: u32) -> Option<u64> {
    None
}

#[cfg(unix)]
mod unix_impl {
    use super::*;
    use crate::ffi::{self, MAP_SHARED, O_CREAT, O_EXCL, O_RDWR, O_TRUNC, PROT_READ, PROT_WRITE};
    use std::ffi::CString;
    use std::fs::OpenOptions;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::ptr;

    // Drop is on the parent `Slot`.

    fn shm_c_name(name: &str) -> io::Result<CString> {
        let full = if name.starts_with('/') {
            name.to_string()
        } else {
            format!("/{name}")
        };
        CString::new(full).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    }

    fn map_fd(fd: i32, create: bool) -> io::Result<*mut u8> {
        if create {
            // SAFETY: fd is open RW.
            if unsafe { ffi::ftruncate(fd, SLOT_BYTES as i64) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        // SAFETY: fd RW; SLOT_BYTES; MAP_SHARED.
        let ptr = unsafe {
            ffi::mmap(
                ptr::null_mut(),
                SLOT_BYTES,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr.is_null() || ptr == (!0usize as *mut u8) {
            return Err(io::Error::last_os_error());
        }
        Ok(ptr)
    }

    pub(super) fn open_slot_file(path: &Path, create: bool) -> io::Result<Slot> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(create)
            .open(path)?;
        if create {
            file.set_len(SLOT_BYTES as u64)?;
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        } else {
            let meta = file.metadata()?;
            ensure_mapped_len(meta.len())?;
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "slot mode"));
            }
        }
        let fd = file.as_raw_fd();
        let ptr = map_fd(fd, false)?;
        finish_slot(
            Slot {
                ptr,
                len: SLOT_BYTES,
                shm_unlink_name: None,
                shm_fd: None,
            },
            create,
        )
    }

    pub(super) fn open_slot_shm(name: &str, create: bool) -> io::Result<Slot> {
        let cname = shm_c_name(name)?;
        if create {
            // SAFETY: best-effort stale clear.
            unsafe {
                let _ = ffi::shm_unlink(cname.as_ptr());
            }
        }
        let oflag = if create {
            O_RDWR | O_CREAT | O_EXCL | O_TRUNC
        } else {
            O_RDWR
        };
        // SAFETY: cname NUL-terminated; mode 0o600.
        let fd = unsafe { ffi::shm_open(cname.as_ptr(), oflag, 0o600) };
        if fd < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(13) {
                return Err(io::Error::other(format!(
                    "shm_open({name}) EACCES — POSIX shm reopen blocked on this macOS; use a file slot instead"
                )));
            }
            return Err(err);
        }
        // SAFETY: fd from shm_open.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let ptr = map_fd(owned.as_raw_fd(), create)?;
        let map = Slot {
            ptr,
            len: SLOT_BYTES,
            shm_unlink_name: None,
            shm_fd: Some(owned),
        };
        finish_slot(map, create)
    }

    pub(super) fn probe_shm() -> io::Result<()> {
        let name = "mmap-lab-probe";
        let cname = shm_c_name(name)?;
        // SAFETY: unlink/create/open probe only.
        unsafe {
            let _ = ffi::shm_unlink(cname.as_ptr());
            let fd = ffi::shm_open(cname.as_ptr(), O_RDWR | O_CREAT | O_EXCL, 0o600);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let _ = ffi::ftruncate(fd, 4096);
            let fd2 = ffi::shm_open(cname.as_ptr(), O_RDWR, 0o600);
            let err2 = io::Error::last_os_error();
            let _ = ffi::close(fd);
            if fd2 >= 0 {
                let _ = ffi::close(fd2);
                let _ = ffi::shm_unlink(cname.as_ptr());
                println!("shm_open: reopen OK (cross-open within process works)");
                return Ok(());
            }
            let _ = ffi::shm_unlink(cname.as_ptr());
            eprintln!(
                "shm_open: reopen FAILED after create (errno={}) — not usable for multi-process mailbox on this host",
                err2.raw_os_error().unwrap_or(-1)
            );
            Err(io::Error::other(format!("shm reopen failed: {err2}")))
        }
    }
}

#[cfg(unix)]
use unix_impl::{open_slot_file, open_slot_shm, probe_shm as probe_shm_inner};

#[cfg(windows)]
mod win_impl {
    use super::*;
    use crate::ffi::{
        self, CloseHandle, CreateFileMappingW, FILE_MAP_ALL_ACCESS, INVALID_HANDLE_VALUE,
        MapViewOfFile, PAGE_READWRITE,
    };
    use std::fs::OpenOptions;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    // Drop is on the parent `Slot`.

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn map_from_file_handle(
        file: ffi::HANDLE,
        create: bool,
        named: Option<&str>,
    ) -> io::Result<Slot> {
        let name_wide = named.map(wide);
        let name_ptr = name_wide
            .as_ref()
            .map(|v| v.as_ptr())
            .unwrap_or(ptr::null());
        // SAFETY: kernel32 CreateFileMappingW.
        let mapping = unsafe {
            CreateFileMappingW(
                file,
                ptr::null_mut(),
                PAGE_READWRITE,
                0,
                SLOT_BYTES as u32,
                name_ptr,
            )
        };
        if mapping.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: MapViewOfFile on mapping.
        let base = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, SLOT_BYTES) };
        if base.is_null() {
            unsafe {
                let _ = CloseHandle(mapping);
            }
            return Err(io::Error::last_os_error());
        }
        let keep_file = if file == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(file)
        };
        finish_slot(
            Slot {
                ptr: base.cast(),
                len: SLOT_BYTES,
                file: keep_file,
                mapping: Some(mapping),
            },
            create,
        )
    }

    pub(super) fn open_slot_file(path: &Path, create: bool) -> io::Result<Slot> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(create)
            .open(path)?;
        if create {
            file.set_len(SLOT_BYTES as u64)?;
        } else {
            ensure_mapped_len(file.metadata()?.len())?;
        }
        // Duplicate: AsRawHandle is borrowed; DuplicateHandle would be safer.
        // Lab: leak the std file into a raw HANDLE we CloseHandle in Drop — use
        // into_raw_handle to transfer ownership.
        use std::os::windows::io::IntoRawHandle;
        let h = file.into_raw_handle() as ffi::HANDLE;
        map_from_file_handle(h, create, None)
    }

    pub(super) fn open_slot_shm(name: &str, create: bool) -> io::Result<Slot> {
        // Pagefile-backed named mapping: Local\<name>
        let full = if name.starts_with("Local\\") || name.starts_with("Global\\") {
            name.to_string()
        } else {
            format!("Local\\{name}")
        };
        if !create {
            // Open existing: size 0 with name.
            let w = wide(&full);
            let mapping = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null_mut(),
                    PAGE_READWRITE,
                    0,
                    SLOT_BYTES as u32,
                    w.as_ptr(),
                )
            };
            if mapping.is_null() {
                return Err(io::Error::last_os_error());
            }
            // If we created accidentally, still OK for lab; prefer open existing via
            // OpenFileMapping — keep CreateFileMapping for simplicity.
            let base = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, SLOT_BYTES) };
            if base.is_null() {
                unsafe {
                    let _ = CloseHandle(mapping);
                }
                return Err(io::Error::last_os_error());
            }
            return finish_slot(
                Slot {
                    ptr: base.cast(),
                    len: SLOT_BYTES,
                    file: None,
                    mapping: Some(mapping),
                },
                false,
            );
        }
        map_from_file_handle(INVALID_HANDLE_VALUE, true, Some(&full))
    }

    pub(super) fn probe_shm() -> io::Result<()> {
        let name = "Local\\mmap-lab-probe";
        let w = wide(name);
        let h1 = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null_mut(),
                PAGE_READWRITE,
                0,
                4096,
                w.as_ptr(),
            )
        };
        if h1.is_null() {
            return Err(io::Error::last_os_error());
        }
        let h2 = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null_mut(),
                PAGE_READWRITE,
                0,
                4096,
                w.as_ptr(),
            )
        };
        let err2 = io::Error::last_os_error();
        unsafe {
            let _ = CloseHandle(h1);
        }
        if h2.is_null() {
            eprintln!("CreateFileMapping reopen FAILED: {err2}");
            return Err(err2);
        }
        unsafe {
            let _ = CloseHandle(h2);
        }
        println!("CreateFileMapping: named reopen OK");
        Ok(())
    }
}

#[cfg(windows)]
use win_impl::{open_slot_file, open_slot_shm, probe_shm as probe_shm_inner};

/// Probe whether named shared memory can be reopened after create.
pub fn probe_shm() -> io::Result<()> {
    probe_shm_inner()
}
