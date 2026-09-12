# Pass a caller-owned buffer

`AbiType::Pointer` describes only an ABI address position. It does not encode
ownership, nullability, alignment, length, initialization, aliasing, or the
pointee type. The caller must uphold all of those contracts.

This Unix example asks `uname` to fill caller-owned storage:

```rust
use std::ffi::c_void;
use agenterm_dyn::{AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi};

let mut output = std::mem::MaybeUninit::<libc::utsname>::zeroed();
let arguments = [AbiValue::Pointer(output.as_mut_ptr().cast::<c_void>())];
let call = NativeCall {
    library: "",
    symbol: "uname",
    signature: AbiSignature {
        result: AbiType::I32,
        params: &[AbiType::Pointer],
    },
    arguments: &arguments,
};

// SAFETY: `uname` has C ABI `i32(ptr)`, and `output` is writable, aligned,
// correctly sized, and live until the synchronous call returns.
let status = unsafe { invoke_abi(&call)? };
assert_eq!(status, AbiValue::I32(0));
// SAFETY: status zero means `uname` initialized the complete structure.
let output = unsafe { output.assume_init() };
# let _ = output;
# Ok::<(), agenterm_dyn::AbiError>(())
```

Guest-memory bounds, `ptr` versus `ptr?`, and public schema errors belong to
`agenterm-qjswasm`; they are intentionally not duplicated in dyn.
