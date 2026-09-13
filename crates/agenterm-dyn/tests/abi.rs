//! The policy-free ABI bridge: the mechanism matrix, and delegation to the
//! existing family entries. Every claim here is about *mechanism*: which shapes
//! can be executed, that a caller-declared shape reaching the same family entry
//! produces the same result as the direct native call, and that the raw ABI
//! carries no nullability policy.

#[cfg(target_os = "windows")]
use std::ffi::CString;
use std::ffi::{CStr, c_void};

use agenterm_dyn::{
    AbiError, AbiSignature, AbiType, AbiValue, LibraryHandle, NativeCall, invoke_abi,
    invoke_abi_with_handle, validate_abi, validate_abi_signature,
};

#[cfg(target_os = "macos")]
const LIB: &str = "libSystem.B.dylib";
#[cfg(target_os = "linux")]
const LIB: &str = "libc.so.6";
#[cfg(target_os = "windows")]
const LIB: &str = "kernel32.dll";

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn oracle(
    library: &str,
    symbol: &str,
    result: AbiType,
    params: &[AbiType],
    arguments: &[AbiValue],
) -> Result<AbiValue, AbiError> {
    unsafe {
        invoke_abi(&NativeCall {
            library,
            symbol,
            signature: AbiSignature { result, params },
            arguments,
        })
    }
}

/// An exact-family representative: `getpid()` is `i32()`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn matrix_exact_representative_matches_the_direct_call() {
    let value = oracle(LIB, "getpid", AbiType::I32, &[], &[]).expect("getpid through the bridge");
    let AbiValue::I32(pid) = value else {
        panic!("getpid must return the declared i32 position, got {value:?}");
    };
    assert_eq!(
        pid,
        unsafe { libc::getpid() },
        "bridge result must equal libc"
    );
}

/// A fixed-family representative: `sysconf(i32) -> isize` is `IsizeI32`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn matrix_fixed_representative_matches_the_direct_call() {
    let value = oracle(
        LIB,
        "sysconf",
        AbiType::Isize,
        &[AbiType::I32],
        &[AbiValue::I32(libc::_SC_PAGESIZE)],
    )
    .expect("sysconf through the bridge");
    let AbiValue::Isize(pages) = value else {
        panic!("sysconf must return the declared isize position, got {value:?}");
    };
    assert_eq!(pages, unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as isize);
}

/// A pointer-family representative: `uname(void *) -> i32` is `I32Pointer`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn matrix_pointer_representative_matches_the_direct_call() {
    let mut bridged = std::mem::MaybeUninit::<libc::utsname>::zeroed();
    let mut direct = std::mem::MaybeUninit::<libc::utsname>::zeroed();
    let address = bridged.as_mut_ptr().cast::<c_void>();
    let value = unsafe {
        invoke_abi(&NativeCall {
            library: LIB,
            symbol: "uname",
            signature: AbiSignature {
                result: AbiType::I32,
                params: &[AbiType::Pointer],
            },
            arguments: &[AbiValue::Pointer(address)],
        })
    }
    .expect("uname through the bridge");
    assert_eq!(value, AbiValue::I32(0), "uname must report success");
    let direct_status = unsafe { libc::uname(direct.as_mut_ptr()) };
    assert_eq!(direct_status, 0, "the direct call must succeed");
    assert_eq!(
        unsafe { bridged.assume_init() }.sysname,
        unsafe { assume_name(&mut direct) }.sysname,
        "both paths must read the same kernel name"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn time_accepts_a_raw_nullable_pointer_position() {
    let value = oracle(
        LIB,
        "time",
        AbiType::I64,
        &[AbiType::Pointer],
        &[AbiValue::Pointer(std::ptr::null_mut())],
    )
    .expect("time(NULL) through the raw ABI");
    let AbiValue::I64(bridged) = value else {
        panic!("time must return i64, got {value:?}")
    };
    let direct = unsafe { libc::time(std::ptr::null_mut()) };
    assert!((bridged - direct).abs() <= 1);
}

/// A void result occupies no return register. `free(NULL)` is the C-defined
/// no-op oracle and therefore exercises the call without acquiring ownership.
#[cfg(unix)]
#[test]
fn void_pointer_shape_calls_free_null() {
    let value = unsafe {
        invoke_abi(&NativeCall {
            library: "",
            symbol: "free",
            signature: AbiSignature {
                result: AbiType::Void,
                params: &[AbiType::Pointer],
            },
            arguments: &[AbiValue::Pointer(std::ptr::null_mut())],
        })
    }
    .expect("free(NULL) through the raw ABI");
    assert_eq!(value, AbiValue::Void);
}

#[cfg(unix)]
#[test]
fn pointer_result_with_pointer_and_usize_matches_getcwd() {
    let mut bridged = [0_u8; 4096];
    let value = unsafe {
        invoke_abi(&NativeCall {
            library: "",
            symbol: "getcwd",
            signature: AbiSignature {
                result: AbiType::Pointer,
                params: &[AbiType::Pointer, AbiType::Usize],
            },
            arguments: &[
                AbiValue::Pointer(bridged.as_mut_ptr().cast()),
                AbiValue::Usize(bridged.len()),
            ],
        })
    }
    .expect("getcwd through the raw ABI");
    assert_eq!(value, AbiValue::Pointer(bridged.as_mut_ptr().cast()));

    let bridged = CStr::from_bytes_until_nul(&bridged).expect("getcwd terminates its output");
    let mut direct = [0_u8; 4096];
    let direct_result = unsafe { libc::getcwd(direct.as_mut_ptr().cast(), direct.len()) };
    assert_eq!(direct_result.cast::<u8>(), direct.as_mut_ptr());
    let direct = CStr::from_bytes_until_nul(&direct).expect("direct getcwd terminates its output");
    assert_eq!(bridged.to_bytes(), direct.to_bytes());
}

/// A borrowed host pointer remains usable by a raw Rust caller. It is not
/// automatically publishable through qjswasm, whose guest-memory policy is a
/// separate upper-layer decision.
#[cfg(unix)]
#[test]
fn pointer_result_with_pointer_matches_getenv() {
    let key = c"PATH";
    let value = unsafe {
        invoke_abi(&NativeCall {
            library: "",
            symbol: "getenv",
            signature: AbiSignature {
                result: AbiType::Pointer,
                params: &[AbiType::Pointer],
            },
            arguments: &[AbiValue::Pointer(key.as_ptr().cast_mut().cast())],
        })
    }
    .expect("getenv through the raw ABI");
    let AbiValue::Pointer(bridged) = value else {
        panic!("getenv must return the declared pointer position")
    };
    let direct = unsafe { libc::getenv(key.as_ptr()) };
    assert_eq!(bridged.cast::<libc::c_char>(), direct);
    assert!(!bridged.is_null(), "the test process must have PATH");
    assert_eq!(
        unsafe { CStr::from_ptr(bridged.cast()) }.to_bytes(),
        unsafe { CStr::from_ptr(direct) }.to_bytes()
    );
}

#[cfg(target_os = "windows")]
#[test]
fn pointer_result_with_pointer_loads_the_available_windows_crt() {
    let key = CString::new("PATH").expect("literal has no NUL");
    let arguments = [AbiValue::Pointer(key.as_ptr().cast_mut().cast())];
    let signature = AbiSignature {
        result: AbiType::Pointer,
        params: &[AbiType::Pointer],
    };
    let invoke = |library| unsafe {
        invoke_abi(&NativeCall {
            library,
            symbol: "getenv",
            signature,
            arguments: &arguments,
        })
    };
    let value = match invoke("ucrtbase.dll") {
        Ok(value) => value,
        Err(AbiError::LibraryLoad { .. }) => {
            invoke("msvcrt.dll").expect("one supported Windows CRT exports getenv")
        }
        Err(error) => panic!("unexpected ucrtbase getenv error: {error}"),
    };
    let AbiValue::Pointer(path) = value else {
        panic!("getenv must return the declared pointer position")
    };
    assert!(!path.is_null(), "the test process must have PATH");
    assert!(!unsafe { CStr::from_ptr(path.cast()) }.to_bytes().is_empty());
}

#[cfg(target_os = "windows")]
#[test]
fn windows_kernel32_covers_exact_fixed_pointer_and_reused_handle_calls() {
    let handle = LibraryHandle::open(LIB).expect("open kernel32 once");
    let pid_signature = AbiSignature {
        result: AbiType::U32,
        params: &[],
    };
    for _ in 0..3 {
        let value = unsafe {
            invoke_abi_with_handle(
                &handle,
                &NativeCall {
                    library: LIB,
                    symbol: "GetCurrentProcessId",
                    signature: pid_signature,
                    arguments: &[],
                },
            )
        }
        .expect("GetCurrentProcessId through one handle");
        assert_eq!(value, AbiValue::U32(std::process::id()));
    }

    let mut bridged_counter = 0_i64;
    let counter_arguments = [AbiValue::Pointer(
        std::ptr::from_mut(&mut bridged_counter).cast(),
    )];
    let value = unsafe {
        invoke_abi_with_handle(
            &handle,
            &NativeCall {
                library: LIB,
                symbol: "QueryPerformanceCounter",
                signature: AbiSignature {
                    result: AbiType::I32,
                    params: &[AbiType::Pointer],
                },
                arguments: &counter_arguments,
            },
        )
    }
    .expect("QueryPerformanceCounter through the fixed-pointer family");
    assert_eq!(value, AbiValue::I32(1));
    assert!(bridged_counter > 0);

    let mut direct_counter = 0_i64;
    assert_ne!(
        unsafe {
            windows_sys::Win32::System::Performance::QueryPerformanceCounter(
                &raw mut direct_counter,
            )
        },
        0
    );
    assert!(direct_counter > 0);
}

/// Pointer results are raw machine addresses. The mechanism preserves their
/// bits; ownership and dereference rules remain with the caller.
#[cfg(target_os = "macos")]
#[test]
fn pointer_result_shapes_match_direct_darwin_calls() {
    unsafe extern "C" {
        fn _NSGetMachExecuteHeader() -> *mut c_void;
        fn _dyld_get_image_name(image_index: u32) -> *mut c_void;
        fn _dyld_get_image_header(image_index: u32) -> *mut c_void;
    }

    let no_arguments = oracle(LIB, "getprogname", AbiType::Pointer, &[], &[])
        .expect("getprogname through the raw ABI");
    assert_eq!(
        no_arguments,
        AbiValue::Pointer(unsafe { libc::getprogname() }.cast_mut().cast())
    );
    let AbiValue::Pointer(program_name) = no_arguments else {
        unreachable!("the declared result is a pointer")
    };
    assert!(!program_name.is_null());
    assert_eq!(
        unsafe { CStr::from_ptr(program_name.cast()) }.to_bytes(),
        unsafe { CStr::from_ptr(libc::getprogname()) }.to_bytes()
    );

    for (symbol, direct) in [
        ("_NSGetArgc", unsafe { libc::_NSGetArgc() }.cast()),
        ("_NSGetArgv", unsafe { libc::_NSGetArgv() }.cast()),
        ("_NSGetEnviron", unsafe { libc::_NSGetEnviron() }.cast()),
        ("_NSGetProgname", unsafe { libc::_NSGetProgname() }.cast()),
        ("_NSGetMachExecuteHeader", unsafe {
            _NSGetMachExecuteHeader()
        }),
    ] {
        let actual = oracle(LIB, symbol, AbiType::Pointer, &[], &[])
            .unwrap_or_else(|error| panic!("{symbol} through the raw ABI: {error}"));
        assert_eq!(actual, AbiValue::Pointer(direct), "{symbol}");
        assert!(!direct.is_null(), "{symbol} must expose a borrowed address");
    }
    let argc = unsafe { libc::_NSGetArgc() };
    assert!(unsafe { *argc } >= 1, "process argc must be positive");
    let argv = unsafe { libc::_NSGetArgv() };
    assert!(!unsafe { *argv }.is_null(), "argv storage must exist");
    assert!(
        !unsafe { **argv }.is_null(),
        "argv[0] must name the process"
    );
    let environ = unsafe { libc::_NSGetEnviron() };
    assert!(
        !unsafe { *environ }.is_null(),
        "environment storage must exist"
    );
    let outer_program_name = unsafe { libc::_NSGetProgname() };
    assert!(!unsafe { *outer_program_name }.is_null());
    assert_eq!(
        unsafe { CStr::from_ptr(*outer_program_name) }.to_bytes(),
        unsafe { CStr::from_ptr(libc::getprogname()) }.to_bytes()
    );

    let u32_argument = oracle(
        LIB,
        "_dyld_get_image_header",
        AbiType::Pointer,
        &[AbiType::U32],
        &[AbiValue::U32(0)],
    )
    .expect("dyld image header through the raw ABI");
    assert_eq!(
        u32_argument,
        AbiValue::Pointer(unsafe { _dyld_get_image_header(0) })
    );
    assert_ne!(u32_argument, AbiValue::Pointer(std::ptr::null_mut()));
    let image_name = oracle(
        LIB,
        "_dyld_get_image_name",
        AbiType::Pointer,
        &[AbiType::U32],
        &[AbiValue::U32(0)],
    )
    .expect("dyld image name through the raw ABI");
    assert_eq!(
        image_name,
        AbiValue::Pointer(unsafe { _dyld_get_image_name(0) })
    );
    let AbiValue::Pointer(image_name) = image_name else {
        unreachable!("the declared result is a pointer")
    };
    assert!(!image_name.is_null());
    assert_eq!(
        unsafe { CStr::from_ptr(image_name.cast()) }.to_bytes(),
        unsafe { CStr::from_ptr(_dyld_get_image_name(0).cast()) }.to_bytes()
    );

    let thread = unsafe { libc::pthread_self() } as u64;
    let u64_argument = oracle(
        LIB,
        "pthread_get_stackaddr_np",
        AbiType::Pointer,
        &[AbiType::U64],
        &[AbiValue::U64(thread)],
    )
    .expect("pthread stack address through the raw ABI");
    assert_eq!(
        u64_argument,
        AbiValue::Pointer(unsafe { libc::pthread_get_stackaddr_np(libc::pthread_self()) })
    );
    assert_ne!(u64_argument, AbiValue::Pointer(std::ptr::null_mut()));
}

/// A direct heterogeneous scalar shape preserves the signed Darwin slide;
/// unlike the retired Lisp court, it does not disguise `isize` as a pointer.
#[cfg(target_os = "macos")]
#[test]
fn dyld_image_slide_matches_the_direct_signed_result() {
    unsafe extern "C" {
        fn _dyld_get_image_vmaddr_slide(image_index: u32) -> isize;
    }

    let value = oracle(
        LIB,
        "_dyld_get_image_vmaddr_slide",
        AbiType::Isize,
        &[AbiType::U32],
        &[AbiValue::U32(0)],
    )
    .expect("dyld image slide through the raw ABI");
    assert_eq!(
        value,
        AbiValue::Isize(unsafe { _dyld_get_image_vmaddr_slide(0) })
    );
}

/// The unified mechanism carries `size_t` without narrowing and leaves the
/// caller-owned string contract to this direct-oracle court.
#[cfg(target_os = "macos")]
#[test]
fn confstr_matches_the_direct_length_and_native_bytes() {
    let mut bridged = [0_u8; 4096];
    let value = oracle(
        LIB,
        "confstr",
        AbiType::Usize,
        &[AbiType::I32, AbiType::Pointer, AbiType::Usize],
        &[
            AbiValue::I32(libc::_CS_PATH),
            AbiValue::Pointer(bridged.as_mut_ptr().cast()),
            AbiValue::Usize(bridged.len()),
        ],
    )
    .expect("confstr through the raw ABI");
    let AbiValue::Usize(written) = value else {
        panic!("confstr must return the declared usize position, got {value:?}")
    };
    assert!(written > 1, "confstr(_CS_PATH) must write a non-empty path");

    let mut direct = [0_u8; 4096];
    let direct_len =
        unsafe { libc::confstr(libc::_CS_PATH, direct.as_mut_ptr().cast(), direct.len()) };
    assert_eq!(written, direct_len);
    assert_eq!(
        CStr::from_bytes_until_nul(&bridged)
            .expect("confstr must NUL-terminate successful output")
            .to_bytes(),
        CStr::from_bytes_until_nul(&direct)
            .expect("direct confstr must NUL-terminate successful output")
            .to_bytes()
    );
}

/// The raw mechanism passes an opaque output address; this court, rather than
/// dyn, owns the Darwin `rusage_info_v4` field interpretation.
#[cfg(target_os = "macos")]
#[test]
fn proc_pid_rusage_matches_direct_v4_fields() {
    let pid = unsafe { libc::getpid() };
    let flavor = libc::RUSAGE_INFO_V4;
    let mut bridged = unsafe { std::mem::zeroed::<libc::rusage_info_v4>() };
    let value = oracle(
        LIB,
        "proc_pid_rusage",
        AbiType::I32,
        &[AbiType::I32, AbiType::I32, AbiType::Pointer],
        &[
            AbiValue::I32(pid),
            AbiValue::I32(flavor),
            AbiValue::Pointer((&raw mut bridged).cast()),
        ],
    )
    .expect("proc_pid_rusage through the raw ABI");
    assert_eq!(value, AbiValue::I32(0));

    let mut direct = unsafe { std::mem::zeroed::<libc::rusage_info_v4>() };
    let direct_status = unsafe {
        libc::proc_pid_rusage(pid, flavor, (&raw mut direct).cast::<libc::rusage_info_t>())
    };
    assert_eq!(direct_status, 0, "direct proc_pid_rusage must succeed");
    assert_eq!(bridged.ri_uuid, direct.ri_uuid);
    assert_eq!(bridged.ri_proc_start_abstime, direct.ri_proc_start_abstime);
}

/// The mechanism transports the five ABI positions; this court alone assigns
/// Darwin `proc_bsdinfo` meaning to the output bytes.
#[cfg(target_os = "macos")]
#[test]
fn proc_pidinfo_matches_direct_bsdinfo_fields() {
    let pid = unsafe { libc::getpid() };
    let ppid = unsafe { libc::getppid() };
    let flavor = libc::PROC_PIDTBSDINFO;
    let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>())
        .expect("proc_bsdinfo size fits i32");
    let mut bridged = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let value = oracle(
        LIB,
        "proc_pidinfo",
        AbiType::I32,
        &[
            AbiType::I32,
            AbiType::I32,
            AbiType::U64,
            AbiType::Pointer,
            AbiType::I32,
        ],
        &[
            AbiValue::I32(pid),
            AbiValue::I32(flavor),
            AbiValue::U64(0),
            AbiValue::Pointer((&raw mut bridged).cast()),
            AbiValue::I32(size),
        ],
    )
    .expect("proc_pidinfo through the raw ABI");
    assert_eq!(value, AbiValue::I32(size));
    assert_eq!(bridged.pbi_pid, pid as u32);
    assert_eq!(bridged.pbi_ppid, ppid as u32);

    let mut direct = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let direct_bytes =
        unsafe { libc::proc_pidinfo(pid, flavor, 0, (&raw mut direct).cast(), size) };
    assert_eq!(direct_bytes, size, "direct proc_pidinfo must fill struct");
    assert_eq!(bridged.pbi_pid, direct.pbi_pid);
    assert_eq!(bridged.pbi_ppid, direct.pbi_ppid);
}

/// The mechanism transports the raw `sysctl` positions. This court owns the
/// MIB and CPU-count meaning, including its independent native comparison.
#[cfg(target_os = "macos")]
#[test]
fn sysctl_writes_the_direct_cpu_count() {
    let mut mib = [libc::CTL_HW, libc::HW_NCPU];
    let mut bridged = 0_i32;
    let mut bridged_len = std::mem::size_of_val(&bridged);
    let value = oracle(
        LIB,
        "sysctl",
        AbiType::I32,
        &[
            AbiType::Pointer,
            AbiType::U32,
            AbiType::Pointer,
            AbiType::Pointer,
            AbiType::Pointer,
            AbiType::Usize,
        ],
        &[
            AbiValue::Pointer(mib.as_mut_ptr().cast()),
            AbiValue::U32(2),
            AbiValue::Pointer((&raw mut bridged).cast()),
            AbiValue::Pointer((&raw mut bridged_len).cast()),
            AbiValue::Pointer(std::ptr::null_mut()),
            AbiValue::Usize(0),
        ],
    )
    .expect("sysctl through the raw ABI");
    assert_eq!(value, AbiValue::I32(0));
    assert!(bridged >= 1, "hw.ncpu must be at least 1");

    let mut direct_mib = [libc::CTL_HW, libc::HW_NCPU];
    let mut direct = 0_i32;
    let mut direct_len = std::mem::size_of_val(&direct);
    let direct_status = unsafe {
        libc::sysctl(
            direct_mib.as_mut_ptr(),
            2,
            (&raw mut direct).cast(),
            &mut direct_len,
            std::ptr::null_mut(),
            0,
        )
    };
    assert_eq!(direct_status, 0, "direct sysctl must succeed");
    assert_eq!(bridged_len, direct_len);
    assert_eq!(bridged, direct);
}

#[cfg(target_os = "macos")]
#[test]
fn two_pointer_result_buffer_matches_direct_dladdr_fields() {
    let address = libc::getpid as *mut c_void;
    let mut bridged = unsafe { std::mem::zeroed::<libc::Dl_info>() };
    let value = oracle(
        LIB,
        "dladdr",
        AbiType::I32,
        &[AbiType::Pointer, AbiType::Pointer],
        &[
            AbiValue::Pointer(address),
            AbiValue::Pointer((&raw mut bridged).cast()),
        ],
    )
    .expect("dladdr through the raw ABI");
    let AbiValue::I32(status) = value else {
        panic!("dladdr must return i32, got {value:?}")
    };
    assert_ne!(status, 0);

    let mut direct = unsafe { std::mem::zeroed::<libc::Dl_info>() };
    assert_ne!(unsafe { libc::dladdr(address, &mut direct) }, 0);
    assert_eq!(bridged.dli_saddr, direct.dli_saddr);
    assert_eq!(bridged.dli_fname.is_null(), direct.dli_fname.is_null());
    if !bridged.dli_fname.is_null() {
        assert_eq!(
            unsafe { CStr::from_ptr(bridged.dli_fname) }.to_bytes(),
            unsafe { CStr::from_ptr(direct.dli_fname) }.to_bytes()
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
unsafe fn assume_name(slot: &mut std::mem::MaybeUninit<libc::utsname>) -> libc::utsname {
    unsafe { slot.assume_init() }
}

#[test]
fn argument_count_and_shape_are_mechanism_errors() {
    let too_few = NativeCall {
        library: LIB,
        symbol: "uname",
        signature: AbiSignature {
            result: AbiType::I32,
            params: &[AbiType::Pointer],
        },
        arguments: &[],
    };
    assert_eq!(
        validate_abi(&too_few),
        Err(AbiError::ArgumentCount {
            expected: 1,
            actual: 0
        })
    );
    let wrong_position = NativeCall {
        library: LIB,
        symbol: "uname",
        signature: AbiSignature {
            result: AbiType::I32,
            params: &[AbiType::Pointer],
        },
        arguments: &[AbiValue::I32(7)],
    };
    assert_eq!(
        validate_abi(&wrong_position),
        Err(AbiError::ArgumentShape {
            index: 0,
            expected: AbiType::Pointer,
            actual: AbiType::I32
        })
    );
}

/// Name every public ABI mechanism refusal without a wildcard.
///
/// This match is deliberately exhaustive: adding, removing, or renaming an
/// `AbiError` variant must stop this owner court at compile time so the stable
/// mechanism vocabulary cannot drift behind broad `matches!(..)` assertions.
fn abi_error_word(error: &AbiError) -> &'static str {
    match error {
        AbiError::SignatureUnsupported { .. } => "signature_unsupported",
        AbiError::LibraryLoad { .. } => "library_load",
        AbiError::SymbolLookup { .. } => "symbol_lookup",
        AbiError::ArgumentCount { .. } => "argument_count",
        AbiError::ArgumentShape { .. } => "argument_shape",
    }
}

#[test]
fn the_public_abi_error_vocabulary_is_one_exhaustive_five_word_algebra() {
    let errors = [
        AbiError::SignatureUnsupported {
            result: AbiType::Void,
            params: vec![],
        },
        AbiError::LibraryLoad {
            library: "missing".into(),
            message: "load".into(),
        },
        AbiError::SymbolLookup {
            library: "library".into(),
            symbol: "missing".into(),
            message: "lookup".into(),
        },
        AbiError::ArgumentCount {
            expected: 1,
            actual: 0,
        },
        AbiError::ArgumentShape {
            index: 0,
            expected: AbiType::I32,
            actual: AbiType::U32,
        },
    ];
    assert_eq!(
        errors.map(|error| abi_error_word(&error)),
        [
            "signature_unsupported",
            "library_load",
            "symbol_lookup",
            "argument_count",
            "argument_shape",
        ]
    );
}

#[test]
fn shapes_outside_the_mechanism_matrix_are_refused() {
    // Four pointer parameters: no family trampoline takes that many addresses.
    let four_pointers = NativeCall {
        library: LIB,
        symbol: "whatever",
        signature: AbiSignature {
            result: AbiType::I32,
            params: &[
                AbiType::Pointer,
                AbiType::Pointer,
                AbiType::Pointer,
                AbiType::Pointer,
            ],
        },
        arguments: &[AbiValue::Pointer(std::ptr::null_mut()); 4],
    };
    assert_eq!(
        validate_abi(&four_pointers),
        Err(AbiError::SignatureUnsupported {
            result: AbiType::I32,
            params: vec![
                AbiType::Pointer,
                AbiType::Pointer,
                AbiType::Pointer,
                AbiType::Pointer
            ],
        })
    );
    // A heterogeneous shape no fixed prototype covers.
    let heterogeneous = NativeCall {
        library: LIB,
        symbol: "whatever",
        signature: AbiSignature {
            result: AbiType::U32,
            params: &[AbiType::F64, AbiType::F64],
        },
        arguments: &[AbiValue::F64(0.0), AbiValue::F64(0.0)],
    };
    assert!(matches!(
        validate_abi(&heterogeneous),
        Err(AbiError::SignatureUnsupported { .. })
    ));
}

/// Nullability is an upper-layer schema decision, not an ABI one: the raw ABI has
/// a single pointer position, an empty pointer is a pointer, and the mechanism
/// admits it wherever it admits a non-empty one. The two-pointer shape therefore
/// maps onto the plain prototype.
#[test]
fn an_empty_pointer_is_still_a_pointer_for_the_mechanism() {
    let empty = NativeCall {
        library: LIB,
        symbol: "whatever",
        signature: AbiSignature {
            result: AbiType::I32,
            params: &[AbiType::Pointer, AbiType::Pointer],
        },
        arguments: &[AbiValue::Pointer(std::ptr::null_mut()); 2],
    };
    assert!(
        validate_abi(&empty).is_ok(),
        "two pointer positions, both empty, are a supported shape"
    );

    let occupied = NativeCall {
        library: LIB,
        symbol: "whatever",
        signature: AbiSignature {
            result: AbiType::I32,
            params: &[AbiType::Pointer, AbiType::Pointer],
        },
        arguments: &[
            AbiValue::Pointer(std::ptr::null_mut()),
            AbiValue::Pointer(std::ptr::dangling_mut::<c_void>()),
        ],
    };
    assert!(
        validate_abi(&occupied).is_ok(),
        "the same shape with one occupied pointer is equally supported"
    );

    // The raw position has no nullable variant to choose from: a pointer is a
    // pointer, so the *same* signed shape is the only declaration available.
    assert_eq!(
        AbiValue::Pointer(std::ptr::null_mut()).ty(),
        AbiType::Pointer,
        "an empty pointer must occupy the one pointer position"
    );
}

/// A shape the mechanism can execute but whose symbol is absent must fail as a
/// mechanism error, not as a fabricated success.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn a_missing_symbol_is_a_symbol_lookup_error() {
    let error = oracle(LIB, "agenterm_no_such_symbol_xyz", AbiType::I32, &[], &[])
        .expect_err("an absent symbol must be refused");
    assert!(
        matches!(error, AbiError::SymbolLookup { .. }),
        "expected a symbol lookup error, got {error:?}"
    );
}

#[test]
fn a_missing_pointer_result_symbol_keeps_the_shared_lookup_error() {
    let call = NativeCall {
        library: "",
        symbol: "agenterm_no_such_pointer_symbol_xyz",
        signature: AbiSignature {
            result: AbiType::Pointer,
            params: &[],
        },
        arguments: &[],
    };
    let error = unsafe { invoke_abi(&call) }.expect_err("the symbol must not exist");
    assert!(
        matches!(error, AbiError::SymbolLookup { .. }),
        "expected SymbolLookup, got {error:?}"
    );
}

#[test]
fn a_missing_fixed_pointer_symbol_keeps_the_mechanism_error_boundary() {
    let call = NativeCall {
        library: "",
        symbol: "agenterm_no_such_fixed_pointer_symbol_xyz",
        signature: AbiSignature {
            result: AbiType::I32,
            params: &[AbiType::Pointer],
        },
        arguments: &[AbiValue::Pointer(std::ptr::null_mut())],
    };
    let error = unsafe { invoke_abi(&call) }.expect_err("the symbol must not exist");
    assert!(
        matches!(error, AbiError::SymbolLookup { .. }),
        "expected SymbolLookup, got {error:?}"
    );
}

/// Every shape the mechanism matrix admits, as `(result, params)`.
///
/// This is the owner's inventory: the sweep below asserts each entry still has a
/// real trampoline in this build, so a shape dropped from the matrix reddens
/// here instead of silently vanishing from the mechanism.
fn mechanism_shapes() -> Vec<(AbiType, Vec<AbiType>)> {
    let scalars = [
        AbiType::I32,
        AbiType::U32,
        AbiType::I64,
        AbiType::U64,
        AbiType::Isize,
        AbiType::Usize,
        AbiType::F64,
    ];
    let mut shapes = Vec::new();
    // exact: 7 homogeneous scalar families x arity 0..=6.
    for ty in scalars {
        for arity in 0..=6 {
            shapes.push((ty, vec![ty; arity]));
        }
    }
    // fixed: 4 heterogeneous scalar shapes.
    shapes.push((AbiType::U64, vec![AbiType::I32]));
    shapes.push((AbiType::Isize, vec![AbiType::I32]));
    shapes.push((AbiType::I32, vec![AbiType::U64, AbiType::U64]));
    shapes.push((AbiType::I64, vec![AbiType::I32, AbiType::I64, AbiType::I32]));
    // fixed-pointer: 8 pointer-bearing shapes.
    shapes.push((AbiType::I32, vec![AbiType::Pointer]));
    shapes.push((AbiType::I32, vec![AbiType::Pointer, AbiType::I32]));
    shapes.push((AbiType::I32, vec![AbiType::Pointer, AbiType::U64]));
    shapes.push((AbiType::I32, vec![AbiType::I32, AbiType::Pointer]));
    shapes.push((
        AbiType::I32,
        vec![AbiType::I32, AbiType::Pointer, AbiType::U32],
    ));
    shapes.push((
        AbiType::I32,
        vec![AbiType::U64, AbiType::Pointer, AbiType::U64],
    ));
    shapes.push((AbiType::I32, vec![AbiType::Pointer, AbiType::Pointer]));
    shapes.push((
        AbiType::I32,
        vec![AbiType::Pointer, AbiType::Pointer, AbiType::Pointer],
    ));
    // pointer-result: 5 shapes that return a raw address.
    shapes.push((AbiType::Pointer, vec![]));
    shapes.push((AbiType::Pointer, vec![AbiType::U32]));
    shapes.push((AbiType::Pointer, vec![AbiType::U64]));
    shapes.push((AbiType::Pointer, vec![AbiType::Pointer]));
    shapes.push((AbiType::Pointer, vec![AbiType::Pointer, AbiType::Usize]));
    // direct-scalar: 9 shapes the unified mechanism implements itself.
    shapes.push((AbiType::Void, vec![AbiType::Pointer]));
    shapes.push((AbiType::I64, vec![AbiType::Pointer]));
    shapes.push((AbiType::Isize, vec![AbiType::U32]));
    shapes.push((
        AbiType::I32,
        vec![AbiType::I32, AbiType::I32, AbiType::Pointer],
    ));
    shapes.push((
        AbiType::I32,
        vec![
            AbiType::I32,
            AbiType::I32,
            AbiType::U64,
            AbiType::Pointer,
            AbiType::I32,
        ],
    ));
    shapes.push((
        AbiType::I32,
        vec![
            AbiType::Pointer,
            AbiType::U32,
            AbiType::Pointer,
            AbiType::Pointer,
            AbiType::Pointer,
            AbiType::Usize,
        ],
    ));
    shapes.push((
        AbiType::Usize,
        vec![AbiType::I32, AbiType::Pointer, AbiType::Usize],
    ));
    shapes.push((AbiType::I32, vec![AbiType::U32, AbiType::U32]));
    shapes.push((AbiType::I32, vec![AbiType::I32, AbiType::U32]));
    shapes
}

#[test]
fn the_signature_query_accepts_every_shape_in_the_mechanism_matrix() {
    let shapes = mechanism_shapes();
    assert_eq!(shapes.len(), 75, "mechanism matrix inventory");
    for (result, params) in &shapes {
        let signature = AbiSignature {
            result: *result,
            params,
        };
        assert!(
            validate_abi_signature(signature).is_ok(),
            "the mechanism lost its trampoline for {result:?}({params:?})"
        );
    }
}

#[test]
fn the_signature_query_refuses_every_shape_outside_the_matrix() {
    for (result, params) in [
        (AbiType::Void, vec![]),
        (AbiType::I32, vec![AbiType::F64]),
        (AbiType::U32, vec![AbiType::U32; 7]),
        (AbiType::Pointer, vec![AbiType::I32]),
        (
            AbiType::I32,
            vec![
                AbiType::Pointer,
                AbiType::Pointer,
                AbiType::Pointer,
                AbiType::Pointer,
            ],
        ),
    ] {
        let signature = AbiSignature {
            result,
            params: &params,
        };
        assert!(
            matches!(
                validate_abi_signature(signature),
                Err(AbiError::SignatureUnsupported { .. })
            ),
            "{result:?}({params:?}) must be refused by the mechanism"
        );
    }
}

/// The query takes only the description, so it needs no argument values and no
/// fabricated buffer. `validate_abi` is the entry that also checks a real
/// argument list, and the two must not disagree about the shape.
#[test]
fn the_signature_query_agrees_with_validate_abi_without_needing_arguments() {
    let params = vec![AbiType::I32, AbiType::Pointer];
    let signature = AbiSignature {
        result: AbiType::I32,
        params: &params,
    };
    assert!(validate_abi_signature(signature).is_ok());
    let arguments = [AbiValue::I32(1), AbiValue::Pointer(std::ptr::null_mut())];
    // A null address is an ABI-level pointer like any other: nullability is the
    // caller's contract, so the mechanism accepts the shape either way.
    let call = NativeCall {
        library: LIB,
        symbol: "uname",
        signature,
        arguments: &arguments,
    };
    assert!(validate_abi(&call).is_ok());
    let short = [AbiValue::I32(1)];
    let call = NativeCall {
        library: LIB,
        symbol: "uname",
        signature,
        arguments: &short,
    };
    assert!(matches!(
        validate_abi(&call),
        Err(AbiError::ArgumentCount { .. })
    ));
    assert!(
        validate_abi_signature(signature).is_ok(),
        "the shape is still supported when the argument list is wrong"
    );
}

/// The reusable handle must return exactly what the one-shot entry returns.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn the_handle_entry_and_the_one_shot_entry_agree_bit_for_bit() {
    let handle = LibraryHandle::open(LIB).expect("open one reusing handle");
    assert_eq!(handle.library_name(), LIB);
    // exact family: `getpid()` is `i32()`.
    let exact = AbiSignature {
        result: AbiType::I32,
        params: &[],
    };
    let one_shot = unsafe {
        invoke_abi(&NativeCall {
            library: LIB,
            symbol: "getpid",
            signature: exact,
            arguments: &[],
        })
    }
    .expect("one-shot getpid");
    let reused = unsafe {
        invoke_abi_with_handle(
            &handle,
            &NativeCall {
                library: LIB,
                symbol: "getpid",
                signature: exact,
                arguments: &[],
            },
        )
    }
    .expect("getpid through the handle");
    assert_eq!(one_shot, reused);
    // Non-exact family: `void(ptr)` direct-scalar, `free(NULL)`.
    let void_pointer = AbiSignature {
        result: AbiType::Void,
        params: &[AbiType::Pointer],
    };
    let arguments = [AbiValue::Pointer(std::ptr::null_mut())];
    let one_shot = unsafe {
        invoke_abi(&NativeCall {
            library: LIB,
            symbol: "free",
            signature: void_pointer,
            arguments: &arguments,
        })
    }
    .expect("one-shot free(NULL)");
    let reused = unsafe {
        invoke_abi_with_handle(
            &handle,
            &NativeCall {
                library: LIB,
                symbol: "free",
                signature: void_pointer,
                arguments: &arguments,
            },
        )
    }
    .expect("free(NULL) through the handle");
    assert_eq!(one_shot, reused);
}

/// One handle serves repeated calls, including a pointer-result shape.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn one_handle_serves_repeated_calls() {
    let handle = LibraryHandle::open(LIB).expect("open one reusing handle");
    let exact = AbiSignature {
        result: AbiType::I32,
        params: &[],
    };
    for _ in 0..3 {
        let value = unsafe {
            invoke_abi_with_handle(
                &handle,
                &NativeCall {
                    library: LIB,
                    symbol: "getpid",
                    signature: exact,
                    arguments: &[],
                },
            )
        }
        .expect("getpid through the handle");
        assert_eq!(value, AbiValue::I32(std::process::id() as i32));
    }
    let buffer_params = [AbiType::Pointer, AbiType::Usize];
    let pointer_result = AbiSignature {
        result: AbiType::Pointer,
        params: &buffer_params,
    };
    let mut buffer = vec![0_u8; 4096];
    let arguments = [
        AbiValue::Pointer(buffer.as_mut_ptr().cast()),
        AbiValue::Usize(buffer.len()),
    ];
    let first = unsafe {
        invoke_abi_with_handle(
            &handle,
            &NativeCall {
                library: LIB,
                symbol: "getcwd",
                signature: pointer_result,
                arguments: &arguments,
            },
        )
    }
    .expect("getcwd through the handle");
    let second = unsafe {
        invoke_abi_with_handle(
            &handle,
            &NativeCall {
                library: LIB,
                symbol: "getcwd",
                signature: pointer_result,
                arguments: &arguments,
            },
        )
    }
    .expect("getcwd again through the same handle");
    assert_eq!(first, second);
    assert_ne!(first, AbiValue::Pointer(std::ptr::null_mut()));
}

/// The handle entry keeps the mechanism error vocabulary and adds no new code.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn the_handle_keeps_the_mechanism_error_vocabulary() {
    let handle = LibraryHandle::open(LIB).expect("open one reusing handle");
    let exact = AbiSignature {
        result: AbiType::I32,
        params: &[],
    };
    let error = unsafe {
        invoke_abi_with_handle(
            &handle,
            &NativeCall {
                library: LIB,
                symbol: "agenterm_dyn_absent_symbol",
                signature: exact,
                arguments: &[],
            },
        )
    }
    .expect_err("an absent symbol is refused");
    assert!(
        matches!(error, AbiError::SymbolLookup { .. }),
        "expected SymbolLookup, got {error:?}"
    );
    let error = unsafe {
        invoke_abi_with_handle(
            &handle,
            &NativeCall {
                library: "agenterm-other-library",
                symbol: "getpid",
                signature: exact,
                arguments: &[],
            },
        )
    }
    .expect_err("a call naming another library is refused");
    assert!(
        matches!(error, AbiError::LibraryLoad { .. }),
        "expected LibraryLoad, got {error:?}"
    );
    let unsupported = AbiSignature {
        result: AbiType::Pointer,
        params: &[AbiType::I32],
    };
    let error = unsafe {
        invoke_abi_with_handle(
            &handle,
            &NativeCall {
                library: LIB,
                symbol: "getpid",
                signature: unsupported,
                arguments: &[AbiValue::I32(1)],
            },
        )
    }
    .expect_err("a shape outside the matrix is refused");
    assert!(
        matches!(error, AbiError::SignatureUnsupported { .. }),
        "expected SignatureUnsupported, got {error:?}"
    );
}

/// The parameter list is borrowed, not `'static`: a caller may build it at run
/// time. This test only compiles if `AbiSignature` accepts a borrowed slice.
#[test]
fn the_parameter_list_may_be_built_at_run_time() {
    let params: Vec<AbiType> = vec![AbiType::Pointer];
    let call = NativeCall {
        library: LIB,
        symbol: "uname",
        signature: AbiSignature {
            result: AbiType::I32,
            params: &params,
        },
        arguments: &[AbiValue::Pointer(std::ptr::null_mut())],
    };
    assert!(validate_abi(&call).is_ok());
}
