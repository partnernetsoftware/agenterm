//! Linux proof of the effective user's unique active local graphical session.

use std::{
    ffi::{CStr, c_char, c_int, c_void},
    fs,
    os::unix::fs::MetadataExt as _,
    path::Path,
    ptr::null_mut,
};

use crate::{
    CapabilityStatus,
    contract::current_target_binding::{CurrentTargetBindingError, CurrentTargetBindingErrorKind},
};

const LIBSYSTEMD_SONAME: &CStr = c"libsystemd.so.0";
const MAX_SESSION_VALUE_BYTES: usize = 256;
const MAX_USER_SESSIONS: usize = 128;

type SdPidGetSession = unsafe extern "C" fn(libc::pid_t, *mut *mut c_char) -> c_int;
type SdUidGetSessions = unsafe extern "C" fn(libc::uid_t, c_int, *mut *mut *mut c_char) -> c_int;
type SdSessionGetUid = unsafe extern "C" fn(*const c_char, *mut libc::uid_t) -> c_int;
type SdSessionPredicate = unsafe extern "C" fn(*const c_char) -> c_int;
type SdSessionGetString = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type SdSessionGetStartTime = unsafe extern "C" fn(*const c_char, *mut u64) -> c_int;
type SdSeatGetActive =
    unsafe extern "C" fn(*const c_char, *mut *mut c_char, *mut libc::uid_t) -> c_int;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 0;

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionSnapshot {
    id: Vec<u8>,
    uid: u32,
    active: bool,
    remote: bool,
    graphical: bool,
    user_class: bool,
    state_active: bool,
    seat: Vec<u8>,
    start_time_usec: u64,
}

impl SessionSnapshot {
    fn eligible(&self, effective_uid: u32) -> bool {
        self.uid == effective_uid
            && self.active
            && !self.remote
            && self.graphical
            && self.user_class
            && self.state_active
            && !self.seat.is_empty()
    }
}

pub(crate) struct NativeCurrentSessionFacts(Vec<u8>);

impl NativeCurrentSessionFacts {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

pub(crate) fn capability_status() -> CapabilityStatus {
    CapabilityStatus::Available
}

pub(crate) fn current_session_facts() -> Result<NativeCurrentSessionFacts, CurrentTargetBindingError>
{
    let effective_uid = effective_uid()?;
    if effective_uid == 0 {
        return Err(unsupported(
            "effective-user-root",
            "root cannot represent a local interactive desktop user",
        ));
    }

    let api = SystemdLogin::load()?;
    let first_pid_session = api.pid_session()?;
    let first_sessions = api.eligible_user_sessions(effective_uid)?;
    let selected = select_unique_session(effective_uid, &first_pid_session, &first_sessions)?;
    api.verify_seat_owner(selected)?;

    // sd-login is a live inventory rather than an atomic snapshot. Re-read all
    // identity-bearing facts before publishing so a login/logout or seat handoff
    // cannot be mistaken for one stable current target.
    let verified = api.session_snapshot(&selected.id)?;
    api.verify_seat_owner(&verified)?;
    let second_pid_session = api.pid_session()?;
    let second_sessions = api.eligible_user_sessions(effective_uid)?;
    let second = select_unique_session(effective_uid, &second_pid_session, &second_sessions)?;
    if selected != &verified || selected != second {
        return Err(native(
            "login-session-changed",
            "the active graphical login session changed during verification",
        ));
    }

    // These native values never cross the adapter boundary in plaintext. The
    // facade immediately combines them with the installation key to derive an
    // opaque session identity; no username, display, or bus address is queried.
    let mut facts = Vec::with_capacity(64 + selected.id.len() + selected.seat.len());
    push_bytes(&mut facts, 1, &effective_uid.to_le_bytes());
    push_bytes(&mut facts, 2, &selected.id);
    push_bytes(&mut facts, 3, &selected.seat);
    push_bytes(&mut facts, 4, &selected.start_time_usec.to_le_bytes());
    Ok(NativeCurrentSessionFacts(facts))
}

pub(crate) fn validate_private_key_file(path: &Path) -> Result<(), CurrentTargetBindingError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        permission(
            "install-key-metadata-unavailable",
            "installation key metadata could not be verified",
        )
    })?;
    let effective_uid = effective_uid().map_err(|_| {
        permission(
            "install-key-owner-unavailable",
            "installation key owner could not be verified",
        )
    })?;
    if !metadata.file_type().is_file()
        || metadata.uid() != effective_uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(permission(
            "install-key-permissions",
            "installation key must be a singly linked regular file owned by the current effective user with mode 0600",
        ));
    }
    Ok(())
}

fn effective_uid() -> Result<u32, CurrentTargetBindingError> {
    let identity = crate::user_identity::current_user_identity().map_err(|_| {
        native(
            "effective-user-unavailable",
            "the current effective user could not be determined",
        )
    })?;
    identity
        .posix_credentials()
        .map(|credentials| credentials.effective_user_id)
        .ok_or_else(|| {
            native(
                "effective-user-unavailable",
                "the current identity is not a POSIX user",
            )
        })
}

fn select_unique_session<'a>(
    effective_uid: u32,
    pid_session: &[u8],
    sessions: &'a [SessionSnapshot],
) -> Result<&'a SessionSnapshot, CurrentTargetBindingError> {
    let mut eligible = sessions
        .iter()
        .filter(|session| session.eligible(effective_uid));
    let selected = eligible.next().ok_or_else(|| {
        unsupported(
            "graphical-session-missing",
            "no active local graphical user session could be proven",
        )
    })?;
    if eligible.next().is_some() {
        return Err(native(
            "graphical-session-ambiguous",
            "more than one active local graphical user session was reported",
        ));
    }
    if selected.id != pid_session {
        return Err(unsupported(
            "process-session-mismatch",
            "the process does not belong to the unique active graphical user session",
        ));
    }
    Ok(selected)
}

struct SystemdLogin {
    handle: *mut c_void,
    pid_get_session: SdPidGetSession,
    uid_get_sessions: SdUidGetSessions,
    session_get_uid: SdSessionGetUid,
    session_is_active: SdSessionPredicate,
    session_is_remote: SdSessionPredicate,
    session_get_type: SdSessionGetString,
    session_get_class: SdSessionGetString,
    session_get_state: SdSessionGetString,
    session_get_seat: SdSessionGetString,
    session_get_start_time: SdSessionGetStartTime,
    seat_get_active: SdSeatGetActive,
}

impl SystemdLogin {
    fn load() -> Result<Self, CurrentTargetBindingError> {
        // SAFETY: the fixed NUL-terminated SONAME is valid for dlopen. The
        // returned handle is retained until after all copied function pointers.
        let handle = unsafe { dlopen(LIBSYSTEMD_SONAME.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if handle.is_null() {
            return Err(unsupported(
                "login-session-provider-unavailable",
                "the local login-session provider is unavailable",
            ));
        }
        let loaded = (|| {
            Ok(Self {
                handle,
                // SAFETY: each fixed symbol name is checked for NULL and cast
                // to the exact signature declared by libsystemd's sd-login API.
                pid_get_session: unsafe { load_symbol(handle, c"sd_pid_get_session")? },
                uid_get_sessions: unsafe { load_symbol(handle, c"sd_uid_get_sessions")? },
                session_get_uid: unsafe { load_symbol(handle, c"sd_session_get_uid")? },
                session_is_active: unsafe { load_symbol(handle, c"sd_session_is_active")? },
                session_is_remote: unsafe { load_symbol(handle, c"sd_session_is_remote")? },
                session_get_type: unsafe { load_symbol(handle, c"sd_session_get_type")? },
                session_get_class: unsafe { load_symbol(handle, c"sd_session_get_class")? },
                session_get_state: unsafe { load_symbol(handle, c"sd_session_get_state")? },
                session_get_seat: unsafe { load_symbol(handle, c"sd_session_get_seat")? },
                session_get_start_time: unsafe {
                    load_symbol(handle, c"sd_session_get_start_time")?
                },
                seat_get_active: unsafe { load_symbol(handle, c"sd_seat_get_active")? },
            })
        })();
        if loaded.is_err() {
            // SAFETY: handle came from the successful dlopen above and no
            // function pointer escapes when construction fails.
            unsafe { dlclose(handle) };
        }
        loaded
    }

    fn pid_session(&self) -> Result<Vec<u8>, CurrentTargetBindingError> {
        let mut value = null_mut();
        // SAFETY: sd_pid_get_session accepts pid 0 for the current process and
        // initializes an allocator-owned string on success.
        let status = unsafe { (self.pid_get_session)(0, &mut value) };
        owned_value(status, value, "process-session-unavailable")
    }

    fn eligible_user_sessions(
        &self,
        effective_uid: u32,
    ) -> Result<Vec<SessionSnapshot>, CurrentTargetBindingError> {
        let mut values = null_mut();
        // SAFETY: output is initialized by sd_uid_get_sessions. Passing zero
        // requests the complete bounded inventory so this adapter, rather than
        // a provider-side shortcut, applies every eligibility predicate.
        let count = unsafe { (self.uid_get_sessions)(effective_uid, 0, &mut values) };
        if count < 0 || (count > 0 && values.is_null()) {
            return Err(native(
                "login-session-inventory-unavailable",
                "login-session inventory could not be read",
            ));
        }
        let count = usize::try_from(count).map_err(|_| {
            native(
                "login-session-inventory-invalid",
                "login-session inventory returned an invalid count",
            )
        })?;
        let values = OwnedStringArray { values, count };
        if count > MAX_USER_SESSIONS {
            return Err(native(
                "login-session-inventory-overflow",
                "login-session inventory exceeded its fixed bound",
            ));
        }
        let mut sessions = Vec::with_capacity(count);
        for index in 0..count {
            // SAFETY: libsystemd returned an array containing `count` pointers.
            let pointer = unsafe { values.values.add(index).read() };
            let id = borrowed_value(pointer, "login-session-inventory-invalid")?;
            sessions.push(self.session_snapshot(&id)?);
        }
        Ok(sessions)
    }

    fn session_snapshot(&self, id: &[u8]) -> Result<SessionSnapshot, CurrentTargetBindingError> {
        let id = c_value(id, "login-session-id-invalid")?;
        let mut uid = 0;
        // SAFETY: id is a bounded NUL-terminated session identifier and uid is
        // writable for the duration of this call.
        if unsafe { (self.session_get_uid)(id.as_ptr(), &mut uid) } < 0 {
            return Err(native(
                "login-session-owner-unavailable",
                "login-session ownership could not be read",
            ));
        }
        let active = predicate(
            self.session_is_active,
            id,
            "login-session-active-unavailable",
        )?;
        let remote = predicate(
            self.session_is_remote,
            id,
            "login-session-remote-unavailable",
        )?;
        let session_type = self.session_string(self.session_get_type, id, "login-session-type")?;
        let class = self.session_string(self.session_get_class, id, "login-session-class")?;
        let state = self.session_string(self.session_get_state, id, "login-session-state")?;
        let seat = self.session_optional_string(self.session_get_seat, id, "login-session-seat")?;
        let mut start_time_usec = 0;
        // SAFETY: id and output storage remain valid for the call.
        if unsafe { (self.session_get_start_time)(id.as_ptr(), &mut start_time_usec) } < 0
            || start_time_usec == 0
        {
            return Err(native(
                "login-session-start-unavailable",
                "login-session start identity could not be read",
            ));
        }
        Ok(SessionSnapshot {
            id: id.to_bytes_with_nul().to_vec(),
            uid,
            active,
            remote,
            graphical: matches!(session_type.as_slice(), b"x11\0" | b"wayland\0"),
            user_class: class == b"user\0",
            state_active: state == b"active\0",
            seat,
            start_time_usec,
        })
    }

    fn session_string(
        &self,
        getter: SdSessionGetString,
        id: &CStr,
        code: &'static str,
    ) -> Result<Vec<u8>, CurrentTargetBindingError> {
        let mut value = null_mut();
        // SAFETY: id is valid and getter follows the sd-login allocated-string
        // convention represented by this exact function-pointer type.
        let status = unsafe { getter(id.as_ptr(), &mut value) };
        owned_value(status, value, code)
    }

    fn session_optional_string(
        &self,
        getter: SdSessionGetString,
        id: &CStr,
        code: &'static str,
    ) -> Result<Vec<u8>, CurrentTargetBindingError> {
        let mut value = null_mut();
        // SAFETY: same ownership contract as session_string. ENODATA means this
        // session has no value (normal for non-seat remote/background sessions),
        // which makes it ineligible rather than invalidating unrelated rows.
        let status = unsafe { getter(id.as_ptr(), &mut value) };
        if status == -libc::ENODATA {
            Ok(Vec::new())
        } else {
            owned_value(status, value, code)
        }
    }

    fn verify_seat_owner(
        &self,
        session: &SessionSnapshot,
    ) -> Result<(), CurrentTargetBindingError> {
        let seat = c_value(&session.seat, "login-session-seat-invalid")?;
        let mut active_session = null_mut();
        let mut active_uid = 0;
        // SAFETY: seat and both output slots remain valid for the call.
        let status =
            unsafe { (self.seat_get_active)(seat.as_ptr(), &mut active_session, &mut active_uid) };
        let active_session = owned_value(status, active_session, "active-seat-unavailable")?;
        if active_uid != session.uid || active_session != session.id {
            return Err(unsupported(
                "active-seat-mismatch",
                "the graphical session is not the active owner of its local seat",
            ));
        }
        Ok(())
    }
}

impl Drop for SystemdLogin {
    fn drop(&mut self) {
        // SAFETY: this is the unique retained handle returned by dlopen and all
        // function-pointer uses have completed before Drop runs.
        unsafe { dlclose(self.handle) };
    }
}

unsafe fn load_symbol<T: Copy>(
    handle: *mut c_void,
    symbol: &CStr,
) -> Result<T, CurrentTargetBindingError> {
    // SAFETY: caller owns a live dlopen handle and symbol is NUL-terminated.
    let pointer = unsafe { dlsym(handle, symbol.as_ptr()) };
    if pointer.is_null() {
        return Err(unsupported(
            "login-session-provider-incomplete",
            "the local login-session provider lacks a required operation",
        ));
    }
    debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
    // SAFETY: all callers instantiate T with the exact function-pointer type for
    // the named libsystemd operation, checked above to have pointer size.
    Ok(unsafe { std::mem::transmute_copy(&pointer) })
}

fn predicate(
    function: SdSessionPredicate,
    id: &CStr,
    code: &'static str,
) -> Result<bool, CurrentTargetBindingError> {
    // SAFETY: id is a valid bounded NUL-terminated session identifier.
    let result = unsafe { function(id.as_ptr()) };
    if result < 0 {
        Err(native(code, "a required login-session fact is unavailable"))
    } else {
        Ok(result > 0)
    }
}

fn c_value<'a>(value: &'a [u8], code: &'static str) -> Result<&'a CStr, CurrentTargetBindingError> {
    if value.is_empty() || value.len() > MAX_SESSION_VALUE_BYTES + 1 {
        return Err(native(
            code,
            "login-session provider returned an invalid value",
        ));
    }
    CStr::from_bytes_with_nul(value)
        .map_err(|_| native(code, "login-session provider returned an invalid value"))
}

fn borrowed_value(
    pointer: *mut c_char,
    code: &'static str,
) -> Result<Vec<u8>, CurrentTargetBindingError> {
    if pointer.is_null() {
        return Err(native(
            code,
            "login-session provider returned an invalid value",
        ));
    }
    // SAFETY: libsystemd promises a NUL-terminated string for each successful
    // inventory entry; the array retains it until explicitly freed below.
    let value = unsafe { CStr::from_ptr(pointer) }.to_bytes_with_nul();
    validate_value(value, code)?;
    Ok(value.to_vec())
}

fn owned_value(
    status: c_int,
    pointer: *mut c_char,
    code: &'static str,
) -> Result<Vec<u8>, CurrentTargetBindingError> {
    if status < 0 || pointer.is_null() {
        if !pointer.is_null() {
            // SAFETY: a non-NULL sd-login output is allocated with malloc.
            unsafe { libc::free(pointer.cast()) };
        }
        return Err(native(code, "a required login-session fact is unavailable"));
    }
    let result = borrowed_value(pointer, code);
    // SAFETY: successful sd-login string outputs are allocated with malloc and
    // ownership transfers to the caller.
    unsafe { libc::free(pointer.cast()) };
    result
}

fn validate_value(value: &[u8], code: &'static str) -> Result<(), CurrentTargetBindingError> {
    if value.len() <= 1
        || value.len() > MAX_SESSION_VALUE_BYTES + 1
        || value[..value.len() - 1]
            .iter()
            .any(|byte| !byte.is_ascii_graphic())
    {
        return Err(native(
            code,
            "login-session provider returned an invalid value",
        ));
    }
    Ok(())
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
            // SAFETY: values points to the `count` entries returned by
            // libsystemd, and this guard is their unique owner.
            let pointer = unsafe { self.values.add(index).read() };
            if !pointer.is_null() {
                // SAFETY: each returned string is independently malloc-allocated.
                unsafe { libc::free(pointer.cast()) };
            }
        }
        // SAFETY: the outer vector is also malloc-allocated by libsystemd.
        unsafe { libc::free(self.values.cast()) };
    }
}

fn push_bytes(output: &mut Vec<u8>, tag: u8, value: &[u8]) {
    output.push(tag);
    output.extend_from_slice(&(value.len() as u32).to_le_bytes());
    output.extend_from_slice(value);
}

fn unsupported(code: &'static str, message: &'static str) -> CurrentTargetBindingError {
    CurrentTargetBindingError::new(CurrentTargetBindingErrorKind::Unsupported, code, message)
}

fn native(code: &'static str, message: &'static str) -> CurrentTargetBindingError {
    CurrentTargetBindingError::new(CurrentTargetBindingErrorKind::Native, code, message)
}

fn permission(code: &'static str, message: &'static str) -> CurrentTargetBindingError {
    CurrentTargetBindingError::new(CurrentTargetBindingErrorKind::Permission, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::OpenOptions,
        os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    fn session(id: &[u8], uid: u32) -> SessionSnapshot {
        SessionSnapshot {
            id: [id, b"\0"].concat(),
            uid,
            active: true,
            remote: false,
            graphical: true,
            user_class: true,
            state_active: true,
            seat: b"seat0\0".to_vec(),
            start_time_usec: 123_456,
        }
    }

    #[test]
    fn selects_only_the_process_owned_unique_local_graphical_session() {
        let selected = session(b"7", 1000);
        assert_eq!(
            select_unique_session(1000, b"7\0", std::slice::from_ref(&selected)).unwrap(),
            &selected
        );

        let mismatch =
            select_unique_session(1000, b"8\0", std::slice::from_ref(&selected)).unwrap_err();
        assert_eq!(mismatch.code(), "process-session-mismatch");

        let ambiguous =
            select_unique_session(1000, b"7\0", &[selected, session(b"8", 1000)]).unwrap_err();
        assert_eq!(ambiguous.code(), "graphical-session-ambiguous");
    }

    #[test]
    fn rejects_root_remote_headless_inactive_and_greeter_sessions() {
        let baseline = session(b"7", 1000);
        for rejected in [
            SessionSnapshot {
                uid: 0,
                ..baseline.clone()
            },
            SessionSnapshot {
                remote: true,
                ..baseline.clone()
            },
            SessionSnapshot {
                graphical: false,
                ..baseline.clone()
            },
            SessionSnapshot {
                active: false,
                ..baseline.clone()
            },
            SessionSnapshot {
                state_active: false,
                ..baseline.clone()
            },
            SessionSnapshot {
                user_class: false,
                ..baseline.clone()
            },
        ] {
            let failure = select_unique_session(1000, b"7\0", &[rejected]).unwrap_err();
            assert_eq!(failure.code(), "graphical-session-missing");
        }
    }

    #[test]
    fn private_key_requires_exact_mode_owner_regular_file_and_single_link() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-platform-linux-binding-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join("key");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        validate_private_key_file(&path).unwrap();

        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            validate_private_key_file(&path).unwrap_err().code(),
            "install-key-permissions"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let link = root.join("link");
        fs::hard_link(&path, &link).unwrap();
        assert_eq!(
            validate_private_key_file(&path).unwrap_err().code(),
            "install-key-permissions"
        );
        fs::remove_file(link).unwrap();
        fs::remove_file(path).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn native_inventory_proves_a_session_or_fails_closed() {
        assert_eq!(capability_status(), CapabilityStatus::Available);
        match current_session_facts() {
            Ok(facts) => assert!(!facts.as_bytes().is_empty()),
            Err(error) => assert!(matches!(
                error.kind(),
                CurrentTargetBindingErrorKind::Unsupported | CurrentTargetBindingErrorKind::Native
            )),
        }
    }
}
