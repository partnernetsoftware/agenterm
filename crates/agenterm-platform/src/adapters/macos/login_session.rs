//! macOS login-session inventory from the IORegistry root.
//!
//! IOKit's registry accessors are public API. The `IOConsoleUsers` and
//! `IOConsoleLocked` property shapes are operating-system implementation
//! details, so every type and bound is checked and any drift fails closed.

#![cfg(target_os = "macos")]

use std::{ffi::c_void, os::raw::c_char};

use crate::login_session::{
    LOGIN_SESSION_DISPLAY_NAME_MAX_BYTES, LOGIN_SESSION_MAX_ROWS, LOGIN_SESSION_USERNAME_MAX_BYTES,
    LoginSessionError, LoginSessionErrorKind, LoginSessionInventory, NativeLoginSessionRow,
    finish_inventory,
};

type CfTypeRef = *const c_void;
type CfStringRef = *const c_void;
type CfArrayRef = *const c_void;
type CfDictionaryRef = *const c_void;
type CfIndex = isize;
type CfTypeId = usize;
type IoRegistryEntry = u32;

#[repr(C)]
#[derive(Clone, Copy)]
struct CfRange {
    location: CfIndex,
    length: CfIndex,
}

#[allow(clippy::duplicated_attributes)]
#[link(name = "IOKit", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn IORegistryGetRootEntry(main_port: u32) -> IoRegistryEntry;
    fn IORegistryEntryCreateCFProperty(
        entry: IoRegistryEntry,
        key: CfStringRef,
        allocator: CfTypeRef,
        options: u32,
    ) -> CfTypeRef;
    fn IOObjectRelease(object: IoRegistryEntry) -> i32;

    fn CFRelease(value: CfTypeRef);
    fn CFGetTypeID(value: CfTypeRef) -> CfTypeId;
    fn CFStringCreateWithCString(
        allocator: CfTypeRef,
        bytes: *const c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFStringGetTypeID() -> CfTypeId;
    fn CFStringGetLength(value: CfStringRef) -> CfIndex;
    fn CFStringGetBytes(
        value: CfStringRef,
        range: CfRange,
        encoding: u32,
        loss_byte: u8,
        external_representation: bool,
        buffer: *mut u8,
        maximum_length: CfIndex,
        used_length: *mut CfIndex,
    ) -> CfIndex;
    fn CFArrayGetTypeID() -> CfTypeId;
    fn CFArrayGetCount(array: CfArrayRef) -> CfIndex;
    fn CFArrayGetValueAtIndex(array: CfArrayRef, index: CfIndex) -> CfTypeRef;
    fn CFDictionaryGetTypeID() -> CfTypeId;
    fn CFDictionaryGetValueIfPresent(
        dictionary: CfDictionaryRef,
        key: CfTypeRef,
        value: *mut CfTypeRef,
    ) -> bool;
    fn CFNumberGetTypeID() -> CfTypeId;
    fn CFNumberGetValue(number: CfTypeRef, kind: CfIndex, value: *mut c_void) -> bool;
    fn CFBooleanGetTypeID() -> CfTypeId;
    fn CFBooleanGetValue(boolean: CfTypeRef) -> bool;

    fn CGPreflightPostEventAccess() -> bool;
}

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT64_TYPE: CfIndex = 4;
const IO_MAIN_PORT_DEFAULT: u32 = 0;

struct OwnedCf(CfTypeRef);

impl OwnedCf {
    fn new(value: CfTypeRef, detail: &'static str) -> Result<Self, LoginSessionError> {
        if value.is_null() {
            Err(provider_unavailable(detail))
        } else {
            Ok(Self(value))
        }
    }

    fn as_ptr(&self) -> CfTypeRef {
        self.0
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        // SAFETY: `OwnedCf` is constructed only for a non-null create/copy
        // result and owns exactly that retain.
        unsafe { CFRelease(self.0) };
    }
}

struct OwnedIoObject(IoRegistryEntry);

impl Drop for OwnedIoObject {
    fn drop(&mut self) {
        // SAFETY: the nonzero registry entry was returned with one owned send
        // right by `IORegistryGetRootEntry`.
        let _ = unsafe { IOObjectRelease(self.0) };
    }
}

pub(crate) fn inventory() -> Result<LoginSessionInventory, LoginSessionError> {
    // SAFETY: the default main-port value is the documented null Mach port;
    // the return is checked before ownership is established.
    let root = unsafe { IORegistryGetRootEntry(IO_MAIN_PORT_DEFAULT) };
    if root == 0 {
        return Err(provider_unavailable(
            "IORegistryGetRootEntry returned no root entry",
        ));
    }
    let root = OwnedIoObject(root);
    let users = property(root.0, c"IOConsoleUsers")?;
    let locked = property(root.0, c"IOConsoleLocked")?;
    let locked = cf_boolean(locked.as_ptr(), "IOConsoleLocked")?;
    let rows = parse_users(users.as_ptr())?;
    finish_inventory(locked, rows)
}

pub(crate) fn lock_console() -> Result<(), LoginSessionError> {
    // Preflight is observational and never opens System Settings or prompts.
    // SAFETY: `CGPreflightPostEventAccess` has no arguments or owned result.
    if !unsafe { CGPreflightPostEventAccess() } {
        return Err(LoginSessionError::new(
            LoginSessionErrorKind::InputPermissionDenied,
            "macOS has not granted permission to post the lock-screen chord",
        ));
    }
    crate::input_inject::send_keys("ctrl+cmd+q").map_err(|error| {
        LoginSessionError::new(
            LoginSessionErrorKind::DeliveryFailed,
            format!("lock-screen chord delivery failed: {error:?}"),
        )
    })
}

fn property(
    entry: IoRegistryEntry,
    key: &'static std::ffi::CStr,
) -> Result<OwnedCf, LoginSessionError> {
    // SAFETY: the C string is NUL terminated; the returned CFString is owned.
    let key = unsafe {
        CFStringCreateWithCString(std::ptr::null(), key.as_ptr(), K_CF_STRING_ENCODING_UTF8)
    };
    let key = OwnedCf::new(key, "could not allocate an IORegistry property key")?;
    // SAFETY: `entry` remains owned for this call and `key` is a live
    // CFString. A non-null result follows the Create ownership rule.
    let value =
        unsafe { IORegistryEntryCreateCFProperty(entry, key.as_ptr(), std::ptr::null(), 0) };
    OwnedCf::new(
        value,
        "required login-session IORegistry property is absent",
    )
}

fn parse_users(value: CfTypeRef) -> Result<Vec<NativeLoginSessionRow>, LoginSessionError> {
    require_type(value, unsafe { CFArrayGetTypeID() }, "IOConsoleUsers array")?;
    // SAFETY: type was checked as CFArray.
    let count = unsafe { CFArrayGetCount(value) };
    if count < 0 || usize::try_from(count).map_or(true, |count| count > LOGIN_SESSION_MAX_ROWS) {
        return Err(shape("IOConsoleUsers count is outside its bound"));
    }
    let mut rows = Vec::new();
    rows.try_reserve(count as usize)
        .map_err(|_| shape("IOConsoleUsers allocation failed"))?;
    for index in 0..count {
        // SAFETY: the index is within the just-read CFArray count.
        let dictionary = unsafe { CFArrayGetValueAtIndex(value, index) };
        require_type(
            dictionary,
            unsafe { CFDictionaryGetTypeID() },
            "IOConsoleUsers row",
        )?;
        rows.push(parse_row(dictionary)?);
    }
    Ok(rows)
}

fn parse_row(dictionary: CfTypeRef) -> Result<NativeLoginSessionRow, LoginSessionError> {
    Ok(NativeLoginSessionRow {
        uuid: dictionary_string(
            dictionary,
            c"CGSSessionUniqueSessionUUID",
            36,
            "session UUID",
        )?,
        session_id: dictionary_number(dictionary, c"kCGSSessionIDKey", "session id")?,
        security_session_id: dictionary_number(
            dictionary,
            c"kSCSecuritySessionID",
            "security session id",
        )?,
        audit_id: dictionary_number(dictionary, c"kCGSSessionAuditIDKey", "audit id")?,
        user_id: dictionary_number(dictionary, c"kCGSSessionUserIDKey", "user id")?,
        group_id: dictionary_number(dictionary, c"kCGSSessionGroupIDKey", "group id")?,
        username: dictionary_string(
            dictionary,
            c"kCGSSessionUserNameKey",
            LOGIN_SESSION_USERNAME_MAX_BYTES,
            "username",
        )?,
        display_name: dictionary_string(
            dictionary,
            c"kCGSessionLongUserNameKey",
            LOGIN_SESSION_DISPLAY_NAME_MAX_BYTES,
            "display name",
        )?,
        on_console: dictionary_boolean(dictionary, c"kCGSSessionOnConsoleKey", "on-console flag")?,
        login_complete: dictionary_boolean(
            dictionary,
            c"kCGSessionLoginDoneKey",
            "login-complete flag",
        )?,
    })
}

fn dictionary_value(
    dictionary: CfTypeRef,
    key: &'static std::ffi::CStr,
    field: &'static str,
) -> Result<CfTypeRef, LoginSessionError> {
    // SAFETY: the static C string is valid and the result is checked.
    let key = unsafe {
        CFStringCreateWithCString(std::ptr::null(), key.as_ptr(), K_CF_STRING_ENCODING_UTF8)
    };
    let key = OwnedCf::new(key, "could not allocate an IOConsoleUsers key")?;
    let mut value = std::ptr::null();
    // SAFETY: `dictionary` was checked as CFDictionary and `key` is a live
    // CFString. The borrowed result stays alive with the dictionary.
    let present = unsafe { CFDictionaryGetValueIfPresent(dictionary, key.as_ptr(), &mut value) };
    if !present || value.is_null() {
        return Err(shape(format!("IOConsoleUsers row is missing {field}")));
    }
    Ok(value)
}

fn dictionary_string(
    dictionary: CfTypeRef,
    key: &'static std::ffi::CStr,
    maximum_bytes: usize,
    field: &'static str,
) -> Result<String, LoginSessionError> {
    cf_string(
        dictionary_value(dictionary, key, field)?,
        maximum_bytes,
        field,
    )
}

fn cf_string(
    value: CfTypeRef,
    maximum_bytes: usize,
    field: &'static str,
) -> Result<String, LoginSessionError> {
    require_type(value, unsafe { CFStringGetTypeID() }, field)?;
    // SAFETY: type was checked as CFString.
    let length = unsafe { CFStringGetLength(value) };
    if length < 0 || usize::try_from(length).map_or(true, |length| length > maximum_bytes) {
        return Err(shape(format!("{field} exceeds its character bound")));
    }
    let mut bytes = vec![0_u8; maximum_bytes];
    let mut used = 0;
    // SAFETY: the range covers the checked CFString length and the writable
    // buffer is exactly `maximum_bytes` long. A zero loss byte forbids lossy
    // conversion.
    let converted = unsafe {
        CFStringGetBytes(
            value,
            CfRange {
                location: 0,
                length,
            },
            K_CF_STRING_ENCODING_UTF8,
            0,
            false,
            bytes.as_mut_ptr(),
            maximum_bytes as CfIndex,
            &mut used,
        )
    };
    if converted != length
        || used < 0
        || usize::try_from(used).map_or(true, |used| used > maximum_bytes)
    {
        return Err(shape(format!("{field} is not bounded UTF-8")));
    }
    bytes.truncate(used as usize);
    String::from_utf8(bytes).map_err(|_| shape(format!("{field} is not valid UTF-8")))
}

fn dictionary_number(
    dictionary: CfTypeRef,
    key: &'static std::ffi::CStr,
    field: &'static str,
) -> Result<u64, LoginSessionError> {
    let value = dictionary_value(dictionary, key, field)?;
    require_type(value, unsafe { CFNumberGetTypeID() }, field)?;
    let mut number = 0_i64;
    // SAFETY: type was checked as CFNumber and `number` is writable.
    if !unsafe { CFNumberGetValue(value, K_CF_NUMBER_SINT64_TYPE, (&raw mut number).cast()) }
        || number < 0
    {
        return Err(shape(format!("{field} is not a non-negative integer")));
    }
    Ok(number as u64)
}

fn dictionary_boolean(
    dictionary: CfTypeRef,
    key: &'static std::ffi::CStr,
    field: &'static str,
) -> Result<bool, LoginSessionError> {
    cf_boolean(dictionary_value(dictionary, key, field)?, field)
}

fn cf_boolean(value: CfTypeRef, field: &'static str) -> Result<bool, LoginSessionError> {
    require_type(value, unsafe { CFBooleanGetTypeID() }, field)?;
    // SAFETY: type was checked as CFBoolean.
    Ok(unsafe { CFBooleanGetValue(value) })
}

fn require_type(
    value: CfTypeRef,
    expected: CfTypeId,
    field: &'static str,
) -> Result<(), LoginSessionError> {
    // SAFETY: callers reject null values before this helper and CoreFoundation
    // accepts every live CF object in `CFGetTypeID`.
    if value.is_null() || unsafe { CFGetTypeID(value) } != expected {
        return Err(shape(format!("{field} has an unexpected native type")));
    }
    Ok(())
}

fn provider_unavailable(detail: impl Into<String>) -> LoginSessionError {
    LoginSessionError::new(LoginSessionErrorKind::ProviderUnavailable, detail)
}

fn shape(detail: impl Into<String>) -> LoginSessionError {
    LoginSessionError::new(LoginSessionErrorKind::ProviderShape, detail)
}
