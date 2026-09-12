use agenterm_dyn::{
    FixedNativeCall, FixedNativeError, FixedNativePrototype, FixedNativeValue, invoke_fixed,
};

#[test]
fn wrong_arguments_are_rejected_before_library_loading() {
    let arguments = [FixedNativeValue::I64(1)];
    let call = FixedNativeCall {
        library: "agenterm-native-library-that-does-not-exist",
        symbol: "unused",
        prototype: FixedNativePrototype::IsizeI32,
        arguments: &arguments,
    };
    // SAFETY: signature rejection occurs before loading or calling.
    assert!(matches!(
        unsafe { invoke_fixed(&call) },
        Err(FixedNativeError::SignatureUnsupported { .. })
    ));
}

#[test]
fn admitted_signature_reaches_library_loading() {
    let arguments = [FixedNativeValue::I32(1)];
    let call = FixedNativeCall {
        library: "agenterm-native-library-that-does-not-exist",
        symbol: "unused",
        prototype: FixedNativePrototype::IsizeI32,
        arguments: &arguments,
    };
    // SAFETY: the deliberately absent library prevents native execution.
    assert!(matches!(
        unsafe { invoke_fixed(&call) },
        Err(FixedNativeError::LibraryLoad { .. })
    ));
}

#[test]
fn u64_i32_rejects_the_wrong_argument_before_library_loading() {
    let arguments = [FixedNativeValue::U64(1)];
    let call = FixedNativeCall {
        library: "agenterm-native-library-that-does-not-exist",
        symbol: "unused",
        prototype: FixedNativePrototype::U64I32,
        arguments: &arguments,
    };
    // SAFETY: signature rejection occurs before loading or calling.
    assert!(matches!(
        unsafe { invoke_fixed(&call) },
        Err(FixedNativeError::SignatureUnsupported { .. })
    ));
}

#[test]
fn i32_u64_u64_rejects_the_wrong_argument_before_library_loading() {
    let arguments = [FixedNativeValue::U64(1), FixedNativeValue::I64(1)];
    let call = FixedNativeCall {
        library: "agenterm-native-library-that-does-not-exist",
        symbol: "unused",
        prototype: FixedNativePrototype::I32U64U64,
        arguments: &arguments,
    };
    // SAFETY: signature rejection occurs before loading or calling.
    assert!(matches!(
        unsafe { invoke_fixed(&call) },
        Err(FixedNativeError::SignatureUnsupported { .. })
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn pthread_equal_recognizes_the_current_thread() {
    let first = unsafe { libc::pthread_self() } as u64;
    let second = unsafe { libc::pthread_self() } as u64;
    let arguments = [FixedNativeValue::U64(first), FixedNativeValue::U64(second)];
    let call = FixedNativeCall {
        library: "",
        symbol: "pthread_equal",
        prototype: FixedNativePrototype::I32U64U64,
        arguments: &arguments,
    };
    // SAFETY: these values came from pthread_self and Darwin's pthread_t is the
    // u64 ABI asserted by this enumerated prototype.
    let actual = unsafe { invoke_fixed(&call) }.expect("pthread_equal should resolve");
    assert!(matches!(actual, FixedNativeValue::I32(value) if value != 0));
    let direct =
        unsafe { libc::pthread_equal(first as libc::pthread_t, second as libc::pthread_t) };
    assert_ne!(direct, 0, "direct C call must recognize the current thread");
}

#[cfg(target_os = "macos")]
#[test]
fn clock_gettime_nsec_np_matches_the_darwin_oracle() {
    let clock_id = i32::try_from(libc::CLOCK_UPTIME_RAW).expect("Darwin clock id fits i32");
    let arguments = [FixedNativeValue::I32(clock_id)];
    let call = FixedNativeCall {
        library: "",
        symbol: "clock_gettime_nsec_np",
        prototype: FixedNativePrototype::U64I32,
        arguments: &arguments,
    };
    // SAFETY: Darwin exports clock_gettime_nsec_np with this uint64_t(int) ABI.
    let actual = unsafe { invoke_fixed(&call) }.expect("clock_gettime_nsec_np should resolve");
    unsafe extern "C" {
        fn clock_gettime_nsec_np(clock_id: libc::clockid_t) -> u64;
    }
    // SAFETY: CLOCK_UPTIME_RAW is a supported Darwin clock id.
    let expected = unsafe { clock_gettime_nsec_np(libc::CLOCK_UPTIME_RAW) };
    let FixedNativeValue::U64(actual) = actual else {
        panic!("clock_gettime_nsec_np must return u64");
    };
    assert!(expected >= actual);
}

#[cfg(unix)]
#[test]
fn sysconf_pagesize_matches_the_platform_oracle() {
    let arguments = [FixedNativeValue::I32(libc::_SC_PAGESIZE)];
    let call = FixedNativeCall {
        library: "",
        symbol: "sysconf",
        prototype: FixedNativePrototype::IsizeI32,
        arguments: &arguments,
    };
    // SAFETY: sysconf has the declared C long(int) signature.
    let actual = unsafe { invoke_fixed(&call) }.expect("sysconf should resolve");
    // SAFETY: libc declares the platform's authoritative sysconf signature.
    let expected = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as isize;
    assert_eq!(actual, FixedNativeValue::Isize(expected));
    assert!(expected > 0);
}

#[cfg(unix)]
#[test]
fn lseek_moves_an_owned_file_descriptor_to_the_exact_offset() {
    use std::fs::{File, OpenOptions};
    use std::io::{Seek, SeekFrom, Write};
    use std::os::fd::AsRawFd;
    use std::path::PathBuf;

    struct OwnedTemp {
        file: File,
        path: PathBuf,
    }

    impl Drop for OwnedTemp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    let path = std::env::temp_dir().join(format!(
        "agenterm-dyn-fixed-native-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("create owned temporary file");
    let mut fixture = OwnedTemp { file, path };
    fixture.file.write_all(b"agenterm").expect("write fixture");
    let arguments = [
        FixedNativeValue::I32(fixture.file.as_raw_fd()),
        FixedNativeValue::I64(3),
        FixedNativeValue::I32(libc::SEEK_SET),
    ];
    let call = FixedNativeCall {
        library: "",
        symbol: "lseek",
        prototype: FixedNativePrototype::I64I32I64I32,
        arguments: &arguments,
    };
    // SAFETY: lseek has the declared off_t(int, off_t, int) signature on the
    // supported Unix targets, and file owns the live descriptor.
    let actual = unsafe { invoke_fixed(&call) }.expect("lseek should resolve");
    assert_eq!(actual, FixedNativeValue::I64(3));
    assert_eq!(fixture.file.stream_position().expect("observe offset"), 3);
    fixture
        .file
        .seek(SeekFrom::Start(0))
        .expect("reset fixture");
}
