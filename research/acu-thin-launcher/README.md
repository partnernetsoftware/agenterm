# ACU thin-launcher research prototype

This directory is the minimal B-route probe specified by
`plan/design-acu-thin-launcher-experiment.md`. It is research code, not a
product integration and not evidence that G1 passes.

## Shape

- `launcher-probe` is a small executable. It bounds raw UTF-8 argv, derives
  exactly one sibling library path from its own executable directory, checks
  ABI version 1 before resolving the call symbol, and keeps provider-boundary
  status separate from the product exit code.
- `provider-main-probe` owns entry classification and ordinary process
  presentation. Ordinary argv calls the existing
  `agenterm_cu::argv::execute_argv_from_environment`; it does not copy the
  `Command` schema or create a second dispatcher.
- `fixtures/bad-abi-provider` exports only a deliberately wrong ABI version for
  a fail-closed launcher court.

The launcher accepts only the platform's canonical sibling name:
`agenterm-cu-provider.dll`, `agenterm-cu-provider.so`, or
`agenterm-cu-provider.dylib`. There is no environment override, `PATH` search,
static fallback, subprocess fallback, MCU lookup, or Bun lookup.

ABI-v1 bounds are 4,096 arguments, 1 MiB aggregate argv, 4 MiB stdout and
1 MiB stderr. Status zero means the provider completed and the separate exit
field is authoritative; launcher/provider boundary failures exit 70. The
launcher validates the result version/size, lengths, exit range, stderr UTF-8,
and the ordinary stdout JSON frame before publishing bytes.

## Reproduce

Run from repository root. A dedicated repo-local target lane is preferred when
space permits:

```bash
CARGO_TARGET_DIR=target/acu-thin-launcher cargo fmt \
  --manifest-path research/acu-thin-launcher/Cargo.toml --all -- --check
CARGO_TARGET_DIR=target/acu-thin-launcher cargo test \
  --manifest-path research/acu-thin-launcher/Cargo.toml --workspace --release
CARGO_TARGET_DIR=target/acu-thin-launcher cargo clippy \
  --manifest-path research/acu-thin-launcher/Cargo.toml \
  --workspace --all-targets -- -D warnings
CARGO_TARGET_DIR=target/acu-thin-launcher cargo build \
  --manifest-path research/acu-thin-launcher/Cargo.toml --release
CARGO_TARGET_DIR=target/acu-thin-launcher cargo zigbuild \
  --manifest-path research/acu-thin-launcher/Cargo.toml --release \
  --target x86_64-unknown-linux-gnu -p acu-thin-launcher
```

For a native macOS arm64 ordinary-argv probe, stage only the fixed names and
run the launcher directly:

```bash
mkdir -p target/acu-thin-launcher-stage
cp target/acu-thin-launcher/release/acu-thin-launcher \
  target/acu-thin-launcher-stage/acu-thin-launcher
cp target/acu-thin-launcher/release/libagenterm_cu_provider.dylib \
  target/acu-thin-launcher-stage/agenterm-cu-provider.dylib
target/acu-thin-launcher-stage/acu-thin-launcher --help
target/acu-thin-launcher-stage/acu-thin-launcher \
  --target current --grant observe capabilities
target/acu-thin-launcher-stage/acu-thin-launcher \
  --target current capabilities
```

Remove or replace the staged sibling with the bad-ABI fixture to reproduce the
typed `provider_missing` and `provider_abi_version_mismatch` exits. The exact
observed commands, bytes and verdict are recorded in `RESULTS.md`.

## Deliberately incomplete entry modes

The provider classifies every current binary-only family before ordinary argv:

- Chromium Native Messaging host;
- network-probe worker and fixture;
- browser-session, managed-job and device-lease resident owners;
- privilege broker;
- device-I/O fixture;
- hotkey host and `verbs` text presentation;
- X11 clipboard owner;
- version text.

All of these currently return the typed boundary status
`provider_entry_mode_unimplemented`. Their stdin/stdout framing, resident
lifetime, cleanup and exact exit behavior are not implemented or tested here.
Consequently G1 is red; the mapping prevents accidental dispatch as an
ordinary command but is not binary-entry parity.
