# Call a heterogeneous scalar signature

`sysconf` demonstrates a shape whose result and argument use different machine
types. Platform constants must come from the target's libc rather than being
copied between operating systems.

```rust
use agenterm_dyn::{AbiSignature, AbiType, AbiValue, NativeCall, invoke_abi};

let arguments = [AbiValue::I32(libc::_SC_PAGESIZE)];
let call = NativeCall {
    library: "",
    symbol: "sysconf",
    signature: AbiSignature {
        result: AbiType::Isize,
        params: &[AbiType::I32],
    },
    arguments: &arguments,
};

// SAFETY: the caller asserts the target libc `sysconf` ABI and supplies its
// target-specific selector. The caller also interprets `-1` and errno.
let value = unsafe { invoke_abi(&call)? };
assert!(matches!(value, AbiValue::Isize(bytes) if bytes > 0));
# Ok::<(), agenterm_dyn::AbiError>(())
```
