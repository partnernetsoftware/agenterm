# agenterm-dyn

`agenterm-dyn` is AgenTerm's policy-free, in-process dynamic ABI mechanism.
It opens a caller-selected library, resolves a caller-selected symbol, and
executes a C ABI shape that the current mechanism can represent. It is roughly
`libdl`-like loading plus a deliberately incomplete `libffi`-like call surface,
implemented with monomorphic Rust `extern "C"` trampolines.

## Boundary

dyn owns only mechanism:

- the single shared-library loader and symbol lookup path;
- `AbiSignature`, `AbiValue`, validation against the real trampoline matrix,
  and `unsafe invoke_abi`;
- raw scalar and pointer transport;
- the Unix variadic `ioctl` ABI exception;
- the separate W^X executable-buffer mechanism and its errors.

The caller owns the asserted native signature, pointer validity, alignment,
aliasing, lifetimes, library/thread requirements, cleanup, and side effects.
An ABI pointer has no pointee or nullability policy inside dyn.

dyn does not own product exposure policy, allowlists, guest-memory decoding,
Wasm span checks, budgets, cancellation, or supervision. Those remain in
`agenterm-qjswasm` and the Script Runtime. Typed OS snapshots belong to
`agenterm-platform` or another upper adapter; product facts, catalogs, and
evidence belong to their product and test layers. None are dyn APIs.

## Primary API

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
let pid = unsafe { invoke_abi(&call)? };
assert!(matches!(pid, AbiValue::I32(value) if value > 0));
# Ok::<(), agenterm_dyn::AbiError>(())
```

`validate_abi` checks argument count/types and whether a real trampoline exists
without loading or calling the symbol. An unsupported shape returns
`AbiError::SignatureUnsupported`; dyn never approximates one ABI as another.

## Relationship to qjswasm

`agenterm-qjswasm` owns the native-call grammar and catalog, `ptr` versus
`ptr?`, scalar canonicalization, guest-span bounds, result-pointer rebasing,
and public typed errors. Once that upper layer admits and decodes a call, its
five scalar/pointer execution paths delegate to `invoke_abi`. Unix `ioctl`
uses the same native door but its dedicated variadic mechanism.

There is no second loader and no second native door.

## Historical material

The former S-expression evaluator (`Dyn`, `Value`, `Symbol`, and textual
`dlcall`) was removed after its executable claims moved to `invoke_abi` tests
or qjswasm WAT courts. Its host catalog and typed-owner side APIs were removed
separately once repository-wide consumer checks proved that dyn was their only
owner and user.

The owning product contract, including the required Markdown tree-DAG and
Mermaid memory-palace view, is
[`prd/PRD_02_34_agenterm_dyn.md`](../../prd/PRD_02_34_agenterm_dyn.md).
