//! The policy-free ABI bridge: the mechanism matrix, and delegation to the
//! existing family entries. Every claim here is about *mechanism*: which shapes
//! can be executed, that a caller-declared shape reaching the same family entry
//! produces the same result as the direct native call, and that the raw ABI
//! carries no nullability policy.

use std::ffi::c_void;

use agenterm_dyn::{
    AbiError, AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi, validate_abi,
};

const LIB: &str = "libSystem.B.dylib";

#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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

/// Pointer results are raw machine addresses. The mechanism preserves their
/// bits; ownership and dereference rules remain with the caller.
#[cfg(target_os = "macos")]
#[test]
fn pointer_result_shapes_match_direct_darwin_calls() {
    unsafe extern "C" {
        fn _dyld_get_image_header(image_index: u32) -> *mut c_void;
    }

    let no_arguments = oracle(LIB, "getprogname", AbiType::Pointer, &[], &[])
        .expect("getprogname through the raw ABI");
    assert_eq!(
        no_arguments,
        AbiValue::Pointer(unsafe { libc::getprogname() }.cast_mut().cast())
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
}

#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
#[test]
fn a_missing_symbol_is_a_symbol_lookup_error() {
    let error = oracle(LIB, "agenterm_no_such_symbol_xyz", AbiType::I32, &[], &[])
        .expect_err("an absent symbol must be refused");
    assert!(
        matches!(error, AbiError::SymbolLookup { .. }),
        "expected a symbol lookup error, got {error:?}"
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
