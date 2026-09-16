# ACU thin-launcher experiment — results

Status: **ordinary seam and Linux L1 pass; G1 is red.** The prototype proves a
small fixed-sibling launcher can execute ordinary ACU argv without owning the
command schema. All twelve binary-entry families now have explicit routes and
repository installers publish the fixed sibling, but incomplete native
lifecycle and installed-activation courts still prevent accepting B or
establishing the product topology.

## Source and tool identity

- Repository HEAD during measurement: `904da6d42c0405b32b849716ff0aef46b702a765`.
- Toolchain: `rustc 1.97.0 (2d8144b78 2026-07-07)`.
- Host: macOS arm64.
- Linux x86_64 launcher and provider: cross-built with `cargo zigbuild`,
  inspected as stripped x86-64 ELF files, and not executed.
- Native macOS artifacts: release-built and executed on arm64.
- The recorded run used separate repo-local target lanes for native macOS and
  Linux cross-build evidence; both lanes were reclaimed after recording.

Core source SHA-256:

| file | SHA-256 |
|---|---|
| historical `provider-main-probe/src/lib.rs` at the recorded measurement revision | `168fa7be9f6e67a6dc12334e4420f5bad0dca8323abd959acd5a92afdc74c6f4` |
| `launcher-probe/src/main.rs` | `4d7094747dc1968aba250aaa620df3afe71edea21ae71f74c1537168690210cb` |
| `fixtures/bad-abi-provider/src/lib.rs` | `7e18830734df58bfe24c04b778afc1b218cfcbb5f3da8698dc936dacef063cfc` |

## L1 / L2 / L3 bytes

Flat sizes are complete stripped release files. L1 is the launcher, L2 is the
semantic provider, and L3 is their sum. Cross-build and native execution are
kept distinct.

| target | level | artifact | bytes | execution / verdict |
|---|---|---|---:|---|
| Linux x86_64 | L1 | `acu-thin-launcher` | 393,144 | cross-built only; stripped ELF; below 4,194,304-byte H0 court |
| Linux x86_64 | L2 | provider | 15,002,344 | cross-built only; stripped ELF shared object |
| Linux x86_64 | L3 | launcher + provider | 15,395,488 | complete flat-file sum; no runtime claim |
| macOS arm64 | L1 | `acu-thin-launcher` | 365,328 | native-executed |
| macOS arm64 | L2 | `agenterm-cu-provider.dylib` | 8,162,016 | native-executed |
| macOS arm64 | L3 | L1 + L2 | 8,527,344 | reported honestly; no launcher-budget implication |

The two L1 results are direct G2 evidence for those launcher cells only. The
complete entry table added 4,048 bytes to Linux L1 and 560 bytes to macOS L1
relative to the earlier measurement, while both remain far below H0. No Windows
launcher was built, and the experiment did not perform a paired current
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

## Production dual-ABI provider rerun

An incremental native macOS arm64 rerun used the uncommitted source state atop
`8fa3b5136fbd163f026fe81321293874b833248b`. The research-only process-main
provider was removed. Its ABI is now additive in the production
`agenterm-cu-provider` artifact beside the pre-existing embedded call ABI, and
both the monolith and provider call the authoritative
`agenterm_cu::process_entry` classifier.

Source SHA-256 for this rerun:

| file | SHA-256 |
|---|---|
| `crates/agenterm-cu-provider/src/process_main.rs` | `b50dbb9be81ba0a21f329adbb96535379970fdcbea38d5ebfe1119223639f364` |
| `crates/agenterm-cu/src/process_entry.rs` | `78bb67e1ff311c6aca147e0b9c9b1e237b1b2deb27199fe49d4277f3c7ed765b` |
| `launcher-probe/src/main.rs` | `4d7094747dc1968aba250aaa620df3afe71edea21ae71f74c1537168690210cb` |

The ABI-release production provider was 8,178,784 bytes and the release
launcher was 365,376 bytes. These are whole-file native artifacts, not a new
L1/L2 comparison with the earlier revision. All twelve bounded paired cases
passed with exact exit/stdout/stderr parity, and the staged bad-ABI control
still diverged with boundary exit 70. Provider tests also passed 18 unit cases
plus two export/header contract cases, proving that one fixed sibling exports
both ABI families. This removes the single-file dual-ABI blocker for a later
Windows Candidate staging attempt; it does not itself run that Windows journey
or promote topology B.

The same research workspace then cross-built its Windows x86_64 boundary
artifacts with `cargo xwin`; none was executed. The release launcher was
177,152 bytes, the parity court 316,416 bytes and the bad-ABI fixture DLL
101,888 bytes. The first attempt exposed a missing Windows `cfg` on the
foreign-origin expectation arm; after the arm was made consistent with its
Unix-only case and enum variant, the complete research workspace built cleanly.
This is build evidence only. The production provider, monolith and required
journey still need a native Windows build/stage/run.

## Gate ledger

| gate | result | evidence / missing work |
|---|---|---|
| G0 safety/authority | partial green | one parser/Executor owner, fixed sibling, bounded ABI, release panic latch, missing/wrong ABI and malformed-result refusals; not all raw failures on three native OSes |
| G1 behavior parity | **red** | all twelve entry families are explicitly routed; a bounded macOS paired court now covers the non-mutating presentation/refusal subset with absolute anchors and a bad-ABI negative control, while native lifecycles, installed activation and Linux/Windows paired evidence remain incomplete |
| G2 launcher budgets | partial green | Linux x86_64 L1 is 393,144 bytes and macOS arm64 L1 is 365,328 bytes; Windows L1 and same-source paired monolith are unmeasured |
| G3 six-cell/native | red | only Linux x86_64 cross-build plus macOS arm64 native ordinary execution |
| G4 footprint | partial | macOS arm64 and Linux x86_64 L1/L2/L3 complete; four other cells unmeasured |
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

### Incremental G1 verbs slice

Entry mode 10 now delegates directly to the library-owned `run_verbs` surface
and carries its returned bytes through the bounded ABI output buffer. The
launcher admits the first-argument `verbs` shape, keeps stderr empty, validates
text, JSON-array and typed-error replies, and does not claim direct fd ownership.

A native staged comparison matched the monolith byte-for-byte for
`verbs --json` (208,307 stdout bytes), `verbs --text` (32,324 stdout bytes), and the
typed `verbs --bogus` refusal (140 stdout bytes, exit 2); all three had empty
stderr. This closes only mode 10. G1 remains red and topology B is not promoted.

### Incremental G1 X11 clipboard-owner slice

The mode-11 entry body now has one library owner shared by the monolith and
provider. It selects text or typed bytes from the existing environment marker,
reads the inherited stdin to EOF, calls the existing clipboard mechanism and
publishes no ABI stdout/stderr bytes. The launcher preserves the first-argument
sentinel shape and treats the provider call as direct process ownership.

On native macOS, staged and monolith binaries matched on two pre-mechanism
refusals: an empty MIME environment value and non-UTF-8 text stdin both returned
exit 1 with empty stdout/stderr. The X11 selection lifetime remains evidenced
only by registered Linux courts requiring `DISPLAY`; it was not rerun here.
G1 remains red and topology B is not promoted.

### Incremental G1 hotkey-host slice

Entry mode 9 now sends both the canonical `host` spelling and compatibility
alias `hotkeys` to the existing library-owned desktop host. The provider call
owns the real process event loop and standard streams directly, so resident
host lifetime, platform diagnostics, Windows JSON self-test output and product
exit codes are not copied into ABI buffers. The launcher predicts the same
first-argument shapes and keeps all trailing process arguments intact, including
`--self-test` and `--json`.

The research unit boundary covers both spellings, trailing self-test arguments,
zero ABI-buffer publication and direct process ownership. A native host
self-test was deliberately not run in this increment because macOS self-test
may perform a real window-placement action when Accessibility is trusted. No
public court currently drives this entry, and the macOS LaunchAgent delivery
now builds, stages and verifies the provider beside both launcher copies. That
is delivery-layout evidence only: no public court or native host self-test owns
the lifecycle. G1 therefore remains red.

### Incremental G1 privilege-broker slice

Entry mode 6 now calls the existing platform broker body directly inside the
service-manager-owned process. Its sentinel remains exact, trailing argv falls
back to the ordinary typed refusal, and broker diagnostics continue to use the
real process stderr while ABI output lengths remain zero. The boundary tests
pin those facts without attempting privileged installation or consent.

Repository delivery now publishes the fixed sibling on both privilege paths:
Linux installs five digest-sealed artifacts in one recoverable transaction, and
macOS stages a second provider beside the Resources helper while bundle and
package courts require it. This remains layout and transaction evidence, not live
privileged activation. Root-owned or signed service activation, peer
authentication, consent and idle-exit journeys were not run here. G0 and G1
therefore remain incomplete even though the mode-6 route itself no longer
returns `provider_entry_mode_unimplemented`.

### Incremental G1 Native Messaging slice

Entry mode 1 now shares one library-owned adapter for extension-origin checks,
the closed platform argv shape, error presentation and browser-owned host
lifetime. The launcher predicts only the `chrome-extension://` prefix and leaves
the exact extension id plus Windows `--parent-window` validation to that product
adapter. Native Messaging owns real stdin/stdout directly; its one-MiB frame
limit and bounded input queue remain at the protocol layer rather than being
replaced by the ABI's whole-call output budget.

A native staged refusal matched the monolith for a foreign extension origin:
both exited 1, wrote zero stdout bytes and emitted exactly
`browser_bridge_origin_invalid` on stderr. This closes the last unimplemented
entry route, not G1 itself: no positive Chromium frame journey ran, the browser
bridge court is registered rather than Candidate-required, and no installed
browser journey has exercised the now-published fixed sibling.

### Incremental G1 paired-parity court

Revision `34b5dfac` adds a repeatable black-box court that runs the same argv
against a same-source monolith and staged launcher/provider, requires identical
exit status plus stdout/stderr bytes, and separately enforces absolute product
expectations. Both children have a ten-second deadline, concurrently drained
64-KiB stream bounds, and kill-plus-reap timeout cleanup. Canonical paths must
identify distinct executables.

The native macOS arm64 run passed these twelve cases:

| cases | absolute expectation | paired result |
|---|---|---|
| `--version` | exit 0; one `agenterm-cu` line; empty stderr | exact |
| capabilities | exit 0; `data.mechanism == "libagenterm"`; empty stderr | exact |
| ordinary capabilities without grant | exit 1; typed `refused`; empty stderr | exact |
| five exact-entry sentinels plus `extra` | exit 1; typed `argv_entry_mode_unsupported`; empty stderr | exact |
| `verbs --help` | exit 2; typed `usage`; empty stderr | exact |
| Native Messaging parent-window argument on non-Windows | exit 1; empty stdout; exact invocation refusal stderr | exact |
| foreign Native Messaging origin | exit 1; empty stdout; exact origin refusal stderr | exact |
| privilege broker without service-manager activation | exit 3; empty stdout; exact transport-failure stderr | exact |

The same run replaced the staged sibling with the bad-ABI fixture. The launcher
then exited 70 with `provider_abi_version_mismatch`, while the monolith version
case remained valid and the court required the observations to differ. This is
a negative control proving the harness can see a known boundary fault rather
than accepting two equally broken sides.

This is native macOS evidence for a deliberately non-mutating subset, not a G1
pass. The Linux-only `host`/`hotkeys` unsupported cases are compiled into the
Linux lane but were not run here; macOS hotkey self-test is excluded because an
Accessibility-trusted host may perform a real window placement. Resident
lifecycle, installed service activation, Windows and Linux paired execution,
and Candidate ownership remain open.

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
  -p acu-thin-launcher -p acu-bad-abi-provider
cargo build --locked --profile abi-release -p agenterm-cu-provider
CARGO_TARGET_DIR=target cargo zigbuild \
  --manifest-path research/acu-thin-launcher/Cargo.toml --release \
  --target x86_64-unknown-linux-gnu -p acu-thin-launcher
stat -f '%N %z' \
  target/x86_64-unknown-linux-gnu/release/acu-thin-launcher \
  target/release/acu-thin-launcher \
  target/abi-release/libagenterm.dylib \
  target/abi-release/libagenterm_cu_provider.dylib
```

## Required next court

Before B can be reconsidered, extend the paired court across Linux and Windows,
and natively test every listed entry mode with its real stdin/stdout framing,
resident lifetime, cleanup and exit semantics. Then measure Windows L1 and
complete all six build cells plus one native court per OS. Product
installers now publish the fixed sibling and test their file transactions; the
remaining delivery question is live installed activation and lifecycle under
the native service managers.
