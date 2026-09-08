# ACU bridge cancellation experiment

This experiment changes no ACU capability state. It decides how a synchronous
`agenterm:acu` call may become cancellable without leaving unbounded native
work behind.

| field | value |
|---|---|
| date | 2026-09-08 |
| purpose | choose between a detached helper, cooperative cancellation, and process containment |
| implementation | `research/acu-bridge-cancellation/` |
| pre-reading | `AGENTS.md`, `docs/agenterm-rust-cheatsheet.md`, `.agents/skills/decisive-experiment/SKILL.md` |
| source discipline | standard-library probe; no product mechanism or authorization changes |

## 0. Settled facts and Markdown-tree DAG

```text
outcome: cancel qjswasm ACU waits without orphaned authority work
├─ current synchronous bridge
│  ├─ behavior: caller blocks until callback returns
│  ├─ evidence: cancel raised during callback is observed only afterward
│  └─ safe failure: outer Script worker deadline kills the owned process tree
├─ A detached helper
│  ├─ behavior: interpreter returns when cancel is observed
│  ├─ risk: callback and provider lock remain alive after return
│  └─ kill: any post-cancel native work or held serialization lock
├─ B cooperative callback
│  ├─ behavior: callback observes the same cancel token and releases ownership
│  ├─ evidence: bounded return plus zero active callback and unlocked provider
│  └─ dependency: Executor/native waits need a cancellation-aware contract
└─ C process containment
   ├─ behavior: existing supervisor sends cancel, then kills after bounded grace
   ├─ evidence: worker/process tree gone after the deadline
   └─ limitation: no graceful receipt from an uncooperative callback
```

Settled and out of scope:

1. `Command -> Executor -> CuReply` remains the only ACU authority.
2. This experiment does not invent a per-verb timeout or retry an uncertain
   mutation.
3. A Script worker is already a process containment boundary. The question is
   whether qjswasm may safely return from the callback before that worker dies.

## 1. Hard constraints

| id | constraint | observable result |
|---|---|---|
| H1 | cancellation cannot report completion while authority work continues | active callback count is zero when cancellation returns |
| H2 | cancellation cannot strand the provider serialization lock | `try_lock` succeeds immediately after return |
| H3 | no unbounded helper growth | one cancelled call leaves zero helpers before a second call |
| H4 | no invented receipt | cancellation returns `Cancelled`, never a synthetic `CuReply` |
| H5 | no change to command, grant, effect, or retry policy | production ACU files remain untouched by the probe |

Disease detector: an urge to detach a thread because it makes the interpreter
return quickly is the bug this experiment is designed to detect. A short return
time is invalid evidence when native work or a lock remains alive.

## 2. Minimal experiment

The probe fixes one 250 ms callback and raises cancellation after 25 ms. It
measures three variants using the same callback duration and lock:

| variant | changed axis | expected discriminant |
|---|---|---|
| synchronous | no helper | cancellation waits for callback completion |
| detached | helper returns on flag | fast return but active callback/held lock; measure the next lock acquisition |
| cooperative | callback polls the flag | fast return and no residual owner; structural control only |

No OS API, provider binary, GUI, or machine-control operation is involved.

## 3. Predetermined criteria

| id | property | pass |
|---|---|---|
| C1 | boolean / safety | active callbacks are zero at return |
| C2 | boolean / safety | serialization lock is immediately available |
| C3 | wall clock | cancellation returns below 100 ms |
| C4 | behavior | synchronous baseline takes at least 200 ms |
| C5 | slope / safety | a second provider call waits below 50 ms after cancellation |

C1 then C2 dominate C3; C5 measures the user-visible cost rather than inferring
it from the lock alone. Fast cancellation with delayed subsequent work loses.

## 4. Decision tree, kill criterion, and timebox

1. If C1, C2, or C5 fails, reject that variant regardless of C3.
2. If C1, C2 and C5 pass but C3 fails, retain process containment and design a
   cooperative cancellation contract before editing the bridge ABI.
3. If all pass, the variant may proceed to a product integration court; this
   experiment alone does not ship it.

Kill criterion: any post-return callback or held lock rejects detached helper
execution. Timebox: stop as soon as all three variants produce C1-C4 once; do
not optimize the probe.

## 5. Files

```text
research/acu-bridge-cancellation/
├─ probe.rs
└─ RESULTS.md
```

## 6. Excluded choices

| choice | reason |
|---|---|
| asynchronous `CuReply` invented in qjswasm | duplicates product protocol and authority |
| retry after worker kill | mutation outcome may be unknown |
| one timeout for every verb | mixes runtime robustness with per-operation semantics |
| `try_lock` and return provider-busy | authority work still runs after cancellation and a valid next call becomes a transient refusal |
| join for a fixed grace | an uncooperative FFI callback has no finite join bound; this moves the same hang |

## 7. Not answered

- Which Executor and native waits should gain cooperative cancellation first.
- Whether a future provider ABI carries a cancellation token or uses an
  interruptible IPC transport.
- The grace interval of the existing Script worker supervisor.

## 8. Result

The 2026-09-08 policy gates reject detached execution by definition when work
continues after cancellation; the probe demonstrates that violation and
measures its consequence rather than discovering the policy. Detached execution
failed C1, C2 and C5 despite passing C3. Cooperative execution passed C1-C3 and
C5 in the structural control; the synchronous baseline passed C4. Therefore
qjswasm must not wrap `AcuBridgeFn` in a detached helper thread. The existing
Script worker remains the hard containment
boundary until Executor/native waits accept and honor one cooperative cancel
token. Exact measurements and the replay command live in
`research/acu-bridge-cancellation/RESULTS.md`.

## 9. Product-integration experiment: one observe-only vertical slice

The next experiment narrows the unanswered cooperative branch to
`process-watch`. It does not claim that every Executor wait or any mutation is
cancellable.

```text
outcome: one ACU observe wait retires before the worker's hard-kill grace
├─ A selected: call-scoped cancellation probe
│  ├─ qjswasm lends the invocation AtomicBool to the synchronous bridge
│  ├─ provider ABI v2 carries callback + opaque context, never AtomicBool layout
│  └─ Executor polls the borrowed probe only inside process-watch
├─ B rejected unless A proves invasive: scoped mirror thread
│  ├─ would copy callback state into a provider-owned Arc<AtomicBool>
│  └─ adds scheduling, unwind and join obligations with no product benefit
└─ unchanged fallback: worker cancel → 150 ms grace → owned-process hard kill
```

Fixed compatibility boundary:

1. Provider ABI v1 and its existing call symbol remain byte-for-byte intact.
2. ABI v2 is one optional new call symbol with a caller-sized versioned cancel
   descriptor. A new provider continues to serve old hosts through v1.
3. An invocation carrying cancellation may not silently use an old v1-only
   provider. It fails before execution with
   `acu_provider_cooperative_cancel_unavailable`.
4. The callback and context are borrowed only for the synchronous v2 call and
   are never stored, called from another thread, or touched after return.

Predetermined product gates:

| id | pass condition |
|---|---|
| P1 | pre-cancelled `process-watch` exits before its first process snapshot with typed `cancelled` / `effect:not_performed` |
| P2 | a running watch with a 60-second requested interval observes cancellation and returns within 50 ms |
| P3 | public Script-worker execution returns inside the existing 150 ms grace and the next provider call starts without lock delay |
| P4 | cancellation emits no receipt, durable request state or machine-control effect |
| P5 | v1 callers behave exactly as before; v2 rejects null, short or unknown descriptors before command execution |
| P6 | compiled `.wasm` and source `.qjs` routes retain the same invocation token and failure class |

Kill criteria: reject the implementation if any callback/context remains live
after the provider call, any provider lock remains held when cancellation is
reported, a controlled call falls back to v1, the 150 ms hard-kill grace wins,
or cancellation hides a reply after an effect was dispatched. The final rule
is why this slice is observe-only: mutation needs phase-aware
`not_performed`/authoritative-reply/`outcome_unknown` semantics, not a generic
post-call cancellation check.

Timebox: stop after the `process-watch` slice and its provider/qjswasm/worker
tests. Do not spread polling through other waits until this slice passes P1-P6.
