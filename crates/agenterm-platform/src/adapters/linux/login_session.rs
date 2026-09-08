//! Linux login-session inventory from systemd-logind via dlopen libsystemd.
//!
//! Screen-lock delivery is intentionally unsupported on Linux; callers must treat
//! [`lock_console`] as an honest host limit rather than a silent no-op.

#![cfg(target_os = "linux")]

use std::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr::null_mut,
};

use sha2::{Digest as _, Sha256};

use crate::login_session::{
    LOGIN_SESSION_DISPLAY_NAME_MAX_BYTES, LOGIN_SESSION_MAX_ROWS, LOGIN_SESSION_USERNAME_MAX_BYTES,
    LoginSessionError, LoginSessionErrorKind, LoginSessionInventory, LoginSessionProvider,
    NativeLoginSessionRow, finish_inventory,
};

const LIBSYSTEMD_SONAME: &CStr = c"libsystemd.so.0";
const MAX_SESSION_VALUE_BYTES: usize = 256;

type SdGetSessions = unsafe extern "C" fn(*mut *mut *mut c_char) -> c_int;
type SdSessionGetUid = unsafe extern "C" fn(*const c_char, *mut libc::uid_t) -> c_int;
type SdSessionGetUser = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type SdSessionGetDisplay = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type SdSessionGetLeader = unsafe extern "C" fn(*const c_char, *mut libc::pid_t) -> c_int;
type SdSessionGetVt = unsafe extern "C" fn(*const c_char, *mut u32) -> c_int;
type SdSessionPredicate = unsafe extern "C" fn(*const c_char) -> c_int;
type SdSessionGetString = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type SdSessionGetLockedHint = unsafe extern "C" fn(*const c_char, *mut c_int) -> c_int;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 0;

struct SystemdLogin {
    handle: *mut c_void,
    get_sessions: SdGetSessions,
    session_get_uid: SdSessionGetUid,
    session_get_user: SdSessionGetUser,
    session_get_display: SdSessionGetDisplay,
    session_get_leader: SdSessionGetLeader,
    session_get_vt: SdSessionGetVt,
    session_is_active: SdSessionPredicate,
    session_is_remote: SdSessionPredicate,
    session_get_type: SdSessionGetString,
    session_get_class: SdSessionGetString,
    session_get_state: SdSessionGetString,
    session_get_seat: SdSessionGetString,
    session_get_locked_hint: SdSessionGetLockedHint,
}

impl SystemdLogin {
    fn load() -> Result<Self, LoginSessionError> {
        // SAFETY: fixed SONAME is NUL-terminated for dlopen.
        let handle = unsafe { dlopen(LIBSYSTEMD_SONAME.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if handle.is_null() {
            return Err(unsupported(
                "libsystemd.so.0 is unavailable on this Linux host",
            ));
        }
        let loaded = (|| {
            Ok(Self {
                handle,
                get_sessions: unsafe { load_symbol(handle, c"sd_get_sessions")? },
                session_get_uid: unsafe { load_symbol(handle, c"sd_session_get_uid")? },
                session_get_user: unsafe { load_symbol(handle, c"sd_session_get_user")? },
                session_get_display: unsafe { load_symbol(handle, c"sd_session_get_display")? },
                session_get_leader: unsafe { load_symbol(handle, c"sd_session_get_leader")? },
                session_get_vt: unsafe { load_symbol(handle, c"sd_session_get_vt")? },
                session_is_active: unsafe { load_symbol(handle, c"sd_session_is_active")? },
                session_is_remote: unsafe { load_symbol(handle, c"sd_session_is_remote")? },
                session_get_type: unsafe { load_symbol(handle, c"sd_session_get_type")? },
                session_get_class: unsafe { load_symbol(handle, c"sd_session_get_class")? },
                session_get_state: unsafe { load_symbol(handle, c"sd_session_get_state")? },
                session_get_seat: unsafe { load_symbol(handle, c"sd_session_get_seat")? },
                session_get_locked_hint: unsafe {
                    load_symbol(handle, c"sd_session_get_locked_hint")?
                },
            })
        })();
        if loaded.is_err() {
            // SAFETY: handle came from successful dlopen above.
            unsafe { dlclose(handle) };
        }
        loaded
    }

    fn inventory(&self) -> Result<(bool, Vec<NativeLoginSessionRow>), LoginSessionError> {
        let mut values = null_mut();
        // SAFETY: sd_get_sessions initializes an allocator-owned string array.
        let count = unsafe { (self.get_sessions)(&mut values) };
        if count < 0 || (count > 0 && values.is_null()) {
            return Err(provider_unavailable(
                "sd_get_sessions could not read the login-session inventory",
            ));
        }
        let count = usize::try_from(count)
            .map_err(|_| shape("sd_get_sessions returned an invalid session count"))?;
        if count > LOGIN_SESSION_MAX_ROWS {
            return Err(shape("native inventory exceeds the session row ceiling"));
        }
        let values = OwnedStringArray { values, count };
        let mut rows = Vec::with_capacity(count);
        let mut locked = false;
        for index in 0..count {
            // SAFETY: libsystemd returned `count` session identifiers.
            let pointer = unsafe { values.values.add(index).read() };
            let id = borrowed_value(pointer, "login-session id")?;
            let row = self.parse_row(&id)?;
            if row.on_console {
                locked = self.session_locked(&id)?;
            }
            rows.push(row);
        }
        Ok((locked, rows))
    }

    fn parse_row(&self, id: &[u8]) -> Result<NativeLoginSessionRow, LoginSessionError> {
        let id_c = c_value(id, "login-session id")?;
        let id_text = id_c
            .to_str()
            .map_err(|_| shape("login-session id is not UTF-8"))?;
        let mut uid = 0;
        // SAFETY: id is a bounded NUL-terminated session identifier.
        if unsafe { (self.session_get_uid)(id_c.as_ptr(), &mut uid) } < 0 {
            return Err(provider_unavailable("sd_session_get_uid failed"));
        }
        let username = self.session_string(self.session_get_user, id_c, "username")?;
        let display_name =
            self.session_optional_string(self.session_get_display, id_c, "display name")?;
        let mut leader = 0;
        if unsafe { (self.session_get_leader)(id_c.as_ptr(), &mut leader) } < 0 {
            return Err(provider_unavailable("sd_session_get_leader failed"));
        }
        let mut vt = 0;
        if unsafe { (self.session_get_vt)(id_c.as_ptr(), &mut vt) } < 0 {
            return Err(provider_unavailable("sd_session_get_vt failed"));
        }
        let active = predicate(self.session_is_active, id_c, "active flag")?;
        let remote = predicate(self.session_is_remote, id_c, "remote flag")?;
        let session_type = self.session_string(self.session_get_type, id_c, "session type")?;
        let class = self.session_string(self.session_get_class, id_c, "session class")?;
        let state = self.session_string(self.session_get_state, id_c, "session state")?;
        let seat = self.session_optional_string(self.session_get_seat, id_c, "session seat")?;
        let graphical = matches!(session_type.as_str(), "x11" | "wayland");
        let user_class = class == "user";
        let state_active = state == "active";
        let on_console =
            active && !remote && graphical && user_class && state_active && !seat.is_empty();
        let login_complete = user_class && state_active;
        let group_id = group_for_uid(uid)?;
        Ok(NativeLoginSessionRow {
            uuid: session_uuid(id_text),
            session_id: session_numeric_id(id_text),
            security_session_id: u64::try_from(leader.max(0)).unwrap_or(0),
            audit_id: u64::from(vt),
            user_id: u64::from(uid),
            group_id: u64::from(group_id),
            username,
            display_name,
            on_console,
            login_complete,
        })
    }

    fn session_locked(&self, id: &[u8]) -> Result<bool, LoginSessionError> {
        let id = c_value(id, "login-session id")?;
        let mut locked = 0;
        // SAFETY: id is valid for the duration of the call.
        let status = unsafe { (self.session_get_locked_hint)(id.as_ptr(), &mut locked) };
        if status < 0 {
            return Err(provider_unavailable("sd_session_get_locked_hint failed"));
        }
        Ok(locked > 0)
    }

    fn session_string(
        &self,
        getter: SdSessionGetString,
        id: &CStr,
        field: &'static str,
    ) -> Result<String, LoginSessionError> {
        let mut value = null_mut();
        // SAFETY: getter follows sd-login allocated-string ownership.
        let status = unsafe { getter(id.as_ptr(), &mut value) };
        let bytes = owned_value(status, value, field)?;
        bytes_to_bounded_text(field, &bytes, 1, LOGIN_SESSION_USERNAME_MAX_BYTES)
    }

    fn session_optional_string(
        &self,
        getter: SdSessionGetString,
        id: &CStr,
        field: &'static str,
    ) -> Result<String, LoginSessionError> {
        let mut value = null_mut();
        // SAFETY: ENODATA means the session has no optional value.
        let status = unsafe { getter(id.as_ptr(), &mut value) };
        if status == -libc::ENODATA {
            return Ok(String::new());
        }
        let bytes = owned_value(status, value, field)?;
        bytes_to_bounded_text(field, &bytes, 0, LOGIN_SESSION_DISPLAY_NAME_MAX_BYTES)
    }
}

impl Drop for SystemdLogin {
    fn drop(&mut self) {
        // SAFETY: unique dlopen handle; all uses finished before Drop.
        unsafe { dlclose(self.handle) };
    }
}

pub(crate) fn inventory() -> Result<LoginSessionInventory, LoginSessionError> {
    let api = SystemdLogin::load()?;
    let (locked, rows) = api.inventory()?;
    finish_inventory(LoginSessionProvider::LinuxSdLogin, locked, rows)
}

pub(crate) fn lock_console() -> Result<(), LoginSessionError> {
    Err(LoginSessionError::new(
        LoginSessionErrorKind::Unsupported,
        "Linux login-session screen-lock delivery is unsupported; use the host screen locker",
    ))
}

unsafe fn load_symbol<T: Copy>(handle: *mut c_void, symbol: &CStr) -> Result<T, LoginSessionError> {
    // SAFETY: live dlopen handle and NUL-terminated symbol name.
    let pointer = unsafe { dlsym(handle, symbol.as_ptr()) };
    if pointer.is_null() {
        return Err(unsupported(
            "libsystemd lacks a required sd-login operation for login-session inventory",
        ));
    }
    debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
    // SAFETY: T matches the named libsystemd function pointer type.
    Ok(unsafe { std::mem::transmute_copy(&pointer) })
}

fn predicate(
    function: SdSessionPredicate,
    id: &CStr,
    field: &'static str,
) -> Result<bool, LoginSessionError> {
    // SAFETY: id is a bounded NUL-terminated session identifier.
    let result = unsafe { function(id.as_ptr()) };
    if result < 0 {
        Err(provider_unavailable(format!("{field} is unavailable")))
    } else {
        Ok(result > 0)
    }
}

fn c_value<'a>(value: &'a [u8], field: &'static str) -> Result<&'a CStr, LoginSessionError> {
    if value.is_empty() || value.len() > MAX_SESSION_VALUE_BYTES + 1 {
        return Err(shape(format!("{field} has an invalid native shape")));
    }
    CStr::from_bytes_with_nul(value)
        .map_err(|_| shape(format!("{field} has an invalid native shape")))
}

fn borrowed_value(pointer: *mut c_char, field: &'static str) -> Result<Vec<u8>, LoginSessionError> {
    if pointer.is_null() {
        return Err(shape(format!("{field} is missing")));
    }
    // SAFETY: libsystemd returns a NUL-terminated string for each inventory entry.
    let value = unsafe { CStr::from_ptr(pointer) }.to_bytes_with_nul();
    validate_value(value, field)?;
    Ok(value.to_vec())
}

fn owned_value(
    status: c_int,
    pointer: *mut c_char,
    field: &'static str,
) -> Result<Vec<u8>, LoginSessionError> {
    if status < 0 || pointer.is_null() {
        if !pointer.is_null() {
            // SAFETY: non-NULL sd-login output is malloc-allocated.
            unsafe { libc::free(pointer.cast()) };
        }
        return Err(provider_unavailable(format!("{field} is unavailable")));
    }
    let result = borrowed_value(pointer, field);
    // SAFETY: successful sd-login string outputs transfer ownership to the caller.
    unsafe { libc::free(pointer.cast()) };
    result
}

fn validate_value(value: &[u8], field: &'static str) -> Result<(), LoginSessionError> {
    if value.len() <= 1
        || value.len() > MAX_SESSION_VALUE_BYTES + 1
        || value[..value.len() - 1]
            .iter()
            .any(|byte| !byte.is_ascii_graphic())
    {
        return Err(shape(format!("{field} has an invalid native shape")));
    }
    Ok(())
}

fn bytes_to_bounded_text(
    field: &'static str,
    value: &[u8],
    minimum: usize,
    maximum: usize,
) -> Result<String, LoginSessionError> {
    let text = std::str::from_utf8(value)
        .map_err(|_| shape(format!("{field} is not valid UTF-8")))?
        .to_owned();
    if text.len() < minimum
        || text.len() > maximum
        || text
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    {
        return Err(shape(format!(
            "{field} must be {minimum}..={maximum} UTF-8 bytes without NUL or newline"
        )));
    }
    Ok(text)
}

fn group_for_uid(uid: libc::uid_t) -> Result<u32, LoginSessionError> {
    let mut buffer = [0_u8; 16_384];
    let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: scratch buffer and record outlive getpwuid_r.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            record.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Ok(0);
    }
    // SAFETY: getpwuid_r succeeded and result points at initialized record.
    let gid = unsafe { (*result).pw_gid };
    u32::try_from(gid).map_err(|_| shape("native group id exceeds the portable range"))
}

fn session_uuid(session_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"agenterm-platform/linux-login-session-uuid/v1\0");
    digest.update(session_id.as_bytes());
    let bytes = digest.finalize();
    format!(
        "{:08X}-{:04X}-4{:03X}-8{:03X}-{:012X}",
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]) & 0x0fff,
        u16::from_be_bytes([bytes[8], bytes[9]]) & 0x3fff,
        u128::from_be_bytes([
            0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16],
            bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
        ]) & 0xffffffffffff
    )
}

fn session_numeric_id(session_id: &str) -> u64 {
    if session_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return session_id.parse().unwrap_or(1).max(1);
    }
    let mut hash = 0_u64;
    for byte in session_id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(byte));
    }
    hash.max(1)
}

struct OwnedStringArray {
    values: *mut *mut c_char,
    count: usize,
}

impl Drop for OwnedStringArray {
    fn drop(&mut self) {
        if self.values.is_null() {
            return;
        }
        for index in 0..self.count {
            // SAFETY: values points to `count` entries owned by this guard.
            let pointer = unsafe { self.values.add(index).read() };
            if !pointer.is_null() {
                // SAFETY: each returned string is independently malloc-allocated.
                unsafe { libc::free(pointer.cast()) };
            }
        }
        // SAFETY: outer vector is also malloc-allocated by libsystemd.
        unsafe { libc::free(self.values.cast()) };
    }
}

fn unsupported(detail: impl Into<String>) -> LoginSessionError {
    LoginSessionError::new(LoginSessionErrorKind::Unsupported, detail)
}

fn provider_unavailable(detail: impl Into<String>) -> LoginSessionError {
    LoginSessionError::new(LoginSessionErrorKind::ProviderUnavailable, detail)
}

fn shape(detail: impl Into<String>) -> LoginSessionError {
    LoginSessionError::new(LoginSessionErrorKind::ProviderShape, detail)
}
