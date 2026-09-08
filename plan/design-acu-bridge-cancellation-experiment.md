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
| detached | helper returns on flag | fast return but active callback/held lock |
| cooperative | callback polls the flag | fast return and no residual owner |

No OS API, provider binary, GUI, or machine-control operation is involved.

## 3. Predetermined criteria

| id | property | pass |
|---|---|---|
| C1 | boolean / safety | active callbacks are zero at return |
| C2 | boolean / safety | serialization lock is immediately available |
| C3 | wall clock | cancellation returns below 100 ms |
| C4 | behavior | synchronous baseline takes at least 200 ms |

C1 then C2 dominate C3. Fast but still-running work loses.

## 4. Decision tree, kill criterion, and timebox

1. If C1 or C2 fails, reject that variant regardless of C3.
2. If C1 and C2 pass but C3 fails, retain process containment and design a
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

## 7. Not answered

- Which Executor and native waits should gain cooperative cancellation first.
- Whether a future provider ABI carries a cancellation token or uses an
  interruptible IPC transport.
- The grace interval of the existing Script worker supervisor.

## 8. Result

The 2026-09-08 run followed the decision tree directly: detached execution
failed C1 and C2 despite passing C3, so it is rejected. Cooperative execution
passed C1-C3; the synchronous baseline passed C4 and confirmed the original
non-preemptible behavior. Therefore qjswasm must not wrap `AcuBridgeFn` in a
detached helper thread. The existing Script worker remains the hard containment
boundary until Executor/native waits accept and honor one cooperative cancel
token. Exact measurements and the replay command live in
`research/acu-bridge-cancellation/RESULTS.md`.
