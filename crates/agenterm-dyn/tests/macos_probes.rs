//! Darwin-only live `dlcall` probes for facts absent from Linux host rows.

#![cfg(target_os = "macos")]

use std::ffi::{CStr, CString, c_void};

use agenterm_dyn::{DlAddressSnapshot, Dyn, SystemProbeStatus, Value, live_cell};

const LIB: &str = "libSystem.B.dylib";

fn eval_native(env: &mut Dyn, source: &str) -> Result<Value, agenterm_dyn::DynError> {
    // SAFETY: each probe documents its C ABI and owns every writable buffer.
    unsafe { env.eval_native(source) }
}

fn live_symbol(name: &str) -> &'static str {
    let probe = live_cell()
        .expect("macOS host cell")
        .system_probes
        .iter()
        .find(|probe| probe.name == name)
        .expect("Darwin probe is catalogued");
    match probe.status {
        SystemProbeStatus::LiveDlcall { lib: LIB, symbol }
        | SystemProbeStatus::LiveDlcallOwned {
            lib: LIB, symbol, ..
        } => symbol,
        other => panic!("{name} must be a live libSystem probe, got {other:?}"),
    }
}

#[test]
fn dlcall_getprogname_matches_libc_c_string() {
    let symbol = live_symbol("getprogname");
    let mut env = Dyn::new();
    let got = eval_native(&mut env, &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr")"#))
        .expect("getprogname dlcall")
        .as_ptr()
        .expect("program name pointer") as *const libc::c_char;
    let direct = unsafe { libc::getprogname() };
    assert!(!got.is_null(), "dlcall must return a program-name pointer");
    assert!(!direct.is_null(), "libc must return a program-name pointer");
    let got = unsafe { CStr::from_ptr(got) };
    let direct = unsafe { CStr::from_ptr(direct) };
    assert_eq!(got.to_bytes(), direct.to_bytes());
}

#[test]
fn dlcall_sysctl_writes_ncpu_into_caller_buffer() {
    let symbol = live_symbol("sysctl");
    let mut mib = [libc::CTL_HW, libc::HW_NCPU];
    let mut ncpu: i32 = 0;
    let mut oldlen = std::mem::size_of_val(&ncpu);
    let mut env = Dyn::new();
    env.bind("mib", mib.as_mut_ptr().cast())
        .expect("bind sysctl mib");
    env.bind("oldp", (&mut ncpu as *mut i32).cast())
        .expect("bind ncpu output");
    env.bind("oldlenp", (&mut oldlen as *mut usize).cast())
        .expect("bind ncpu output length");
    let got = eval_native(&mut env, &format!(
            r#"(dlcall "{LIB}" "{symbol}" "i32" "ptr" mib "u32" 2 "ptr" oldp "ptr" oldlenp "ptr" 0 "u64" 0)"#
        ))
        .expect("sysctl dlcall");
    assert_eq!(got, Value::Int(0));
    assert!(ncpu >= 1, "hw.ncpu must be at least 1");

    let mut direct: i32 = 0;
    let mut direct_len = std::mem::size_of_val(&direct);
    let mut direct_mib = [libc::CTL_HW, libc::HW_NCPU];
    let direct_status = unsafe {
        libc::sysctl(
            direct_mib.as_mut_ptr(),
            2,
            (&mut direct as *mut i32).cast(),
            &mut direct_len,
            std::ptr::null_mut(),
            0,
        )
    };
    assert_eq!(direct_status, 0, "direct sysctl must succeed");
    assert_eq!(ncpu, direct);
}

#[test]
fn dlcall_sysctlnametomib_writes_caller_owned_mib() {
    unsafe extern "C" {
        fn sysctlnametomib(
            name: *const libc::c_char,
            mibp: *mut libc::c_int,
            sizep: *mut usize,
        ) -> libc::c_int;
    }

    let symbol = live_symbol("sysctlnametomib");
    let name = CString::new("hw.ncpu").expect("literal has no NUL");
    let mut mib = [0 as libc::c_int; 8];
    let mut len = mib.len();
    let mut env = Dyn::new();
    env.bind("name", name.as_ptr().cast_mut().cast::<c_void>())
        .expect("bind sysctl name");
    env.bind("mib", mib.as_mut_ptr().cast())
        .expect("bind MIB output");
    env.bind("len", (&mut len as *mut usize).cast())
        .expect("bind MIB output length");
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "i32" "ptr" name "ptr" mib "ptr" len)"#),
    )
    .expect("sysctlnametomib dlcall");
    assert_eq!(got, Value::Int(0));
    assert!((1..=mib.len()).contains(&len), "MIB length must fit output");

    let mut direct = [0 as libc::c_int; 8];
    let mut direct_len = direct.len();
    let direct_status =
        unsafe { sysctlnametomib(name.as_ptr(), direct.as_mut_ptr(), &mut direct_len) };
    assert_eq!(direct_status, 0, "direct sysctlnametomib must succeed");
    assert_eq!(len, direct_len);
    assert_eq!(&mib[..len], &direct[..direct_len]);
}

#[test]
fn dlcall_proc_pidinfo_writes_caller_owned_bsdinfo() {
    let symbol = live_symbol("proc_pidinfo");
    let pid = unsafe { libc::getpid() };
    let ppid = unsafe { libc::getppid() };
    let flavor = libc::PROC_PIDTBSDINFO;
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let bufsize =
        i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).expect("struct fits i32");
    let mut env = Dyn::new();
    env.bind("info", (&raw mut info).cast())
        .expect("bind proc_bsdinfo");
    let got = eval_native(&mut env, &format!(
            r#"(dlcall "{LIB}" "{symbol}" "i32" "i32" {pid} "i32" {flavor} "u64" 0 "ptr" info "i32" {bufsize})"#
        ))
        .expect("proc_pidinfo dlcall")
        .as_int()
        .expect("proc_pidinfo byte count");
    assert_eq!(got, i64::from(bufsize));
    assert_eq!(info.pbi_pid, pid as u32);
    assert_eq!(info.pbi_ppid, ppid as u32);

    let mut direct = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let direct_bytes =
        unsafe { libc::proc_pidinfo(pid, flavor, 0, (&raw mut direct).cast(), bufsize) };
    assert_eq!(
        direct_bytes, bufsize,
        "direct proc_pidinfo must fill struct"
    );
    assert_eq!(info.pbi_pid, direct.pbi_pid);
    assert_eq!(info.pbi_ppid, direct.pbi_ppid);
}

#[test]
fn dlcall_nsget_argc_matches_libc_pointer_and_count() {
    let symbol = live_symbol("nsget_argc");
    let mut env = Dyn::new();
    let got = eval_native(&mut env, &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr")"#))
        .expect("_NSGetArgc dlcall")
        .as_ptr()
        .expect("_NSGetArgc pointer") as *mut i32;
    assert!(
        !got.is_null(),
        "_NSGetArgc must return a non-null int pointer"
    );
    let argc = unsafe { *got };
    assert!(argc >= 1, "process argc must be at least 1");
    let direct = unsafe { libc::_NSGetArgc() };
    assert_eq!(got, direct);
    assert_eq!(argc, unsafe { *direct });
}

#[test]
fn dlcall_nsget_argv_matches_libc_borrowed_pointer() {
    let symbol = live_symbol("nsget_argv");
    let mut env = Dyn::new();
    let got = eval_native(&mut env, &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr")"#))
        .expect("_NSGetArgv dlcall")
        .as_ptr()
        .expect("_NSGetArgv pointer") as *mut *mut *mut libc::c_char;
    assert!(
        !got.is_null(),
        "_NSGetArgv must return a non-null outer pointer"
    );
    let direct = unsafe { libc::_NSGetArgv() };
    assert_eq!(got, direct);
    let argv = unsafe { *got };
    assert!(!argv.is_null(), "_NSGetArgv must expose argv storage");
    assert_eq!(argv, unsafe { *direct });
    assert!(!unsafe { *argv }.is_null(), "argv[0] must name the process");
}

#[test]
fn dlcall_nsget_environ_matches_libc_borrowed_pointer() {
    let symbol = live_symbol("nsget_environ");
    let mut env = Dyn::new();
    let got = eval_native(&mut env, &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr")"#))
        .expect("_NSGetEnviron dlcall")
        .as_ptr()
        .expect("_NSGetEnviron pointer") as *mut *mut *mut libc::c_char;
    assert!(
        !got.is_null(),
        "_NSGetEnviron must return a non-null outer pointer"
    );
    let direct = unsafe { libc::_NSGetEnviron() };
    assert_eq!(got, direct);
    let environ = unsafe { *got };
    assert!(
        !environ.is_null(),
        "_NSGetEnviron must expose environ storage"
    );
    assert_eq!(environ, unsafe { *direct });
}

#[test]
fn dlcall_proc_pid_rusage_writes_caller_owned_v4() {
    let symbol = live_symbol("proc_pid_rusage");
    let pid = unsafe { libc::getpid() };
    let flavor = libc::RUSAGE_INFO_V4;
    let mut ri = unsafe { std::mem::zeroed::<libc::rusage_info_v4>() };
    let mut env = Dyn::new();
    env.bind("ri", (&raw mut ri).cast())
        .expect("bind rusage_info_v4");
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "i32" "i32" {pid} "i32" {flavor} "ptr" ri)"#),
    )
    .expect("proc_pid_rusage dlcall");
    assert_eq!(got, Value::Int(0));

    let mut direct = unsafe { std::mem::zeroed::<libc::rusage_info_v4>() };
    let direct_status = unsafe {
        libc::proc_pid_rusage(pid, flavor, (&raw mut direct).cast::<libc::rusage_info_t>())
    };
    assert_eq!(direct_status, 0, "direct proc_pid_rusage must succeed");
    assert_eq!(ri.ri_uuid, direct.ri_uuid);
    assert_eq!(ri.ri_proc_start_abstime, direct.ri_proc_start_abstime);
}

#[test]
fn dlcall_getentropy_fills_caller_owned_buffer() {
    const BYTES: usize = 16;

    let symbol = live_symbol("getentropy");
    let mut bytes = [0_u8; BYTES];
    let mut env = Dyn::new();
    env.bind("bytes", bytes.as_mut_ptr().cast())
        .expect("bind entropy output");
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "i32" "ptr" bytes "u64" {BYTES})"#),
    )
    .expect("getentropy dlcall");
    assert_eq!(got, Value::Int(0));

    let mut direct = [0_u8; BYTES];
    let direct_status = unsafe { libc::getentropy(direct.as_mut_ptr().cast(), BYTES) };
    assert_eq!(direct_status, 0, "direct getentropy must succeed");
}

#[test]
fn dlcall_pthread_get_stackaddr_np_matches_libc_current_thread() {
    let symbol = live_symbol("pthread_get_stackaddr_np");
    let thread = unsafe { libc::pthread_self() } as u64;
    let mut env = Dyn::new();
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr" "u64" {thread})"#),
    )
    .expect("pthread_get_stackaddr_np dlcall")
    .as_ptr()
    .expect("pthread_get_stackaddr_np pointer") as *mut c_void;
    let direct = unsafe { libc::pthread_get_stackaddr_np(libc::pthread_self()) };
    assert!(
        !got.is_null(),
        "current thread stack address must be non-null"
    );
    assert!(
        !direct.is_null(),
        "direct thread stack address must be non-null"
    );
    assert_eq!(got, direct);
}

#[test]
fn dlcall_nsget_progname_matches_libc_outer_pointer_and_c_string() {
    let symbol = live_symbol("nsget_progname");
    let mut env = Dyn::new();
    let got = eval_native(&mut env, &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr")"#))
        .expect("_NSGetProgname dlcall")
        .as_ptr()
        .expect("_NSGetProgname outer pointer") as *mut *mut libc::c_char;
    let direct = unsafe { libc::_NSGetProgname() };
    assert!(
        !got.is_null(),
        "_NSGetProgname must return an outer pointer"
    );
    assert_eq!(got, direct);
    let name = unsafe { *got };
    let direct_name = unsafe { libc::getprogname() };
    assert!(!name.is_null(), "_NSGetProgname must expose a program name");
    assert!(
        !direct_name.is_null(),
        "getprogname must return a program name"
    );
    assert_eq!(
        unsafe { CStr::from_ptr(name) }.to_bytes(),
        unsafe { CStr::from_ptr(direct_name) }.to_bytes()
    );
}

#[test]
fn dlcall_confstr_writes_cs_path() {
    let symbol = live_symbol("confstr");
    let name = libc::_CS_PATH;
    let mut buffer = [0_u8; 4096];
    let len = buffer.len();
    let mut env = Dyn::new();
    env.bind("buf", buffer.as_mut_ptr().cast())
        .expect("bind confstr buffer");
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "u64" "i32" {name} "ptr" buf "u64" {len})"#),
    )
    .expect("confstr dlcall")
    .as_int()
    .expect("confstr size");
    assert!(got > 1, "confstr(_CS_PATH) must write a non-empty path");

    let mut direct = [0_u8; 4096];
    let direct_len = unsafe { libc::confstr(name, direct.as_mut_ptr().cast(), direct.len()) };
    assert_eq!(got, direct_len as i64);
    assert_eq!(
        CStr::from_bytes_until_nul(&buffer)
            .expect("confstr must NUL-terminate successful output")
            .to_bytes(),
        CStr::from_bytes_until_nul(&direct)
            .expect("direct confstr must NUL-terminate successful output")
            .to_bytes()
    );
}

#[test]
fn dlcall_nsget_mach_execute_header_matches_direct_c() {
    unsafe extern "C" {
        fn _NSGetMachExecuteHeader() -> *mut c_void;
    }

    let symbol = live_symbol("nsget_mach_execute_header");
    let mut env = Dyn::new();
    let got = eval_native(&mut env, &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr")"#))
        .expect("_NSGetMachExecuteHeader dlcall")
        .as_ptr()
        .expect("_NSGetMachExecuteHeader pointer") as *mut c_void;
    assert!(
        !got.is_null(),
        "_NSGetMachExecuteHeader must return a non-null header"
    );
    let direct = unsafe { _NSGetMachExecuteHeader() };
    assert_eq!(got, direct);
}

#[test]
fn dlcall_dyld_get_image_name_matches_image_zero() {
    unsafe extern "C" {
        fn _dyld_get_image_name(image_index: u32) -> *const libc::c_char;
    }

    let symbol = live_symbol("dyld_get_image_name");
    let mut env = Dyn::new();
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr" "u32" 0)"#),
    )
    .expect("_dyld_get_image_name dlcall")
    .as_ptr()
    .expect("_dyld_get_image_name pointer") as *const libc::c_char;
    assert!(
        !got.is_null(),
        "_dyld_get_image_name(0) must return a C string"
    );
    let direct = unsafe { _dyld_get_image_name(0) };
    assert!(!direct.is_null(), "direct image-zero name must be non-null");
    assert_eq!(
        unsafe { CStr::from_ptr(got) }.to_bytes(),
        unsafe { CStr::from_ptr(direct) }.to_bytes()
    );
}

#[test]
fn dlcall_dyld_get_image_vmaddr_slide_matches_image_zero() {
    unsafe extern "C" {
        fn _dyld_get_image_vmaddr_slide(image_index: u32) -> isize;
    }

    let symbol = live_symbol("dyld_get_image_vmaddr_slide");
    let mut env = Dyn::new();
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr" "u32" 0)"#),
    )
    .expect("_dyld_get_image_vmaddr_slide dlcall")
    .as_ptr()
    .expect("_dyld_get_image_vmaddr_slide pointer") as *mut c_void;
    let direct = unsafe { _dyld_get_image_vmaddr_slide(0) } as *mut c_void;
    assert_eq!(got, direct);
}

#[test]
fn dlcall_dladdr_writes_caller_owned_info() {
    let symbol = live_symbol("dladdr");
    let addr = libc::getpid as *mut c_void;
    let mut info = unsafe { std::mem::zeroed::<libc::Dl_info>() };
    let mut env = Dyn::new();
    env.bind("addr", addr).expect("bind live function address");
    env.bind("info", (&raw mut info).cast())
        .expect("bind Dl_info output");
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "i32" "ptr" addr "ptr" info)"#),
    )
    .expect("dladdr dlcall")
    .as_int()
    .expect("dladdr integer status");
    assert_ne!(got, 0, "dladdr must resolve a live function address");

    let mut direct = unsafe { std::mem::zeroed::<libc::Dl_info>() };
    let direct_status = unsafe { libc::dladdr(addr, &mut direct) };
    assert_ne!(direct_status, 0, "direct dladdr must succeed");
    assert_eq!(info.dli_saddr, direct.dli_saddr);
    assert_eq!(info.dli_fname.is_null(), direct.dli_fname.is_null());
    if !info.dli_fname.is_null() {
        assert_eq!(
            unsafe { CStr::from_ptr(info.dli_fname) }.to_bytes(),
            unsafe { CStr::from_ptr(direct.dli_fname) }.to_bytes()
        );
    }

    let owned = DlAddressSnapshot::current_image().expect("typed current-image snapshot");
    let mut local = unsafe { std::mem::zeroed::<libc::Dl_info>() };
    let local_status = unsafe {
        libc::dladdr(
            dlcall_dladdr_writes_caller_owned_info as *const () as *const c_void,
            &mut local,
        )
    };
    assert_ne!(local_status, 0, "direct local-image dladdr must succeed");
    assert!(!local.dli_fname.is_null(), "local image has a path");
    assert_eq!(
        owned.image_path(),
        unsafe { CStr::from_ptr(local.dli_fname) }.to_bytes(),
        "typed snapshot copies the same current-image native path"
    );
}

#[test]
fn dlcall_dyld_get_image_header_matches_image_zero() {
    unsafe extern "C" {
        fn _dyld_get_image_header(image_index: u32) -> *const c_void;
    }

    let symbol = live_symbol("dyld_get_image_header");
    let mut env = Dyn::new();
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr" "u32" 0)"#),
    )
    .expect("_dyld_get_image_header dlcall")
    .as_ptr()
    .expect("_dyld_get_image_header pointer") as *const c_void;
    assert!(
        !got.is_null(),
        "_dyld_get_image_header(0) must return a header"
    );
    let direct = unsafe { _dyld_get_image_header(0) };
    assert!(
        !direct.is_null(),
        "direct image-zero header must be non-null"
    );
    assert_eq!(got, direct);
}

#[test]
fn dlcall_realpath_resolves_root() {
    let symbol = live_symbol("realpath");
    let path = CString::new("/").expect("root path literal has no NUL");
    let mut buf = vec![0_u8; libc::PATH_MAX as usize];
    let bound_buf = buf.as_mut_ptr();
    let mut env = Dyn::new();
    env.bind("path", path.as_ptr().cast_mut().cast::<c_void>())
        .expect("bind realpath input");
    env.bind("buf", bound_buf.cast())
        .expect("bind realpath output");
    let got = eval_native(
        &mut env,
        &format!(r#"(dlcall "{LIB}" "{symbol}" "ptr" "ptr" path "ptr" buf)"#),
    )
    .expect("realpath dlcall")
    .as_ptr()
    .expect("realpath pointer");
    assert_eq!(
        got, bound_buf as usize,
        "realpath must return the bound buffer"
    );
    let resolved =
        CStr::from_bytes_until_nul(&buf).expect("realpath must NUL-terminate successful output");
    assert_eq!(resolved.to_bytes(), b"/");

    let mut later = vec![0_u8; libc::PATH_MAX as usize];
    let later_ptr = unsafe { libc::realpath(path.as_ptr(), later.as_mut_ptr().cast()) };
    assert_eq!(
        later_ptr.cast::<u8>(),
        later.as_mut_ptr(),
        "direct realpath must return its caller buffer"
    );
    assert_eq!(
        CStr::from_bytes_until_nul(&later)
            .expect("direct realpath must NUL-terminate successful output")
            .to_bytes(),
        b"/"
    );
}
