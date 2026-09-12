# Keep mechanism errors explicit

Library loading, symbol lookup, unsupported ABI shapes, and argument mismatch
are distinct mechanism failures. They are not authorization decisions.

```rust
use agenterm_dyn::{AbiError, AbiSignature, AbiType, NativeCall, invoke_abi};

let call = NativeCall {
    library: "",
    symbol: "agenterm_dyn_deliberately_missing_symbol",
    signature: AbiSignature {
        result: AbiType::Pointer,
        params: &[],
    },
    arguments: &[],
};
// SAFETY: no function is entered because symbol lookup must fail first.
let error = unsafe { invoke_abi(&call) }.expect_err("symbol must be absent");
assert!(matches!(error, AbiError::SymbolLookup { .. }));
```
