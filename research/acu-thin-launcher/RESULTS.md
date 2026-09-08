# ACU thin-launcher experiment — results

Status: **ordinary seam and Linux L1 pass; G1 is red.** The prototype proves a
small fixed-sibling launcher can execute ordinary ACU argv without owning the
command schema. It does not implement the binary-only entry modes, so it does
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
| G1 behavior parity | **red** | ordinary help/capabilities/refusal run, but every binary-only entry mode is explicitly unimplemented; no paired monolith stdout/stderr corpus |
| G2 launcher budgets | partial green | Linux x86_64 L1 is 389,096 bytes; Windows L1 and same-source paired monolith are unmeasured |
| G3 six-cell/native | red | only Linux x86_64 cross-build plus macOS arm64 native ordinary execution |
| G4 footprint | partial | macOS L1/L2/L3 complete; Linux L2/L3 and other cells unmeasured |
| G5 slope | unmeasured | no synthetic verb addition court |

Decision-tree path: A's historical over-budget fact opened B; this prototype
reaches a real ordinary seam and passes the measured Linux L1 branch, then
stops at **G1 red**. Mapping a binary mode and refusing it is the safe failure
result, not parity. B is therefore not accepted and no G1 claim is made.

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
