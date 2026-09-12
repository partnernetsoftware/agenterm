//! Darwin-only live `dlcall` probes for facts absent from Linux host rows.

#![cfg(target_os = "macos")]

use agenterm_dyn::{Dyn, SystemProbeStatus, Value, live_cell};

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
