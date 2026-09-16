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
  `Command` schema or create a second dispatcher. The first explicit
  binary-entry slices present the library-owned version text and preserve the
  network-probe worker/fixture process boundary plus the detached managed-job
  and browser-session owner boundaries.
- `fixtures/bad-abi-provider` exports only a deliberately wrong ABI version for
  a fail-closed launcher court.
- `parity-court` runs one bounded argv table against a same-source monolith and
  staged launcher, requires exact exit/stdout/stderr equality, and also checks
  absolute positive and typed-refusal expectations so two equally broken sides
  cannot produce a false green.

The launcher accepts only the platform's canonical sibling name:
`agenterm-cu-provider.dll`, `agenterm-cu-provider.so`, or
`agenterm-cu-provider.dylib`. There is no environment override, `PATH` search,
static fallback, subprocess fallback, MCU lookup, or Bun lookup.

ABI-v1 bounds are 4,096 arguments, 1 MiB aggregate argv, 4 MiB stdout and
1 MiB stderr. Status zero means the provider completed and the separate exit
field is authoritative; launcher/provider boundary failures exit 70. The
launcher validates the result version/size, exact argv-bound entry mode,
lengths and exit range before publishing bytes. Ordinary mode additionally
requires stderr UTF-8 and one JSON stdout frame; version mode requires one
`agenterm-cu` text line and empty stderr. Network-probe and resident-owner child
modes own their process lifetime and any stdin/stdout directly, and must publish
zero bytes through the ABI buffers. The network-probe parent retains the 4 KiB
request, 64 KiB reply and kill/reap deadline; the resident owners keep their
existing durable readiness and early-exit courts.

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

After staging the launcher and provider as below, compare it with the
same-source monolith. Every child has a ten-second deadline and is killed and
reaped on timeout:

```bash
target/acu-thin-launcher/release/acu-thin-launcher-parity-court \
  --monolith target/release/agenterm-cu \
  --launcher target/acu-thin-launcher-stage/acu-thin-launcher \
  --abi-library target/release/libagenterm.dylib \
  --bad-abi \
    target/acu-thin-launcher/release/libagenterm_cu_bad_abi_provider.dylib
```

Use `--list` to inspect the closed case set or `--case NAME` to isolate one
failure. Every run also stages the bad-ABI fixture under the canonical provider
name and requires the launcher to diverge from the healthy monolith with the
boundary exit; `--case` never disables this control. This proves that the
comparison harness detects a real fault.
The native privilege refusal assumes an ordinary non-root process with no
service-manager activation. The court deliberately omits the hotkey
`--self-test`: on a macOS host with Accessibility trust it performs a real
window-placement action, so it is not a non-mutating parity case. The closed
table may contain only non-mutating, promptly exiting cases whose stdout and
stderr streams each stay below 64 KiB; adding a stateful, resident or larger
case requires a separate isolation and containment design.

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

Version text is implemented for the exact one-argument `--version` and `-V`
forms. The network-probe worker and loopback fixture retain their direct stdio
and child lifetime contracts. The managed-job owner runs directly inside the
detached launcher child, without redispatch or respawn. The browser-session
owner does the same while retaining its exact directory argv and null stdio.
The device-lease owner now likewise consumes its inherited launch pipe directly,
without moving its lease secret through argv, environment or ABI buffers. Its
Unix PTY fixture also retains its direct stdout and bounded lifetime contract.
The library-owned `verbs` surface retains its text, JSON and typed-usage output
through the bounded ABI buffer. The X11 clipboard owner now shares one library
entry between monolith and provider, preserving its inherited stdin and
zero-output process contract. The hotkey host now retains direct ownership of
the process event loop and real stdout/stderr, including the `hotkeys` alias and
bounded `--self-test` argv. The privilege broker also remains inside the
service-manager-owned process and keeps its exact one-argument sentinel and
direct diagnostics. Finally, the Native Messaging entry shares one library
adapter for origin/argv validation and lets Chromium retain direct ownership of
its bounded frame stream. All twelve entry families now have explicit routes,
but G1 remains red until their native lifecycle and delivery courts are complete.
The browser-session lifecycle was exercised by its macOS public builder/court;
that result is not Candidate evidence because the court is platform-limited and
it is deliberately a registered macOS court rather than a Windows Candidate gate.
