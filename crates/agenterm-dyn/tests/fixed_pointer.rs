#[cfg(unix)]
mod unix {
    use std::ffi::{CString, c_void};

    use agenterm_dyn::{
        FixedPointerCall, FixedPointerError, FixedPointerPrototype, FixedPointerType,
        FixedPointerValue, invoke_fixed_pointer,
    };

    #[test]
    fn prototype_shapes_name_pointer_positions_exactly() {
        assert_eq!(
            FixedPointerPrototype::I32Pointer.parameters(),
            &[FixedPointerType::Pointer]
        );
        assert_eq!(
            FixedPointerPrototype::I32I32Pointer.parameters(),
            &[FixedPointerType::I32, FixedPointerType::Pointer]
        );
        assert_eq!(
            FixedPointerPrototype::I32PointerI32.parameters(),
            &[FixedPointerType::Pointer, FixedPointerType::I32]
        );
    }

    #[test]
    fn wrong_arguments_are_rejected_before_library_loading() {
        let arguments = [FixedPointerValue::I32(1)];
        let call = FixedPointerCall {
            library: "agenterm-native-library-that-does-not-exist",
            symbol: "unused",
            prototype: FixedPointerPrototype::I32Pointer,
            arguments: &arguments,
        };
        // SAFETY: signature rejection occurs before loading or calling.
        assert!(matches!(
            unsafe { invoke_fixed_pointer(&call) },
            Err(FixedPointerError::SignatureUnsupported { .. })
        ));
    }

    #[test]
    fn uname_writes_the_same_pointer_free_facts_as_the_platform_oracle() {
        // SAFETY: zero is a valid initial representation for utsname output storage.
        let mut actual: libc::utsname = unsafe { std::mem::zeroed() };
        // SAFETY: same as above.
        let mut expected: libc::utsname = unsafe { std::mem::zeroed() };
        let arguments = [FixedPointerValue::Pointer(
            (&mut actual as *mut libc::utsname).cast::<c_void>(),
        )];
        let call = FixedPointerCall {
            library: "",
            symbol: "uname",
            prototype: FixedPointerPrototype::I32Pointer,
            arguments: &arguments,
        };
        // SAFETY: uname has C int(void *) ABI and actual is live writable utsname storage.
        let rc = unsafe { invoke_fixed_pointer(&call) }.expect("uname should resolve");
        // SAFETY: libc owns the authoritative declaration and expected is writable.
        let oracle_rc = unsafe { libc::uname(&mut expected) };
        assert_eq!(rc, oracle_rc);
        assert_eq!(actual.sysname, expected.sysname);
        assert_eq!(actual.release, expected.release);
        assert_eq!(actual.version, expected.version);
        assert_eq!(actual.machine, expected.machine);
    }

    #[test]
    fn getrlimit_writes_the_same_struct_as_the_platform_oracle() {
        #[cfg(target_os = "linux")]
        let resource = libc::RLIMIT_NOFILE as i32;
        #[cfg(not(target_os = "linux"))]
        let resource = libc::RLIMIT_NOFILE;
        let mut actual = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let mut expected = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        let arguments = [
            FixedPointerValue::I32(resource),
            FixedPointerValue::Pointer((&mut actual as *mut libc::rlimit).cast::<c_void>()),
        ];
        let call = FixedPointerCall {
            library: "",
            symbol: "getrlimit",
            prototype: FixedPointerPrototype::I32I32Pointer,
            arguments: &arguments,
        };
        // SAFETY: getrlimit has the declared ABI and actual is live writable rlimit storage.
        let rc = unsafe { invoke_fixed_pointer(&call) }.expect("getrlimit should resolve");
        // SAFETY: libc owns the authoritative declaration and expected is writable.
        let oracle_rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut expected) };
        assert_eq!(rc, oracle_rc);
        assert_eq!(actual.rlim_cur, expected.rlim_cur);
        assert_eq!(actual.rlim_max, expected.rlim_max);
    }

    #[test]
    fn access_matches_the_platform_oracle_for_an_existing_path() {
        let path = CString::new("/").expect("fixed path has no NUL");
        let arguments = [
            FixedPointerValue::Pointer(path.as_ptr().cast_mut().cast::<c_void>()),
            FixedPointerValue::I32(libc::F_OK),
        ];
        let call = FixedPointerCall {
            library: "",
            symbol: "access",
            prototype: FixedPointerPrototype::I32PointerI32,
            arguments: &arguments,
        };
        // SAFETY: access has the declared ABI and path stays live through the call.
        let rc = unsafe { invoke_fixed_pointer(&call) }.expect("access should resolve");
        // SAFETY: libc owns the authoritative declaration and path is a live C string.
        let oracle_rc = unsafe { libc::access(path.as_ptr(), libc::F_OK) };
        assert_eq!(rc, oracle_rc);
        assert_eq!(rc, 0);
    }
}
