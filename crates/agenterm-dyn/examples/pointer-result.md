# Return a pointer into caller-owned storage

This example exercises `getcwd(char *, size_t) -> char *`. Dyn transports the
returned address but does not decide whether it is valid or owned.

```rust
use std::ffi::{CStr, c_void};
use agenterm_dyn::{AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi};

let mut output = [0_u8; 4096];
let expected = output.as_mut_ptr().cast::<c_void>();
let arguments = [AbiValue::Pointer(expected), AbiValue::Usize(output.len())];
let call = NativeCall {
    library: "",
    symbol: "getcwd",
    signature: AbiSignature {
        result: AbiType::Pointer,
        params: &[AbiType::Pointer, AbiType::Usize],
    },
    arguments: &arguments,
};

// SAFETY: the declared ABI is exact and `output` is writable, correctly sized,
// aligned, and live through the synchronous call.
let value = unsafe { invoke_abi(&call)? };
assert_eq!(value, AbiValue::Pointer(expected));
let path = CStr::from_bytes_until_nul(&output)?;
assert!(!path.to_bytes().is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

