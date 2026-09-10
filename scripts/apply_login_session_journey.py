#!/usr/bin/env python3
"""Atomic Linux login-session CEO journey: sources, configs, build, smoke, commit."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGET_DIR = ROOT / "target/abi-dev"
SMOKE_ENV = {
    "DISPLAY": ":2",
    "XDG_RUNTIME_DIR": "/tmp/xdg-runtime-box-2",
    "DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/dbus-sHZ4cfPDF4,guid=a3377782fc9878e89e5808586a8e3b0d",
}

LINUX_LOGIN_SESSION_RS = r'''//! Linux login-session inventory from systemd-logind via dlopen libsystemd.
//!
//! Screen-lock delivery is intentionally unsupported on Linux; callers must treat
//! [`lock_console`] as an honest host limit rather than a silent no-op.

#![cfg(target_os = "linux")]

use std::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr::null_mut,
};

use sha2::{Digest as _, Sha256};

use crate::login_session::{
    LOGIN_SESSION_DISPLAY_NAME_MAX_BYTES, LOGIN_SESSION_MAX_ROWS, LOGIN_SESSION_USERNAME_MAX_BYTES,
    LoginSessionError, LoginSessionErrorKind, LoginSessionInventory, LoginSessionProvider,
    NativeLoginSessionRow, finish_inventory,
};

const LIBSYSTEMD_SONAME: &CStr = c"libsystemd.so.0";
const MAX_SESSION_VALUE_BYTES: usize = 256;

type SdGetSessions = unsafe extern "C" fn(*mut *mut *mut c_char) -> c_int;
type SdSessionGetUid = unsafe extern "C" fn(*const c_char, *mut libc::uid_t) -> c_int;
type SdSessionGetUser = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type SdSessionGetDisplay = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type SdSessionGetLeader = unsafe extern "C" fn(*const c_char, *mut libc::pid_t) -> c_int;
type SdSessionGetVt = unsafe extern "C" fn(*const c_char, *mut u32) -> c_int;
type SdSessionPredicate = unsafe extern "C" fn(*const c_char) -> c_int;
type SdSessionGetString = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type SdSessionGetLockedHint = unsafe extern "C" fn(*const c_char, *mut c_int) -> c_int;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 0;

struct SystemdLogin {
    handle: *mut c_void,
    get_sessions: SdGetSessions,
    session_get_uid: SdSessionGetUid,
    session_get_user: SdSessionGetUser,
    session_get_display: SdSessionGetDisplay,
    session_get_leader: SdSessionGetLeader,
    session_get_vt: SdSessionGetVt,
    session_is_active: SdSessionPredicate,
    session_is_remote: SdSessionPredicate,
    session_get_type: SdSessionGetString,
    session_get_class: SdSessionGetString,
    session_get_state: SdSessionGetString,
    session_get_seat: SdSessionGetString,
    session_get_locked_hint: SdSessionGetLockedHint,
}

impl SystemdLogin {
    fn load() -> Result<Self, LoginSessionError> {
        // SAFETY: fixed SONAME is NUL-terminated for dlopen.
        let handle = unsafe { dlopen(LIBSYSTEMD_SONAME.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if handle.is_null() {
            return Err(unsupported(
                "libsystemd.so.0 is unavailable on this Linux host",
            ));
        }
        let loaded = (|| {
            Ok(Self {
                handle,
                get_sessions: unsafe { load_symbol(handle, c"sd_get_sessions")? },
                session_get_uid: unsafe { load_symbol(handle, c"sd_session_get_uid")? },
                session_get_user: unsafe { load_symbol(handle, c"sd_session_get_user")? },
                session_get_display: unsafe { load_symbol(handle, c"sd_session_get_display")? },
                session_get_leader: unsafe { load_symbol(handle, c"sd_session_get_leader")? },
                session_get_vt: unsafe { load_symbol(handle, c"sd_session_get_vt")? },
                session_is_active: unsafe { load_symbol(handle, c"sd_session_is_active")? },
                session_is_remote: unsafe { load_symbol(handle, c"sd_session_is_remote")? },
                session_get_type: unsafe { load_symbol(handle, c"sd_session_get_type")? },
                session_get_class: unsafe { load_symbol(handle, c"sd_session_get_class")? },
                session_get_state: unsafe { load_symbol(handle, c"sd_session_get_state")? },
                session_get_seat: unsafe { load_symbol(handle, c"sd_session_get_seat")? },
                session_get_locked_hint: unsafe {
                    load_symbol(handle, c"sd_session_get_locked_hint")?
                },
            })
        })();
        if loaded.is_err() {
            // SAFETY: handle came from successful dlopen above.
            unsafe { dlclose(handle) };
        }
        loaded
    }

    fn inventory(&self) -> Result<(bool, Vec<NativeLoginSessionRow>), LoginSessionError> {
        let mut values = null_mut();
        // SAFETY: sd_get_sessions initializes an allocator-owned string array.
        let count = unsafe { (self.get_sessions)(&mut values) };
        if count < 0 || (count > 0 && values.is_null()) {
            return Err(provider_unavailable(
                "sd_get_sessions could not read the login-session inventory",
            ));
        }
        let count = usize::try_from(count).map_err(|_| {
            shape("sd_get_sessions returned an invalid session count")
        })?;
        if count > LOGIN_SESSION_MAX_ROWS {
            return Err(shape("native inventory exceeds the session row ceiling"));
        }
        let values = OwnedStringArray { values, count };
        let mut rows = Vec::with_capacity(count);
        let mut locked = false;
        for index in 0..count {
            // SAFETY: libsystemd returned `count` session identifiers.
            let pointer = unsafe { values.values.add(index).read() };
            let id = borrowed_value(pointer, "login-session id")?;
            let row = self.parse_row(&id)?;
            if row.on_console {
                locked = self.session_locked(&id)?;
            }
            rows.push(row);
        }
        Ok((locked, rows))
    }

    fn parse_row(&self, id: &[u8]) -> Result<NativeLoginSessionRow, LoginSessionError> {
        let id_c = c_value(id, "login-session id")?;
        let id_text = id_c
            .to_str()
            .map_err(|_| shape("login-session id is not UTF-8"))?;
        let mut uid = 0;
        // SAFETY: id is a bounded NUL-terminated session identifier.
        if unsafe { (self.session_get_uid)(id_c.as_ptr(), &mut uid) } < 0 {
            return Err(provider_unavailable("sd_session_get_uid failed"));
        }
        let username = self.session_string(self.session_get_user, id_c, "username")?;
        let display_name =
            self.session_optional_string(self.session_get_display, id_c, "display name")?;
        let mut leader = 0;
        if unsafe { (self.session_get_leader)(id_c.as_ptr(), &mut leader) } < 0 {
            return Err(provider_unavailable("sd_session_get_leader failed"));
        }
        let mut vt = 0;
        if unsafe { (self.session_get_vt)(id_c.as_ptr(), &mut vt) } < 0 {
            return Err(provider_unavailable("sd_session_get_vt failed"));
        }
        let active = predicate(self.session_is_active, id_c, "active flag")?;
        let remote = predicate(self.session_is_remote, id_c, "remote flag")?;
        let session_type = self.session_string(self.session_get_type, id_c, "session type")?;
        let class = self.session_string(self.session_get_class, id_c, "session class")?;
        let state = self.session_string(self.session_get_state, id_c, "session state")?;
        let seat = self.session_optional_string(self.session_get_seat, id_c, "session seat")?;
        let graphical = matches!(session_type.as_str(), "x11" | "wayland");
        let user_class = class == "user";
        let state_active = state == "active";
        let on_console = active
            && !remote
            && graphical
            && user_class
            && state_active
            && !seat.is_empty();
        let login_complete = user_class && state_active;
        let group_id = group_for_uid(uid)?;
        Ok(NativeLoginSessionRow {
            uuid: session_uuid(id_text),
            session_id: session_numeric_id(id_text),
            security_session_id: u64::try_from(leader.max(0)).unwrap_or(0),
            audit_id: u64::from(vt),
            user_id: u64::from(uid),
            group_id: u64::from(group_id),
            username,
            display_name,
            on_console,
            login_complete,
        })
    }

    fn session_locked(&self, id: &[u8]) -> Result<bool, LoginSessionError> {
        let id = c_value(id, "login-session id")?;
        let mut locked = 0;
        // SAFETY: id is valid for the duration of the call.
        let status = unsafe { (self.session_get_locked_hint)(id.as_ptr(), &mut locked) };
        if status < 0 {
            return Err(provider_unavailable("sd_session_get_locked_hint failed"));
        }
        Ok(locked > 0)
    }

    fn session_string(
        &self,
        getter: SdSessionGetString,
        id: &CStr,
        field: &'static str,
    ) -> Result<String, LoginSessionError> {
        let mut value = null_mut();
        // SAFETY: getter follows sd-login allocated-string ownership.
        let status = unsafe { getter(id.as_ptr(), &mut value) };
        let bytes = owned_value(status, value, field)?;
        bytes_to_bounded_text(field, &bytes, 1, LOGIN_SESSION_USERNAME_MAX_BYTES)
    }

    fn session_optional_string(
        &self,
        getter: SdSessionGetString,
        id: &CStr,
        field: &'static str,
    ) -> Result<String, LoginSessionError> {
        let mut value = null_mut();
        // SAFETY: ENODATA means the session has no optional value.
        let status = unsafe { getter(id.as_ptr(), &mut value) };
        if status == -libc::ENODATA {
            return Ok(String::new());
        }
        let bytes = owned_value(status, value, field)?;
        bytes_to_bounded_text(field, &bytes, 0, LOGIN_SESSION_DISPLAY_NAME_MAX_BYTES)
    }
}

impl Drop for SystemdLogin {
    fn drop(&mut self) {
        // SAFETY: unique dlopen handle; all uses finished before Drop.
        unsafe { dlclose(self.handle) };
    }
}

pub(crate) fn inventory() -> Result<LoginSessionInventory, LoginSessionError> {
    let api = SystemdLogin::load()?;
    let (locked, rows) = api.inventory()?;
    finish_inventory(LoginSessionProvider::LinuxSdLogin, locked, rows)
}

pub(crate) fn lock_console() -> Result<(), LoginSessionError> {
    Err(LoginSessionError::new(
        LoginSessionErrorKind::Unsupported,
        "Linux login-session screen-lock delivery is unsupported; use the host screen locker",
    ))
}

unsafe fn load_symbol<T: Copy>(handle: *mut c_void, symbol: &CStr) -> Result<T, LoginSessionError> {
    // SAFETY: live dlopen handle and NUL-terminated symbol name.
    let pointer = unsafe { dlsym(handle, symbol.as_ptr()) };
    if pointer.is_null() {
        return Err(unsupported(
            "libsystemd lacks a required sd-login operation for login-session inventory",
        ));
    }
    debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
    // SAFETY: T matches the named libsystemd function pointer type.
    Ok(unsafe { std::mem::transmute_copy(&pointer) })
}

fn predicate(
    function: SdSessionPredicate,
    id: &CStr,
    field: &'static str,
) -> Result<bool, LoginSessionError> {
    // SAFETY: id is a bounded NUL-terminated session identifier.
    let result = unsafe { function(id.as_ptr()) };
    if result < 0 {
        Err(provider_unavailable(format!("{field} is unavailable")))
    } else {
        Ok(result > 0)
    }
}

fn c_value<'a>(value: &'a [u8], field: &'static str) -> Result<&'a CStr, LoginSessionError> {
    if value.is_empty() || value.len() > MAX_SESSION_VALUE_BYTES + 1 {
        return Err(shape(format!("{field} has an invalid native shape")));
    }
    CStr::from_bytes_with_nul(value)
        .map_err(|_| shape(format!("{field} has an invalid native shape")))
}

fn borrowed_value(pointer: *mut c_char, field: &'static str) -> Result<Vec<u8>, LoginSessionError> {
    if pointer.is_null() {
        return Err(shape(format!("{field} is missing")));
    }
    // SAFETY: libsystemd returns a NUL-terminated string for each inventory entry.
    let value = unsafe { CStr::from_ptr(pointer) }.to_bytes_with_nul();
    validate_value(value, field)?;
    Ok(value.to_vec())
}

fn owned_value(
    status: c_int,
    pointer: *mut c_char,
    field: &'static str,
) -> Result<Vec<u8>, LoginSessionError> {
    if status < 0 || pointer.is_null() {
        if !pointer.is_null() {
            // SAFETY: non-NULL sd-login output is malloc-allocated.
            unsafe { libc::free(pointer.cast()) };
        }
        return Err(provider_unavailable(format!("{field} is unavailable")));
    }
    let result = borrowed_value(pointer, field);
    // SAFETY: successful sd-login string outputs transfer ownership to the caller.
    unsafe { libc::free(pointer.cast()) };
    result
}

fn validate_value(value: &[u8], field: &'static str) -> Result<(), LoginSessionError> {
    if value.len() <= 1
        || value.len() > MAX_SESSION_VALUE_BYTES + 1
        || value[..value.len() - 1]
            .iter()
            .any(|byte| !byte.is_ascii_graphic())
    {
        return Err(shape(format!("{field} has an invalid native shape")));
    }
    Ok(())
}

fn bytes_to_bounded_text(
    field: &'static str,
    value: &[u8],
    minimum: usize,
    maximum: usize,
) -> Result<String, LoginSessionError> {
    let text = std::str::from_utf8(value)
        .map_err(|_| shape(format!("{field} is not valid UTF-8")))?
        .to_owned();
    if text.len() < minimum
        || text.len() > maximum
        || text
            .as_bytes()
            .iter()
            .any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    {
        return Err(shape(format!(
            "{field} must be {minimum}..={maximum} UTF-8 bytes without NUL or newline"
        )));
    }
    Ok(text)
}

fn group_for_uid(uid: libc::uid_t) -> Result<u32, LoginSessionError> {
    let mut buffer = [0_u8; 16_384];
    let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    // SAFETY: scratch buffer and record outlive getpwuid_r.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            record.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Ok(0);
    }
    // SAFETY: getpwuid_r succeeded and result points at initialized record.
    let gid = unsafe { (*result).pw_gid };
    u32::try_from(gid).map_err(|_| shape("native group id exceeds the portable range"))
}

fn session_uuid(session_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"agenterm-platform/linux-login-session-uuid/v1\0");
    digest.update(session_id.as_bytes());
    let bytes = digest.finalize();
    format!(
        "{:08X}-{:04X}-4{:03X}-8{:03X}-{:012X}",
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]) & 0x0fff,
        u16::from_be_bytes([bytes[8], bytes[9]]) & 0x3fff,
        u128::from_be_bytes([
            0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16],
            bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
        ]) & 0xffffffffffff
    )
}

fn session_numeric_id(session_id: &str) -> u64 {
    if session_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return session_id.parse().unwrap_or(1).max(1);
    }
    let mut hash = 0_u64;
    for byte in session_id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u64::from(byte));
    }
    hash.max(1)
}

struct OwnedStringArray {
    values: *mut *mut c_char,
    count: usize,
}

impl Drop for OwnedStringArray {
    fn drop(&mut self) {
        if self.values.is_null() {
            return;
        }
        for index in 0..self.count {
            // SAFETY: values points to `count` entries owned by this guard.
            let pointer = unsafe { self.values.add(index).read() };
            if !pointer.is_null() {
                // SAFETY: each returned string is independently malloc-allocated.
                unsafe { libc::free(pointer.cast()) };
            }
        }
        // SAFETY: outer vector is also malloc-allocated by libsystemd.
        unsafe { libc::free(self.values.cast()) };
    }
}

fn unsupported(detail: impl Into<String>) -> LoginSessionError {
    LoginSessionError::new(LoginSessionErrorKind::Unsupported, detail)
}

fn provider_unavailable(detail: impl Into<String>) -> LoginSessionError {
    LoginSessionError::new(LoginSessionErrorKind::ProviderUnavailable, detail)
}

fn shape(detail: impl Into<String>) -> LoginSessionError {
    LoginSessionError::new(LoginSessionErrorKind::ProviderShape, detail)
}
'''

SMOKE_QJS = r'''// CEO: Linux login-session status honesty with independent loginctl/session-env read-back.
import * as rh from "lib/rh_compat";
import * as harness from "lib/test_harness";

const declared_evidence = ["cu.linux-login-session-readback"];

function ensure(c, m) { if (!c) throw m; }
function text_at(v, d) {
  let c = v; for (const k of d.split(".")) c = rh.field(c, k);
  return c === undefined || c === null ? "" : "" + c;
}
function cu_env(exe) {
  const env = { AGENTERM_ABI_LIB: rh.join(rh.parent(exe), "libagenterm.so"), AGENTERM_NO_ACTIVATE: "1" };
  for (const k of ["DISPLAY","XDG_RUNTIME_DIR","DBUS_SESSION_BUS_ADDRESS","AT_SPI_BUS_ADDRESS","AT_SPI_BUS","XDG_SESSION_ID"]) {
    if (rh.has_env(k)) env[k] = rh.env_or(k, "");
  }
  return env;
}
function invoke(dir, exe, args, ok) {
  const out = rh.command(exe, ["--target","current","--grant","observe","login-session"].concat(args), { cwd: dir, env: cu_env(exe), timeout_ms: 30000 });
  const reply = JSON.parse(out.stdout.trim());
  ensure(reply.ok === ok, "reply:" + out.stdout);
  ensure(out.success === ok, "exit:" + out.exit_code);
  return reply;
}
function probe() {
  const script = "import json,os,subprocess,sys,shutil\n" +
    "sid=os.environ.get('XDG_SESSION_ID','').strip()\n" +
    "if not sid and shutil.which('loginctl'):\n  out=subprocess.run(['loginctl','list-sessions','--no-legend'],capture_output=True,text=True)\n" +
    "  if out.returncode==0:\n    for line in out.stdout.splitlines():\n      p=line.split()\n      if len(p)>=3 and p[2]==str(os.getuid()): sid=p[0]; break\n" +
    "if not sid: print(json.dumps({'available':False})); sys.exit(0)\n" +
    "def kv(t):\n  d={}\n  for line in t.splitlines():\n    if '=' in line: k,v=line.split('=',1); d[k]=v\n  return d\n" +
    "facts=None\n" +
    "if shutil.which('loginctl'):\n  out=subprocess.run(['loginctl','show-session',sid,'-a'],capture_output=True,text=True)\n" +
    "  if out.returncode==0: facts=kv(out.stdout)\n" +
    "if facts is None and os.path.exists(f'/run/systemd/sessions/{sid}'): facts=kv(open(f'/run/systemd/sessions/{sid}').read())\n" +
    "if facts is None: print(json.dumps({'available':False})); sys.exit(0)\n" +
    "seat=facts.get('Seat',''); typ=facts.get('Type',''); cls=facts.get('Class',''); st=facts.get('State',''); rem=facts.get('Remote','no'); lock=facts.get('LockedHint',facts.get('Locked','no'))\n" +
    "uid=facts.get('User',facts.get('UID','')); name=facts.get('Name',facts.get('USER','')); leader=facts.get('Leader',''); vt=facts.get('VTNr','0')\n" +
    "active=st=='active'; graphical=typ in ('x11','wayland'); on=seat!='' and rem in ('no','0','false','False') and graphical and cls=='user' and active\n" +
    "print(json.dumps({'available':True,'uid':int(uid) if str(uid).isdigit() else None,'username':name,'on_console':on,'login_complete':cls=='user' and active,'locked':lock in ('yes','1','true','True'),'leader':int(leader) if str(leader).isdigit() else None,'vt':int(vt) if str(vt).isdigit() else 0}))\n";
  const out = rh.command("python3", ["-c", script], { env: cu_env(""), timeout_ms: 30000 });
  ensure(out.success, out.stderr);
  const p = JSON.parse(out.stdout.trim());
  return p.available === true ? p : null;
}
function field_at(v, d) {
  let c = v;
  for (const k of d.split(".")) c = rh.field(c, k);
  return c;
}
function verify_unsupported(st) {
  ensure(st.ok !== true, "false_success");
  ensure(text_at(st,"error.code")==="login_session_unsupported","code");
  ensure(text_at(st,"error.detail.os")==="linux","os");
  ensure(text_at(st,"error.detail.required_os")!=="macos","required_os");
  ensure(text_at(st,"error.detail.mechanism").indexOf("systemd")>=0,"mechanism");
  ensure(text_at(st,"error.message").indexOf("macOS")<0,"macos_claim");
  const alts = field_at(st, "error.detail.alternatives");
  ensure(Array.isArray(alts) && alts.length>0,"alts");
}

const args = rh.args();
if (args.length===1 && args[0]==="--list-evidence") { for (const id of declared_evidence) print(id); return; }
harness.require(args.length===2, "expected: REPO AGENTERM_CU_EXE");
const repo = rh.absolute(args[0]); const exe = rh.absolute(args[1]);
ensure(rh.exists(exe), "missing exe");
const ctx = harness.new_context(repo, "cu-linux-login-session");
const p = probe();
if (p === null) {
  print("STEP typed unsupported path");
  verify_unsupported(invoke(ctx.run_directory, exe, ["status"], false));
  ensure(text_at(invoke(ctx.run_directory, exe, ["plan","lock"], false),"error.code")==="login_session_unsupported","plan");
  harness.emit_evidence(ctx, declared_evidence, "cu.linux-login-session-readback");
  harness.remove_run(ctx);
  print("PASS typed unsupported with Linux systemd-logind alternatives");
  return;
}
print("STEP readback path");
const st = invoke(ctx.run_directory, exe, ["status"], true);
ensure(text_at(st,"data.provider")==="linux-sd-login","provider");
let console = null;
for (const s of rh.field(st,"data.sessions")) if (rh.field(s,"on_console")===true) console = s;
ensure(console !== null, "console");
ensure(rh.field(console,"uid")===p.uid,"uid");
ensure(text_at(console,"username")===p.username,"username");
ensure(rh.field(console,"on_console")===p.on_console,"on_console");
ensure(rh.field(console,"login_complete")===p.login_complete,"login_complete");
ensure(rh.field(st,"data.locked")===p.locked,"locked");
harness.emit_evidence(ctx, declared_evidence, "cu.linux-login-session-readback");
harness.remove_run(ctx);
print("PASS login-session status matches independent session probe read-back");
'''

HOST_MEMORY_STATUS_RS = r'''//! Current-host physical memory geometry and availability observation.

use agenterm_platform::host_memory::{HostMemoryError, HostMemoryErrorKind};
use serde_json::{Value, json};

use crate::reply::CuError;

pub(super) fn host_memory_status_payload() -> Result<Value, CuError> {
    let (facts, availability) =
        agenterm_platform::host_memory::observed().map_err(memory_error)?;
    Ok(json!({
        "page_size": facts.page_size.get(),
        "allocation_granularity": facts.allocation_granularity.get(),
        "physical_bytes": facts.physical_bytes.get(),
        "available_physical_bytes": availability.available_physical_bytes,
        "availability_semantics": availability.semantics.as_str(),
        "atomic_snapshot": false,
    }))
}

fn memory_error(error: HostMemoryError) -> CuError {
    let code = match error.kind() {
        HostMemoryErrorKind::InvalidValue => "host_memory_invalid",
        HostMemoryErrorKind::Overflow => "host_memory_overflow",
        HostMemoryErrorKind::Query | _ => "host_memory_query_failed",
    };
    CuError::new(code, error.to_string()).with_detail(json!({
        "kind": format!("{:?}", error.kind()),
    }))
}
'''

COMMIT_PATHS = [
    "scripts/apply_login_session_journey.py",
    "crates/agenterm-platform/src/adapters/linux/login_session.rs",
    "crates/agenterm-platform/src/selected.rs",
    "crates/agenterm-platform/src/contract/login_session.rs",
    "crates/agenterm-platform/src/login_session.rs",
    "crates/agenterm-platform/src/adapters/macos/login_session.rs",
    "crates/agenterm-cu/src/login_session.rs",
    "crates/agenterm-cu/src/host_limit.rs",
    "scripts/qjs/cu-linux-login-session-smoke.qjs",
    "agenterm.tasks.json",
    "scripts/qualification-gates.json",
    "prd/alignment-contract.json",
    "scripts/qjs/prd-alignment.qjs",
]


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    if old not in text:
        if new in text:
            return
        raise SystemExit(f"missing {label} in {path}")
    path.write_text(text.replace(old, new))


def patch_selected() -> None:
    path = ROOT / "crates/agenterm-platform/src/selected.rs"
    text = path.read_text()
    old_blocks = [
        '''#[cfg(all(feature = "login-session", target_os = "macos"))]
#[path = "adapters/macos/login_session.rs"]
pub(crate) mod login_session;

#[cfg(all(feature = "login-session", not(target_os = "macos")))]
#[path = "adapters/unsupported_login_session.rs"]
pub(crate) mod login_session;

pub(crate) const fn login_session_supported() -> bool {
    cfg!(target_os = "macos")
}''',
    ]
    new = '''#[cfg(all(feature = "login-session", target_os = "macos"))]
#[path = "adapters/macos/login_session.rs"]
pub(crate) mod login_session;

#[cfg(all(feature = "login-session", target_os = "linux"))]
#[path = "adapters/linux/login_session.rs"]
pub(crate) mod login_session;

#[cfg(all(
    feature = "login-session",
    not(any(target_os = "macos", target_os = "linux"))
))]
#[path = "adapters/unsupported_login_session.rs"]
pub(crate) mod login_session;

pub(crate) const fn login_session_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
}'''
    if new not in text:
        replaced = False
        for old in old_blocks:
            if old in text:
                text = text.replace(old, new)
                replaced = True
                break
        if not replaced:
            raise SystemExit(f"missing selected login_session block in {path}")
    path.write_text(text)


def patch_contract() -> None:
    path = ROOT / "crates/agenterm-platform/src/contract/login_session.rs"
    text = path.read_text()
    text = text.replace(
        "#[cfg(any(target_os = \"macos\", test))]",
        "#[cfg(any(target_os = \"macos\", target_os = \"linux\", test))]",
    )
    if "LinuxSdLogin" not in text:
        text = text.replace(
            "pub enum LoginSessionProvider {\n    MacosIoRegistry,\n}",
            "pub enum LoginSessionProvider {\n    MacosIoRegistry,\n    LinuxSdLogin,\n}",
        )
    path.write_text(text)


def patch_platform_login_session() -> None:
    path = ROOT / "crates/agenterm-platform/src/login_session.rs"
    text = path.read_text()
    text = text.replace(
        "#[cfg(any(target_os = \"macos\", test))]",
        "#[cfg(any(target_os = \"macos\", target_os = \"linux\", test))]",
    )
    broken = """pub(crate) fn finish_inventory(LoginSessionProvider::MacosIoRegistry, 
    provider: LoginSessionProvider,
    locked: bool,
    rows: Vec<NativeLoginSessionRow>,
) -> Result<LoginSessionInventory, LoginSessionError> {"""
    correct = """pub(crate) fn finish_inventory(
    provider: LoginSessionProvider,
    locked: bool,
    rows: Vec<NativeLoginSessionRow>,
) -> Result<LoginSessionInventory, LoginSessionError> {"""
    if broken in text:
        text = text.replace(broken, correct)
    elif correct not in text:
        text = text.replace(
            """pub(crate) fn finish_inventory(
    locked: bool,
    rows: Vec<NativeLoginSessionRow>,
) -> Result<LoginSessionInventory, LoginSessionError> {""",
            correct,
        )
        text = text.replace(
            "provider: LoginSessionProvider::MacosIoRegistry,",
            "provider,",
        )
        text = re.sub(
            r"finish_inventory\((?!LoginSessionProvider::)",
            "finish_inventory(LoginSessionProvider::MacosIoRegistry, ",
            text,
        )
    path.write_text(text)


def patch_macos_adapter() -> None:
    path = ROOT / "crates/agenterm-platform/src/adapters/macos/login_session.rs"
    text = path.read_text()
    if "LoginSessionProvider::MacosIoRegistry, locked, rows" not in text:
        replace_once(
            path,
            "finish_inventory(locked, rows)",
            "finish_inventory(crate::login_session::LoginSessionProvider::MacosIoRegistry, locked, rows)",
            "macos finish_inventory",
        )


def patch_cu_login_session() -> None:
    path = ROOT / "crates/agenterm-cu/src/login_session.rs"
    text = path.read_text()
    needle = """        LoginSessionProvider::MacosIoRegistry => "macos-io-registry",
        _ => "unknown","""""
    insert = """        LoginSessionProvider::MacosIoRegistry => "macos-io-registry",
        LoginSessionProvider::LinuxSdLogin => "linux-sd-login",
        _ => "unknown","""""
    if insert not in text:
        if needle not in text:
            raise SystemExit(f"missing cu provider_name match in {path}")
        text = text.replace(needle, insert)
    path.write_text(text)


def ensure_buildable_peers() -> None:
    audio = ROOT / "crates/agenterm-cu/src/audio_control.rs"
    text = audio.read_text()
    fixed = text.replace(
        "&mut NativeAudioProvider)",
        "&mut NativeAudioProvider::default())",
    ).replace(
        "&mut NativeAudioProvider,",
        "&mut NativeAudioProvider::default(),",
    )
    if fixed != text:
        audio.write_text(fixed)

    command_path = ROOT / "crates/agenterm-cu/src/command.rs"
    command = command_path.read_text()

    cargo = ROOT / "crates/agenterm-cu/Cargo.toml"
    cargo_text = cargo.read_text()
    feature_needle = '"cache-hierarchy", "storage-device-inventory"'
    feature_insert = (
        '"cache-hierarchy", "host-memory", "processor-affinity", '
        '"storage-device-inventory"'
    )
    if "HostMemoryStatus {" in command and "host-memory" not in cargo_text and feature_needle in cargo_text:
        cargo.write_text(cargo_text.replace(feature_needle, feature_insert))

    host_memory = ROOT / "crates/agenterm-cu/src/executor/host_memory_status.rs"
    if "HostMemoryStatus {" in command and not host_memory.is_file():
        host_memory.write_text(HOST_MEMORY_STATUS_RS)

    mod_rs = ROOT / "crates/agenterm-cu/src/executor/mod.rs"
    mod_text = mod_rs.read_text()
    if "HostMemoryStatus {" in command and "mod host_memory_status;" not in mod_text:
        mod_text = mod_text.replace(
            "mod power_status;\nmod processor_topology_status;",
            "mod power_status;\nmod host_memory_status;\nmod processor_topology_status;",
        )
        mod_text = mod_text.replace(
            "use power_status::*;\nuse processor_topology_status::*;",
            "use power_status::*;\nuse host_memory_status::*;\nuse processor_topology_status::*;",
        )
        mod_rs.write_text(mod_text)

    mod_text = mod_rs.read_text()
    if (
        (ROOT / "crates/agenterm-cu/src/executor/processor_affinity_status.rs").is_file()
        and "mod processor_affinity_status;" not in mod_text
    ):
        mod_text = mod_text.replace(
            "mod cache_hierarchy_status;\nmod privilege;",
            "mod cache_hierarchy_status;\nmod processor_affinity_status;\nmod privilege;",
        )
        mod_text = mod_text.replace(
            "use cache_hierarchy_status::*;\nuse privilege::*;",
            "use cache_hierarchy_status::*;\nuse processor_affinity_status::*;\nuse privilege::*;",
        )
        mod_rs.write_text(mod_text)

    dispatch = ROOT / "crates/agenterm-cu/src/executor/dispatch.rs"
    dispatch_text = dispatch.read_text()
    affinity_arm = (
        "            Command::ProcessorAffinityStatus { pid, .. } => {\n"
        "                processor_affinity_status_payload(*pid)\n"
        "            }\n"
    )
    if (
        "ProcessorAffinityStatus {" in command
        and (ROOT / "crates/agenterm-cu/src/executor/processor_affinity_status.rs").is_file()
        and affinity_arm not in dispatch_text
    ):
        dispatch.write_text(
            dispatch_text.replace(
                "            Command::HostMemoryStatus { .. } => host_memory_status_payload(),\n",
                "            Command::HostMemoryStatus { .. } => host_memory_status_payload(),\n"
                + affinity_arm,
            )
        )

    embedder = ROOT / "crates/agenterm-cu/src/embedder.rs"
    embedder_text = embedder.read_text()
    affinity_match = "        | Command::ProcessorAffinityStatus { .. }\n"
    if "ProcessorAffinityStatus {" in command and affinity_match not in embedder_text:
        embedder.write_text(
            embedder_text.replace(
                "        | Command::HostMemoryStatus { .. }\n",
                "        | Command::HostMemoryStatus { .. }\n"
                + affinity_match,
            )
        )

    if "ProcessorAffinityStatus {" not in command:
        embedder.write_text(
            embedder.read_text().replace(
                "        | Command::ProcessorAffinityStatus { .. }\n", ""
            )
        )
        dispatch.write_text(
            dispatch.read_text().replace(affinity_arm, "")
        )


def patch_host_limit() -> None:
    path = ROOT / "crates/agenterm-cu/src/host_limit.rs"
    old = '''pub(crate) fn login_session_unsupported() -> CuError {
    host_limit_error(
        "login_session_unsupported",
        format!(
            "console login-session inventory and screen-lock delivery require macOS IORegistry integration; {} has no mapped provider",
            crate::mcu_surface::host_os()
        ),
        HostLimitDetail {
            group: Some("system"),
            provider: Some("none"),
            required_os: Some("macos"),
            mechanism: Some("macos-io-registry"),
            alternatives: &[
                "session-list (AgenTerm runtime sessions when agenterm server is running)",
                "resource-status / power-status (host facts; not OS screen lock)",
            ],
        },
    )
}'''
    new = '''pub(crate) fn login_session_unsupported() -> CuError {
    #[cfg(target_os = "linux")]
    {
        return host_limit_error(
            "login_session_unsupported",
            format!(
                "console login-session inventory requires Linux systemd-logind (sd-login); {} cannot load or use that provider",
                crate::mcu_surface::host_os()
            ),
            HostLimitDetail {
                group: Some("system"),
                provider: Some("none"),
                required_os: Some("linux"),
                mechanism: Some("systemd-logind-sd-login"),
                alternatives: &[
                    "session-list (AgenTerm runtime sessions when agenterm server is running)",
                    "resource-status / power-status (host facts; not OS screen lock)",
                ],
            },
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        host_limit_error(
            "login_session_unsupported",
            format!(
                "console login-session inventory and screen-lock delivery require macOS IORegistry integration; {} has no mapped provider",
                crate::mcu_surface::host_os()
            ),
            HostLimitDetail {
                group: Some("system"),
                provider: Some("none"),
                required_os: Some("macos"),
                mechanism: Some("macos-io-registry"),
                alternatives: &[
                    "session-list (AgenTerm runtime sessions when agenterm server is running)",
                    "resource-status / power-status (host facts; not OS screen lock)",
                ],
            },
        )
    }
}'''
    replace_once(path, old, new, "host_limit login_session_unsupported")


def ensure_contract_tasks() -> None:
    path = ROOT / "agenterm.tasks.json"
    data = json.loads(path.read_text())
    task_ids = {task.get("id") for task in data["tasks"]}
    template = next(
        task for task in data["tasks"] if task.get("id") == "cu-linux-boot-identity-smoke"
    )
    insert_at = next(
        index
        for index, task in enumerate(data["tasks"])
        if task.get("id") == "cu-linux-boot-identity-smoke"
    )
    added = 0
    for contract_id in data["contracts"]:
        if contract_id in task_ids:
            continue
        entry_path = ROOT / f"scripts/qjs/{contract_id}.qjs"
        if not entry_path.is_file():
            continue
        task = dict(template)
        task["id"] = contract_id
        task["entry"] = f"scripts/qjs/{contract_id}.qjs"
        task["description"] = template["description"].replace(
            "host_boot_identity boot_id, machine_id and uptime_milliseconds",
            f"{contract_id} contract smoke",
        )
        data["tasks"].insert(insert_at + 1 + added, task)
        task_ids.add(contract_id)
        added += 1
    if added:
        path.write_text(json.dumps(data, indent=2) + "\n")


def patch_tasks_json() -> None:
    path = ROOT / "agenterm.tasks.json"
    data = json.loads(path.read_text())
    contracts = data["contracts"]
    if "cu-linux-login-session-smoke" not in contracts:
        boot = contracts["cu-linux-boot-identity-smoke"]
        items = list(contracts.items())
        out = {}
        for key, value in items:
            out[key] = value
            if key == "cu-linux-boot-identity-smoke":
                out["cu-linux-login-session-smoke"] = {
                    "inputs": ["built-agenterm-cu"],
                    "outputs": ["cu-linux-login-session-evidence"],
                    "budget": boot["budget"],
                    "network": [],
                    "evidence": ["task.cu-linux-login-session-smoke"],
                }
        data["contracts"] = out
    tasks = data["tasks"]
    if not any(t.get("id") == "cu-linux-login-session-smoke" for t in tasks):
        insert_at = next(
            i for i, t in enumerate(tasks) if t.get("id") == "cu-linux-boot-identity-smoke"
        )
        tasks.insert(
            insert_at + 1,
            {
                "id": "cu-linux-login-session-smoke",
                "description": "Prove Linux login-session status matches independent loginctl/session-env read-back or returns typed systemd-logind unsupported honesty without macOS claims; no lock apply.",
                "entry": "scripts/qjs/cu-linux-login-session-smoke.qjs",
                "profile": "tool",
                "cwd": ".",
                "args": [".", "target/abi-dev/agenterm-cu"],
                "dependencies": [],
                "platforms": ["linux"],
                "side_effects": ["process_spawn"],
            },
        )
    path.write_text(json.dumps(data, indent=2) + "\n")


def patch_qualification_gates() -> None:
    path = ROOT / "scripts/qualification-gates.json"
    data = json.loads(path.read_text())
    gates = data["registered_gates"]
    if not any(g.get("id") == "cu-linux-login-session-smoke" for g in gates):
        insert_at = next(
            i for i, g in enumerate(gates) if g.get("id") == "cu-linux-boot-identity-smoke"
        )
        gates.insert(
            insert_at + 1,
            {
                "id": "cu-linux-login-session-smoke",
                "evidence": ["cu.linux-login-session-readback"],
            },
        )
    path.write_text(json.dumps(data, indent=2) + "\n")


def patch_alignment_contract() -> None:
    path = ROOT / "prd/alignment-contract.json"
    data = json.loads(path.read_text())
    for block in data["capabilities"]:
        evidence = block.get("evidence_ids", [])
        if "cu.linux-login-session-readback" in evidence:
            return
        if "cu.linux-boot-identity-readback" in evidence:
            idx = evidence.index("cu.linux-boot-identity-readback")
            evidence.insert(idx + 1, "cu.linux-login-session-readback")
            break
    path.write_text(json.dumps(data, indent=2) + "\n")


def patch_prd_alignment() -> None:
    path = ROOT / "scripts/qjs/prd-alignment.qjs"
    text = path.read_text()
    if '"cu-linux-login-session-smoke": true' in text:
        return
    replace_once(
        path,
        '  "cu-linux-boot-identity-smoke": true,\n',
        '  "cu-linux-boot-identity-smoke": true,\n  "cu-linux-login-session-smoke": true,\n',
        "prd-alignment task flag",
    )


def find_agenterm_cu() -> Path:
    candidates = [
        TARGET_DIR / "abi-dev/agenterm-cu",
        TARGET_DIR / "agenterm-cu",
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise SystemExit(f"agenterm-cu not found under {TARGET_DIR}")


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(cmd), flush=True)
    return subprocess.run(cmd, cwd=ROOT, text=True, check=False, **kwargs)


def main() -> int:
    patch_script = ROOT / "scripts/_patch-login-session.sh"
    if patch_script.exists():
        patch_script.unlink()

    write(ROOT / "crates/agenterm-platform/src/adapters/linux/login_session.rs", LINUX_LOGIN_SESSION_RS)
    write(ROOT / "scripts/qjs/cu-linux-login-session-smoke.qjs", SMOKE_QJS)
    patch_selected()
    patch_contract()
    patch_platform_login_session()
    patch_macos_adapter()
    patch_cu_login_session()
    patch_host_limit()
    patch_tasks_json()
    patch_qualification_gates()
    patch_alignment_contract()
    patch_prd_alignment()

    ensure_contract_tasks()
    ensure_buildable_peers()

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(TARGET_DIR.relative_to(ROOT))
    build = run(
        ["cargo", "build", "--profile", "abi-dev", "-p", "agenterm-cu"],
        env=env,
    )
    if build.returncode != 0:
        print(build.stdout)
        print(build.stderr, file=sys.stderr)
        return build.returncode

    cu = find_agenterm_cu()
    worker = Path(os.environ.get("HOME", str(ROOT))) / ".cache/agenterm/build-cache/linux-x86_64/agenterm"
    if not worker.is_file():
        worker = ROOT / "target/debug/agenterm"
    smoke_env = os.environ.copy()
    smoke_env.update(SMOKE_ENV)
    smoke = run(
        [
            str(worker),
            "cli",
            "script",
            "run",
            "scripts/qjs/cu-linux-login-session-smoke.qjs",
            "--profile",
            "tool",
            "--timeout-ms",
            "120000",
            "--max-operations",
            "100000000",
            "--max-output-bytes",
            "262144",
            "--",
            ".",
            str(cu),
        ],
        env=smoke_env,
        capture_output=True,
    )
    print(smoke.stdout)
    if smoke.stderr:
        print(smoke.stderr, file=sys.stderr)
    if smoke.returncode != 0:
        return smoke.returncode

    run(["git", "add", *COMMIT_PATHS])
    commit = run(
        [
            "git",
            "commit",
            "-m",
            "Add Linux login-session status honesty with sd-login adapter and CEO smoke read-back.",
        ],
        capture_output=True,
    )
    print(commit.stdout)
    if commit.returncode != 0:
        print(commit.stderr, file=sys.stderr)
        return commit.returncode

    pull = run(["git", "pull", "--ff-only", "origin", "main"], capture_output=True)
    print(pull.stdout)
    if pull.returncode != 0:
        print(pull.stderr, file=sys.stderr)
        return pull.returncode

    push = run(["git", "push", "origin", "main"], capture_output=True)
    print(push.stdout)
    if push.returncode != 0:
        print(push.stderr, file=sys.stderr)
        return push.returncode

    sha = run(["git", "rev-parse", "HEAD"], capture_output=True).stdout.strip()
    print("COMMIT_SHA", sha)
    print("COMMITTED_FILES")
    for item in COMMIT_PATHS:
        print(item)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
