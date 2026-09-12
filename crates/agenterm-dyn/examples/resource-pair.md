# Pair native acquisition with native cleanup

Dyn deliberately does not turn raw pointers into owned Rust values. A raw
caller that acquires a native resource must still guarantee cleanup on every
path. This sketch shows the two mechanism calls for `getifaddrs` and
`freeifaddrs`; production code should put `head` in an RAII guard immediately
after successful acquisition.

```rust
use std::ffi::c_void;
use agenterm_dyn::{AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi};

let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
let acquire_arguments = [AbiValue::Pointer(
    (&mut head as *mut *mut libc::ifaddrs).cast::<c_void>(),
)];
// SAFETY: `getifaddrs` has ABI `i32(ptr)` and the pointer-to-pointer output slot
// is writable, aligned, and live through the call.
let status = unsafe {
    invoke_abi(&NativeCall {
        library: "",
        symbol: "getifaddrs",
        signature: AbiSignature {
            result: AbiType::I32,
            params: &[AbiType::Pointer],
        },
        arguments: &acquire_arguments,
    })?
};
assert_eq!(status, AbiValue::I32(0));

let release_arguments = [AbiValue::Pointer(head.cast::<c_void>())];
// SAFETY: successful acquisition returned this list head, no node is used
// after this call, and this is the list's unique cleanup.
unsafe {
    invoke_abi(&NativeCall {
        library: "",
        symbol: "freeifaddrs",
        signature: AbiSignature {
            result: AbiType::Void,
            params: &[AbiType::Pointer],
        },
        arguments: &release_arguments,
    })?;
}
# Ok::<(), agenterm_dyn::AbiError>(())
```
