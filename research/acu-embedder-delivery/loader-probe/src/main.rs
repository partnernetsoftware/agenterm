//! Opaque, load-once client for the measurement-only ACU provider.

use std::{env, process::ExitCode, time::Instant};

use libloading::{Library, Symbol};

const EXPECTED_ABI: u32 = 1;
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

type VersionFn = unsafe extern "C" fn() -> u32;
type CallFn = unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize) -> i32;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("boundary_error={error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), &'static str> {
    let mut args = env::args_os().skip(1);
    let library_path = args.next().ok_or("invalid_input:missing_provider_path")?;
    let request = args.next().ok_or("invalid_input:missing_request")?;
    let repeat = args
        .next()
        .ok_or("invalid_input:missing_repeat")?
        .to_str()
        .ok_or("invalid_input:repeat_not_utf8")?
        .parse::<usize>()
        .map_err(|_| "invalid_input:repeat_not_integer")?;
    if repeat == 0 || repeat > 1024 {
        return Err("invalid_input:repeat_out_of_range");
    }
    let request = request
        .to_str()
        .ok_or("invalid_input:request_not_utf8")?
        .as_bytes();

    let load_started = Instant::now();
    // SAFETY: the library remains owned until after all resolved symbols and
    // calls are complete. Symbol signatures are checked by ABI version before
    // the first command is sent.
    let library =
        unsafe { Library::new(&library_path) }.map_err(|_| "provider_missing:load_failed")?;
    // SAFETY: names and signatures are the version-1 provider contract.
    let version: Symbol<'_, VersionFn> = unsafe { library.get(b"acu_provider_abi_version\0") }
        .map_err(|_| "provider_abi:symbol_missing")?;
    // SAFETY: the function has no arguments and the library is live.
    if unsafe { version() } != EXPECTED_ABI {
        return Err("provider_abi:version_mismatch");
    }
    // SAFETY: name and signature are the version-1 provider contract.
    let call: Symbol<'_, CallFn> = unsafe { library.get(b"acu_provider_call\0") }
        .map_err(|_| "provider_abi:symbol_missing")?;
    let load_ns = load_started.elapsed().as_nanos();

    let mut reply = vec![0_u8; MAX_REPLY_BYTES];
    let mut elapsed = Vec::with_capacity(repeat);
    let mut last_len = 0_usize;
    let mut first_call_ns = 0_u128;
    for _ in 0..repeat {
        let started = Instant::now();
        // SAFETY: request/reply buffers remain live for the complete call and
        // the provider has passed the ABI-version check.
        let status = unsafe {
            call(
                request.as_ptr(),
                request.len(),
                reply.as_mut_ptr(),
                reply.len(),
                &mut last_len,
            )
        };
        let call_ns = started.elapsed().as_nanos();
        if elapsed.is_empty() {
            first_call_ns = call_ns;
        }
        elapsed.push(call_ns);
        if status != 0 {
            return Err(provider_status(status));
        }
        if last_len > reply.len() {
            return Err("provider_protocol:reply_length_out_of_range");
        }
        if std::str::from_utf8(&reply[..last_len]).is_err() {
            return Err("provider_protocol:reply_not_utf8");
        }
    }

    elapsed.sort_unstable();
    let median = elapsed[elapsed.len() / 2];
    let p95 = elapsed[(elapsed.len() * 95).div_ceil(100).saturating_sub(1)];
    println!("abi={EXPECTED_ABI}");
    println!("calls={repeat}");
    println!("load_ns={load_ns}");
    println!("first_call_ns={first_call_ns}");
    println!("median_call_ns={median}");
    println!("p95_call_ns={p95}");
    println!("reply_len={last_len}");
    println!("reply={}", String::from_utf8_lossy(&reply[..last_len]));
    Ok(())
}

fn provider_status(status: i32) -> &'static str {
    match status {
        1 => "provider_call:invalid_pointer",
        2 => "provider_call:request_too_large",
        3 => "provider_call:request_not_utf8",
        4 => "provider_call:reply_too_large",
        5 => "provider_call:serialize_failed",
        6 => "provider_call:provider_panicked",
        _ => "provider_call:unknown_status",
    }
}
