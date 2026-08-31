//! macOS managed Space inventory via SkyLight private read SPI.
//!
//! Names match the public SPI aliases (`SLS*` / `CGS*`). This crate does not
//! call Space *move* performers; `moveProvider.available` is a symbol probe.

#![cfg(target_os = "macos")]

use std::ffi::{CStr, c_void};
use std::ptr;

use libloading::{Library, Symbol};
use serde_json::{Value, json};

type CfTypeRef = *const c_void;
type CfArrayRef = *const c_void;
type CfDictionaryRef = *const c_void;
type CfStringRef = *const c_void;

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT64: i32 = 4;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CfTypeRef);
    fn CFArrayGetCount(array: CfArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CfArrayRef, idx: isize) -> CfTypeRef;
    fn CFDictionaryGetValue(dict: CfDictionaryRef, key: CfTypeRef) -> CfTypeRef;
    fn CFStringCreateWithCString(alloc: CfTypeRef, c_str: *const i8, encoding: u32) -> CfStringRef;
    fn CFStringGetCStringPtr(s: CfStringRef, encoding: u32) -> *const i8;
    fn CFStringGetCString(s: CfStringRef, buf: *mut i8, size: isize, encoding: u32) -> u8;
    fn CFNumberGetValue(number: CfTypeRef, the_type: i32, value_ptr: *mut c_void) -> u8;
    fn CFGetTypeID(cf: CfTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFArrayCreate(
        alloc: CfTypeRef,
        values: *const CfTypeRef,
        count: isize,
        callbacks: CfTypeRef,
    ) -> CfArrayRef;
    fn CFNumberCreate(alloc: CfTypeRef, the_type: i32, value_ptr: *const c_void) -> CfTypeRef;
}

type MainConnection = unsafe extern "C" fn() -> u32;
type CopyManaged = unsafe extern "C" fn(u32) -> CfArrayRef;
/// `SLSCopySpacesForWindows(cid, selector, windows) -> spaces`.
type CopySpacesForWindows = unsafe extern "C" fn(u32, i32, CfArrayRef) -> CfArrayRef;

/// `kCGSSpaceIncludesCurrent | kCGSSpaceIncludesOthers | kCGSSpaceIncludesUser`:
/// every Space a window is on, not just the one on screen now.
const SPACE_SELECTOR_ALL: i32 = 0x7;
const K_CF_NUMBER_SINT32: i32 = 3;

const SKYLIGHT: &str = "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";

#[derive(Debug)]
pub struct SpacesError {
    pub reason: String,
}

fn cfstr(name: &str) -> CfStringRef {
    let c = std::ffi::CString::new(name).expect("key");
    unsafe { CFStringCreateWithCString(ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
}

fn cf_string(value: CfTypeRef) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe {
        if CFGetTypeID(value) != CFStringGetTypeID() {
            return String::new();
        }
        let ptr = CFStringGetCStringPtr(value as CfStringRef, K_CF_STRING_ENCODING_UTF8);
        if !ptr.is_null() {
            return CStr::from_ptr(ptr).to_string_lossy().into_owned();
        }
        let mut buf = [0i8; 256];
        if CFStringGetCString(
            value as CfStringRef,
            buf.as_mut_ptr(),
            buf.len() as isize,
            K_CF_STRING_ENCODING_UTF8,
        ) != 0
        {
            CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
        } else {
            String::new()
        }
    }
}

fn cf_u64(value: CfTypeRef) -> Option<u64> {
    if value.is_null() {
        return None;
    }
    unsafe {
        if CFGetTypeID(value) != CFNumberGetTypeID() {
            return None;
        }
        let mut n: i64 = 0;
        if CFNumberGetValue(value, K_CF_NUMBER_SINT64, &mut n as *mut i64 as *mut c_void) != 0 {
            Some(n as u64)
        } else {
            None
        }
    }
}

fn dict_get(dict: CfDictionaryRef, key: &str) -> CfTypeRef {
    let k = cfstr(key);
    let v = unsafe { CFDictionaryGetValue(dict, k as CfTypeRef) };
    unsafe { CFRelease(k as CfTypeRef) };
    v
}

fn load_syms() -> Result<(Library, MainConnection, CopyManaged), SpacesError> {
    let lib = unsafe { Library::new(SKYLIGHT) }.map_err(|_| SpacesError {
        reason: "SkyLight.framework is not loadable".into(),
    })?;
    let main: MainConnection = unsafe {
        [
            b"SLSMainConnectionID\0".as_slice(),
            b"CGSMainConnectionID\0".as_slice(),
        ]
        .into_iter()
        .find_map(|name| {
            lib.get::<MainConnection>(name)
                .ok()
                .map(|s: Symbol<MainConnection>| *s)
        })
    }
    .ok_or_else(|| SpacesError {
        reason: "SLSMainConnectionID / CGSMainConnectionID missing".into(),
    })?;
    let copy: CopyManaged = unsafe {
        [
            b"SLSCopyManagedDisplaySpaces\0".as_slice(),
            b"CGSCopyManagedDisplaySpaces\0".as_slice(),
        ]
        .into_iter()
        .find_map(|name| {
            lib.get::<CopyManaged>(name)
                .ok()
                .map(|s: Symbol<CopyManaged>| *s)
        })
    }
    .ok_or_else(|| SpacesError {
        reason: "SLSCopyManagedDisplaySpaces / CGSCopyManagedDisplaySpaces missing".into(),
    })?;
    Ok((lib, main, copy))
}

/// Which managed Spaces one window sits on.
///
/// This is the attribution the Space *inventory* cannot give: the
/// inventory says which Spaces exist, and this says where a given window
/// lives among them -- a window on another Space is present but not on
/// screen, which is a different thing from minimized or closed.
///
/// `Ok(None)` means the SPI for this question is missing on the host,
/// which is separate from a window that is on no Space at all
/// (`Ok(Some(vec![]))`). Read-only: nothing is moved.
pub fn spaces_for_window(window: isize) -> Result<Option<Vec<u64>>, SpacesError> {
    let Ok(id) = u32::try_from(window) else {
        return Ok(Some(Vec::new()));
    };
    let lib = unsafe { Library::new(SKYLIGHT) }.map_err(|error| SpacesError {
        reason: format!("SkyLight could not be opened: {error}"),
    })?;
    let main: MainConnection = unsafe {
        [
            b"SLSMainConnectionID\0".as_slice(),
            b"CGSMainConnectionID\0".as_slice(),
        ]
        .into_iter()
        .find_map(|name| {
            lib.get::<MainConnection>(name)
                .ok()
                .map(|s: Symbol<MainConnection>| *s)
        })
    }
    .ok_or_else(|| SpacesError {
        reason: "SLSMainConnectionID / CGSMainConnectionID missing".into(),
    })?;
    let copy_spaces: Option<CopySpacesForWindows> = unsafe {
        [
            b"SLSCopySpacesForWindows\0".as_slice(),
            b"CGSCopySpacesForWindows\0".as_slice(),
        ]
        .into_iter()
        .find_map(|name| {
            lib.get::<CopySpacesForWindows>(name)
                .ok()
                .map(|s: Symbol<CopySpacesForWindows>| *s)
        })
    };
    let Some(copy_spaces) = copy_spaces else {
        drop(lib);
        return Ok(None);
    };
    let cid = unsafe { main() };
    let spaces = unsafe {
        let number = CFNumberCreate(
            ptr::null(),
            K_CF_NUMBER_SINT32,
            &id as *const u32 as *const c_void,
        );
        if number.is_null() {
            drop(lib);
            return Err(SpacesError {
                reason: "CFNumberCreate returned null for a window id".into(),
            });
        }
        let values = [number];
        let array = CFArrayCreate(ptr::null(), values.as_ptr(), 1, ptr::null());
        CFRelease(number);
        if array.is_null() {
            drop(lib);
            return Err(SpacesError {
                reason: "CFArrayCreate returned null for the window list".into(),
            });
        }
        let spaces = copy_spaces(cid, SPACE_SELECTOR_ALL, array);
        CFRelease(array as CfTypeRef);
        spaces
    };
    if spaces.is_null() {
        drop(lib);
        return Ok(Some(Vec::new()));
    }
    let mut ids = Vec::new();
    unsafe {
        if CFGetTypeID(spaces as CfTypeRef) == CFArrayGetTypeID() {
            let count = CFArrayGetCount(spaces).min(32);
            for index in 0..count {
                if let Some(value) = cf_u64(CFArrayGetValueAtIndex(spaces, index)) {
                    ids.push(value);
                }
            }
        }
        CFRelease(spaces as CfTypeRef);
    }
    drop(lib);
    Ok(Some(ids))
}

/// Bounded managed-display Space inventory. Does not move windows.
pub fn inventory() -> Result<Value, SpacesError> {
    let (lib, main, copy) = load_syms()?;
    let cid = unsafe { main() };
    let raw = unsafe { copy(cid) };
    if raw.is_null() {
        drop(lib);
        return Ok(json!({
            "provider": "unavailable",
            "displays": [],
            "visitedDisplays": 0,
            "visitedSpaces": 0,
            "truncated": false,
            "moveProvider": {
                "available": false,
                "provider": "none",
                "reason": "managed Space inventory returned null"
            }
        }));
    }
    let result = parse_inventory(raw);
    unsafe { CFRelease(raw as CfTypeRef) };
    drop(lib);
    result
}

fn parse_inventory(raw: CfArrayRef) -> Result<Value, SpacesError> {
    unsafe {
        if CFGetTypeID(raw as CfTypeRef) != CFArrayGetTypeID() {
            return Err(SpacesError {
                reason: "managed Space inventory is not a CFArray".into(),
            });
        }
        let count = CFArrayGetCount(raw);
        let truncated = count > 32;
        let mut displays = Vec::new();
        let mut visited_spaces = 0usize;
        let n = count.min(32);
        for i in 0..n {
            let item = CFArrayGetValueAtIndex(raw, i);
            if item.is_null() || CFGetTypeID(item) != CFDictionaryGetTypeID() {
                continue;
            }
            let dict = item as CfDictionaryRef;
            let display_id = cf_string(dict_get(dict, "Display Identifier"));
            let current = dict_get(dict, "Current Space");
            let current_id =
                if !current.is_null() && CFGetTypeID(current) == CFDictionaryGetTypeID() {
                    cf_u64(dict_get(current as CfDictionaryRef, "ManagedSpaceID"))
                        .or_else(|| cf_u64(dict_get(current as CfDictionaryRef, "id64")))
                } else {
                    None
                };
            let spaces_ref = dict_get(dict, "Spaces");
            let mut spaces = Vec::new();
            if !spaces_ref.is_null() && CFGetTypeID(spaces_ref) == CFArrayGetTypeID() {
                let sc = CFArrayGetCount(spaces_ref as CfArrayRef);
                visited_spaces += sc as usize;
                let sn = sc.min(128);
                if sc > 128 {
                    // truncated at the inventory level
                }
                for j in 0..sn {
                    let space = CFArrayGetValueAtIndex(spaces_ref as CfArrayRef, j);
                    if space.is_null() || CFGetTypeID(space) != CFDictionaryGetTypeID() {
                        continue;
                    }
                    let sd = space as CfDictionaryRef;
                    let Some(id) = cf_u64(dict_get(sd, "ManagedSpaceID"))
                        .or_else(|| cf_u64(dict_get(sd, "id64")))
                    else {
                        continue;
                    };
                    let uuid = cf_string(dict_get(sd, "uuid"));
                    let kind = cf_u64(dict_get(sd, "type")).map(|n| n as i64).unwrap_or(-1);
                    spaces.push(json!({
                        "id": id.to_string(),
                        "ordinal": j + 1,
                        "uuid": if uuid.is_empty() { Value::Null } else { json!(uuid) },
                        "type": kind,
                        "current": current_id == Some(id),
                    }));
                }
            }
            displays.push(json!({
                "displayIdentifier": display_id,
                "currentSpaceId": current_id.map(|id| json!(id.to_string())).unwrap_or(Value::Null),
                "spaces": spaces,
            }));
        }
        Ok(json!({
            "provider": "skylight-private-read",
            "displays": displays,
            "visitedDisplays": count,
            "visitedSpaces": visited_spaces,
            "truncated": truncated || visited_spaces > 128,
            "moveProvider": {
                "available": false,
                "provider": "none",
                "reason": "cu does not map Space move; inventory is read-only"
            }
        }))
    }
}
