//! Load-on-first-use client for the versioned `agenterm-cu-provider` artifact.
//!
//! The provider is a fixed-name sibling of the running AgenTerm executable.
//! There is deliberately no path search, environment override, static ACU
//! fallback, or child-process fallback: delivery either supplies the matching
//! provider or qjs/MCP fails with a typed boundary diagnostic.

use std::{
    ffi::c_void,
    path::Path,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(all(feature = "script-qjswasm", not(feature = "script-acu-embedder")))]
use agenterm_qjswasm::AcuBridgeFn;
use libloading::{Library, Symbol};
#[cfg(all(feature = "script-qjswasm", not(feature = "script-acu-embedder")))]
use std::sync::Arc;

const EXPECTED_ABI_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
enum Presence<T> {
    #[default]
    Missing,
    Present(T),
}

impl<'de, T> serde::Deserialize<'de> for Presence<T>
where
    T: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderErrorWire {
    code: String,
    message: String,
    #[serde(default, rename = "count")]
    _count: Option<usize>,
    #[serde(default, rename = "detail")]
    _detail: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderReplyWire {
    ok: bool,
    #[serde(rename = "target")]
    _target: String,
    command: String,
    #[serde(default)]
    data: Presence<serde_json::Value>,
    #[serde(default)]
    error: Presence<ProviderErrorWire>,
}

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type CallFn = unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize) -> i32;
type IsCancelledFn = unsafe extern "C" fn(*const c_void) -> u8;

#[repr(C)]
struct CancelV1 {
    struct_size: usize,
    version: u32,
    context: *const c_void,
    is_cancelled: Option<IsCancelledFn>,
}

type CallV2Fn =
    unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize, *const CancelV1) -> i32;

struct Provider {
    // Symbols are copied to plain function pointers only after their version
    // is checked. Keeping the library owned here makes those pointers valid.
    _library: Library,
    call: CallFn,
    call_v2: Option<CallV2Fn>,
    // The provider is synchronous. One reusable bounded buffer both avoids a
    // 4 MiB allocation per call and serializes access to provider-global state.
    reply: Mutex<Vec<u8>>,
}

static PROVIDER: OnceLock<Result<Provider, String>> = OnceLock::new();

#[cfg(all(feature = "script-qjswasm", not(feature = "script-acu-embedder")))]
pub(crate) fn bridge() -> AcuBridgeFn {
    Arc::new(|request, cancel, acknowledged| {
        let reply = call_controlled(request, cancel)?;
        if reply_is_cooperative_cancel(&reply) {
            acknowledged.store(true, Ordering::Release);
        }
        Ok(reply)
    })
}

pub(crate) fn call(request: &str) -> Result<String, String> {
    call_controlled(request, None)
}

pub(crate) fn call_controlled(
    request: &str,
    cancel: Option<&AtomicBool>,
) -> Result<String, String> {
    if request.len() > MAX_REQUEST_BYTES {
        return Err("acu_provider_request_too_large".to_owned());
    }
    let provider = PROVIDER
        .get_or_init(|| Provider::load_sibling().map_err(|error| error.to_owned()))
        .as_ref()
        .map_err(Clone::clone)?;
    provider.execute(request, cancel)
}

impl Provider {
    fn load_sibling() -> Result<Self, &'static str> {
        let executable = std::env::current_exe().map_err(|_| "acu_provider_executable_unknown")?;
        let directory = executable
            .parent()
            .ok_or("acu_provider_executable_has_no_parent")?;
        Self::load_from(&directory.join(provider_file_name()))
    }

    fn load_from(path: &Path) -> Result<Self, &'static str> {
        if path.file_name().and_then(|name| name.to_str()) != Some(provider_file_name()) {
            return Err("acu_provider_path_not_fixed_sibling_name");
        }
        // SAFETY: the library stays owned by Provider until process teardown;
        // only the two versioned symbols below are resolved and called.
        let library = unsafe { Library::new(path) }.map_err(|_| "acu_provider_missing")?;
        // SAFETY: the symbol is invoked only with its version-1 signature, and
        // the library remains alive while the temporary Symbol is borrowed.
        let version: Symbol<'_, AbiVersionFn> = unsafe {
            library
                .get(b"agenterm_cu_provider_abi_version\0")
                .map_err(|_| "acu_provider_abi_symbol_missing")?
        };
        // SAFETY: the resolved version function takes no arguments.
        if unsafe { version() } != EXPECTED_ABI_VERSION {
            return Err("acu_provider_abi_version_mismatch");
        }
        // SAFETY: the ABI version above identifies this exact call signature.
        let call: CallFn = unsafe {
            *library
                .get::<CallFn>(b"agenterm_cu_provider_call\0")
                .map_err(|_| "acu_provider_call_symbol_missing")?
        };
        let call_v2 = unsafe {
            library
                .get::<CallV2Fn>(b"agenterm_cu_provider_call_v2\0")
                .ok()
                .map(|symbol| *symbol)
        };
        Ok(Self {
            _library: library,
            call,
            call_v2,
            reply: Mutex::new(vec![0_u8; MAX_REPLY_BYTES]),
        })
    }

    fn execute(&self, request: &str, cancel: Option<&AtomicBool>) -> Result<String, String> {
        let mut reply = self
            .reply
            .lock()
            .map_err(|_| "acu_provider_reply_buffer_poisoned".to_owned())?;
        let mut reply_len = 0_usize;
        // SAFETY: both slices and reply_len remain live for this synchronous
        // call; load_from verified the provider's ABI before saving the pointer.
        let status = unsafe {
            if let Some(cancel) = cancel {
                let call_v2 = self
                    .call_v2
                    .ok_or_else(|| "acu_provider_cooperative_cancel_unavailable".to_owned())?;
                let descriptor = CancelV1 {
                    struct_size: std::mem::size_of::<CancelV1>(),
                    version: 1,
                    context: (cancel as *const AtomicBool).cast(),
                    is_cancelled: Some(read_cancelled),
                };
                call_v2(
                    request.as_ptr(),
                    request.len(),
                    reply.as_mut_ptr(),
                    reply.len(),
                    &mut reply_len,
                    &descriptor,
                )
            } else {
                (self.call)(
                    request.as_ptr(),
                    request.len(),
                    reply.as_mut_ptr(),
                    reply.len(),
                    &mut reply_len,
                )
            }
        };
        if status != 0 {
            return Err(provider_status(status).to_owned());
        }
        if reply_len > reply.len() {
            return Err("acu_provider_reply_length_out_of_range".to_owned());
        }
        decode_reply(&reply[..reply_len])
    }
}

unsafe extern "C" fn read_cancelled(context: *const c_void) -> u8 {
    let flag = unsafe { &*context.cast::<AtomicBool>() };
    u8::from(flag.load(Ordering::Acquire))
}

fn decode_reply(reply: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(reply).map_err(|_| "acu_provider_reply_not_utf8".to_owned())?;
    validate_reply(text)?;
    Ok(text.to_owned())
}

pub(crate) fn parse_reply(encoded: &str) -> Result<serde_json::Value, String> {
    validate_reply(encoded)?;
    serde_json::from_str(encoded).map_err(|_| "acu_provider_reply_not_json".to_owned())
}

pub(crate) fn parse_reply_for(
    encoded: &str,
    expected_target: &str,
    expected_command: &str,
) -> Result<serde_json::Value, String> {
    let reply = parse_reply(encoded)?;
    if reply.get("target").and_then(serde_json::Value::as_str) != Some(expected_target)
        || reply.get("command").and_then(serde_json::Value::as_str) != Some(expected_command)
    {
        return Err("acu_provider_reply_identity_mismatch".to_owned());
    }
    Ok(reply)
}

fn validate_reply(encoded: &str) -> Result<(), String> {
    if encoded.len() > MAX_REPLY_BYTES {
        return Err("acu_provider_reply_too_large".to_owned());
    }
    let reply: ProviderReplyWire = serde_json::from_str(encoded).map_err(|error| {
        if error.is_data() {
            "acu_provider_reply_invalid_shape".to_owned()
        } else {
            "acu_provider_reply_not_json".to_owned()
        }
    })?;
    // Usage/help and malformed-request replies intentionally have no resolved
    // target yet, but every CuReply still owns a non-empty command identity.
    if reply.command.is_empty() {
        return Err("acu_provider_reply_invalid_shape".to_owned());
    }
    match (reply.ok, reply.data, reply.error) {
        (true, Presence::Present(_), Presence::Missing) => Ok(()),
        (false, Presence::Missing, Presence::Present(ProviderErrorWire { code, message, .. }))
            if !code.is_empty() && !message.is_empty() =>
        {
            Ok(())
        }
        _ => Err("acu_provider_reply_invalid_shape".to_owned()),
    }
}

#[cfg(all(feature = "script-qjswasm", not(feature = "script-acu-embedder")))]
fn reply_is_cooperative_cancel(reply: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(reply) else {
        return false;
    };
    value
        .pointer("/error/code")
        .and_then(serde_json::Value::as_str)
        == Some("cancelled")
        && value
            .pointer("/error/detail/effect")
            .and_then(serde_json::Value::as_str)
            == Some("not_performed")
}

fn provider_file_name() -> &'static str {
    provider_file_name_for(agenterm_platform::platform_kind())
}

fn provider_file_name_for(platform: agenterm_platform::PlatformKind) -> &'static str {
    match platform {
        agenterm_platform::PlatformKind::Windows => "agenterm-cu-provider.dll",
        agenterm_platform::PlatformKind::Macos => "agenterm-cu-provider.dylib",
        agenterm_platform::PlatformKind::Linux => "agenterm-cu-provider.so",
        _ => unreachable!("unsupported agenterm platform kind"),
    }
}

fn provider_status(status: i32) -> &'static str {
    match status {
        1 => "acu_provider_invalid_pointer",
        2 => "acu_provider_request_too_large",
        3 => "acu_provider_request_not_utf8",
        4 => "acu_provider_reply_too_large",
        5 => "acu_provider_serialize_failed",
        6 => "acu_provider_panicked",
        7 => "acu_provider_invalid_cancel",
        _ => "acu_provider_unknown_status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn platform_name_is_fixed_and_contains_no_search_component() {
        let name = provider_file_name();
        assert!(name.starts_with("agenterm-cu-provider."));
        assert_eq!(PathBuf::from(name).components().count(), 1);
    }

    #[test]
    fn staged_provider_names_are_closed_for_every_host_kind() {
        for (platform, expected) in [
            (
                agenterm_platform::PlatformKind::Windows,
                "agenterm-cu-provider.dll",
            ),
            (
                agenterm_platform::PlatformKind::Macos,
                "agenterm-cu-provider.dylib",
            ),
            (
                agenterm_platform::PlatformKind::Linux,
                "agenterm-cu-provider.so",
            ),
        ] {
            let name = provider_file_name_for(platform);
            assert_eq!(name, expected);
            assert_eq!(PathBuf::from(name).components().count(), 1);
        }
    }

    #[test]
    fn arbitrary_provider_names_are_refused_before_loading() {
        assert_eq!(
            Provider::load_from(Path::new("not-the-provider"))
                .err()
                .expect("refused"),
            "acu_provider_path_not_fixed_sibling_name"
        );
    }

    #[test]
    fn every_version_one_status_has_a_stable_name() {
        assert_eq!(provider_status(1), "acu_provider_invalid_pointer");
        assert_eq!(provider_status(6), "acu_provider_panicked");
        assert_eq!(provider_status(7), "acu_provider_invalid_cancel");
        assert_eq!(provider_status(99), "acu_provider_unknown_status");
    }

    #[test]
    #[cfg(all(feature = "script-qjswasm", not(feature = "script-acu-embedder")))]
    fn only_pre_effect_typed_cancellation_is_acknowledged() {
        assert!(reply_is_cooperative_cancel(
            r#"{"ok":false,"target":"current","command":"wait","error":{"code":"cancelled","message":"cancelled","detail":{"effect":"not_performed"}}}"#
        ));
        assert!(!reply_is_cooperative_cancel(
            r#"{"ok":false,"target":"current","command":"wait","error":{"code":"cancelled","message":"cancelled","detail":{"effect":"unknown"}}}"#
        ));
        assert!(!reply_is_cooperative_cancel(
            r#"{"ok":true,"target":"current","command":"wait","data":{"effect":"committed"}}"#
        ));
    }

    #[test]
    fn provider_reply_is_one_closed_cu_reply_and_preserves_exact_valid_bytes() {
        assert_eq!(
            decode_reply(b"not-json").expect_err("invalid JSON refused"),
            "acu_provider_reply_not_json"
        );
        assert_eq!(
            decode_reply(&[0xff]).expect_err("invalid UTF-8 refused"),
            "acu_provider_reply_not_utf8"
        );
        let valid =
            b" {\"ok\":true,\"target\":\"current\",\"command\":\"capabilities\",\"data\":null} \n";
        assert_eq!(decode_reply(valid).unwrap().as_bytes(), valid);

        for invalid in [
            r#"{}"#,
            r#"{"ok":true,"target":"current","command":"capabilities"}"#,
            r#"{"ok":true,"target":"current","command":"capabilities","data":{},"error":{"code":"bad","message":"bad"}}"#,
            r#"{"ok":false,"target":"current","command":"capabilities"}"#,
            r#"{"ok":false,"target":"current","command":"capabilities","data":{},"error":{"code":"bad","message":"bad"}}"#,
            r#"{"ok":false,"target":"current","command":"capabilities","error":{"code":"","message":"bad"}}"#,
            r#"{"ok":false,"target":"current","command":"capabilities","error":{"code":"bad","message":""}}"#,
            r#"{"ok":true,"target":"current","command":"capabilities","data":{},"extra":true}"#,
            r#"{"ok":true,"ok":false,"target":"current","command":"capabilities","data":{}}"#,
        ] {
            assert_eq!(
                decode_reply(invalid.as_bytes()).expect_err("invalid shape refused"),
                "acu_provider_reply_invalid_shape",
                "{invalid}"
            );
        }
    }

    #[test]
    fn provider_reply_identity_is_bound_without_inspecting_payload_data() {
        let encoded =
            r#"{"ok":true,"target":"current","command":"capabilities","data":{"dynamic":true}}"#;
        assert!(parse_reply_for(encoded, "current", "capabilities").is_ok());
        assert_eq!(
            parse_reply_for(encoded, "current", "runtime-status").unwrap_err(),
            "acu_provider_reply_identity_mismatch"
        );
        assert_eq!(
            parse_reply_for(encoded, "ssh", "capabilities").unwrap_err(),
            "acu_provider_reply_identity_mismatch"
        );
    }
}
