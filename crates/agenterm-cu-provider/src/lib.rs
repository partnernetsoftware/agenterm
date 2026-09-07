//! Versioned dynamic provider for the qjs `agenterm:acu` embedder.
//!
//! This crate owns only the native delivery seam. The request is opaque JSON
//! passed to `agenterm-cu`, which remains the sole owner of `Command`,
//! `Executor`, authorization, effects, receipts, and `CuReply`.

#[cfg(panic = "abort")]
compile_error!(
    "agenterm-cu-provider must be built with panic=unwind: use --profile abi-release (or abi-dev)"
);

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice, str,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// Native provider ABI implemented by this artifact.
pub const ABI_VERSION: u32 = 1;
/// Maximum opaque command JSON accepted at the native boundary.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// Maximum encoded `CuReply` accepted at the native boundary.
pub const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

/// The call completed and the reply buffer contains one complete `CuReply`.
pub const STATUS_OK: i32 = 0;
/// A required pointer was null for its nonzero extent.
pub const STATUS_INVALID_POINTER: i32 = 1;
/// The opaque request exceeded [`MAX_REQUEST_BYTES`].
pub const STATUS_REQUEST_TOO_LARGE: i32 = 2;
/// The opaque request was not valid UTF-8.
pub const STATUS_REQUEST_NOT_UTF8: i32 = 3;
/// The caller buffer or the provider's encoded reply violated the reply bound.
pub const STATUS_REPLY_TOO_LARGE: i32 = 4;
/// The existing `CuReply` could not be encoded as JSON.
pub const STATUS_SERIALIZE_FAILED: i32 = 5;
/// A provider panic was contained; this loaded provider is permanently failed.
pub const STATUS_PROVIDER_PANICKED: i32 = 6;

static PROVIDER_FAILED: AtomicBool = AtomicBool::new(false);
// The public ABI is synchronous and process-global. Serialize the complete
// failed-latch check and execution so a second caller can never cross a panic
// before the first caller publishes the permanent failure state.
static PROVIDER_CALL_LOCK: Mutex<()> = Mutex::new(());

/// Return the native ABI version without executing ACU product code.
#[unsafe(no_mangle)]
pub extern "C" fn agenterm_cu_provider_abi_version() -> u32 {
    ABI_VERSION
}

/// Execute one opaque command and copy one complete opaque `CuReply` JSON.
///
/// `STATUS_OK` describes boundary success, including a legal `CuReply` whose
/// `ok` field is false. Every nonzero status is a raw provider-boundary failure
/// and leaves `reply_len` as zero. The command is executed at most once; a
/// short reply buffer never causes an automatic retry.
///
/// # Safety
///
/// `request` must name `request_len` readable bytes unless the length is zero.
/// `reply` must name `reply_capacity` writable bytes unless the capacity is
/// zero. `reply_len` must be writable for one `usize`. All regions must remain
/// valid for the complete synchronous call. Request and reply may overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agenterm_cu_provider_call(
    request: *const u8,
    request_len: usize,
    reply: *mut u8,
    reply_capacity: usize,
    reply_len: *mut usize,
) -> i32 {
    let _call_guard = PROVIDER_CALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if PROVIDER_FAILED.load(Ordering::Acquire) {
        if !reply_len.is_null() {
            // SAFETY: the caller promised a writable non-null length pointer.
            unsafe { ptr::write(reply_len, 0) };
        }
        return STATUS_PROVIDER_PANICKED;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the exported ABI passes its documented call-scoped pointer
        // contract unchanged into the contained implementation.
        unsafe { call_inner(request, request_len, reply, reply_capacity, reply_len) }
    }));
    match result {
        Ok(status) => status,
        Err(_) => {
            PROVIDER_FAILED.store(true, Ordering::Release);
            if !reply_len.is_null() {
                // SAFETY: the caller promised reply_len is writable whenever
                // it is non-null. This prevents stale length after a panic.
                unsafe { ptr::write(reply_len, 0) };
            }
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
    if reply_len.is_null() {
        return STATUS_INVALID_POINTER;
    }
    // SAFETY: reply_len was checked non-null and is call-scoped.
    unsafe { ptr::write(reply_len, 0) };
    if (request_len != 0 && request.is_null()) || (reply_capacity != 0 && reply.is_null()) {
        return STATUS_INVALID_POINTER;
    }
    if request_len > MAX_REQUEST_BYTES {
        return STATUS_REQUEST_TOO_LARGE;
    }
    if reply_capacity > MAX_REPLY_BYTES {
        return STATUS_REPLY_TOO_LARGE;
    }
    let request = if request_len == 0 {
        &[]
    } else {
        // SAFETY: the ABI requires request to name request_len readable bytes.
        unsafe { slice::from_raw_parts(request, request_len) }
    };
    let Ok(request) = str::from_utf8(request) else {
        return STATUS_REQUEST_NOT_UTF8;
    };
    #[cfg(test)]
    if request == "__AGENTERM_CU_PROVIDER_TEST_PANIC__" {
        panic!("test-only provider panic");
    }

    let response = agenterm_cu::embedder::execute_json_from_environment(request);
    let Ok(encoded) = serde_json::to_vec(&response) else {
        return STATUS_SERIALIZE_FAILED;
    };
    if encoded.len() > MAX_REPLY_BYTES || encoded.len() > reply_capacity {
        return STATUS_REPLY_TOO_LARGE;
    }
    // SAFETY: reply names at least encoded.len() writable bytes. `ptr::copy`
    // permits overlap, so a caller may reuse request storage as reply storage.
    unsafe { ptr::copy(encoded.as_ptr(), reply, encoded.len()) };
    // SAFETY: reply_len remains the checked call-scoped output pointer.
    unsafe { ptr::write(reply_len, encoded.len()) };
    STATUS_OK
}

#[cfg(test)]
mod tests {
    use std::{
        ptr::{self, NonNull},
        sync::Mutex,
    };

    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn isolate() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().expect("test lock");
        PROVIDER_FAILED.store(false, Ordering::Release);
        guard
    }

    fn call(request: &[u8], reply: &mut [u8], reply_len: &mut usize) -> i32 {
        // SAFETY: slices and reply_len remain live for the synchronous call.
        unsafe {
            agenterm_cu_provider_call(
                request.as_ptr(),
                request.len(),
                reply.as_mut_ptr(),
                reply.len(),
                reply_len,
            )
        }
    }

    #[test]
    fn abi_version_is_exact() {
        let _guard = isolate();
        assert_eq!(agenterm_cu_provider_abi_version(), 1);
    }

    #[test]
    fn rejects_invalid_pointers_and_clears_valid_length_outputs() {
        let _guard = isolate();
        let mut reply_len = 99;
        // SAFETY: the null request is deliberate invalid-input evidence; the
        // non-null reply_len remains writable for the call.
        let status = unsafe {
            agenterm_cu_provider_call(ptr::null(), 1, ptr::null_mut(), 0, &mut reply_len)
        };
        assert_eq!(status, STATUS_INVALID_POINTER);
        assert_eq!(reply_len, 0);

        let request = b"{";
        // SAFETY: a null reply with nonzero capacity is deliberate invalid
        // input; the request and length pointer are valid.
        let status = unsafe {
            agenterm_cu_provider_call(
                request.as_ptr(),
                request.len(),
                ptr::null_mut(),
                1,
                &mut reply_len,
            )
        };
        assert_eq!(status, STATUS_INVALID_POINTER);
    }

    #[test]
    fn rejects_request_and_reply_size_overflow_before_dereference() {
        let _guard = isolate();
        let dangling = NonNull::<u8>::dangling();
        let mut reply_len = 77;
        // SAFETY: the deliberately oversized request must be rejected before
        // the dangling pointer is dereferenced; zero reply capacity is valid.
        let status = unsafe {
            agenterm_cu_provider_call(
                dangling.as_ptr(),
                MAX_REQUEST_BYTES + 1,
                ptr::null_mut(),
                0,
                &mut reply_len,
            )
        };
        assert_eq!(status, STATUS_REQUEST_TOO_LARGE);
        assert_eq!(reply_len, 0);

        // SAFETY: empty request permits a null request pointer. Oversized reply
        // capacity must be rejected before the dangling pointer is written.
        let status = unsafe {
            agenterm_cu_provider_call(
                ptr::null(),
                0,
                dangling.as_ptr(),
                MAX_REPLY_BYTES + 1,
                &mut reply_len,
            )
        };
        assert_eq!(status, STATUS_REPLY_TOO_LARGE);
        assert_eq!(reply_len, 0);
    }

    #[test]
    fn rejects_non_utf8_as_typed_boundary_failure() {
        let _guard = isolate();
        let mut reply = [0_u8; 32];
        let mut reply_len = 55;
        assert_eq!(
            call(&[0xff], &mut reply, &mut reply_len),
            STATUS_REQUEST_NOT_UTF8
        );
        assert_eq!(reply_len, 0);
    }

    #[test]
    fn legal_ok_false_is_status_zero_and_exact_existing_reply_json() {
        let _guard = isolate();
        let request = b"{";
        let expected =
            serde_json::to_vec(&agenterm_cu::embedder::execute_json_from_environment("{"))
                .expect("existing CuReply serializes");
        let mut reply = vec![0_u8; MAX_REPLY_BYTES];
        let mut reply_len = 0;
        assert_eq!(call(request, &mut reply, &mut reply_len), STATUS_OK);
        assert_eq!(&reply[..reply_len], expected);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&reply[..reply_len]).expect("CuReply JSON")
                ["ok"],
            false
        );
    }

    #[test]
    fn short_reply_buffer_is_typed_and_does_not_publish_partial_data() {
        let _guard = isolate();
        let mut reply = [0xa5_u8; 1];
        let mut reply_len = 99;
        assert_eq!(
            call(b"{", &mut reply, &mut reply_len),
            STATUS_REPLY_TOO_LARGE
        );
        assert_eq!(reply_len, 0);
        assert_eq!(reply, [0xa5]);
    }

    #[test]
    fn request_and_reply_may_alias() {
        let _guard = isolate();
        let request = b"{";
        let expected =
            serde_json::to_vec(&agenterm_cu::embedder::execute_json_from_environment("{"))
                .expect("existing CuReply serializes");
        let mut shared = vec![0_u8; MAX_REPLY_BYTES];
        shared[..request.len()].copy_from_slice(request);
        let mut reply_len = 0;
        // SAFETY: one live allocation provides both declared regions. The ABI
        // explicitly permits their overlap and the output capacity is exact.
        let status = unsafe {
            agenterm_cu_provider_call(
                shared.as_ptr(),
                request.len(),
                shared.as_mut_ptr(),
                shared.len(),
                &mut reply_len,
            )
        };
        assert_eq!(status, STATUS_OK);
        assert_eq!(&shared[..reply_len], expected);
    }

    #[test]
    fn panic_is_contained_and_permanently_latched() {
        let _guard = isolate();
        let mut reply = [0_u8; 1024];
        let mut reply_len = 99;
        assert_eq!(
            call(
                b"__AGENTERM_CU_PROVIDER_TEST_PANIC__",
                &mut reply,
                &mut reply_len
            ),
            STATUS_PROVIDER_PANICKED
        );
        assert_eq!(reply_len, 0);
        assert_eq!(
            call(b"{", &mut reply, &mut reply_len),
            STATUS_PROVIDER_PANICKED
        );
        assert_eq!(reply_len, 0);
    }
}
