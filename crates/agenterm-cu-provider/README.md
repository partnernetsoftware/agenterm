# agenterm-cu-provider

This independently built `cdylib` is the versioned native delivery boundary
for qjs `agenterm:acu`, MCP, and the thin CU process launcher. Product meaning
remains in `agenterm-cu`.

The artifact exports two independently versioned, additive ABI families:

- `agenterm_cu_provider_*` accepts one bounded opaque Command JSON value and
  returns one bounded opaque `CuReply` JSON value for embedded callers;
- `agenterm_cu_process_main_*` accepts bounded raw argv, uses the shared product
  process-entry table, and returns buffered presentation plus product exit
  status. Resident and framed child modes retain direct ownership of their real
  process stdio and lifetime.

Both families share one panic latch and one call lock. A panic through either
entry permanently closes the complete loaded provider; callers never fall back
to a different implementation.

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
