//! Measurement-only dynamic provider for the ACU embedder delivery court.
//!
//! The provider owns no command schema. It passes the input to the existing
//! `agenterm-cu` adapter and serializes the resulting existing `CuReply`.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice, str,
    sync::atomic::{AtomicBool, Ordering},
};

const ABI_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

pub const STATUS_OK: i32 = 0;
pub const STATUS_INVALID_POINTER: i32 = 1;
pub const STATUS_REQUEST_TOO_LARGE: i32 = 2;
pub const STATUS_REQUEST_NOT_UTF8: i32 = 3;
pub const STATUS_REPLY_TOO_LARGE: i32 = 4;
pub const STATUS_SERIALIZE_FAILED: i32 = 5;
pub const STATUS_PROVIDER_PANICKED: i32 = 6;

static PROVIDER_FAILED: AtomicBool = AtomicBool::new(false);

#[unsafe(no_mangle)]
pub extern "C" fn acu_provider_abi_version() -> u32 {
    ABI_VERSION
}

/// Execute once and copy the complete opaque JSON reply into caller storage.
///
/// Status zero describes transport success, including a legal `CuReply` whose
/// `ok` field is false. A nonzero value is a raw provider-boundary failure.
///
/// # Safety
///
/// `request` must name `request_len` readable bytes unless the length is zero.
/// `reply` must name `reply_capacity` writable bytes unless the capacity is
/// zero. `reply_len` must be writable for one `usize`. All three regions must
/// remain valid for the complete synchronous call; request and reply may
/// overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn acu_provider_call(
    request: *const u8,
    request_len: usize,
    reply: *mut u8,
    reply_capacity: usize,
    reply_len: *mut usize,
) -> i32 {
    if PROVIDER_FAILED.load(Ordering::Acquire) {
        return STATUS_PROVIDER_PANICKED;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the exported ABI passes the same call-scoped pointer contract
        // through to the contained implementation.
        unsafe { call_inner(request, request_len, reply, reply_capacity, reply_len) }
    }));
    match result {
        Ok(status) => status,
        Err(_) => {
            PROVIDER_FAILED.store(true, Ordering::Release);
            STATUS_PROVIDER_PANICKED
        }
    }
}

unsafe fn call_inner(
    request: *const u8,
    request_len: usize,
    reply: *mut u8,
    reply_capacity: usize,
    reply_len: *mut usize,
) -> i32 {
    if reply_len.is_null()
        || (request_len != 0 && request.is_null())
        || (reply_capacity != 0 && reply.is_null())
    {
        return STATUS_INVALID_POINTER;
    }
    // SAFETY: reply_len was checked non-null and is call-scoped.
    unsafe { ptr::write(reply_len, 0) };
    if request_len > MAX_REQUEST_BYTES {
        return STATUS_REQUEST_TOO_LARGE;
    }
    if reply_capacity > MAX_REPLY_BYTES {
        return STATUS_REPLY_TOO_LARGE;
    }
    let request = if request_len == 0 {
        &[]
    } else {
        // SAFETY: the C ABI requires request to name request_len readable bytes.
        unsafe { slice::from_raw_parts(request, request_len) }
    };
    let Ok(request) = str::from_utf8(request) else {
        return STATUS_REQUEST_NOT_UTF8;
    };
    if cfg!(debug_assertions) && request == "__ACU_DELIVERY_TEST_PANIC__" {
        panic!("measurement-only panic containment probe");
    }
    let response = agenterm_cu::embedder::execute_json_from_environment(request);
    let Ok(encoded) = serde_json::to_vec(&response) else {
        return STATUS_SERIALIZE_FAILED;
    };
    if encoded.len() > MAX_REPLY_BYTES || encoded.len() > reply_capacity {
        return STATUS_REPLY_TOO_LARGE;
    }
    // SAFETY: the C ABI requires reply to name reply_capacity writable bytes;
    // the length comparison above proves the copy is bounded. `ptr::copy`
    // remains valid even when a caller deliberately aliases request/reply.
    unsafe { ptr::copy(encoded.as_ptr(), reply, encoded.len()) };
    // SAFETY: reply_len remains the same checked call-scoped output pointer.
    unsafe { ptr::write(reply_len, encoded.len()) };
    STATUS_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_is_contained_and_permanently_latched() {
        let mut reply = [0_u8; 1024];
        let mut reply_len = 0_usize;
        let panic_request = b"__ACU_DELIVERY_TEST_PANIC__";
        // SAFETY: all pointers refer to live arrays for their declared lengths.
        let first = unsafe {
            acu_provider_call(
                panic_request.as_ptr(),
                panic_request.len(),
                reply.as_mut_ptr(),
                reply.len(),
                &mut reply_len,
            )
        };
        assert_eq!(first, STATUS_PROVIDER_PANICKED);

        let valid_request = br#"{"verb":"capabilities","target":"current"}"#;
        // SAFETY: all pointers refer to live arrays for their declared lengths.
        let second = unsafe {
            acu_provider_call(
                valid_request.as_ptr(),
                valid_request.len(),
                reply.as_mut_ptr(),
                reply.len(),
                &mut reply_len,
            )
        };
        assert_eq!(second, STATUS_PROVIDER_PANICKED);
        assert_eq!(reply_len, 0);
    }
}
