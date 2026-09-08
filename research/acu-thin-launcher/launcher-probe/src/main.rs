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
    validate_result(&result, stdout.len(), stderr.len())?;
    validate_ordinary_output(&stdout[..result.stdout_len], &stderr[..result.stderr_len])?;
    io::stdout()
        .lock()
        .write_all(&stdout[..result.stdout_len])
        .map_err(|_| "stdout_write_failed")?;
    io::stderr()
        .lock()
        .write_all(&stderr[..result.stderr_len])
        .map_err(|_| "stderr_write_failed")?;
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
    stdout_capacity: usize,
    stderr_capacity: usize,
) -> Result<(), &'static str> {
    if result.abi_version != EXPECTED_ABI
        || result.struct_size as usize != mem::size_of::<ProcessMainResultV1>()
    {
        return Err("provider_result_shape_invalid");
    }
    if result.entry_mode != 0 {
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
        assert!(validate_result(&result, 4, 3).is_ok());
        assert!(validate_ordinary_output(b"{}\n", b"usage").is_ok());
        assert_eq!(
            validate_ordinary_output(b"not-json\n", b"").unwrap_err(),
            "provider_stdout_json_invalid"
        );
        assert_eq!(provider_status(8), "provider_entry_mode_unimplemented");
        assert_eq!(BOUNDARY_EXIT, 70);
    }
}
