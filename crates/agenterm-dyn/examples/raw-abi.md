# Call a native symbol through the raw ABI mechanism

This example calls the current process's C `getpid()` symbol. The empty library
name selects the current process image on Unix. A product-facing caller should
choose its own library and symbol exposure policy before constructing the call.

```rust
use agenterm_dyn::{AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi};

let call = NativeCall {
    library: "",
    symbol: "getpid",
    signature: AbiSignature {
        result: AbiType::I32,
        params: &[],
    },
    arguments: &[],
};

// SAFETY: the caller asserts that `getpid` really has C ABI `i32()`.
let value = unsafe { invoke_abi(&call)? };
assert!(matches!(value, AbiValue::I32(pid) if pid > 0));
# Ok::<(), agenterm_dyn::AbiError>(())
```

`validate_abi(&call)` can check the argument types and the implemented
trampoline matrix without loading or executing the symbol. It cannot discover
the symbol's true C declaration; asserting the correct signature remains the
caller's unsafe obligation.
