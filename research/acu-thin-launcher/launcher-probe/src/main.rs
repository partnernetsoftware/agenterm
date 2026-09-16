//! Measurement-only fixed-sibling launcher for the ACU process-main ABI.

use std::{
    env,
    ffi::OsString,
    io::{self, Write},
    mem,
    path::{Path, PathBuf},
    process::ExitCode,
};

use libloading::{Library, Symbol};

const EXPECTED_ABI: u32 = 1;
const MAX_ARGV_COUNT: usize = 4_096;
const MAX_ARGV_BYTES: usize = 1_048_576;
const MAX_STDOUT_BYTES: usize = 4 * 1_048_576;
const MAX_STDERR_BYTES: usize = 1_048_576;
const BOUNDARY_EXIT: u8 = 70;

const STATUS_OK: i32 = 0;
const ENTRY_ORDINARY_ARGV: u32 = 0;
const ENTRY_NETWORK_PROBE_WORKER: u32 = 2;
const ENTRY_BROWSER_SESSION_OWNER: u32 = 3;
const ENTRY_MANAGED_JOB_OWNER: u32 = 4;
const ENTRY_DEVICE_LEASE_OWNER: u32 = 5;
const ENTRY_DEVICE_IO_FIXTURE: u32 = 7;
const ENTRY_NETWORK_PROBE_FIXTURE: u32 = 8;
const ENTRY_VERBS_TEXT: u32 = 10;
const ENTRY_X11_CLIPBOARD_OWNER: u32 = 11;
const ENTRY_VERSION_TEXT: u32 = 12;

const NETWORK_PROBE_WORKER_ARG: &[u8] = b"--agenterm-cu-internal-network-probe-worker";
const BROWSER_SESSION_OWNER_ARG: &[u8] = b"--agenterm-cu-internal-browser-session-owner";
const MANAGED_JOB_OWNER_ARG: &[u8] = b"--agenterm-cu-internal-managed-job-owner";
const DEVICE_LEASE_OWNER_ARG: &[u8] = b"--agenterm-cu-internal-device-lease-owner";
const DEVICE_IO_FIXTURE_ARG: &[u8] = b"--agenterm-cu-internal-device-io-fixture";
const NETWORK_PROBE_FIXTURE_ARG: &[u8] = b"--agenterm-cu-internal-network-probe-fixture";
const VERBS_ARG: &[u8] = b"verbs";
const X11_CLIPBOARD_OWNER_ARG: &[u8] = b"__agenterm-internal-x11-clipboard-own";

#[repr(C)]
#[derive(Clone, Copy)]
struct ByteSpanV1 {
    data: *const u8,
    len: usize,
}

#[repr(C)]
struct ProcessMainRequestV1 {
    abi_version: u32,
    struct_size: u32,
    argc: usize,
    argv: *const ByteSpanV1,
}

#[repr(C)]
struct ProcessMainResultV1 {
    abi_version: u32,
    struct_size: u32,
    entry_mode: u32,
    exit_code: i32,
    stdout_len: usize,
    stderr_len: usize,
}

type VersionFn = unsafe extern "C" fn() -> u32;
type ProcessMainFn = unsafe extern "C" fn(
    *const ProcessMainRequestV1,
    *mut u8,
    usize,
    *mut u8,
    usize,
    *mut ProcessMainResultV1,
) -> i32;

fn main() -> ExitCode {
    match run() {
        Ok(exit) => ExitCode::from(exit),
        Err(error) => {
            eprintln!("acu-launcher:{error}");
            ExitCode::from(BOUNDARY_EXIT)
        }
    }
}

fn run() -> Result<u8, &'static str> {
    let argv = collect_bounded_argv(env::args_os().skip(1))?;
    let executable = env::current_exe().map_err(|_| "launcher_identity_unavailable")?;
    let provider = fixed_sibling_path(&executable)?;

    // SAFETY: the absolute path is derived only from current_exe plus the one
    // compile-time OS filename. The library remains live through all symbols.
    let library = unsafe { Library::new(provider) }.map_err(|_| "provider_missing")?;
    // SAFETY: this no-argument symbol is the version probe for the closed ABI.
    let version: Symbol<'_, VersionFn> =
        unsafe { library.get(b"agenterm_cu_process_main_abi_version\0") }
            .map_err(|_| "provider_abi_symbol_missing")?;
    // SAFETY: the library is live and the version function takes no arguments.
    if unsafe { version() } != EXPECTED_ABI {
        return Err("provider_abi_version_mismatch");
    }
    // Resolve the call symbol only after the provider declares the exact ABI.
    // SAFETY: the checked ABI fixes this symbol name and C signature.
    let process_main: Symbol<'_, ProcessMainFn> =
        unsafe { library.get(b"agenterm_cu_process_main_v1\0") }
            .map_err(|_| "provider_process_main_symbol_missing")?;

    let spans: Vec<ByteSpanV1> = argv
        .iter()
        .map(|arg| ByteSpanV1 {
            data: arg.as_ptr(),
            len: arg.len(),
        })
        .collect();
    let request = ProcessMainRequestV1 {
        abi_version: EXPECTED_ABI,
        struct_size: mem::size_of::<ProcessMainRequestV1>() as u32,
        argc: spans.len(),
        argv: spans.as_ptr(),
    };
    let mut result = ProcessMainResultV1 {
        abi_version: 0,
        struct_size: 0,
        entry_mode: 0,
        exit_code: 1,
        stdout_len: 0,
        stderr_len: 0,
    };
    let mut stdout = vec![0_u8; MAX_STDOUT_BYTES];
    let mut stderr = vec![0_u8; MAX_STDERR_BYTES];
    // SAFETY: every argv/output/result allocation remains live and stable for
    // the complete synchronous call through the checked ABI symbol.
    let status = unsafe {
        process_main(
            &request,
            stdout.as_mut_ptr(),
            stdout.len(),
            stderr.as_mut_ptr(),
            stderr.len(),
            &mut result,
        )
    };
    if status != STATUS_OK {
        return Err(provider_status(status));
    }
    let expected_entry_mode = expected_entry_mode(&argv);
    validate_result(&result, expected_entry_mode, stdout.len(), stderr.len())?;
    validate_output(
        result.entry_mode,
        &stdout[..result.stdout_len],
        &stderr[..result.stderr_len],
    )?;
    if !entry_mode_owns_stdio(result.entry_mode) {
        io::stdout()
            .lock()
            .write_all(&stdout[..result.stdout_len])
            .map_err(|_| "stdout_write_failed")?;
        io::stderr()
            .lock()
            .write_all(&stderr[..result.stderr_len])
            .map_err(|_| "stderr_write_failed")?;
    }
    u8::try_from(result.exit_code).map_err(|_| "provider_exit_code_invalid")
}

fn collect_bounded_argv(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Vec<Vec<u8>>, &'static str> {
    let mut argv = Vec::new();
    let mut total = 0_usize;
    for arg in args {
        if argv.len() == MAX_ARGV_COUNT {
            return Err("argv_count_exceeded");
        }
        let text = arg.into_string().map_err(|_| "argv_not_utf8")?;
        if text.as_bytes().contains(&0) {
            return Err("argv_contains_nul");
        }
        total = total
            .checked_add(text.len())
            .and_then(|bytes| bytes.checked_add(1))
            .filter(|bytes| *bytes <= MAX_ARGV_BYTES)
            .ok_or("argv_bytes_exceeded")?;
        argv.push(text.into_bytes());
    }
    Ok(argv)
}

fn fixed_sibling_path(executable: &Path) -> Result<PathBuf, &'static str> {
    let directory = executable.parent().ok_or("launcher_parent_unavailable")?;
    Ok(directory.join(provider_filename()))
}

const fn provider_filename() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "agenterm-cu-provider.dll"
    }
    #[cfg(target_os = "macos")]
    {
        "agenterm-cu-provider.dylib"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "agenterm-cu-provider.so"
    }
}

fn validate_result(
    result: &ProcessMainResultV1,
    expected_entry_mode: u32,
    stdout_capacity: usize,
    stderr_capacity: usize,
) -> Result<(), &'static str> {
    if result.abi_version != EXPECTED_ABI
        || result.struct_size as usize != mem::size_of::<ProcessMainResultV1>()
    {
        return Err("provider_result_shape_invalid");
    }
    if result.entry_mode != expected_entry_mode {
        return Err("provider_result_entry_mode_invalid");
    }
    if result.stdout_len > stdout_capacity || result.stderr_len > stderr_capacity {
        return Err("provider_result_length_invalid");
    }
    if !(0..=255).contains(&result.exit_code) {
        return Err("provider_exit_code_invalid");
    }
    Ok(())
}

fn expected_entry_mode(argv: &[Vec<u8>]) -> u32 {
    if matches!(argv, [arg] if arg.as_slice() == NETWORK_PROBE_WORKER_ARG) {
        ENTRY_NETWORK_PROBE_WORKER
    } else if matches!(argv, [first, ..] if first.as_slice() == BROWSER_SESSION_OWNER_ARG) {
        ENTRY_BROWSER_SESSION_OWNER
    } else if matches!(argv, [arg] if arg.as_slice() == MANAGED_JOB_OWNER_ARG) {
        ENTRY_MANAGED_JOB_OWNER
    } else if matches!(argv, [arg] if arg.as_slice() == DEVICE_LEASE_OWNER_ARG) {
        ENTRY_DEVICE_LEASE_OWNER
    } else if matches!(argv, [first, ..] if first.as_slice() == DEVICE_IO_FIXTURE_ARG) {
        ENTRY_DEVICE_IO_FIXTURE
    } else if matches!(argv, [first, ..] if first.as_slice() == NETWORK_PROBE_FIXTURE_ARG) {
        ENTRY_NETWORK_PROBE_FIXTURE
    } else if matches!(argv, [first, ..] if first.as_slice() == VERBS_ARG) {
        ENTRY_VERBS_TEXT
    } else if matches!(argv, [first, ..] if first.as_slice() == X11_CLIPBOARD_OWNER_ARG) {
        ENTRY_X11_CLIPBOARD_OWNER
    } else if matches!(argv, [arg] if matches!(arg.as_slice(), b"--version" | b"-V")) {
        ENTRY_VERSION_TEXT
    } else {
        ENTRY_ORDINARY_ARGV
    }
}

fn validate_output(entry_mode: u32, stdout: &[u8], stderr: &[u8]) -> Result<(), &'static str> {
    match entry_mode {
        ENTRY_ORDINARY_ARGV => validate_ordinary_output(stdout, stderr),
        ENTRY_NETWORK_PROBE_WORKER
        | ENTRY_BROWSER_SESSION_OWNER
        | ENTRY_MANAGED_JOB_OWNER
        | ENTRY_DEVICE_LEASE_OWNER
        | ENTRY_DEVICE_IO_FIXTURE
        | ENTRY_NETWORK_PROBE_FIXTURE
        | ENTRY_X11_CLIPBOARD_OWNER => {
            if stdout.is_empty() && stderr.is_empty() {
                Ok(())
            } else {
                Err("provider_direct_stdio_buffer_invalid")
            }
        }
        ENTRY_VERSION_TEXT => validate_version_output(stdout, stderr),
        ENTRY_VERBS_TEXT => validate_verbs_output(stdout, stderr),
        _ => Err("provider_result_entry_mode_invalid"),
    }
}

fn entry_mode_owns_stdio(entry_mode: u32) -> bool {
    matches!(
        entry_mode,
        ENTRY_NETWORK_PROBE_WORKER
            | ENTRY_BROWSER_SESSION_OWNER
            | ENTRY_MANAGED_JOB_OWNER
            | ENTRY_DEVICE_LEASE_OWNER
            | ENTRY_DEVICE_IO_FIXTURE
            | ENTRY_NETWORK_PROBE_FIXTURE
            | ENTRY_X11_CLIPBOARD_OWNER
    )
}

fn validate_ordinary_output(stdout: &[u8], stderr: &[u8]) -> Result<(), &'static str> {
    let json = stdout
        .strip_suffix(b"\n")
        .ok_or("provider_stdout_framing_invalid")?;
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|_| "provider_stdout_json_invalid")?;
    if !value.is_object() {
        return Err("provider_stdout_shape_invalid");
    }
    std::str::from_utf8(stderr).map_err(|_| "provider_stderr_not_utf8")?;
    Ok(())
}

fn validate_version_output(stdout: &[u8], stderr: &[u8]) -> Result<(), &'static str> {
    if !stderr.is_empty() {
        return Err("provider_version_stderr_invalid");
    }
    let text = std::str::from_utf8(stdout).map_err(|_| "provider_version_stdout_invalid")?;
    if !text.starts_with("agenterm-cu ") || !text.ends_with('\n') || text.lines().count() != 1 {
        return Err("provider_version_stdout_invalid");
    }
    Ok(())
}

fn validate_verbs_output(stdout: &[u8], stderr: &[u8]) -> Result<(), &'static str> {
    if !stderr.is_empty() {
        return Err("provider_verbs_stderr_invalid");
    }
    let text = std::str::from_utf8(stdout).map_err(|_| "provider_verbs_stdout_invalid")?;
    if text.starts_with("NAME") {
        return Ok(());
    }
    let json = stdout
        .strip_suffix(b"\n")
        .ok_or("provider_verbs_stdout_invalid")?;
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|_| "provider_verbs_stdout_invalid")?;
    if value.is_array() || value.get("ok") == Some(&serde_json::Value::Bool(false)) {
        Ok(())
    } else {
        Err("provider_verbs_stdout_invalid")
    }
}

fn provider_status(status: i32) -> &'static str {
    match status {
        1 => "provider_invalid_pointer",
        2 => "provider_bad_request",
        3 => "provider_argv_too_large",
        4 => "provider_argv_not_utf8",
        5 => "provider_output_too_large",
        6 => "provider_serialize_failed",
        7 => "provider_panicked",
        8 => "provider_entry_mode_unimplemented",
        _ => "provider_status_unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_name_is_fixed_and_path_is_not_searched() {
        let executable = Path::new("root/bin/acu-thin-launcher");
        assert_eq!(
            fixed_sibling_path(executable).unwrap(),
            Path::new("root/bin").join(provider_filename())
        );
        assert!(!provider_filename().contains('/'));
        assert!(!provider_filename().contains('\\'));
    }

    #[test]
    fn argv_collection_is_bounded() {
        assert_eq!(
            collect_bounded_argv([OsString::from("x".repeat(MAX_ARGV_BYTES - 1))]).unwrap()[0]
                .len(),
            MAX_ARGV_BYTES - 1
        );
        assert!(collect_bounded_argv([OsString::from("x".repeat(MAX_ARGV_BYTES + 1))]).is_err());
        assert!(collect_bounded_argv((0..=MAX_ARGV_COUNT).map(|_| OsString::from(""))).is_err());
    }

    #[test]
    fn product_and_boundary_statuses_remain_separate() {
        let result = ProcessMainResultV1 {
            abi_version: EXPECTED_ABI,
            struct_size: mem::size_of::<ProcessMainResultV1>() as u32,
            entry_mode: 0,
            exit_code: 2,
            stdout_len: 4,
            stderr_len: 3,
        };
        assert!(validate_result(&result, ENTRY_ORDINARY_ARGV, 4, 3).is_ok());
        assert!(validate_ordinary_output(b"{}\n", b"usage").is_ok());
        assert_eq!(
            validate_ordinary_output(b"not-json\n", b"").unwrap_err(),
            "provider_stdout_json_invalid"
        );
        assert_eq!(provider_status(8), "provider_entry_mode_unimplemented");
        assert_eq!(BOUNDARY_EXIT, 70);
    }

    #[test]
    fn version_mode_is_exact_and_has_its_own_output_contract() {
        assert_eq!(
            expected_entry_mode(&[b"--version".to_vec()]),
            ENTRY_VERSION_TEXT
        );
        assert_eq!(expected_entry_mode(&[b"-V".to_vec()]), ENTRY_VERSION_TEXT);
        assert_eq!(
            expected_entry_mode(&[b"--version".to_vec(), b"extra".to_vec()]),
            ENTRY_ORDINARY_ARGV
        );
        assert!(validate_version_output(b"agenterm-cu 0.1.16\n", b"").is_ok());
        assert_eq!(
            validate_version_output(b"agenterm-cu 0.1.16\n", b"unexpected").unwrap_err(),
            "provider_version_stderr_invalid"
        );
        assert_eq!(
            validate_version_output(b"{}\n", b"").unwrap_err(),
            "provider_version_stdout_invalid"
        );
    }

    #[test]
    fn verbs_mode_accepts_text_json_and_typed_usage_only() {
        for argv in [
            vec![VERBS_ARG.to_vec()],
            vec![VERBS_ARG.to_vec(), b"--json".to_vec()],
            vec![VERBS_ARG.to_vec(), b"--text".to_vec()],
            vec![VERBS_ARG.to_vec(), b"--bogus".to_vec()],
        ] {
            assert_eq!(expected_entry_mode(&argv), ENTRY_VERBS_TEXT);
        }
        assert!(validate_verbs_output(b"NAME  SUMMARY\n", b"").is_ok());
        assert!(validate_verbs_output(b"[]\n", b"").is_ok());
        assert!(validate_verbs_output(b"{\"ok\":false}\n", b"").is_ok());
        assert_eq!(
            validate_verbs_output(b"{}\n", b"").unwrap_err(),
            "provider_verbs_stdout_invalid"
        );
        assert_eq!(
            validate_verbs_output(b"[]\n", b"unexpected").unwrap_err(),
            "provider_verbs_stderr_invalid"
        );
        assert!(!entry_mode_owns_stdio(ENTRY_VERBS_TEXT));
    }

    #[test]
    fn network_probe_child_modes_own_stdio_and_never_publish_abi_bytes() {
        assert_eq!(
            expected_entry_mode(&[NETWORK_PROBE_WORKER_ARG.to_vec()]),
            ENTRY_NETWORK_PROBE_WORKER
        );
        assert_eq!(
            expected_entry_mode(&[NETWORK_PROBE_WORKER_ARG.to_vec(), b"extra".to_vec(),]),
            ENTRY_ORDINARY_ARGV
        );
        assert_eq!(
            expected_entry_mode(&[
                NETWORK_PROBE_FIXTURE_ARG.to_vec(),
                b"3".to_vec(),
                b"30000".to_vec(),
            ]),
            ENTRY_NETWORK_PROBE_FIXTURE
        );
        assert!(validate_output(ENTRY_NETWORK_PROBE_WORKER, b"", b"").is_ok());
        assert!(validate_output(ENTRY_NETWORK_PROBE_FIXTURE, b"", b"").is_ok());
        assert_eq!(
            validate_output(ENTRY_NETWORK_PROBE_WORKER, b"{}", b"").unwrap_err(),
            "provider_direct_stdio_buffer_invalid"
        );
        assert!(entry_mode_owns_stdio(ENTRY_NETWORK_PROBE_WORKER));
        assert!(entry_mode_owns_stdio(ENTRY_NETWORK_PROBE_FIXTURE));
        assert!(!entry_mode_owns_stdio(ENTRY_VERSION_TEXT));
    }

    #[test]
    fn managed_job_owner_mode_is_exact_and_never_publishes_abi_bytes() {
        assert_eq!(
            expected_entry_mode(&[MANAGED_JOB_OWNER_ARG.to_vec()]),
            ENTRY_MANAGED_JOB_OWNER
        );
        assert!(validate_output(ENTRY_MANAGED_JOB_OWNER, b"", b"").is_ok());
        assert!(entry_mode_owns_stdio(ENTRY_MANAGED_JOB_OWNER));
        assert_eq!(
            expected_entry_mode(&[MANAGED_JOB_OWNER_ARG.to_vec(), b"extra".to_vec()]),
            ENTRY_ORDINARY_ARGV
        );
        assert_eq!(
            validate_output(ENTRY_MANAGED_JOB_OWNER, b"unexpected", b"").unwrap_err(),
            "provider_direct_stdio_buffer_invalid"
        );
    }

    #[test]
    fn device_lease_owner_mode_is_exact_and_never_publishes_abi_bytes() {
        assert_eq!(
            expected_entry_mode(&[DEVICE_LEASE_OWNER_ARG.to_vec()]),
            ENTRY_DEVICE_LEASE_OWNER
        );
        assert!(validate_output(ENTRY_DEVICE_LEASE_OWNER, b"", b"").is_ok());
        assert!(entry_mode_owns_stdio(ENTRY_DEVICE_LEASE_OWNER));
        assert_eq!(
            expected_entry_mode(&[DEVICE_LEASE_OWNER_ARG.to_vec(), b"extra".to_vec()]),
            ENTRY_ORDINARY_ARGV
        );
        assert_eq!(
            validate_output(ENTRY_DEVICE_LEASE_OWNER, b"unexpected", b"").unwrap_err(),
            "provider_direct_stdio_buffer_invalid"
        );
    }

    #[test]
    fn device_io_fixture_mode_preserves_tail_arguments_and_owns_stdio() {
        for argv in [
            vec![DEVICE_IO_FIXTURE_ARG.to_vec()],
            vec![
                DEVICE_IO_FIXTURE_ARG.to_vec(),
                b"root".to_vec(),
                b"30000".to_vec(),
            ],
            vec![
                DEVICE_IO_FIXTURE_ARG.to_vec(),
                b"root".to_vec(),
                b"30000".to_vec(),
                b"--initial-baud".to_vec(),
                b"19200".to_vec(),
            ],
        ] {
            assert_eq!(expected_entry_mode(&argv), ENTRY_DEVICE_IO_FIXTURE);
        }
        assert!(validate_output(ENTRY_DEVICE_IO_FIXTURE, b"", b"").is_ok());
        assert!(entry_mode_owns_stdio(ENTRY_DEVICE_IO_FIXTURE));
        assert_eq!(
            validate_output(ENTRY_DEVICE_IO_FIXTURE, b"unexpected", b"").unwrap_err(),
            "provider_direct_stdio_buffer_invalid"
        );
    }

    #[test]
    fn x11_clipboard_owner_mode_preserves_first_argument_and_owns_stdio() {
        for argv in [
            vec![X11_CLIPBOARD_OWNER_ARG.to_vec()],
            vec![X11_CLIPBOARD_OWNER_ARG.to_vec(), b"ignored-tail".to_vec()],
        ] {
            assert_eq!(expected_entry_mode(&argv), ENTRY_X11_CLIPBOARD_OWNER);
        }
        assert!(validate_output(ENTRY_X11_CLIPBOARD_OWNER, b"", b"").is_ok());
        assert!(entry_mode_owns_stdio(ENTRY_X11_CLIPBOARD_OWNER));
        assert_eq!(
            validate_output(ENTRY_X11_CLIPBOARD_OWNER, b"unexpected", b"").unwrap_err(),
            "provider_direct_stdio_buffer_invalid"
        );
    }

    #[test]
    fn browser_session_owner_mode_preserves_first_argument_classification() {
        for argv in [
            vec![BROWSER_SESSION_OWNER_ARG.to_vec()],
            vec![BROWSER_SESSION_OWNER_ARG.to_vec(), b"session".to_vec()],
            vec![
                BROWSER_SESSION_OWNER_ARG.to_vec(),
                b"session".to_vec(),
                b"extra".to_vec(),
            ],
        ] {
            assert_eq!(expected_entry_mode(&argv), ENTRY_BROWSER_SESSION_OWNER);
        }
        assert!(validate_output(ENTRY_BROWSER_SESSION_OWNER, b"", b"").is_ok());
        assert!(entry_mode_owns_stdio(ENTRY_BROWSER_SESSION_OWNER));
        assert_eq!(
            validate_output(ENTRY_BROWSER_SESSION_OWNER, b"unexpected", b"").unwrap_err(),
            "provider_direct_stdio_buffer_invalid"
        );
    }
}
