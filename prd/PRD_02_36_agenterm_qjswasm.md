# `agenterm-qjswasm` — AgenTerm's `.qjs` engine

Parent: [AgenTerm product tree](../PRD.md#product-tree)
Family contract: [PRD 10](PRD_02_10_rhai_scripting.md)

Status: **`[~]` active product engine**.

**`9ac2598`**（当前 pin）applies to both `tinyvm` and `tinyvm-qjs`; the source of truth is
`crates/agenterm-qjswasm/Cargo.toml`, and tests must reject PRD/pin drift.
This revision adds a generic, call-scoped cooperative-interruption seam: one
invocation or start-section instantiation borrows one `AtomicBool`; pure guest
computation polls it without a host callback, and qjswasm maps the distinct
core `Interruption` class to `QjswasmError::Cancelled`. The identity is never
stored in `Limits`, a module, or a persistent slot. The static core remains
exactly 101,256 bytes; native
host callbacks that are already blocked still require their own cooperative
wait path or the Script worker's hard process-containment deadline.
The `agenterm:acu` door passes that same borrowed identity into the product
provider and distinguishes a cancellation request from an acknowledged
pre-effect cancellation. Only the acknowledgement becomes an uncatchable
`QjswasmError::Cancelled`; a late request without acknowledgement preserves the
authoritative ACU reply. PRD 28 owns which Executor operations can acknowledge.

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

At the host boundary this crate is AgenTerm's **native import compiler/adapter**.
It lowers a guest declaration into a native target, ABI values, call-scoped
storage and a transport-specific result plan, then invokes agenterm-dyn's
policy-free **ABI Importer mechanism**. The wider product concept is the
**Native Importer**: ABI calls are its mature first family, dedicated `ioctl` is
already a sibling path, and future evidence may admit syscall, direct host entry or other
native mechanism families without pretending they are dynamic-library calls.
Dyn connects a lowered native operation to the host; qjswasm owns the
declaration and lowering; tinyvm only executes the resulting Wasm and host
bridge; CU consumes selected capabilities through its fixed-sibling provider
and typed projection. This is not JavaScript module import, a general libffi,
an authorization system, or a direct `agenterm-cu -> agenterm-dyn` dependency.

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
│  ├─ [x] shipped host imports retire their former subprocess witnesses
│  │  └─ startup-smoke now reads `process.list` directly: one bounded platform
│  │     facade replaces the pgrep/tasklist branches and their text parser,
│  │     deleting 37 script LOC and one child process per inventory snapshot
│  │  └─ `fs.symlink_metadata` projects the platform facade's Unix mode as octal
│  │     text: native-ipc-smoke no longer launches GNU/BSD `stat` probes or owns
│  │     their platform-specific parsing branch
│  │  └─ the same metadata snapshot projects the Unix owner id as decimal text;
│  │     control-center-linux-smoke keeps its independent effective-uid oracle
│  │     while deleting the second `stat` process used to reread owner and mode
│  │  └─ the shared command journal publishes through `fs.append` on every host;
│  │     native-ipc-smoke no longer launches one `sh -c cat` child per record or
│  │     maintains a second copy of the JSONL record projection
│  │  └─ `process.observe(pid)` projects the platform crate's bounded single-PID
│  │     `live | dead | unknown` record instead of transporting a full process
│  │     inventory; `rh_compat.process_alive` removes tasklist/kill subprocesses
│  │     across eleven checks in six gates while preserving its boolean surface
│  │  └─ `process.parent(pid)` projects one direct-parent relationship through
│  │     the same exact-key platform family; native-ipc-compat's five ownership
│  │     checks no longer launch PowerShell/ps or parse their process-table text
│  │  └─ task-side process witnesses reuse the shipped process door: script-smoke
│  │     removes repeated `ps` parsing and `kill -9`, while control-center-linux
│  │     replaces `kill -0` polling with exact-PID observation; a real 20-round
│  │     court measured 26.78% steps, 77.84% bridge bytes and 29.46% wall time
│  │     versus the former `ps` path at equal host-op count
│  │  └─ fresh-clone rehearsal consumes the same shipped process door for its
│  │     full descendant graph, raw-PID cleanup and handle-to-PID root identity;
│  │     three stale fail-closed gaps are removed without a new host operation;
│  │     the cross-platform door self-test is green, while the Windows clean-main
│  │     owning rehearsal remains the delivery gate rather than inferred evidence
│  │  └─ owned CU fixtures retire command-line `kill -TERM` only where the same
│  │     scope retains the Script child handle and no graceful-exit result is a
│  │     product assertion; a11y-status and gtk-copy are the first Linux slice,
│  │     using the cross-platform owned-handle cleanup whose primitive waits boundedly
│  ├─ [x] declaration-driven Native Importer composition
│  │  ├─ user problem: scripts need an extensible native surface without copying a loader or ABI executor
│  │  ├─ invariant: spec/schema/catalog/nullability and exact-family cardinality are local qjswasm policy
│  │  ├─ mechanism: every non-ioctl call lowers to caller-provided AbiSignature/AbiValue and one dyn invoke_abi
│  │  │     (or its reuse entry with the handle this engine already loaded)
│  │  ├─ layered identity
│  │  │  ├─ qjswasm = import declaration + lowering + storage/result ownership
│  │  │  ├─ agenterm-dyn = ABI Importer mechanism; no exposure or caller policy
│  │  │  ├─ tinyvm = no-JIT Wasm executor; no dyn, symbol or CU knowledge
│  │  │  └─ CU = typed product projection through the fixed-sibling provider; no direct dyn dependency
│  │  ├─ [x] one engine keeps at most 32 loaded libraries, keyed by the declared string, never evicted
│  │  │  ├─ user problem: a script that calls one library in a loop paid one load and one close per call
│  │  │  ├─ invariant: reuse changes only where the load comes from — a full table falls back to the one-shot entry, a failed load is reported once by dyn's own error, so no refusal code is added and no capability is narrowed
│  │  │  └─ evidence: the native_cache unit court (one string hands back one allocation, 32 then the 33rd declined without eviction, a failed load owns no slot, keys are the declared bytes) plus unchanged door behavior
│  │  ├─ black-box owner: native_door + native_door_schema + native/ACU composition smoke
│  │  │  └─ six-cell delivery: the Candidate runtime-control step runs that court after the
│  │  │     ACU provider courts and publishes `cu.retirement-cell.native-acu-composition`
│  │  ├─ safe failure: malformed, unlisted, noncanonical, out-of-span and dyn mechanism failures remain typed
│  │  ├─ [x] native pipeline folding stops at the one shared invoke seam
│  │  │  ├─ user problem: dyn already executes one policy-free ABI path, but raw and JSON transports
│  │  │  │     still repeat family-specific decode, preparation, result conversion and error plumbing
│  │  │  ├─ invariant: qjswasm keeps exposure, nullability, guest storage, budget and error ownership;
│  │  │  │     folding turns that policy into declarations and never moves it into dyn or tinyvm
│  │  │  ├─ [x] declaration/mechanism ownership remains deliberately split
│  │  │  │  ├─ qjswasm's 14 pointer declarations retain ptr/ptr? nullability and JSON exposure policy;
│  │  │  │  │     dyn's 8 pointer mechanisms answer only whether an ABI trampoline exists
│  │  │  │  └─ rejected fold: nullability is already consumed during decode, but dyn's 75 mechanism
│  │  │  │        shapes still exceed the 66 distinct exposed shapes by 9; deriving dispatch would
│  │  │  │        widen the product surface, while a negative exclusion list merely reverses the
│  │  │  │        duplicate truth, and the separate UnixIoctl route still remains qjswasm-owned
│  │  │  ├─ [x] the unread register-class projection is retired
│  │  │  │  ├─ qjswasm no longer stores `NativeSignatureClasses` on every decoded call or
│  │  │  │  │     publishes the hypothetical 381-pattern GP/F64 account
│  │  │  │  └─ dyn's 75-shape mechanism query and qjswasm's exposure catalog remain the
│  │  │  │        two live owners; parser and door behavior are unchanged
│  │  │  ├─ [x] one private invoke seam owns ABI-position conversion, NativeCall construction,
│  │  │  │     handle reuse and dyn-error mapping for all six non-ioctl execution sites
│  │  │  │  ├─ evidence: raw exact/fixed/fixed-pointer + JSON exact/fixed/region all call
│  │  │  │  │     `invoke_prepared`; `NativeCall` has one production construction site;
│  │  │  │  │     qjswasm 350/0 and dyn 37/0 remain green
│  │  │  │  └─ economy: `native.rs` +113/−182, net −69 LOC; two error mappers,
│  │  │  │        two ABI-parameter helpers and six repeated call blocks collapse without
│  │  │  │        adding a struct, trait, branch or public API
│  │  │  ├─ [x] raw and JSON transports share one scalar canonicalizer per ABI type
│  │  │  │  ├─ user problem: the fixed-family copies drifted from the exact-family grammar,
│  │  │  │  │     so JSON refused the already-exposed `u32` position that raw calls executed
│  │  │  │  ├─ invariant: family dispatch still selects the admitted trampoline; canonical
│  │  │  │  │     scalar bytes and typed value failures have one owner across transports
│  │  │  │  ├─ evidence: the JSON `getpriority` oracle exercises `i32(i32,u32)` on macOS and
│  │  │  │  │     `i32(u32,u32)` on Linux against libc, while the raw oracle remains unchanged
│  │  │  │  └─ economy: two strict-subset helpers are deleted; no trait, table, ABI shape,
│  │  │  │        loader, policy, handle or public entry is added
│  │  │  │        the raw fixed-pointer family now delegates its admitted I32/U32/U64/Usize
│  │  │  │        positions to that same canonicalizer instead of repeating four conversions
│  │  │  ├─ [x] exact and fixed scalar dispatch share one execution arm per transport
│  │  │  │  ├─ invariant: `native_dispatch` still chooses the admitted trampoline family;
│  │  │  │  │     only their now-identical canonicalize → invoke → transport-result path is shared
│  │  │  │  └─ economy: raw and JSON each delete one duplicate match arm and its parallel
│  │  │  │        SAFETY explanation without adding a helper, wrapper, branch or public surface
│  │  │  ├─ [x] dispatch carries only data needed after family classification
│  │  │  │  ├─ exact homogeneous and six heterogeneous scalar declarations converge on one
│  │  │  │  │     `Scalar` execution token because no downstream stage observes their family identity
│  │  │  │  └─ economy: the six heterogeneous signatures live as declaration data instead of an
│  │  │  │        enum + variant inventory + exhaustive projection; admission remains two explicit
│  │  │  │        truths while execution carries only the one distinction it consumes
│  │  │  ├─ [x] pointer dispatch carries a signature, not a prototype identity
│  │  │  │  ├─ the 14 ptr/ptr? declarations live in one data table; raw conversion, JSON admission,
│  │  │  │  │     region ownership and result rebasing continue to read the original declaration
│  │  │  │  └─ economy: one `Pointer` token replaces 14 enum variants, their inventory and their
│  │  │  │        exhaustive projection; JSON still admits exactly the 10 i32-result rows
│  │  │  ├─ [x] no `PreparedAbiCall` wrapper: `invoke_prepared` already owns the one call
│  │  │  │     construction site; a struct over the same borrowed spec and arguments removes no truth
│  │  │  ├─ [x] no `NativeOutcome` wrapper: raw bits, JSON scalars and region snapshots are
│  │  │  │     intentionally different transport results; a shared wrapper adds a shape without
│  │  │  │     removing their distinct validation or encoding rules
│  │  │  ├─ evidence: each leaf removes a named parallel arm/helper/table while preserving public
│  │  │  │     bytes, typed failures, check-before-loader and native+ACU composition
│  │  │  ├─ economic account: record removed duplication, preserved semantics, released
│  │  │  │     LOC/bytes/steps/build-time/maintenance touchpoints, and the capability funded next
│  │  │  ├─ net-reduction gate: an added abstraction that removes no parallel truth is overhead,
│  │  │  │     not folding, and must not consume the space it claims to release
│  │  │  ├─ safe failure: if ownership, ordering or byte parity cannot be retained, keep the old path
│  │  │  └─ non-goal: no new ABI shape, symbol policy, tinyvm→dyn dependency or JIT authorization
│  │  ├─ [~] import lowering becomes the reusable product grammar
│  │  │  ├─ [x] raw and JSON paths share NativeCall construction in the private unsafe
│  │  │  │     `invoke_prepared` seam; result encoding remains transport-specific
│  │  │  ├─ [x] raw scalar and pointer dispatch share one execution/result tail while
│  │  │  │     retaining separate argument mappers and their distinct typed refusals
│  │  │  ├─ [x] raw bits, JSON scalars and JSON regions retain distinct result plans
│  │  │  ├─ [x] dedicated UnixIoctl remains a sibling mechanism, not a counterfeit ABI family
│  │  │  ├─ [x] mechanism-only shapes are derived from dyn and differenced against exposure
│  │  │  │     instead of being restated as a second mechanism table
│  │  │  ├─ [ ] add a new lowering/storage/result form only when a named consumer is blocked
│  │  │  │     and the leaf either deletes parallel machinery or proves measured value
│  │  │  ├─ [ ] admit syscall, direct host entry or another mechanism family through the same declaration
│  │  │  │     grammar only after its owner, error boundary and evidence court are named
│  │  │  └─ economic target: adding a host ability approaches adding one owned declaration
│  │  │        plus its evidence, not another family-specific Rust dispatch pipeline
│  │  ├─ known limits of the current Native Importer
│  │  │  ├─ the compatibility court currently derives 75 dyn mechanism shapes and
│  │  │  │     69 qjswasm declarations / 66 distinct exposed shapes; the court owns these counts
│  │  │  ├─ JSON regions currently admit the 10 court-derived pointer prototypes whose native result is i32;
│  │  │  │     pointer results and the other result families remain intentionally unexposed there
│  │  │  ├─ one Engine reuses at most 32 library handles; symbols are still resolved per call
│  │  │  ├─ an ABI prototype does not encode pointee width, alignment, termination or C struct layout
│  │  │  ├─ raw GuestSpan proves only that the caller-declared range lies in guest memory;
│  │  │  │     it does not prove the foreign callee stays within that range, and a false ABI/pointee
│  │  │  │     assertion remains contained-worker failure just like a false function signature
│  │  │  ├─ rejected hardening: a `(target,library,symbol,signature)` pointee table would validate
│  │  │  │     only a closed known-symbol set and refuse every other import, turning robustness into
│  │  │  │     a symbol allowlist; arbitrary native-call containment belongs at the worker boundary
│  │  │  └─ non-host target checks are compile evidence unless a native runner court says otherwise
│  │  ├─ [x] JSON pointer calls take one call-scoped host region per pointer position
│  │  │  ├─ user problem: a JSON caller has no guest linear memory to point into, so every
│  │  │  │     pointer prototype answered `native_invocation_signature_unsupported`
│  │  │  ├─ invariant: for exactly one synchronous call the host owns the storage's address,
│  │  │  │     16-byte alignment, zero fill and lifetime, while the caller owns capacity,
│  │  │  │     termination and output; no address, handle, digest or guest offset is
│  │  │  │     published and there is no cross-call lifetime; catalog, nullability and
│  │  │  │     admission remain this crate's policy
│  │  │  ├─ mechanism: decode every argument into a plan without allocating a pointee →
│  │  │  │     preflight the call's capacity total and its worst-case encoded answer bound →
│  │  │  │     materialize → the same `invoke_abi` core through the Engine's loaded-handle
│  │  │  │     cache → post-call snapshot readback; a refusal precedes the loader
│  │  │  └─ evidence: `native::json_adapter_tests` (10 admitted `i32` pointer prototypes, POSIX
│  │  │        `uname` oracle, 5 stable codes, malformed-shape table, alignment, preflight
│  │  │        before load) + `tests/native_door.rs` WAT court + the product run in
│  │  │        `src/script_engine.rs` (built-in `agenterm:native` module from `.qjs`)
│  │  └─ non-goal: no agenterm-cu → agenterm-dyn dependency, second loader/door, or typed CU effect in dyn
│  ├─ [~] embedder `agenterm:acu` object: same typed schema/Executor/errors/receipts as CLI and MCP
│  │  ├─ [x] raw bounded door + non-shadowable qjs module + shared Command/Executor/CuReply adapter
│  │  ├─ [x] versioned `command|argv` envelope; library-owned argv parser; qjs→CU has no child process, while public CLI keeps common Script Worker isolation
│  │  ├─ [x] fixed-sibling dynamic provider; Windows exact release PE 3,738,112 B ≤ 4 MiB
│  │  │  ├─ separate ABI-versioned artifact; Win/macOS packaging and signing fail closed if absent
│  │  │  ├─ public `acu-provider-smoke` executes typed command + argv capabilities in-process
│  │  │  └─ no path search, static implementation, child process or MCU fallback
│  │  ├─ [~] MCP consumes the same adapter: capabilities + canonical observation are public; bounded shell mutation stays internal
│  │  │  ├─ lazy connection session; one dispatched + one queued; queued cancel is zero-provider/zero-effect
│  │  │  ├─ dispatched cancel preserves authoritative CuReply; EOF drains, suppresses output and ends once
│  │  │  └─ fake lifecycle + local provider effect green; target-bound authorization and packaged six-cell court pending
│  │  ├─ [x] check-many resolves the same non-shadowable built-in module as execution
│  │  ├─ [x] bootstrap worker identity covers the complete embedded `skills/acu` module closure and production assets
│  │  │  ├─ tracked, dirty and untracked bytes all invalidate the worker; imported `.qjs` cannot execute stale code
│  │  │  └─ schema-4 policy test plus a controlled untracked-module probe prove rebuild instead of timestamp guessing
│  │  ├─ object lands before `acu.ts` is replaced; it is the replacement's dependency
│  │  ├─ [~] `skills/acu/acu.qjs`: bounded argv/native path + frozen compatibility court are green
│  │  │  ├─ 42/42 positive legacy probes execute in-process; kill preserves its two-call identity bracket
│  │  │  ├─ 95 dynamic witnesses remain frozen with source digests and redacted argv as the
│  │  │  │  explicit gap/typed-rejection queue; no MCU fallback exists
│  │  │  ├─ both dynamic-TODO rows retain their exact gap id; two permanent-scope rows reject locally
│  │  │  ├─ 32 resolved rows: 18 exec · 9 compound · 5 usage · 0 TODO
│  │  │  │  └─ `job resources JOB_ID`: typed status→generation→resource point or bounded watch; top/max project complete membership without partial aggregates
│  │  │  ├─ `agenterm cli acu` runs the compiled-in source through the normal worker/budget/audit path
│  │  │  └─ no Bun, executable discovery, CU child process, repository cwd or MCU fallback
│  │  │     the real production resolver compiles the complete closure; an exact Wasm-import allowlist is the
│  │  │     semantic gate, while retirement source scans remain cheap defense in depth
│  │  ├─ no shell-out fallback or duplicated machine-control mechanism
│  │  ├─ MCU-absent black-box parity precedes switching the default entry
│  │  └─ generic tinyvm remains free of AgenTerm machine-control authority
│  ├─ [~] tool.* and release-task coverage grows by product need
│  └─ [ ] every v0.1.18 release-critical journey has live .qjs evidence
├─ robustness
│  ├─ [x] steps, pages, table, call-depth and activation-slot limits
│  ├─ [x] typed load, host, throw and budget failures; failed stdout retained
│  ├─ [x] child stdout/stderr truncation is explicit through read/wait/command
│  ├─ [x] process.spawn refuses a 33rd retained handle before native spawn/drain allocation
│  ├─ [x] evidence-declaration scans use synchronous commands, so completed probes do not consume retained handles
│  ├─ [x] advisory locks retain stable tombstones and refuse a 33rd lifetime handle before file creation
│  ├─ [x] text and i32-returning host operations apply one parked-result cap to diagnostics
│  ├─ [x] bare declared-host values fail by name; no implicit zero-argument effect
│  ├─ [x] every child entry uses the shared first-instruction contained launcher
│  ├─ [x] invocation-owned process-tree cleanup; no cross-run global backend state
│  ├─ [~] ACU cancellation ownership
│  │  ├─ [x] detached helper rejected: it returns while callback and provider lock remain live
│  │  ├─ [x] Script worker process remains the hard-containment boundary for blocked native calls
│  │  ├─ [x] pure qjs/wasm computation observes the invocation's borrowed cancel token
│  │  ├─ [x] `agenterm cli acu` supplies a 650-second worker envelope for provider-owned
│  │  │      deadlines up to 600 seconds; a leading `--timeout-ms N` explicitly overrides it,
│  │  │      including longer compatibility operations such as process kill or device watch;
│  │  │      the same spelling after the legacy verb remains a verb-owned option
│  │  ├─ [x] Unix hard-timeout cleanup preserves a descendant that crossed the explicit
│  │  │      `setsid` ownership boundary while still terminating same-session descendants
│  │  ├─ [x] pass the same token through the fixed-sibling provider into the observe-only
│  │  │      `process-watch` wait without storing the callback or spawning a helper thread
│  │  └─ [x] named native observation waits use operation-specific phase/effect semantics:
│  │         `pty-wait`, shared `terminal-wait` and `pty-wait-exit` sample the same borrowed
│  │         token between bounded authority rounds while same-round terminal/error replies win;
│  │         public `wait` window/tree/text/ready/expect variants now follow the same ordering
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

The host-reply wire cost decision experiment,
[`plan/design-qjswasm-host-reply-wire-cost-experiment.md`](../plan/design-qjswasm-host-reply-wire-cost-experiment.md).
It asks whether the bytes the host hands a guest as a reply are a large enough
owner of journey steps that a product-side wire change (compact reply text
and/or host-side field selection) is the next action. **The experiment has now
run** (2026-09-13, receipt in
[`research/qjswasm-host-reply-wire-cost/RESULTS.md`](../research/qjswasm-host-reply-wire-cost/RESULTS.md))
returned
`V0 yes → W0-C yes → W1 no → owner = host-side field selection`: the wire route
is real at the measured pin (two usable journeys removed 49.58% and 30.00% of
their own total steps by carrying only the fields they read) and its owner is
field selection, not compact reply text (the indentation-only share was 12.59%
and 30.14%, well under the frozen 75%). Its first product leaf is now delivered
at the CLI producer boundary: `protocol-info --json --select` projects the
producer's `serde_json::Value` before the unchanged serializer, and the real
`native-ipc-smoke` consumer no longer starts `sh` and `grep` to cut seven keys.
At the same pin and budget the journey moves from 21,073,417 to 20,120,279 guest
steps (−4.52%) and from 187,538 to 173,974 host bytes (−7.23%); one full answer
moves from 61,670 bytes to 213 bytes. The qjswasm door, tinyvm pin, IPC wire and
budget are unchanged. This is intentionally recorded as a **first-producer
foundation**, not a completed fold: its production core is still net additive,
and the economic payback requires a second real producer to reuse it while
deleting parallel filtering or serialization code.

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
  NATIVEPOLICY["Native Importer declaration schema<br/>nullability · guest-span checks"]
  DECL["import lowering plan<br/>target · ABI values · storage · result"]
  INVOKE["invoke_prepared<br/>one NativeCall construction seam"]
  JSONREGION["JSON pointer call<br/>one call-scoped host region per ptr<br/>16-byte aligned · zero-filled<br/>snapshot readback · no address published"]
  DYNABI["agenterm-dyn ABI Importer mechanism<br/>loader · symbol · trampoline · invoke_abi"]
  ENCODERS["transport encoders<br/>raw bits · JSON scalar · region answer"]
  CUCALLER["CU typed product projection<br/>fixed-sibling provider"]
  SCRIPTRUNTIME["Script Runtime"]
  NODIRECT["boundary invariant<br/>agenterm-cu does not depend on agenterm-dyn"]
  EXPLICIT["explicit call sites only<br/>bare host value → typed compile refusal"]
  CAPTURE["bounded child capture<br/>per-stream loss flags · JSON-fit"]
  SELECT["host-side JSON field selection<br/>producer Value → bounded projection<br/>protocol-info first consumer · door unchanged"]
  OBSERVE["bounded single-PID observation<br/>live · dead · unknown<br/>no inventory transport"]
  PARENT["bounded direct-parent observation<br/>one PID · no process-table text"]
  TASKWIT["task process witnesses reuse the door<br/>no ps parsing · no kill subprocess"]
  REHEARSE["fresh-clone descendant ownership<br/>inventory · raw-PID cleanup · root identity"]
  OWNEDKILL["owned CU fixture cleanup<br/>child handle · no kill subprocess"]
  HANDLES["per-slot child ledger<br/>32 retained · pre-spawn refusal"]
  LOCKS["per-slot lock ledger<br/>32 lifetime handles · stable tombstones<br/>pre-open refusal"]
  PATHS["shared path helper<br/>`.` / `./` lexical normalization"]
  PRODUCT["AgenTerm operations<br/>Fleet · tools · process · fs · net"]
  ACUOBJ["agenterm:acu embedder object<br/>shared schema · Executor · failures · receipts"]
  ACUSIZE{"dynamic provider court<br/>Windows PE ≤ 4 MiB?"}
  ACUDYN["fixed sibling dynamic provider<br/>ABI checked · bounded · serialized<br/>separate signed artifact"]
  TS["archived acu.ts reference<br/>immutable Git provenance"]
  ABSENT{"zero STAY + MCU absent<br/>black-box parity"}
  COMPAT["embedded acu.qjs<br/>legacy mapping only · no Bun"]
  IDENTITY["bootstrap worker identity schema 4<br/>skills/acu closure · production assets<br/>tracked · dirty · untracked bytes"]
  ACUCLI["ACU consumers<br/>CLI · MCP · qjs"]
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
  NATIVEPOLICY -->|derive once| DECL
  DECL -->|lower target + values| INVOKE
  JSONREGION -. call-scoped storage .-> INVOKE
  INVOKE --> DYNABI -->|raw ABI value| ENCODERS
  ENCODERS -. transport-specific writeback to guest .-> DOOR
  CORE["Core Wasm conformance<br/>malformed + differential fuzz"]
  COURT{"size · cold start · throughput<br/>security · embedder parity"}
  STANDARD["WASI / Component compatibility<br/>in generic tinyvm layer"]

  MANY --> SRC --> COMP --> WASM --> LOAD
  MANY -. bytes · modules · deadline .-> COMP
  UP -. exact git rev .-> COMP & LOAD
  LOAD -->|yes| SLOT --> DOOR --> EXPLICIT --> PRODUCT --> RECEIPT
  DOOR --> NATIVEPOLICY --> DYNABI --> RECEIPT
  NATIVEPOLICY -. JSON caller has no guest span · one host region per ptr .-> JSONREGION --> DYNABI
  CUCALLER -->|typed command / reply| SCRIPTRUNTIME --> DOOR
  CUCALLER -. Cargo boundary + composition court .-> NODIRECT
  NODIRECT -. native extension continues through Script Runtime .-> NATIVEPOLICY
  DOOR --> ACUOBJ --> ACUSIZE
  ACUSIZE -->|3,738,112 B · green| ACUDYN --> ACUCLI --> RECEIPT
  ACUSIZE -->|regression| REJECT
  TS --> ABSENT
  ACUOBJ --> ABSENT
  ABSENT --> COMPAT
  IDENTITY --> COMPAT
  COMPAT -. legacy syntax projected to typed calls .-> ACUCLI
  PRODUCT -. child process .-> HANDLES --> CAPTURE --> RECEIPT
  PRODUCT -. producer-owned JSON value .-> SELECT --> DOOR
  PRODUCT -. arbitrary PID liveness .-> OBSERVE --> DOOR
  PRODUCT -. direct child identity .-> PARENT --> OBSERVE
  PARENT -. task-side reuse .-> TASKWIT --> DOOR
  TASKWIT -. release rehearsal .-> REHEARSE --> DOOR
  TASKWIT -. owned fixture cleanup .-> OWNEDKILL --> DOOR
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
- A call-scoped region is storage the host owns for exactly one synchronous
  call: the host owns its address, alignment, zero fill and lifetime, the caller
  owns its width and termination contract, and no address, handle, digest or
  cross-call identity is ever published. A region answer is a post-call buffer
  snapshot, never a written length, and a readback refusal keeps the native
  status.
- A guest-sized host allocation is budgeted before it is materialized, by the
  worst-case encoded multiple rather than the raw byte count, with the final
  serialized-answer cap retained as defense in depth.
- Nullability is upper-layer schema: a nullable pointer position and a
  non-nullable one are the same ABI position to dyn, and the raw block door's
  `null` is not available to a JSON caller, which owns no address that `null`
  could stand for.

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

The ACU convergence follows the same boundary. `agenterm:acu` is an AgenTerm
embedder object, not a tinyvm builtin: generic tinyvm validates and executes
Wasm, while the AgenTerm embedder supplies the versioned machine-control host
contract. CLI, MCP and qjswasm must enter the same Rust schema and `Executor`;
none may own a second implementation. That cutover is now live: embedded
`acu.qjs` preserves legacy argument mapping against the object, while the
historical `acu.ts` is only a read-only archive reference with immutable Git
provenance. The qjs shell
retires after its callers migrate to typed calls. This does not rewrite CU in JavaScript or move native
machine-control effects into generic qjswasm/tinyvm.
The owning retirement gates and public behavior remain in
[PRD 28](PRD_02_28_agenterm_cu.md); this module owns only the qjswasm host-door
integration.

## Current acceptance

- [x] active qjswasm runtime executes `.qjs` check/run and expression eval.
- [x] Public `script run FILE.wasm` carries an explicit artifact convention:
  `compiled-qjs` preserves the packed JS-V1 ABI and `plain` runs a
  hand-authored module without guessing from its exports. Artifact input is
  read only through the smaller of the invocation budget and the 1 MiB
  transport ceiling, then crosses the ordinary framed `WorkerSupervisor`.
  A native-door signature mismatch or blocking call therefore becomes the
  typed worker-crash or hard-timeout result and the worker is reaped instead
  of sharing the CLI process's fate. The public black-box court is
  `tests/script_native_artifact_supervisor.rs`; this is qjswasm/Script Runtime
  evidence and does not retire or replace `agenterm-dyn`.
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
  creating stdout/stderr drain threads. Real gates that need PID-tree sampling
  stay on this retained-handle path. The unbounded-by-catalog declaration scan
  uses synchronous `process.command` instead, so adding a 33rd evidence suite
  cannot consume a long-lived handle or weaken the ceiling.
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
- [x] `check-many` admits up to 1024 manifest entries so the growing owned qjs
  corpus remains one repository-wide gate, then charges entry files and
  recursively resolved imports to one
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
- Exact, fixed and fixed-pointer native calls now keep their declaration parser,
  prototype catalog, nullability and guest-span checks in this crate, while all
  five raw and JSON execution arms delegate through `agenterm-dyn::invoke_abi`.
  The local catalog no longer imports dyn's legacy prototype/value enums, its
  exact-family validator, or a dyn-side exposure-cardinality helper: this
  crate's own exact scalar set and arity derive the catalog cardinality, while
  canonical argument conversion produces raw `AbiValue` positions directly. The
  two nullable pointer positions remain
  distinct here while both lower to dyn's single machine-level `Pointer` type.
  Unix `ioctl` retains its separate dyn mechanism entry. This crate maps dyn's
  mechanism signature/library/symbol failures back into the existing
  `NativeDoorError` codes using the original spec; it contains no second loader
  or stub table.
- One `Engine` owns one bounded table of loaded libraries (`src/native_cache.rs`):
  at most 32 distinct declared strings, keyed **verbatim** (the empty string means
  this process, as it does for `NativeCall::library`), never evicted, and no
  symbols cached. A hit runs the same five arms through
  `agenterm_dyn::invoke_abi_with_handle`; a miss opens the library with no
  `RefCell` borrow alive and adopts the handle; a full table takes the one-shot
  `invoke_abi`, which is exactly this crate's behaviour before the table existed.
  A load that **fails** is attempted once per call and is never retried:
  `resolve` returns dyn's own `AbiError`, the existing `map_abi_error` /
  `map_abi_error_for_spec` mapping reports it, and the slot stays unoccupied,
  because opening a library runs its initialisers and a second attempt would run
  them twice for one call. The reuse entry was added to dyn for this caller
  (`prd/PRD_02_34_agenterm_dyn.md`: the handle belongs to the caller, `Drop`
  closes it, dyn keeps no global cache). The table is therefore an optimization
  that cannot refuse: it adds no `NativeDoorError` code, no permission meaning, no
  process-global state, no symbol cache, and no cross-guest or cross-thread
  sharing — it is a field on the engine, so a second engine starts empty and the
  whole path stays single-threaded. Its owning court is the unit module in
  `src/native_cache.rs` (one string hands back one allocation; the 33rd distinct
  string is declined without evicting an adopted one; a failed load is reported
  once by the mechanism's own error and occupies no slot; keys are the declared
  bytes, so a near-miss is a different library), and
  `tests/native_door.rs` keeps door behavior unchanged, including a repeated
  declaration that must answer the same value on every call of one slot. **The
  door's own reuse path is now owned evidence**: `native::json_adapter_tests`
  carries a per-table `#[cfg(test)]` count on `invoke_with`'s adopted-handle arm
  and calls the real `|getpid|i32()` three times on one table — the first call
  adopts (table length 0 → 1), every call takes that arm, and a second engine's
  table and count are its own. That count is what falsifies "resolved a handle,
  then ran the one-shot entry anyway", which no behavioral assertion can see; the
  matching dyn-side proof is a `cfg(test)` loader-entry delta (N one-shot calls →
  +N entries; one handle open plus N handle calls → +1). **An OS-level load count
  is still not claimed here**: an already-loaded system library does not move any
  cheap oracle, so proving fewer `dlopen` calls needs its own fixture library in
  its own leaf.
- The exposure catalog is pinned inside dyn's mechanism matrix by an owning gate
  (`native::mechanism_compatibility`): the 69 exposed declarations (49 exact + 6
  fixed + 14 pointer, which name 66 distinct ABI shapes because `ptr` and `ptr?`
  are one position) are enumerated from the very tables dispatch uses and each
  one is asked of `agenterm_dyn::validate_abi_signature`, which answers with the
  shape alone and needs no argument values. The inclusion is one-way —
  `exposure ⊆ mechanism`. The gate **derives** the mechanism-only account instead
  of listing it: it asks dyn the same shape-only question over
  `agenterm_dyn::AbiType`'s own vocabulary up to this door's arity bound, and
  requires the difference to be exactly the 9 distinct shapes dyn executes that
  this catalog does not expose today — 4 pointer-result (`ptr()`, `ptr(u32)`,
  `ptr(u64)`, `ptr(ptr)`) and 5 direct-scalar (`isize(u32)`, `i32(i32,i32,ptr)`,
  `i32(i32,i32,u64,ptr,i32)`, `i32(ptr,u32,ptr,ptr,ptr,usize)`,
  `usize(i32,ptr,usize)`) — failing by name on a shape either side gained or
  lost, so a silent omission cannot stay green. It also records that the two
  `ioctl` requests stay on their own mechanism entry (the u64 request has no ABI
  trampoline at all).
- A proposed fold of the then-current 53 tool and 11 host raw signature rows into their richer
  declaration constructors was implemented and then rejected by the economic
  gate. Deriving the raw view inside `check_declarations` kept all 74 owning
  library tests green and removed the parallel source rows, but replaced static
  data with `HostFn`, `String` and `Vec` construction on every slot load. That is
  source compression paid for with unmeasured cold-start allocation, not a net
  runtime fold, so no product code from the experiment remains. Reopen this leaf
  only when one compile-time descriptor can emit both views without per-load
  allocation, or when a precommitted slot-load measurement proves that runtime
  derivation wins the complete byte/time/allocation account.
- `.qjs` callers use the built-in `agenterm:native` module as a typed language
  adapter over that same opt-in door. It accepts a signature plus JSON values,
  preserves wide integers as decimal strings, and shares the raw door's
  budget, cancellation, failure codes, and dyn execution path. The public
  `native-acu-composition-smoke` proves the two extension bridges coexist in
  one supervised guest without an `agenterm-cu` → `agenterm-dyn` dependency,
  and it proves the ACU bridge is not merely wired: that guest takes one
  authorized typed `capabilities` reply through Command/Executor, takes the
  typed `refused` reply for the same verb when the invocation carries no
  granted selector, and still resolves dyn-backed raw ABI calls before and
  after both ACU requests. `--help` remains in the court only as the
  target-free CLI short-circuit that answers before verb dispatch; it is never
  quoted as evidence that the Executor ran. The Candidate runtime-control step
  now runs that court on every packaged six-cell runtime cell after the ACU
  provider courts and publishes `cu.retirement-cell.native-acu-composition` in
  the cell receipt. That path is wired but not yet remotely executed, so this
  leaf claims no six-cell verdict.
- The same `agenterm:native` module serves the pointer prototypes through one
  **call-scoped host region** per pointer position. The host allocates that
  storage, zero-fills it, keeps 16 bytes of natural alignment in the allocation
  itself, copies the record's `bytes` into its front, and drops it when the
  synchronous call returns; the caller states `capacity`, `termination` and
  `output` and remains responsible for a pointee large enough for the selected C
  symbol. The answer carries the scalar result plus one entry per pointer
  position and states no address, handle, digest or written length: it is a
  post-call buffer snapshot, so bytes the callee left untouched remain the host's
  zero fill or the caller's own input. Admission is derived from the one
  prototype table: the 10 `i32`-returning pointer prototypes are served, while a
  pointer result (no guest span to rebase onto), the `i64`/`void` pointer shapes
  and the Unix `ioctl` requests keep the refusal they had. `ptr` and `ptr?` both
  require a region here, because a JSON caller owns no guest address that `null`
  could stand for; the raw block door keeps admitting `null` at a `ptr?`
  position, and both nullable prototypes stay in this crate's catalog while dyn
  receives the single machine-level pointer.
- Five stable codes are the whole region refusal surface:
  `native_region_required`, `native_region_shape_invalid`,
  `native_region_too_large`, `native_region_unterminated` and
  `native_region_not_utf8`. The `termination`/`output` pair is one choice of
  three admitted combinations rather than two independent fields, so the fourth
  (`raw` + `text`) is unrepresentable and the readback cannot run in a state the
  decoder never validated. Readback failures retain the native status inside the
  typed error, because refusing an encoding contract must not erase the C result,
  and the readback runs whether the callee reported success or failure.
- Those five codes are one subset of the native door's 40-word machine-readable
  error algebra. The owning `native_door_schema` court constructs every variant,
  pins every spelling, requires every code to be distinct, and uses a second
  exhaustive match with no wildcard so adding a 41st word is a compile-time
  compatibility event rather than an unreviewed diagnostic change.
- Region storage is budgeted before it exists. The call's capacity total and its
  worst-case encoded JSON answer bound (`\u00XX` escaping as six bytes per byte
  for text, three digits plus a separator for byte arrays, plus fixed envelope
  allowances) are both checked against the slot's `max_bridge_result_bytes` in
  one preflight that runs after argument decoding and before any pointee is
  allocated, so an over-budget call loads no library and leaves no earlier buffer
  materialized. The host's check of the actually serialized answer remains as
  defense in depth and replaces an oversized answer whole instead of truncating
  it. A call with no pointer position answers the same bytes it answered before.
- No `agenterm-dyn`, `tinyvm` or `tinyvm-qjs` change accompanies the region work:
  the pin is unchanged and every admitted call still enters
  `agenterm_dyn::invoke_abi`, or its reuse entry with the handle this engine
  already loaded. The region path is the same mechanism with a different
  upper-layer storage decision, so the owning evidence is the crate's
  `native::json_adapter_tests` (POSIX `uname` oracle, malformed-shape table,
  alignment, preflight-before-load, admission count) plus `tests/native_door.rs`
  and the product `.qjs` run in `src/script_engine.rs`, not a second ABI court.
- public Script CLI black boxes own `.qjs` route, diagnostics, receipts and
  product-host calls.
- v0.1.18 G4 owns the release-critical task/journey migration. Quick-only green
  cannot substitute for the complete Candidate gate.
- Performance changes require before/after step, wall-time, memory and output
  measurements on the same guest and pin; a larger limit is not an optimization.

Current execution plan: [`plan/plan-v0.1.18.md`](../plan/plan-v0.1.18.md).
