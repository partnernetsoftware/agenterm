# ACU thin-launcher experiment — results

Status: **ordinary seam and Linux L1 pass; G1 is red.** The prototype proves a
small fixed-sibling launcher can execute ordinary ACU argv without owning the
command schema. Five binary-only entry families remain unimplemented, so it does
not accept B or establish the product topology.

## Source and tool identity

- Repository HEAD during measurement: `f1248853dac22703df12f18d02842210a144cf5f`.
- Toolchain: `rustc 1.97.0 (2d8144b78 2026-07-07)`.
- Host: macOS arm64.
- Linux x86_64 launcher: cross-built with `cargo zigbuild`, inspected as a
  stripped x86-64 ELF, and not executed.
- Native macOS artifacts: release-built and executed on arm64.
- Because the host had little free disk, the recorded run reused the existing
  repo-local `target/` cache instead of creating the preferred isolated target
  lane. No other target lane was removed.

Core source SHA-256:

| file | SHA-256 |
|---|---|
| `provider-main-probe/src/lib.rs` | `1b1b5a4b408852f791e457d338f17f2f448ab93a6a9b264386e688bd8221ad5c` |
| `launcher-probe/src/main.rs` | `6a3a1f86d1a6e53bd95a825d95e05c7891c61d46010cd0a39b0c8b80ee44f2f7` |
| `fixtures/bad-abi-provider/src/lib.rs` | `7e18830734df58bfe24c04b778afc1b218cfcbb5f3da8698dc936dacef063cfc` |

## L1 / L2 / L3 bytes

Flat sizes are complete stripped release files. L1 is the launcher, L2 is the
semantic provider, and L3 is their sum. Cross-build and native execution are
kept distinct.

| target | level | artifact | bytes | execution / verdict |
|---|---|---|---:|---|
| Linux x86_64 | L1 | `acu-thin-launcher` | 389,096 | cross-built only; stripped ELF; below 4,194,304-byte H0 court |
| Linux x86_64 | L2 | provider | unmeasured | not built in this bounded local run |
| Linux x86_64 | L3 | launcher + provider | unmeasured | cannot be inferred from macOS L2 |
| macOS arm64 | L1 | `acu-thin-launcher` | 364,768 | native-executed |
| macOS arm64 | L2 | `agenterm-cu-provider.dylib` | 6,925,760 | native-executed |
| macOS arm64 | L3 | L1 + L2 | 7,290,528 | reported honestly; no launcher-budget implication |

The Linux L1 result is direct G2 evidence for that one launcher cell only. No
Windows launcher was built, and the experiment did not perform a paired current
monolith build, so it does not establish all of G2 or the G5 slope.

## Native macOS ordinary and failure court

All runs used the release launcher and provider staged under their canonical
sibling names. The black-box observations were:

| case | process exit | stdout | stderr / boundary result |
|---|---:|---|---|
| `--target current --grant observe capabilities` | 0 | 53,040-byte `CuReply`, `ok:true`, `command:capabilities` | empty |
| `--target current capabilities` | 1 | 166-byte `CuReply`, `ok:false`, `error.code:refused` | empty |
| `--help` | 0 | 14,392-byte `CuReply`, `ok:true`, `command:help` | 14,166-byte human help |
| managed-job owner sentinel | 70 | empty | `provider_entry_mode_unimplemented` |
| missing canonical sibling | 70 | empty | `provider_missing` |
| wrong-ABI canonical sibling | 70 | empty | `provider_abi_version_mismatch` |

The ordinary path really crossed the dynamic ABI and called the existing
library argv adapter. Legal product refusal remained status zero at the ABI and
became product exit 1; loader errors remained boundary exit 70. The release
unit court also exercised versioned request/result layouts, argv count/byte
bounds, malformed output rejection, caught-panic permanent latching, fixed
sibling derivation, entry classification and status/exit separation.

No fallback was observed or implemented. The missing-sibling run failed even
though the ordinary monolithic `agenterm-cu` artifact existed elsewhere in the
repository target tree.

## Gate ledger

| gate | result | evidence / missing work |
|---|---|---|
| G0 safety/authority | partial green | one parser/Executor owner, fixed sibling, bounded ABI, release panic latch, missing/wrong ABI and malformed-result refusals; not all raw failures on three native OSes |
| G1 behavior parity | **red** | ordinary help/capabilities/refusal, exact version text, network-probe and device-I/O fixtures, and managed-job, browser-session and device-lease owners run; the other five binary-entry families remain explicitly unimplemented; no complete paired monolith stdout/stderr corpus |
| G2 launcher budgets | partial green | Linux x86_64 L1 is 389,096 bytes; Windows L1 and same-source paired monolith are unmeasured |
| G3 six-cell/native | red | only Linux x86_64 cross-build plus macOS arm64 native ordinary execution |
| G4 footprint | partial | macOS L1/L2/L3 complete; Linux L2/L3 and other cells unmeasured |
| G5 slope | unmeasured | no synthetic verb addition court |

Decision-tree path: A's historical over-budget fact opened B; this prototype
reaches a real ordinary seam and passes the measured Linux L1 branch, then
stops at **G1 red**. Mapping a binary mode and refusing it is the safe failure
result, not parity. B is therefore not accepted and no G1 claim is made.

### Incremental G1 version slice

The first independently bounded binary-entry slice now carries exact
`--version` / `-V` invocations through entry mode 12. The version line is owned
by the `agenterm-cu` library, so the monolith and provider cannot silently use
different package versions. The launcher accepts mode 12 only for those exact
argv shapes, requires empty stderr and one `agenterm-cu` version line, and
continues to reject every other nonordinary mode at the provider boundary.
`--version extra` remains ordinary argv and returns the existing
`argv_entry_mode_unsupported` refusal. This is partial G1 progress, not a gate
verdict or topology promotion; the earlier measured byte table remains the
record of its stated source revision.

### Incremental G1 network-probe slice

Entry modes 2 and 8 now preserve the existing isolated network-probe journey.
The provider runs the worker or fixture body directly inside the already-owned
child process, where it owns stdin/stdout; the ABI result lengths remain zero,
so the launcher cannot duplicate the protocol frame. The public command still
self-spawns the launcher once, bounds the request and response, kills and reaps
on deadline, and accepts output only after the child exits successfully.

A native staged journey started the mode-8 loopback fixture, then invoked the
public `network-probe` command. That command self-spawned the same launcher in
mode 2 and returned `provider=system-resolver-owned-worker`, `status=reachable`,
one connected attempt, empty stderr and exit zero; the fixture also exited
zero. This closes the network-probe worker/fixture pair only. It does not change
the G1 verdict or promote topology B.

### Incremental G1 managed-job-owner slice

Entry mode 4 now preserves the existing managed-job resident-owner process
boundary. The provider calls the owner body directly inside the already
detached launcher child; it never
redispatches through the public command path and therefore cannot recursively
spawn another launcher. The launch frame remains on stdin and the provider
publishes zero ABI stdout/stderr bytes.

The existing parent side observes durable readiness and checks for early child
exit inside a bounded startup window. A staged native dispatch probe supplied
EOF instead of a launch frame and observed product exit 1, not launcher
boundary exit 70, proving the mode reached its library owner. This is an
entry-boundary probe, not a substitute for the already owning public lifecycle
courts, and it does not change the G1 verdict or promote topology B.

### Incremental G1 browser-session-owner slice

Entry mode 3 now preserves the browser-session owner's first-argument routing:
the provider passes every trailing argv token to `run_owner`, while the launcher
requires zero ABI stdout/stderr bytes and never redispatches the public command.
The owner therefore remains the already detached process whose exact directory
argument names its durable spec and registry.

The macOS public synthetic-browser builder ran against a staged launcher and
provider and emitted both existing lifecycle and typed-failure evidence IDs.
It proved ready/collision/stop/remove, exact owner and browser identities,
malformed and oversized endpoint handling, early exit, and final process/root
cleanup. This is native macOS court evidence, not Candidate evidence: the court
is registered and platform-limited rather than a Windows Candidate gate. No
execution-set or court change is included in this slice.

### Incremental G1 device-lease slice

Entry mode 5 now preserves the resident device-lease owner's exact sentinel and
direct inherited-stdin contract. The provider calls `run_device_lease_owner`
inside the already detached launcher child; the lease secret remains confined
to that launch pipe, both ABI output lengths stay zero, and no public command is
redispatched. Exact-arity and zero-buffer boundary tests are green.

Entry mode 7 now runs the existing Unix PTY fixture directly in the launcher
child, preserving its registry-root/lifetime/baud argv, direct stdout token and
bounded fixture lifetime without duplicating output through ABI buffers. A
staged launcher/provider pair then passed the existing public device-lease
court and emitted `cu.device-lease` plus `cu.device-serial-preserve`. This proves
the fixture and owner together on native macOS; the court remains registered
and Unix-only, so G1 stays red and topology B is not promoted.

## Commands used for the recorded result

The recorded run reused `target/`; replace it with the isolated lane from the
README when capacity permits.

```bash
CARGO_TARGET_DIR=target cargo test \
  --manifest-path research/acu-thin-launcher/Cargo.toml --workspace --release
CARGO_TARGET_DIR=target cargo clippy \
  --manifest-path research/acu-thin-launcher/Cargo.toml \
  --workspace --all-targets -- -D warnings
CARGO_TARGET_DIR=target cargo build \
  --manifest-path research/acu-thin-launcher/Cargo.toml --release \
  -p acu-thin-launcher -p acu-provider-main-probe -p acu-bad-abi-provider
CARGO_TARGET_DIR=target cargo zigbuild \
  --manifest-path research/acu-thin-launcher/Cargo.toml --release \
  --target x86_64-unknown-linux-gnu -p acu-thin-launcher
stat -f '%N %z' \
  target/x86_64-unknown-linux-gnu/release/acu-thin-launcher \
  target/release/acu-thin-launcher \
  target/release/libagenterm_cu_provider.dylib
```

## Required next court

Before B can be reconsidered, the provider process-main ABI must implement and
natively test every listed entry mode with its real stdin/stdout framing,
resident lifetime, cleanup and exit semantics. Then run paired monolith versus
launcher presentation on all ordinary and binary cases, measure Windows L1,
and complete all six build cells plus one native court per OS. Provider
installer/signing/update atomicity remains outside this prototype, as specified
by the experiment.
