# Local changes to `vnc-rs`

Vendored from [`vnc-rs` 0.5.3](https://github.com/HsuJv/vnc-rs), by Jovi Hsu
and contributors, under MIT OR Apache-2.0; the upstream licence texts are kept
alongside this file. Upstream `src/` is otherwise unmodified; every local edit
is marked with an `AGENTERM PATCH` comment so a future re-vendor can find them.

To re-vendor: fetch the upstream crate, replace `src/`, then reapply the two
changes below (search the old tree for `AGENTERM PATCH` to see them in place).

The package is renamed `agenterm-vnc-rs` to avoid colliding with the crates.io
name, but keeps the library name `vnc`, so dependent code is unchanged.

## 1. Remove two unsound `transmute`s (`src/client/auth.rs`)

`AuthResult::from<u32>` and `SecurityType::try_from<u8>` built enums by
transmuting an untrusted network value. Any value outside the declared variants
is undefined behaviour; in practice a malformed `SecurityResult` word aborted
the process with a **non-unwinding** panic that no `catch_unwind` in the
embedding application could contain, taking the whole GUI down with it.

Observed on 2026-08-23 against a server that answered `0x01000000`:

```
core::panicking::panic_invalid_enum_construction
  → vnc::client::auth::AuthResult::from
  → abort()
```

Both are now explicit `match`es. An unrecognised auth result is treated as a
failure, which is the safe reading of a word the protocol does not define.
Covered by `crates/agenterm-vnc/tests/poison.rs`.

## 2. Add Apple Remote Management, security type 30 (`src/client/`)

macOS Screen Sharing offers type 30 when authenticating against a real macOS
account, which is the only way to sign in with a username. Upstream rejects it
with "Security type apart from Vnc Auth has not been implemented".

Added:

- `SecurityType::AppleRemoteDesktop` and its decoding.
- `ArdChallenge` / `ArdHandler` and `VncConnector::set_ard_handler`.
- A connector branch that performs the RFB framing: read the generator, key
  length, prime modulus and server public key; invoke the handler; write the
  ciphertext and the client public key; check `SecurityResult`.

The cryptography deliberately stays **outside** this vendored copy, in
`crates/agenterm-vnc/src/ard.rs`, so the local diff remains small and the
Diffie-Hellman / MD5 / AES-ECB work is tested against an independent
implementation (`crates/agenterm-vnc/tests/ard.rs`). Without a handler the
connector behaves exactly as upstream.
