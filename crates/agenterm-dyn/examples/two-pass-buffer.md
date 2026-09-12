# Use a native two-pass length contract

`confstr` first reports a required size, then writes into a caller-owned buffer.
The null query, inclusion of the trailing NUL, and selector meaning are libc
contracts asserted by the caller—not policy implemented by dyn.

```rust
use std::ffi::c_void;
use agenterm_dyn::{AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi};

let signature = AbiSignature {
    result: AbiType::Usize,
    params: &[AbiType::I32, AbiType::Pointer, AbiType::Usize],
};
let query_arguments = [
    AbiValue::I32(libc::_CS_PATH),
    AbiValue::Pointer(std::ptr::null_mut()),
    AbiValue::Usize(0),
];
// SAFETY: the target libc defines `confstr` with this ABI and permits the
// null-plus-zero length query.
let required = unsafe {
    invoke_abi(&NativeCall {
        library: "",
        symbol: "confstr",
        signature,
        arguments: &query_arguments,
    })?
};
let AbiValue::Usize(required) = required else { unreachable!() };
let mut output = vec![0_u8; required];
let write_arguments = [
    AbiValue::I32(libc::_CS_PATH),
    AbiValue::Pointer(output.as_mut_ptr().cast::<c_void>()),
    AbiValue::Usize(output.len()),
];
// SAFETY: `output` provides exactly the writable extent passed to libc.
let written = unsafe {
    invoke_abi(&NativeCall {
        library: "",
        symbol: "confstr",
        signature,
        arguments: &write_arguments,
    })?
};
assert_eq!(written, AbiValue::Usize(required));
# Ok::<(), agenterm_dyn::AbiError>(())
```

