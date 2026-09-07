# agenterm-cu-provider

This independently built `cdylib` is the versioned native delivery boundary
for qjs `agenterm:acu`. It accepts one bounded opaque Command JSON value and
returns one bounded opaque `CuReply` JSON value. Product meaning remains in
`agenterm-cu`.

Build it from repository root with its required unwind profile and an isolated
repo-local target directory:

```bash
CARGO_TARGET_DIR=target/acu-provider-agent cargo build --locked \
  --profile abi-release --package agenterm-cu-provider
```

Rust emits platform library target names (`agenterm_cu_provider.dll`,
`libagenterm_cu_provider.so`, or `libagenterm_cu_provider.dylib`). Packaging
stages that exact artifact beside the consumer as respectively
`agenterm-cu-provider.dll`, `agenterm-cu-provider.so`, or
`agenterm-cu-provider.dylib`; the loader must use only that fixed sibling name.
The rename is packaging only and does not change ABI identity.

Status zero means the payload is one complete `CuReply`, including legal
`ok:false` replies. Nonzero status is a native boundary failure. After a caught
panic, the loaded provider permanently returns
`AGENTERM_CU_PROVIDER_PANICKED`; callers must fail closed and must not fall back
to a static implementation, shell, or child process.
