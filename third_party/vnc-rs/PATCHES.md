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

## 3. Accept Apple's version banner and unknown security types

Two further changes, both found against a real macOS Screen Sharing server
that announces `RFB 003.889` and offers types 30, 33, 36, 31, 32, 2 and 35.

- `VncVersion::from` mapped anything it did not recognise to RFB 3.3, citing
  RFC 6143's advice for unknown versions. That advice assumes such a server
  does not implement the newer handshake; Apple does. Read as 3.3, the client
  waits for the server to dictate a security type while the server waits for
  the client to choose one, and the connection hangs. Any `003.<minor>` at or
  above 008 is now read as 3.8.
- `SecurityType::read` propagated an error for any unrecognised byte while
  merely *reading* the offered list, so a usable type further along it was
  never reached. Unknown types are now skipped, and the list is only an error
  when nothing in it is understood.

## Notes

The cryptography deliberately stays **outside** this vendored copy, in
`crates/agenterm-vnc/src/ard.rs`, so the local diff remains small and the
Diffie-Hellman / MD5 / AES-ECB work is tested against an independent
implementation (`crates/agenterm-vnc/tests/ard.rs`). Without a handler the
connector behaves exactly as upstream.
