//! Measurement-only process-main provider for the ACU thin-launcher court.
//!
//! Raw argv crosses one versioned C ABI. This provider alone classifies
//! binary entry modes and owns ordinary argv parsing, execution, presentation,
//! and product exit status. Entry modes whose streaming/lifetime contracts are
//! not implemented by this prototype fail explicitly at the provider boundary.

#[cfg(panic = "abort")]
compile_error!("the process-main provider must be built with panic=unwind");

use std::{
    mem,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr, slice, str,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub const ABI_VERSION: u32 = 1;
pub const MAX_ARGV_COUNT: usize = 4_096;
pub const MAX_ARGV_BYTES: usize = 1_048_576;
pub const MAX_STDOUT_BYTES: usize = 4 * 1_048_576;
pub const MAX_STDERR_BYTES: usize = 1_048_576;

pub const STATUS_OK: i32 = 0;
pub const STATUS_INVALID_POINTER: i32 = 1;
pub const STATUS_BAD_REQUEST: i32 = 2;
pub const STATUS_ARGV_TOO_LARGE: i32 = 3;
pub const STATUS_ARGV_NOT_UTF8: i32 = 4;
pub const STATUS_OUTPUT_TOO_LARGE: i32 = 5;
pub const STATUS_SERIALIZE_FAILED: i32 = 6;
pub const STATUS_PROVIDER_PANICKED: i32 = 7;
pub const STATUS_ENTRY_MODE_UNIMPLEMENTED: i32 = 8;

pub const ENTRY_ORDINARY_ARGV: u32 = 0;
pub const ENTRY_NATIVE_MESSAGING_HOST: u32 = 1;
pub const ENTRY_NETWORK_PROBE_WORKER: u32 = 2;
pub const ENTRY_BROWSER_SESSION_OWNER: u32 = 3;
pub const ENTRY_MANAGED_JOB_OWNER: u32 = 4;
pub const ENTRY_DEVICE_LEASE_OWNER: u32 = 5;
pub const ENTRY_PRIVILEGE_BROKER: u32 = 6;
pub const ENTRY_DEVICE_IO_FIXTURE: u32 = 7;
pub const ENTRY_NETWORK_PROBE_FIXTURE: u32 = 8;
pub const ENTRY_HOTKEY_HOST: u32 = 9;
pub const ENTRY_VERBS_TEXT: u32 = 10;
pub const ENTRY_X11_CLIPBOARD_OWNER: u32 = 11;
pub const ENTRY_VERSION_TEXT: u32 = 12;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ByteSpanV1 {
    pub data: *const u8,
    pub len: usize,
}

#[repr(C)]
pub struct ProcessMainRequestV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub argc: usize,
    pub argv: *const ByteSpanV1,
}

#[repr(C)]
pub struct ProcessMainResultV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub entry_mode: u32,
    pub exit_code: i32,
    pub stdout_len: usize,
    pub stderr_len: usize,
}

static PROVIDER_FAILED: AtomicBool = AtomicBool::new(false);
static PROVIDER_CALL_LOCK: Mutex<()> = Mutex::new(());

#[unsafe(no_mangle)]
pub extern "C" fn agenterm_cu_process_main_abi_version() -> u32 {
    ABI_VERSION
}

/// Execute one process invocation through the process-main ABI.
///
/// Boundary status and product exit code are independent: `STATUS_OK` means
/// the result contains a product exit status, including ordinary refusals.
/// Every nonzero status leaves both published output lengths at zero.
///
/// # Safety
///
/// Every non-null pointer must name its declared readable or writable region
/// for the complete synchronous call. Each argv span must be live and valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn agenterm_cu_process_main_v1(
    request: *const ProcessMainRequestV1,
    stdout: *mut u8,
    stdout_capacity: usize,
    stderr: *mut u8,
    stderr_capacity: usize,
    result: *mut ProcessMainResultV1,
) -> i32 {
    let _guard = PROVIDER_CALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if PROVIDER_FAILED.load(Ordering::Acquire) {
        // SAFETY: reset_result handles a null result without dereferencing it.
        unsafe { reset_result(result) };
        return STATUS_PROVIDER_PANICKED;
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the public ABI's call-scoped pointer contract is unchanged.
        unsafe {
            process_main_inner(
                request,
                stdout,
                stdout_capacity,
                stderr,
                stderr_capacity,
                result,
            )
        }
    }));
    match outcome {
        Ok(status) => status,
        Err(_) => {
            PROVIDER_FAILED.store(true, Ordering::Release);
            // SAFETY: reset_result handles a null result without dereferencing it.
            unsafe { reset_result(result) };
            STATUS_PROVIDER_PANICKED
        }
    }
}

unsafe fn process_main_inner(
    request: *const ProcessMainRequestV1,
    stdout: *mut u8,
    stdout_capacity: usize,
    stderr: *mut u8,
    stderr_capacity: usize,
    result: *mut ProcessMainResultV1,
) -> i32 {
    if request.is_null() || result.is_null() {
        return STATUS_INVALID_POINTER;
    }
    // SAFETY: both pointers were checked and are call-scoped by the ABI.
    unsafe { reset_result(result) };
    if (stdout_capacity != 0 && stdout.is_null()) || (stderr_capacity != 0 && stderr.is_null()) {
        return STATUS_INVALID_POINTER;
    }
    if stdout_capacity > MAX_STDOUT_BYTES || stderr_capacity > MAX_STDERR_BYTES {
        return STATUS_OUTPUT_TOO_LARGE;
    }
    // SAFETY: request was checked non-null and is readable for one request.
    let request = unsafe { &*request };
    if request.abi_version != ABI_VERSION
        || request.struct_size as usize != mem::size_of::<ProcessMainRequestV1>()
    {
        return STATUS_BAD_REQUEST;
    }
    if request.argc > MAX_ARGV_COUNT {
        return STATUS_ARGV_TOO_LARGE;
    }
    if request.argc != 0 && request.argv.is_null() {
        return STATUS_INVALID_POINTER;
    }
    let spans = if request.argc == 0 {
        &[]
    } else {
        // SAFETY: the ABI requires argv to name argc readable spans.
        unsafe { slice::from_raw_parts(request.argv, request.argc) }
    };
    let mut argv = Vec::with_capacity(spans.len());
    let mut argv_bytes = 0_usize;
    for span in spans {
        if span.len != 0 && span.data.is_null() {
            return STATUS_INVALID_POINTER;
        }
        argv_bytes = match argv_bytes
            .checked_add(span.len)
            .and_then(|bytes| bytes.checked_add(1))
        {
            Some(bytes) if bytes <= MAX_ARGV_BYTES => bytes,
            _ => return STATUS_ARGV_TOO_LARGE,
        };
        let bytes = if span.len == 0 {
            &[]
        } else {
            // SAFETY: each ABI span names len readable bytes for this call.
            unsafe { slice::from_raw_parts(span.data, span.len) }
        };
        let Ok(text) = str::from_utf8(bytes) else {
            return STATUS_ARGV_NOT_UTF8;
        };
        if text.as_bytes().contains(&0) {
            return STATUS_BAD_REQUEST;
        }
        argv.push(text.to_owned());
    }

    #[cfg(test)]
    if matches!(argv.as_slice(), [arg] if arg == "__ACU_THIN_LAUNCHER_TEST_PANIC__") {
        panic!("test-only provider panic");
    }

    let mode = classify_entry(&argv);
    // SAFETY: result is a valid writable ABI result.
    unsafe { (*result).entry_mode = mode };
    if mode != ENTRY_ORDINARY_ARGV {
        return STATUS_ENTRY_MODE_UNIMPLEMENTED;
    }

    let reply = agenterm_cu::argv::execute_argv_from_environment(argv.clone());
    let encoded = match serde_json::to_vec(&reply) {
        Ok(mut encoded) => {
            encoded.push(b'\n');
            encoded
        }
        Err(_) => return STATUS_SERIALIZE_FAILED,
    };
    let diagnostic = agenterm_cu::argv::human_diagnostic(&argv, &reply).unwrap_or_default();
    if encoded.len() > stdout_capacity
        || encoded.len() > MAX_STDOUT_BYTES
        || diagnostic.len() > stderr_capacity
        || diagnostic.len() > MAX_STDERR_BYTES
    {
        return STATUS_OUTPUT_TOO_LARGE;
    }
    // SAFETY: the capacity checks prove each nonempty copy fits. The ABI does
    // not require a non-null destination for an empty output.
    unsafe {
        if !encoded.is_empty() {
            ptr::copy_nonoverlapping(encoded.as_ptr(), stdout, encoded.len());
        }
        if !diagnostic.is_empty() {
            ptr::copy_nonoverlapping(diagnostic.as_ptr(), stderr, diagnostic.len());
        }
        (*result).exit_code = reply_exit_code(&reply);
        (*result).stdout_len = encoded.len();
        (*result).stderr_len = diagnostic.len();
    }
    STATUS_OK
}

unsafe fn reset_result(result: *mut ProcessMainResultV1) {
    if !result.is_null() {
        // SAFETY: the caller promised that a non-null result is writable.
        unsafe {
            ptr::write(
                result,
                ProcessMainResultV1 {
                    abi_version: ABI_VERSION,
                    struct_size: mem::size_of::<ProcessMainResultV1>() as u32,
                    entry_mode: ENTRY_ORDINARY_ARGV,
                    exit_code: 1,
                    stdout_len: 0,
                    stderr_len: 0,
                },
            )
        };
    }
}

fn classify_entry(args: &[String]) -> u32 {
    let Some(first) = args.first().map(String::as_str) else {
        return ENTRY_ORDINARY_ARGV;
    };
    if first.starts_with("chrome-extension://") {
        ENTRY_NATIVE_MESSAGING_HOST
    } else if first == agenterm_cu::network_probe::WORKER_ARG {
        ENTRY_NETWORK_PROBE_WORKER
    } else if first == agenterm_cu::browser_session_owner::OWNER_ARG {
        ENTRY_BROWSER_SESSION_OWNER
    } else if first == agenterm_cu::MANAGED_JOB_OWNER_ARG {
        ENTRY_MANAGED_JOB_OWNER
    } else if first == agenterm_cu::DEVICE_LEASE_OWNER_ARG {
        ENTRY_DEVICE_LEASE_OWNER
    } else if first == agenterm_cu::PRIVILEGE_BROKER_ARG {
        ENTRY_PRIVILEGE_BROKER
    } else if first == agenterm_cu::DEVICE_IO_FIXTURE_ARG {
        ENTRY_DEVICE_IO_FIXTURE
    } else if first == agenterm_cu::network_probe::FIXTURE_ARG {
        ENTRY_NETWORK_PROBE_FIXTURE
    } else if agenterm_cu::cli::verbs::lookup(first).map(|spec| spec.name) == Some("host") {
        ENTRY_HOTKEY_HOST
    } else if agenterm_cu::cli::verbs::lookup(first).map(|spec| spec.name) == Some("verbs") {
        ENTRY_VERBS_TEXT
    } else if first == agenterm_cu::mechanism::clipboard::X11_CLIPBOARD_OWNER_ARG {
        ENTRY_X11_CLIPBOARD_OWNER
    } else if matches!(first, "--version" | "-V") {
        ENTRY_VERSION_TEXT
    } else {
        ENTRY_ORDINARY_ARGV
    }
}

fn reply_exit_code(reply: &agenterm_cu::CuReply) -> i32 {
    if reply.ok {
        0
    } else if reply
        .error
        .as_ref()
        .is_some_and(|error| error.code == "usage")
    {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use std::ptr::NonNull;

    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn maps_every_current_binary_entry_family_before_ordinary_dispatch() {
        let cases = [
            (
                &["chrome-extension://foreign/"][..],
                ENTRY_NATIVE_MESSAGING_HOST,
            ),
            (
                &[agenterm_cu::network_probe::WORKER_ARG][..],
                ENTRY_NETWORK_PROBE_WORKER,
            ),
            (
                &[agenterm_cu::browser_session_owner::OWNER_ARG, "tail"][..],
                ENTRY_BROWSER_SESSION_OWNER,
            ),
            (
                &[agenterm_cu::MANAGED_JOB_OWNER_ARG][..],
                ENTRY_MANAGED_JOB_OWNER,
            ),
            (
                &[agenterm_cu::DEVICE_LEASE_OWNER_ARG][..],
                ENTRY_DEVICE_LEASE_OWNER,
            ),
            (
                &[agenterm_cu::PRIVILEGE_BROKER_ARG][..],
                ENTRY_PRIVILEGE_BROKER,
            ),
            (
                &[agenterm_cu::DEVICE_IO_FIXTURE_ARG, "tail"][..],
                ENTRY_DEVICE_IO_FIXTURE,
            ),
            (
                &[agenterm_cu::network_probe::FIXTURE_ARG, "tail"][..],
                ENTRY_NETWORK_PROBE_FIXTURE,
            ),
            (&["host"][..], ENTRY_HOTKEY_HOST),
            (&["verbs"][..], ENTRY_VERBS_TEXT),
            (
                &[agenterm_cu::mechanism::clipboard::X11_CLIPBOARD_OWNER_ARG][..],
                ENTRY_X11_CLIPBOARD_OWNER,
            ),
            (&["--version"][..], ENTRY_VERSION_TEXT),
        ];
        for (argv, expected) in cases {
            assert_eq!(classify_entry(&strings(argv)), expected, "{argv:?}");
        }
        assert_eq!(
            classify_entry(&strings(&["capabilities"])),
            ENTRY_ORDINARY_ARGV
        );
    }

    #[test]
    fn ordinary_help_capabilities_and_refusal_preserve_presentation_and_exit() {
        for (argv, expected_exit, expected_command, stderr_nonempty) in [
            (
                &["--target", "current", "--grant", "observe", "capabilities"][..],
                0,
                "capabilities",
                false,
            ),
            (
                &["--target", "current", "capabilities"][..],
                1,
                "capabilities",
                false,
            ),
            (&["--help"][..], 0, "help", true),
            (&["not-a-real-verb"][..], 2, "usage", true),
        ] {
            let argv = strings(argv);
            let reply = agenterm_cu::argv::execute_argv_from_environment(argv.clone());
            let stdout = serde_json::to_vec(&reply).expect("CuReply JSON");
            let stderr = agenterm_cu::argv::human_diagnostic(&argv, &reply).unwrap_or_default();
            assert_eq!(reply_exit_code(&reply), expected_exit);
            assert_eq!(reply.command, expected_command);
            assert_eq!(!stderr.is_empty(), stderr_nonempty);
            assert!(!stdout.is_empty());
        }
    }

    #[test]
    fn request_and_result_layouts_are_versioned() {
        assert_eq!(ABI_VERSION, 1);
        assert!(mem::size_of::<ProcessMainRequestV1>() >= 24);
        assert!(mem::size_of::<ProcessMainResultV1>() >= 32);
    }

    #[test]
    fn public_abi_executes_ordinary_argv_and_fails_closed() {
        let _guard = TEST_LOCK.lock().expect("test lock");
        PROVIDER_FAILED.store(false, Ordering::Release);

        let argv = strings(&["--target", "current", "--grant", "observe", "capabilities"]);
        let spans: Vec<ByteSpanV1> = argv
            .iter()
            .map(|arg| ByteSpanV1 {
                data: arg.as_ptr(),
                len: arg.len(),
            })
            .collect();
        let request = ProcessMainRequestV1 {
            abi_version: ABI_VERSION,
            struct_size: mem::size_of::<ProcessMainRequestV1>() as u32,
            argc: spans.len(),
            argv: spans.as_ptr(),
        };
        let mut stdout = vec![0_u8; MAX_STDOUT_BYTES];
        let mut stderr = vec![0_u8; MAX_STDERR_BYTES];
        let mut result = ProcessMainResultV1 {
            abi_version: 0,
            struct_size: 0,
            entry_mode: u32::MAX,
            exit_code: -1,
            stdout_len: 0,
            stderr_len: 0,
        };
        // SAFETY: all request spans and output/result buffers remain live.
        let status = unsafe {
            agenterm_cu_process_main_v1(
                &request,
                stdout.as_mut_ptr(),
                stdout.len(),
                stderr.as_mut_ptr(),
                stderr.len(),
                &mut result,
            )
        };
        assert_eq!(status, STATUS_OK);
        assert_eq!(result.entry_mode, ENTRY_ORDINARY_ARGV);
        assert_eq!(result.exit_code, 0);
        let reply: serde_json::Value =
            serde_json::from_slice(&stdout[..result.stdout_len]).expect("CuReply JSON");
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["command"], "capabilities");

        let oversized = ProcessMainRequestV1 {
            abi_version: ABI_VERSION,
            struct_size: mem::size_of::<ProcessMainRequestV1>() as u32,
            argc: MAX_ARGV_COUNT + 1,
            argv: NonNull::<ByteSpanV1>::dangling().as_ptr(),
        };
        // SAFETY: the oversized count must be rejected before argv is read;
        // both zero-capacity output pointers may be null.
        let status = unsafe {
            agenterm_cu_process_main_v1(
                &oversized,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                0,
                &mut result,
            )
        };
        assert_eq!(status, STATUS_ARGV_TOO_LARGE);
        assert_eq!(result.stdout_len, 0);
        assert_eq!(result.stderr_len, 0);

        let panic_arg = "__ACU_THIN_LAUNCHER_TEST_PANIC__".to_owned();
        let panic_span = ByteSpanV1 {
            data: panic_arg.as_ptr(),
            len: panic_arg.len(),
        };
        let panic_request = ProcessMainRequestV1 {
            abi_version: ABI_VERSION,
            struct_size: mem::size_of::<ProcessMainRequestV1>() as u32,
            argc: 1,
            argv: &panic_span,
        };
        // SAFETY: the input span and result remain live; outputs are empty.
        let status = unsafe {
            agenterm_cu_process_main_v1(
                &panic_request,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                0,
                &mut result,
            )
        };
        assert_eq!(status, STATUS_PROVIDER_PANICKED);
        // SAFETY: the same valid request remains live; the permanent latch is
        // checked before any output or command execution.
        let status = unsafe {
            agenterm_cu_process_main_v1(
                &request,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                0,
                &mut result,
            )
        };
        assert_eq!(status, STATUS_PROVIDER_PANICKED);
        assert_eq!(result.stdout_len, 0);
    }
}
