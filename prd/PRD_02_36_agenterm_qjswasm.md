# `agenterm-qjswasm` — AgenTerm's `.qjs` engine

Parent: [AgenTerm product tree](../PRD.md#product-tree)
Family contract: [PRD 10](PRD_02_10_rhai_scripting.md)

Status: **`[~]` active product engine**.

**`e77d668`**（当前 pin）applies to both `tinyvm` and `tinyvm-qjs`; the source of truth is
`crates/agenterm-qjswasm/Cargo.toml`, and tests must reject PRD/pin drift.
The earlier opt-in allocation attribution remains diagnostic-only: D0 rejected
the recovery specialization and no allocator rewind/reuse landed. This revision
also closes one host-effect ambiguity: under the declared product door, a bare
host name in value position now fails at compile time by function name instead
of silently becoming a zero-argument host call.

Detailed invention, rejected alternatives, historical pass counts and earlier
pins are preserved in
[`prd/archive/PRD_02_36_agenterm_qjswasm_history_through_v0.1.16.md`](archive/PRD_02_36_agenterm_qjswasm_history_through_v0.1.16.md)
and the earlier focused archive
[`prd/archive/PRD_02_36_agenterm_qjswasm_history_2026-08.md`](archive/PRD_02_36_agenterm_qjswasm_history_2026-08.md).

## Product sentence

`.qjs` is compiled by the pure-Rust `tinyvm-qjs` compiler into standard Wasm,
then validated and interpreted by tinyvm under explicit resource limits. No
QuickJS C library, rquickjs, wasmtime, JIT or executable-memory path is linked.
This crate owns AgenTerm business integration; generic language and VM work is
made in the tinyvm repository and consumed by one exact git revision.

## Markdown-tree DAG

```text
agenterm-qjswasm
├─ pipeline
│  ├─ [x] .qjs parse/lower/encode in upstream tinyvm-qjs
│  ├─ [x] standard .wasm validation + bounded interpretation in tinyvm
│  ├─ [x] persistent slot, named call, typed value and failure translation
│  └─ [x] direct .wasm load/call uses the same validation and Limits
├─ product doors
│  ├─ [x] print and engine-neutral Script host bridge
│  ├─ [x] Fleet facade and public CLI route
│  ├─ [x] qualify / pack / run / bounded check-many, including recursive imports
│  ├─ [x] qjswasm → process.command → ACU headless PTY public journey
│  ├─ [~] tool.* and release-task coverage grows by product need
│  └─ [ ] every v0.1.18 release-critical journey has live .qjs evidence
├─ robustness
│  ├─ [x] steps, pages, table, call-depth and activation-slot limits
│  ├─ [x] typed load, host, throw and budget failures; failed stdout retained
│  ├─ [x] child stdout/stderr truncation is explicit through read/wait/command
│  ├─ [x] process.spawn refuses a 33rd retained handle before native spawn/drain allocation
│  ├─ [x] advisory locks retain stable tombstones and refuse a 33rd lifetime handle before file creation
│  ├─ [x] text and i32-returning host operations apply one parked-result cap to diagnostics
│  ├─ [x] bare declared-host values fail by name; no implicit zero-argument effect
│  ├─ [x] every child entry uses the shared first-instruction contained launcher
│  ├─ [x] invocation-owned process-tree cleanup; no cross-run global backend state
│  ├─ [x] check-many entry + canonical recursive imports share bytes/modules/deadline budgets
│  └─ [x] shared path helper normalizes `.` / `./` before native identity comparison
├─ upstream performance frontier
│  ├─ [x] host-op and string/JSON cost measured before changing limits
│  ├─ [x] cached-length/all-ASCII experiment rejected: 166 > 160-step hard gate
│  ├─ [x] static-length dispatch experiment rejected: 160-step gate met, existing workloads regressed
│  ├─ [x] direct producer metadata experiment rejected: its frozen search court reported 10.5 steps/character against <10
│  ├─ [x] ruler audit: old 7.2 subtracted O(n) `.length`; 10.5 was absolute search cost, not a slower loop
│  ├─ [x] corrected attribution: compare/branch owned 6.5 of 10.5 steps/byte
│  ├─ [x] direct i32.xor: search 10.5 → 9.5 steps/byte; emitted modules −6 B
│  ├─ [x] harness journal: serialize once + fs.append; 33-row court 7.43M → 5.45M steps
│  ├─ [x] temporary-region lifetime court rejected: JSON is 56.69%/59.20% gross allocation, but live return records leave zero operation-return suffix
│  ├─ [x] immediate host-argument D0 rejected: server 8.286749%, wake 6.197612%; two failures make ≥2/3 impossible, so no reuse code
│  └─ [-] never raise a product gate merely to hide engine cost
├─ long horizon: tinyvm as a Wasmtime-class alternative
│  ├─ [ ] WebAssembly core conformance + malformed-module differential court
│  ├─ [ ] interpreter cold-start/size/determinism court wins its chosen workloads
│  ├─ [ ] standard embedder API, diagnostics, fuzzing and multi-instance lifecycle
│  ├─ [ ] optional WASI/Component compatibility in tinyvm, never a hidden AgenTerm OS door
│  └─ [ ] replace Wasmtime only per workload after precommitted parity/security/performance gates
└─ non-goals
   ├─ no Node.js / browser-global compatibility promise
   ├─ no WASI as a second OS authority surface
   ├─ no vendored tinyvm source
   └─ no machine-code JIT in the current engine
```

## Mermaid flowchart memory palace

```mermaid
flowchart LR
  SRC[".qjs source"]
  MANY["check-many manifest<br/>entry + canonical import ledger"]
  COMP["tinyvm-qjs<br/>parse · lower · encode"]
  WASM["standard .wasm bytes"]
  LOAD{"tinyvm validate<br/>Limits accepted?"}
  SLOT["persistent bounded slot"]
  DOOR["versioned Script host door"]
  EXPLICIT["explicit call sites only<br/>bare host value → typed compile refusal"]
  CAPTURE["bounded child capture<br/>per-stream loss flags · JSON-fit"]
  HANDLES["per-slot child ledger<br/>32 retained · pre-spawn refusal"]
  LOCKS["per-slot lock ledger<br/>32 lifetime handles · stable tombstones<br/>pre-open refusal"]
  PATHS["shared path helper<br/>`.` / `./` lexical normalization"]
  PRODUCT["AgenTerm operations<br/>Fleet · tools · process · fs · net"]
  QPTY["ACU headless PTY journey<br/>snapshot/diff · verified resize · send/wait · events · restart refusal"]
  RECEIPT["typed value / stdout / steps<br/>or named failure"]
  UP["tinyvm repository<br/>generic engine write knife"]
  REJECT["reject load/call<br/>host survives"]
  PERF["precommitted performance court"]
  ROLLBACK["gate miss → rollback<br/>retain evidence only"]
  STATIC["static length dispatch court<br/>160 steps · zero slope"]
  REJECT2["reject candidate<br/>join · JSON · object courts regress"]
  DIRECT["direct producer metadata court<br/>join/split pass · search 10.5 misses &lt;10"]
  REJECT3["reject + rollback<br/>preserve evidence, not engine diff"]
  RULER["ruler audit<br/>old 7.2 = absolute search − O(n) length"]
  NEXT["corrected attribution phase A<br/>10.50 absolute − 7.25 historical = 3.25 length"]
  LAYERS["attribution decided<br/>compare/branch owns 6.5 of 10.5 steps/byte"]
  XOR["direct i32.xor accepted<br/>9.5 steps/byte · module −6 B"]
  JOURNAL["harness journal append<br/>33 rows · steps −26.6%<br/>host bytes −84.4%"]
  REGION["temporary-region lifetime court<br/>gross JSON attribution measured"]
  REGION_GATE{"L0 >=25% proven-dead<br/>in >=2 real journeys?"}
  REGION_KILL["L0 failed: live return at heap tail<br/>kill rewind · retain diagnostics"]
  ARG_D0["immediate host-argument D0<br/>server 8.286749% · wake 6.197612%"]
  ARG_GATE{"&gt;=64 KiB + &gt;=10%<br/>in at least two? · NO"}
  ARG_STOP["kill exact specialization<br/>retain attribution only"]
  PIN["AgenTerm exact pin<br/>tinyvm + tinyvm-qjs same rev"]
  NORTH["long horizon<br/>tinyvm replaces Wasmtime<br/>workload by workload"]
  CORE["Core Wasm conformance<br/>malformed + differential fuzz"]
  COURT{"size · cold start · throughput<br/>security · embedder parity"}
  STANDARD["WASI / Component compatibility<br/>in generic tinyvm layer"]

  MANY --> SRC --> COMP --> WASM --> LOAD
  MANY -. bytes · modules · deadline .-> COMP
  UP -. exact git rev .-> COMP & LOAD
  LOAD -->|yes| SLOT --> DOOR --> EXPLICIT --> PRODUCT --> RECEIPT
  PRODUCT -. child process .-> HANDLES --> CAPTURE --> RECEIPT
  PRODUCT -. advisory lock .-> LOCKS --> RECEIPT
  PRODUCT -. process.command .-> QPTY --> RECEIPT
  PRODUCT -. native path identity .-> PATHS --> RECEIPT
  LOAD -->|no| REJECT
  SLOT -. budget / throw / host error .-> REJECT
  SLOT -. persistent heap high-water .-> REGION --> REGION_GATE
  REGION_GATE -->|no| REGION_KILL
  REGION_GATE -->|yes| PERF
  REGION_KILL --> ARG_D0 --> ARG_GATE
  ARG_GATE -->|no| ARG_STOP
  ARG_GATE -->|yes| PERF
  COMP -. measured candidate .-> PERF
  PERF -->|all frozen gates pass| UP
  PERF -->|166 > 160| ROLLBACK --> STATIC
  STATIC -->|C2 workload regression| REJECT2 --> DIRECT
  DIRECT -->|frozen D4 miss| REJECT3 --> RULER --> NEXT --> LAYERS --> XOR --> PIN
  PRODUCT --> JOURNAL --> RECEIPT
  UP -. accumulated generic runtime .-> CORE --> COURT
  STANDARD --> COURT
  COURT -->|selected workload wins| NORTH
  COURT -->|gate misses| UP
```

## Invariants

- The execution core receives Wasm bytes, not JavaScript source.
- All untrusted modules pass load-time validation before instantiation.
- tinyvm `Limits` own VM budgets; AgenTerm does not create a second inconsistent
  step/memory model.
- Host capability discovery describes compatibility, not permission.
- Generic compiler/VM fixes land upstream with upstream tests; AgenTerm changes
  only its pin and product integration.
- Both upstream crates use the same exact revision and are never vendored.
- A pin bump updates both Cargo dependencies, `Cargo.lock`, the PRD's first
  bold current-pin revision and `UPSTREAM_TINYVM_REV` in one coherent commit.
- Performance gates are precommitted. A near miss is recorded and rolled back,
  not converted into a pass by moving its threshold after measurement.
- A cost subtraction is part of the measuring instrument. When an experiment
  changes the subtracted operation, that historical ruler cannot compare the
  two implementations; preserve the old verdict, then use a build-only control
  and an independent closure equation in the next experiment.

## Long-horizon north star: replace Wasmtime, not merely coexist

The strategic ambition is for `tinyvm` to become a credible replacement for
Wasmtime, while `tinyvm-qjs` / qjswasm is its flagship language and automation
front end. This is a staged evidence claim, not a current compatibility claim.
The replacement unit is one declared workload at a time; no global claim is
allowed while its required WebAssembly features, host contract or security
court remains missing.

```text
Wasmtime-class replacement ladder
├─ H0 [x] AgenTerm-owned .qjs automation: bounded interpreter + typed host door
├─ H1 [ ] Core Wasm: proposal inventory, spec tests, malformed modules, differential fuzz
├─ H2 [ ] Runtime: stable embedder API, multi-instance lifecycle, cancellation, diagnostics
├─ H3 [ ] Performance: cold start, resident size, throughput and concurrency by workload
├─ H4 [ ] Compatibility: optional WASI/Component adapters in generic tinyvm
└─ H5 [ ] Adoption: replace an existing Wasmtime workload only after its frozen court passes
```

The first battleground deliberately favors the architecture we are building:
small cross-platform automation programs, no executable memory, deterministic
resource accounting, fast cold start, typed host calls and six-cell behavioral
parity. Later courts widen toward general Wasm. Wasmtime remains a reference
oracle and benchmark source during that climb; copying its dependency surface
into AgenTerm would not count as replacement.

WASI and the Component Model, if implemented, belong to the generic tinyvm
repository. AgenTerm still exposes operating-system authority through its
versioned typed Script door. A WASI adapter must not become an undocumented
second route around ACU/AgenTerm product contracts.

## Current acceptance

- [x] active qjswasm runtime executes `.qjs` check/run and expression eval.
- [x] `script api [MODULE] [--status shipped|planned|all] [--tree|--json]` renders one deterministic hierarchical object tree with reviewed Node.js/Bun analogues and returns the same filtered versioned catalog with explicit view and comparison metadata.
- [x] qjswasm computation budget fails closed with the public limit exit class.
- [x] syntax/compiler refusals and unsupported source methods use the same
  public `script` failure class through direct run, task run, and check-many;
  loader, signature, and host-door setup failures remain `configuration`.
- [x] qjswasm tool profile executes bounded child processes with typed failures.
- [x] Synchronous `process.command`, `process.command_stdout` and
  `process.status` calls apply the documented 60-second deadline when the spec
  omits `timeout_ms`; the wasm step budget cannot bound time spent inside a
  host call. `process.spawn` remains explicitly long-lived and is instead
  bounded by its retained-handle ceiling, optional deadline and slot cleanup.
- [x] One qjswasm slot retains at most 32 `process.spawn` handles, including
  completed handles whose first wait answer remains replayable. The 33rd call
  is rejected before parsing into a host command, spawning an OS process, or
  creating stdout/stderr drain threads. Current public scripts need at most
  nine, so the ceiling leaves measured headroom without turning the general
  host-operation budget into thousands of native resources.
- [x] child-process wait and incremental-read limits reject negative values
  before consuming or mutating the owned handle. A negative timeout is not an
  alias for an unbounded wait, and a negative capture size is not an alias for
  the host address-space maximum; the caller can correct the argument and
  still wait/clean up the same child.
- [x] privacy-bounded audit records contain identity without storing secret payloads.
- [x] qjswasm tool process returns bounded child stdout and stderr.
- [x] Child capture never hides loss: `process.read`, `process.wait`, and
  `process.command` publish per-stream truncation flags, including additional
  cuts required after JSON escaping. `process.command_stdout` refuses a
  truncated result because its raw-text return cannot carry those flags.
- [x] All four tool child entry paths (`command`, `command_stdout`, `status`,
  `spawn`) use the same `agenterm-platform` contained launcher. Windows creates
  the child suspended, assigns its kill-on-close Job before the first user
  instruction and only then resumes it; Unix establishes the owned process
  group in `pre_exec`. Working directory, inherited environment mutation,
  stdin, capture and file redirection retain their public behavior. Explicit
  kill, timeout, cancellation and slot reclamation terminate the owned tree and
  reap the direct child; containment setup failure returns typed instead of
  exposing an uncontained handle.
- [x] `check-many` charges entry files and recursively resolved imports to one
  aggregate source ledger, applies the per-source byte limit to every imported
  module, caps resolved modules at 1024 and checks the same wall deadline during
  resolution and after compilation. A canonical-path cache charges and counts a
  shared module once across repeated imports and manifest entries; whitespace-
  indented `export` declarations still enter the library check path. Budget
  failures keep the public `limit` exit class; unresolved modules remain ordinary
  script diagnostics.
- [x] The shared qjswasm task compatibility helper maps `.` to the exact
  current directory and strips only the host-valid leading dot segment from
  `./...` (plus `.\...` on Windows). Native path identity can therefore be
  compared without a false `/./` mismatch, while POSIX backslashes remain
  ordinary filename characters. The Script smoke owns the lexical regression;
  the macOS ACU journey proved the native `process-cwd` comparison end to end.
- [x] qjswasm tool process reports a missing child as a typed failure.
- [x] The qjswasm six-cell build and qualification orchestrators recursively
  enter named tasks through the live `agenterm cli script task run` front door.
  A policy regression rejects the retired `agenterm rh task run` argv before a
  six-cell attempt can report six misleading pre-build failures.
- [x] The platform-neutral `cu-pty-smoke` task drives the public ACU facade
  through qjswasm: absent/running/stale/pruned inventory reconciliation, exact
  epoch-bound structured snapshot/diff, exact-grid resize with lease cleanup,
  non-empty event continuation and same-name restart refusal,
  literal input receipt, loss-aware raw-output match,
  exact exit status, typed finalized-without-match, and verified authority
  disappearance. The native macOS court completed in 728 ms at 2,081,763
  steps, 44 host operations, 12,279 host bytes and 3 heap pages. Native Linux
  x86_64 is also green in the UTM desktop court after the task stopped assuming
  that a QGA/system execution session has `HOME`: every ACU child receives a
  private `HOME`/XDG tree rooted in the journey directory. The same public
  journey is also green in the native Linux aarch64 UTM desktop court. Windows
  x86_64 is green through the interactive UTM job agent and public
  `agenterm.com` route, including native ConPTY execution and verified cleanup;
  the native Windows aarch64 UTM court passes the identical contract. Exact
  x86_64 macOS product/qjswasm/ACU/ABI bytes also pass under Rosetta. The local
  six-cell user-space projection is therefore green; this is not a claim of an
  Intel macOS kernel court.
  The enlarged inventory/prune journey was rerun from exact source `a6a1c7b9`
  and is green in the same six cells. Its `release-fast` transfer bundles were
  8.1 MiB (Linux arm64), 8.3 MiB (Linux x86_64), 4.8 MiB (Windows arm64), and
  5.0 MiB (Windows x86_64); the earlier 55 MiB debug bundle was rejected as an
  inefficient court input. Windows qualification requires a job-specific
  marker plus the PASS line and exit receipt, because a readiness probe can
  leave a stale zero-exit file that must not satisfy the next job.
  The next local macOS enlargement added persisted `pty-snapshot`, bounded
  `pty-diff`, a 37×91 `pty-resize`, and `pty-events` after the exact output
  match. It asserts the same durable job, stable tab, server epoch and event
  cursor, exact resize read-back, detached temporary lease, separate
  row/metadata diffs and a non-empty terminal event. It then restarts the same
  name and requires the old baseline to fail `pty_snapshot_authority_changed`.
  That enlarged journey now passes Windows x86_64 at `121b76ed`. Windows arm64
  found a native-only boundary case after resize: ConPTY can expose its legal
  wrap-pending cursor one column beyond the visible grid. `735b7e0c` normalizes
  that wire projection, after which the arm64 journey passed all nine stages
  and cleanup. The remaining leaf is identity, not known behavior: rerun the
  current `735b7e0c` bytes on x86_64 once its TCG interactive agent is live;
  do not splice the two differently pinned Windows passes into a new exact-SHA
  six-cell claim.
- `cargo test -p agenterm-qjswasm` owns crate behavior; do not pin a historical
  pass count because the suite grows.
- public Script CLI black boxes own `.qjs` route, diagnostics, receipts and
  product-host calls.
- v0.1.18 G4 owns the release-critical task/journey migration. Quick-only green
  cannot substitute for the complete Candidate gate.
- Performance changes require before/after step, wall-time, memory and output
  measurements on the same guest and pin; a larger limit is not an optimization.

Current execution plan: [`plan/plan-v0.1.18.md`](../plan/plan-v0.1.18.md).
