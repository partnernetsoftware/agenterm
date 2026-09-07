//! Load-on-first-use client for the versioned `agenterm-cu-provider` artifact.
//!
//! The provider is a fixed-name sibling of the running AgenTerm executable.
//! There is deliberately no path search, environment override, static ACU
//! fallback, or child-process fallback: delivery either supplies the matching
//! provider or qjs/MCP fails with a typed boundary diagnostic.

use std::{
    path::Path,
    sync::{Mutex, OnceLock},
};

#[cfg(all(feature = "script-qjswasm", not(feature = "script-acu-embedder")))]
use agenterm_qjswasm::AcuBridgeFn;
use libloading::{Library, Symbol};
#[cfg(all(feature = "script-qjswasm", not(feature = "script-acu-embedder")))]
use std::sync::Arc;

const EXPECTED_ABI_VERSION: u32 = 1;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

type AbiVersionFn = unsafe extern "C" fn() -> u32;
type CallFn = unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize) -> i32;

struct Provider {
    // Symbols are copied to plain function pointers only after their version
    // is checked. Keeping the library owned here makes those pointers valid.
    _library: Library,
    call: CallFn,
    // The provider is synchronous. One reusable bounded buffer both avoids a
    // 4 MiB allocation per call and serializes access to provider-global state.
    reply: Mutex<Vec<u8>>,
}

static PROVIDER: OnceLock<Result<Provider, String>> = OnceLock::new();

#[cfg(all(feature = "script-qjswasm", not(feature = "script-acu-embedder")))]
pub(crate) fn bridge() -> AcuBridgeFn {
    Arc::new(call)
}

pub(crate) fn call(request: &str) -> Result<String, String> {
    if request.len() > MAX_REQUEST_BYTES {
        return Err("acu_provider_request_too_large".to_owned());
    }
    let provider = PROVIDER
        .get_or_init(|| Provider::load_sibling().map_err(|error| error.to_owned()))
        .as_ref()
        .map_err(Clone::clone)?;
    provider.execute(request)
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
        Ok(Self {
            _library: library,
            call,
            reply: Mutex::new(vec![0_u8; MAX_REPLY_BYTES]),
        })
    }

    fn execute(&self, request: &str) -> Result<String, String> {
        let mut reply = self
            .reply
            .lock()
            .map_err(|_| "acu_provider_reply_buffer_poisoned".to_owned())?;
        let mut reply_len = 0_usize;
        // SAFETY: both slices and reply_len remain live for this synchronous
        // call; load_from verified the provider's ABI before saving the pointer.
        let status = unsafe {
            (self.call)(
                request.as_ptr(),
                request.len(),
                reply.as_mut_ptr(),
                reply.len(),
                &mut reply_len,
            )
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

fn decode_reply(reply: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(reply).map_err(|_| "acu_provider_reply_not_utf8".to_owned())?;
    serde_json::from_str::<serde_json::Value>(text)
        .map_err(|_| "acu_provider_reply_not_json".to_owned())?;
    Ok(text.to_owned())
}

fn provider_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "agenterm-cu-provider.dll"
    } else if cfg!(target_os = "macos") {
        "agenterm-cu-provider.dylib"
    } else {
        "agenterm-cu-provider.so"
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
        assert_eq!(provider_status(99), "acu_provider_unknown_status");
    }

    #[test]
    fn provider_reply_must_be_json_but_preserves_exact_valid_bytes() {
        assert_eq!(
            decode_reply(b"not-json").expect_err("invalid JSON refused"),
            "acu_provider_reply_not_json"
        );
        assert_eq!(
            decode_reply(&[0xff]).expect_err("invalid UTF-8 refused"),
            "acu_provider_reply_not_utf8"
        );
        assert_eq!(
            decode_reply(b" {\"ok\":false} \n").unwrap(),
            " {\"ok\":false} \n"
        );
    }
}
