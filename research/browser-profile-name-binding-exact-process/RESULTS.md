# RESULTS — browser profile-name binding exact-process experiment

**Status: in progress. This file deliberately contains no conclusion.**

The platform-neutral self-test, the admission-free host capability preflight
and the disposable broker self-test have landed. No live ordinal has been
reserved, no browser has been launched, and no A0/A1/B design has been selected
or rejected. The owning PRD leaf `acu.dynamic.075.profile-name-binding` and the
capability ledger are unchanged.

The broker self-test proves only that the admission machinery enforces its own
discipline. It admits no attempt and proves no criterion.

## What is proven today (machine-checked)

Command (from the repository root):

```sh
research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --self-test
```

The runner must emit `IDENTITY_SOURCE_PROVEN` followed by `SELF_TEST_PASS`,
66 / 66 cases and `formal_attempts_consumed: false`. Neither record alone is
the first-slice proof.

| Criterion | Proven | How |
|---|---|---|
| V2 · identity-source contract | ✅ first-slice model/static proof | The runner rejects forbidden process-table constructs in court source; the model accepts only `process.pid` / `process.observe` / `process.parent` call records. No live call has run yet |
| V6 · evidence-persistence contract | ✅ first-slice model proof | Every modeled validity failure publishes a bounded read-back-equal stage row first; forbidden keys, unknown facts and control characters are refused with typed codes. Live throw sites do not exist yet |
| V7 · selector-independence contract | ✅ first-slice model/static proof | Cleanup input whitelist rejects selector-derived fields. The future live ownership/cleanup region must pass the same source scan before an ordinal is reserved |
| V1-V7 / D1-D3 tree | ✅ | Every single-criterion failure, both cleanup-failure combinations, the V6-before-V7 precedence, validity-precedes-design, and every design exit (`A1_SELECTED`, `B_SELECTED`, `INCONCLUSIVE_MECHANISM`) has an asserting case |
| Chain and cleanup | ✅ | Cycle, repeat, ceiling, unbracketed PID, null identity, identity change, foreign terminus, skipped termination step, surviving process, inventory drift, hygiene-cannot-heal, incomplete inventory |

## Admission-free host capability preflight

Command (from the repository root):

```sh
research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --capability-preflight
```

The runner first emits `IDENTITY_SOURCE_PROVEN`, then the registered
tool-profile task must emit an all-true
`agenterm.profile-binding-exact-process-capability-preflight/v1` record,
`EVIDENCE research.profile-binding-exact-process.capability-preflight` and
`PASS`. This host run proves successful `process.pid`, two-observation identity
brackets, the direct parent relation, durable create/read-back, exclusive lock
contention and an independently frozen SHA-256 reference. It launches only an
owned sleeping shell child, removes its scratch directory, launches no browser
and reserves no rehearsal or decision ordinal. It does not measure V3-V5 or
D1-D3.

## First-slice implementation deviations

The precommitment criteria were not changed to favor a design; no live design
measurement exists. Implementation review nevertheless found and corrected
five model/court defects before any ordinal could be reserved:

1. The first chain model treated a terminal `parent_id: 0` as a broken chain.
2. The first D3 precondition made `B_SELECTED` unreachable when A1 had no edge.
3. The first court discarded the runner's static V2 record, then printed
   `V2:true` even though the qjs embedder could not inspect its source. The
   runner now emits the required static record and the court labels only its
   `V2_runtime` model result.
4. The first V6 model accepted any earlier journal row as proof that a later
   failure stage was persisted. It now requires an exact terminal row for
   V1-V5/V7. V6 persistence failure is the sole non-evidence exception: it has
   the fixed `INVALID_EVIDENCE` result, no design fact and no fabricated
   terminal receipt.
5. The first validity projection dropped that `persistence_failed` fact between
   `validate` and `decide`. The stricter V6 case turned the suite red at 65/66;
   preserving the fact restored 66/66 without weakening the oracle.

## Red gate

A single broken case must fail the self-test. Verified by mutation: changing
`A0_REJECTED` to an unasserted code yields
`SELF_TEST_FAILED` with `missing: ["case-count-mismatch", "decision-tree"]`,
and restoring it yields `SELF_TEST_PASS`.

## What is NOT proven (the live boundary)

The live macOS court is **not implemented** and fails closed with
`LIVE_COURT_NOT_IMPLEMENTED`. Specifically, nothing below has been executed:

- No browser was launched and no synthetic HOME or Profile fixture was created.
- The model self-test made no `process.observe` / `process.parent` /
  `process.pid` call. The admission-free preflight called all three against its
  current worker and one owned shell child; it did not observe a browser tree.
- V3 (real ownership chain), V4 (real termination), V5 (real inventory
  restoration), D1 (armed alias trap), D2 (real A1 edge) and D3 (real durable
  store) are **unmeasured** on this host.
- No rehearsal or decision ordinal has been reserved in any external ledger.

Per spec §4 kill criterion 4, a live ordinal must not be reserved until the
first self-test proves every throw site is preceded by a persisted stage.

## Provenance and non-import

`binding-model.qjs` reuses the *shape* of the v2 pure model by copying with
provenance. This directory imports no mutable result state and no attempt
ledger from either exhausted experiment, and neither exhausted court reached a
verdict, so no value from them is authoritative here.

## Reproduce

```sh
research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --self-test
```

Independent reference for the model's decision terminals:

| Terminal | Meaning |
|---|---|
| `INVALID_INPUT` | V1 failed before fixture creation |
| `INCONCLUSIVE_IDENTITY_SOURCE` | V2 failed: a process-table command or a non-exact-key op was used |
| `INCONCLUSIVE_OWNERSHIP` | V3 failed: the chain is cyclic, too long, unanchored or truncated |
| `INCONCLUSIVE_CLEANUP` | V4 or V5 failed: termination or inventory restoration is inexact |
| `INVALID_EVIDENCE` | V6 failed: a failure had no persisted stage row; persistence failure itself is the sole non-evidence terminal, uses the §1.7 field contract and exits nonzero |
| `INVALID_EXPERIMENT` | V7 failed: ownership or cleanup consumed selector-derived data |
| `A1_SELECTED` | D2: the authenticated two-sided edge passed every arm |
| `B_SELECTED` | D3: A1 was ineligible or rejected, and B passed every arm |
| `INCONCLUSIVE_MECHANISM` | Neither A1 nor B could be selected; the typed TODO remains |

## Broker self-test (§1.7–1.9, in progress)

The external ledger / admission / stage persistence broker is exercised by a
disposable self-test that reserves no ordinal:

```sh
sh research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --broker-self-test
```

Admission dry-run (verified through the registered tool-profile task): all 24
checks true, ending in `PASS: exact-process admission dry-run`
with `formal_root_touched: false` and `browser_launched: false`.

Red gate: `ADMISSION_RED_GATE_PASS` (the entry is refused by the plain run path,
a facts payload carrying `argv` is refused, a non-whitelisted fact key is
refused, an in-place byte mutation changes the tree snapshot, and the formal
root is unchanged).

Last run on this host: `BROKER_SELF_TEST_PASS requests=168 failures=0`, and
`BROKER_SELF_TEST_HARNESS_PASS`.

Source sizes (`wc -l`): `broker-spine.sh` 1216, `broker-self-test.sh` 1548,
`broker-self-test-harness.sh` 111.

The suite asserts, against a real disposable root rather than a mock:

| Property | How it is checked |
|---|---|
| External ledger is authoritative | a reservation is published, then read back **through the broker** |
| Ordinal non-reuse | R1 and D1 are each refused once reserved, finished or abandoned |
| State lock is exclusive | a second process holding `state.lock` makes the broker refuse |
| `reserved`→`finished` | only `finish KIND ORDINAL RUN_ID RECEIPT_SHA` closes an attempt, against the receipt on disk |
| `reserved`→`abandoned` | `abandon` closes the attempt with no receipt |
| Receipt digest binding | at `finish` the digest is **re-derived from the journal on disk**, not trusted from the receipt's claim |
| Terminal row required | `finish` refuses a journal whose last row is not the `terminal` stage |
| Receipt close-time verification | the receipt is re-read and re-validated at `finish`, so a receipt tampered after staging is refused |
| Terminal code agreement | `stage.code`, `facts.terminal_code`, receipt `code` and ledger `terminal_code` must all agree and name a template terminal |
| Terminal↔criteria consistency | a terminal's criteria must match **one of** its legal decision-tree shapes; `INCONCLUSIVE_CLEANUP` has three (V4-only fail, V5-after-V4 fail, and V4+V5 fail), and `INVALID_EVIDENCE`/`INVALID_EXPERIMENT` require every earlier gate `pass` |
| Every legal shape is exercised | each mapped terminal gets a positive case and at least one near-miss negative case, so a mapping that is too narrow and one that is too wide are both caught |
| Rehearsal records validity | a rehearsal **may** publish V1-V7 pass/fail; it may never publish a design fact |
| No unverified design selection | `A1_SELECTED` / `B_SELECTED` are refused by name when closing a rehearsal, because this broker holds no D1-D3 evidence |
| `inspect` is side-effect free | on a fresh root it reports `state_absent` and creates nothing; on an existing root it creates no lock file and no `stage-journal`, and a before/after filesystem snapshot is identical |
| Frozen-input recheck is real | the digest is recomputed from a supplied manifest under the lock and compared, and the pinned source revision is checked; a wrong digest and a wrong revision are each refused |
| Recheck honesty | an omitted manifest is reported as `not-supplied`, and is never reported as `verified` |
| Frozen files are re-read | changing a declared file's **bytes** while leaving the manifest and claimed digest untouched is refused (`frozen_input_content_changed`), so a manifest-only checker could not pass this |
| Forged per-file digest | a fabricated `files[].sha256` is caught by the recomputation |
| Manifest path confinement | absolute paths, `..` traversal, a symlink escaping the root, and a missing file are each refused |
| Independent revision read | when the manifest declares a repository, the revision is read from it; a disagreement is refused |
| Journal bounds | the template's three exclusive bounds are enforced on the **candidate** journal before publication; the row-count bound is filled to the limit and the next row refused with the journal **byte-identical** afterwards |
| Refusal leaves no trace | after a refused over-limit row the journal digest, line count, byte count and ledger digest are all unchanged, and the reservation stays `reserved` |
| Authoritative read | `inspect` re-derives every `finished` transition from disk: the receipt must exist and hash to the ledger's digest, validate against the reservation, and reproduce the journal's digest and sequence; the journal's last row must be the terminal stage; and the terminal code must survive the kind and criteria gates |
| Forgery is refused | a hand-forged `finished` row (design-selection code, fabricated digest, no receipt) is refused and never presented as finished |
| Tampering is refused | a modified real receipt, a modified journal, an appended journal row, a changed terminal code, a ledger/receipt code disagreement, and journal facts that disagree with the receipt are each refused |
| Mutating paths share the check | a forged `finished` row also blocks a later `reserve`, so it cannot be used as a foothold |
| The summary is fail-closed | a run with any failed assertion prints `BROKER_SELF_TEST_FAILED`, exits nonzero, and prints **no** pass token; proven by an injected-failure harness, not assumed |
| No unverified design selection | `A1_SELECTED` / `B_SELECTED` are refused by name for **every** kind, so a `decision` attempt cannot write a terminal receipt or reach `finished` |
| No receipt-less close | a `finished` row without a receipt digest is refused, and no public operation can write one |
| Durable stage + read-back | journal and receipt both exist, and the journal row carries its chain sequence |
| A non-terminal stage may not close | a `preflight` row carrying a terminal code is refused |
| Persistence failure | a forced write failure yields the fixed seven-key record, no design fact, nonzero exit, **attempt still `reserved`**, no committed receipt, no `finished` row |
| Prior row cannot substitute for the failing stage | an attempt may hold a genuine, read-back-equal `preflight` row and still be refused at `finish`, because the closing row must be the `terminal` stage; the attempt stays `reserved`, so the earlier row buys nothing |
| A tampered ledger is refused | a `finished` row with an unknown code, or with a null receipt digest, is rejected by the broker's own validation rather than served as authoritative |
| Cleanup | no formal state directory is created; the disposable root is removed unless `--keep` |

Fifty red-gate mutations were applied in total; each was reverted to
restore full green. Every mutation is recorded, **including the four that are
not independently caught** — those prove real defence in depth, and saying so is
more honest than presenting a 29/29 score.

| # | Mutation | Red |
|---|---|---|
| R1 | lock exclusivity removed | 1 |
| R2 | ordinal-reuse check dropped | 2 |
| R3 | receipt bound to a constant digest | 5 |
| R4 | ledger terminal-code check dropped | 1 |
| R5 | persistence stdout gains an extra key | 1 |
| R6 | persistence stdout loses a required key | 1 |
| N1 | an unjustified receipt-less close reintroduced | 1 |
| N2 | `finish` without a `terminal` stage allowed | 1 |
| N3 | receipt digest not verified at `finish` | 2 |
| N4 | terminal↔criteria mapping unenforced | 1 |
| N5 | `finish` journal-digest binding dropped | 1 |
| N6 | `finished` row accepts a null receipt | 1 |
| N7 | rehearsal design fact allowed | 1 |
| N8 | design-selection terminal allowed in a rehearsal | 2 |
| N9 | `INCONCLUSIVE_CLEANUP` collapsed to one legal shape | 2 |
| N10 | `INVALID_EVIDENCE` mapping ignores V4/V5 | 2 |
| N11 | `INVALID_EXPERIMENT` mapping ignores V4-V6 | 1 |
| N12 | `inspect` regains side effects (`O_CREAT` + `ensure_tree`) | 2 |
| N13 | frozen-input digest not recomputed | 1 |
| N14 | a supplied recheck mislabeled non-verified | 1 |
| N15 | an unrelated prior row may substitute for the failing terminal | 1 |
| N16 | journal row-count bound removed | 1 |
| N17 | journal single-row byte bound removed | **0** (unreachable through a legal request — defence in depth) |
| N18 | journal total byte bound removed | **0** (the row-count bound fires first) |
| N19 | manifest path-escape confinement removed | 1 |
| N20 | journal bounds not checked before the write | 1 |
| N21 | frozen symlink not refused | **0** (the realpath containment check also catches it) |
| N22 | missing frozen file silently skipped | **0** (the content-hash comparison also refuses it) |
| N23 | declared repository revision not read | 1 |
| N24 | candidate pre-write journal check downgraded to a no-op | 3 |
| N25 | self-test summary restored to fail-open (counts but still prints `PASS`) | 3 (caught by the harness) |
| A1 | authoritative check never called by `inspect` | 14 |
| A2 | receipt existence not required | 5 |
| A3 | receipt digest not compared to the ledger | 1 |
| A5 | journal digest not recomputed from disk | 2 |
| A7 | ledger `terminal_code` not compared to the receipt | 1 |
| A8 | kind gate not applied on load | 1 |
| A9 | criteria gate not applied on load | 1 |
| A10 | journal facts not compared to the receipt | 1 |
| A12+A13 | authoritative check removed from BOTH mutating load sites | 2 |
| A4 | receipt not validated against the reservation | **0** (behind the digest and identity checks) |
| A6 | journal's last row not required to be terminal | **0** (the digest cross-check fires first; re-signing would need the broker's private digest) |
| A11 | finished row may bind no reservation | **0** (the structural ledger check refuses first) |
| A12 *or* A13 alone | one mutating load site unchecked | **0** (the two sites are redundant; removing both goes red) |

Two of these gates were **green on the first attempt and had to be fixed**:
N5 (nothing re-derived the journal digest at `finish`, so a receipt tampered
after staging would have closed the attempt) and N7 (the design-fact guard was
unreachable because the whitelist check fired first). Both were repaired — the
N5 scene now exists, and the design-fact check now runs first and fails by its
own name — and only then did the gates bite.

RED WARNING: none of this admits a live attempt. It proves the broker's own
discipline, not V3-V5 validity, not D1-D3 design facts, and not any verdict.
The broker executes the structural contract; the court owns the semantics. The
terminal↔criteria mapping is enforced here because it is a pure function of the
template's terminal vocabulary and the decision tree in the specification, and
enforcing it is what stops a `PREFLIGHT_OK` row from closing an attempt.

### Scope of the persistence claim

The broker implements **atomic publication with an enforced read-back**: a
uniquely named temporary is written, `fsync`'d, `rename`d over the destination
and then read back and compared byte for byte. That is **process-crash
evidence** — after a crash the destination is the old or the new complete
bytes, never a torn write.

It is deliberately **not** a power-loss durability claim: the parent directory
is not fsynced, so a power loss may lose the rename itself. Unlike v2, this
broker does not assert power-loss persistence; the live runner owns that
stronger guarantee. Frozen-input recheck *is* implemented here (recomputed
under the lock from a supplied manifest); when no manifest is supplied the
result is reported as `not-supplied` rather than being described as verified.

## Live court self-test and red gate

`--live-self-test` proves the door adapter, the ownership chain and the cleanup
classifier against injected fixtures. All 56 checks pass; no browser is launched,
no owned process is observed for ownership and no ordinal is reserved.

| Group | Checks |
|-------|--------|
| Door adapter | `live`/`dead`/`unknown` observe variants parsed; parent variants parsed; `reason` preserved; wrong-shape, empty-reason and unknown-state records refused; non-numeric parent id refused; parent id 0 accepted |
| Chain | clean chain proves ownership; single-edge chain; exactly-ceiling chain allowed |
| Chain refusal | cycle; PID 0 before terminus; self-parent; over-ceiling; `unknown` is an identity failure; `dead` refused; identity drift; wrong terminus identity; non-terminus `unknown` parent |
| **Terminus parent** | `unknown`, `dead` and PID 0 parent records at the terminus all still prove ownership |
| Cleanup | all frozen identities dead; `unknown` is never death and records no death; survivor refused; PID reuse recorded; stalled clock exhausts the poll ceiling; clock progress recorded |
| Selector independence | selector-bearing input refused; well-formed input admissible; court region selector-free |
| Model agreement | `classify_chain` accepts the chain this court walks |

### The door emits a PER-STATE shape

`tool.rs` (`process_observation_json`, `process_parent_observation_json`) emits:

```
live    -> {state:"live",    start_identity:<string|null>} (parent: parent_id)
dead    -> {state:"dead",    reason:<string>}
unknown -> {state:"unknown", reason:<string>}
```

A parser that demanded `start_identity` on every record would kill a `dead` or
`unknown` read at the shape check, before the chain logic could classify it —
which is exactly backwards, since `unknown` must reach the identity logic and
`dead` must reach the termination logic. The adapters are therefore split per
state, preserve `reason`, and are **pure** so every variant is testable without a
live process. A one-off manual implementation audit additionally observed a live
pid, an unused pid (`process_not_found`) and pid 1 (`process_access_denied`) from
the real door. That probe is not part of the registered, reproducible gate.

### The terminus is decided before the parent-live constraint

Each hop is still bracketed (`observe` → `parent` → `observe`), because the
bracket is what proves the node did not change across its relation read — that
fact is needed at the terminus too. But the **advancement** requirement (a
`live` parent record with a numeric id) applies only when the walk must move to a
successor. The owned browser is the root of the owned tree, so a `live` parent
id, PID 0, a self-parent or even a non-`live` parent record must all be able to
close the chain; the terminus is proven by the owned pid plus the frozen start
identity, both bracketed. `binding-model.classify_chain` agrees: `chain_step`
requires only the two brackets to be `live`, and reads the parent record solely
through `parent_id_of`.

### The cleanup clock is injected, and its bound is not only the clock

Cleanup here is the **classifier**, not a live wall-clock wait: it takes an
observation source plus an explicit `{now, sleep}` clock and decides
`TERMINATION_PROVEN` or `INCONCLUSIVE_CLEANUP`. No live cleanup path is enabled in
this slice, and nothing here claims a real observe-to-dead wait exists.

The poll ceiling is deliberately not redundant with the deadline. A clock that
fails to advance makes a deadline-only loop non-terminating, so the bound cannot
depend solely on something the classifier cannot verify. Removing the ceiling was
observed to exhaust the engine's step budget instead of terminating — it is
load-bearing. That is also why no red gate mutates the ceiling directly: a gate
that hangs is not a gate, so the exhaustion *report* is mutated instead.

### Red gate

Twelve gates; each mutation removes one guard through an exact-string replacement
that must apply and must change the source, and the suite must then report
`live_self_test_failed`. A non-compiling mutant, or one that crashes the suite
instead of failing a named check, is **rejected** rather than counted as red.

| Mutation | Guard removed |
|----------|---------------|
| `before !== after` → `false` | per-bracket identity comparison |
| `unknown` branch returns dead | `unknown` is never death |
| terminus identity → `false` | frozen-identity terminus check |
| `after = observe(cur)` → `after = before` | closing bracket |
| `}).ok === true` → `!== true` | selector-independence wiring |
| `hop < CHAIN_CEILING` → `hop < 100000` | finite chain ceiling |
| `const state = raw.state` → `"live"` | per-state door adapter |
| non-terminus parent requirement → always true | terminus before parent-live |
| exhaustion return → `TERMINATION_PROVEN` | exhaustion is not death |
| PID-reuse branch → `false` | cleanup PID-reuse detection |

Plus two controls: the court refuses a live mode itself
(`LIVE_COURT_NOT_ENABLED`, `browser_launched:false`, `ordinal_reserved:false`),
and the region handed to V7 is non-empty and selector-free.

### Honest limits of this evidence

- The mutations run against a **copy** of the court under the ignored local
  research state, driven through a probe manifest that replaces only the task
  and contract tables of the repository manifest. The unmutated self-test runs
  through the registered tool-profile task.
- `AGENTERM_LIVE_REGION_SOURCE` reaches the court through the runner's own
  environment, which the child inherits. No manifest field declares it: this slice
  invents no task-env schema. A task's `env` list and a contract's `env_allow`
  exist for other purposes and are not repurposed here.
- Stage publication around each side effect is **not** exercised. That is the §4
  kill criterion 4 condition that still blocks a live ordinal, and it is the main
  untested boundary of this slice.
- The cleanup whitelist lives in `binding-model.qjs`, a module with `export` that
  cannot be driven as a task entry, so a model mutation is **not** a red gate for
  this court and none is claimed.
- `court-current-host.qjs` still reports `live_path: "unimplemented-fail-closed"`,
  which is now imprecise. The model court is outside this slice's write set.

## Real-process preflight (`--live-process-preflight`)

First real-host ownership evidence in this experiment. No browser, no ordinal, no
ledger row, no design verdict. The subject is one owned, non-browser, short-lived
child (`/bin/sleep 10`), so a leaked process would outlive the ~1s run and be
visible.

The unmutated evidence below is produced by the registered tool-profile task
`profile-binding-exact-process-live-preflight`; probe manifests are reserved for
the mutation gate.

All 12 checks pass. Representative envelope:

```
ok: true | chain_length: 2 | slept_ms: 126
subject_kind: owned non-browser child | subject_seconds: 10
teardown: {ok: true, killed: true, reaped: true, released: true}
orphan_free: true | real_host_process: true | injected_fixture: false
browser_launched: false | ordinal_reserved: false
formal_root_touched: false | design_verdict: false
```

| Check | What it establishes |
|-------|---------------------|
| `host_clock_is_real` | epoch-scale clock AND a requested 120ms sleep actually elapsed |
| `identity_is_frozen` | `process.spawn_frozen` yielded a real `{pid, start_identity}` |
| `pid_door_agrees` | `process.pid(handle)` agrees with the frozen identity |
| `chain_worker_owns_subject` | the real walk proves THIS worker owns the subject, length 2 |
| `subject_identity_is_frozen` | the chain's subject identity EQUALS the frozen `process.identity(handle)` value |
| `subject_was_live_during_chain` | the first bracket really observed it live (derived, never asserted) |
| `teardown_releases_handle` | kill→wait→release all succeeded |
| `termination_observed_dead` | the frozen identity reached `dead` via real observation |
| `termination_never_unknown_as_dead` | no observation was a `dead`-shaped lie about `unknown` |
| `no_orphan_after_release` | the frozen pid is not still alive under the same identity |
| `worker_excluded_from_cleanup` | the worker is never part of termination cleanup |
| `no_outcome_error` | no throw escaped the real-path work |

### Why the ordering is load-bearing

The termination observation runs **after** teardown, and the chain runs
**before** it. Killing first would make "saw live first" impossible and would hide
a chain that never proved a live identity; observing before killing is also what
makes "observed dead" a real transition rather than a tautology. The first
implementation had this backwards and the `subject_was_live_during_chain` check
was added specifically because that bug was invisible without it.

### Door contract, measured not assumed

| Op | Style | Return |
|----|-------|--------|
| `process.spawn_frozen` | direct | handle ≥ 0; negative = refusal |
| `process.pid` | direct | **the pid itself**, not a status |
| `process.kill` | direct | 0 / negative |
| `process.identity` | answer | `{"pid":u32,"start_identity":String}` |
| `process.state` | answer | `"running"` / `"exited"` / `"unknown"` |
| `process.observe` / `parent` | answer | per-state JSON |
| `process.wait` | answer | command-result JSON |
| `process.release` | answer | empty string on success |
| `time.now_ms` | answer | **epoch ms as text** (NOT a direct value) |
| `time.sleep_ms` | answer | empty payload |

`time.now_ms` being answer-style is a real trap: reading its *return value* yields
a constant `0`, which is indistinguishable from a frozen clock. The first
implementation did exactly that and `host_clock_is_real` caught it.

### Red gate (baseline + 14 mutations + the fail-closed token gate)

Every mutation must turn the preflight envelope to `ok:false`; a mutation that
cannot be applied, fails to compile, crashes or hangs is **rejected** as a red.

| Mutation | Guard proven load-bearing |
|----------|---------------------------|
| arm `DRIFT_MODE` | the closing bracket reads a new record |
| arm `DRIFT_MODE` + `if (false)` | the identity comparison |
| arm `UNKNOWN_MODE` | `unknown` is never death |
| `parseInt(value,10)` → `1` | the real clock (epoch floor) |
| `kill_status = -1` | teardown aggregation |
| derived liveness → `false` | the subject was live during the chain |
| first-bracket derivation disabled | liveness comes from the bracket, not a constant |
| frozen identity tampered + opening binding off | the frozen subject identity is bound (opening side) |
| frozen identity tampered + closing binding off | the frozen subject identity is bound (closing side) |
| `release_status = -1` | the handle is released |
| deadline loop → `while (false)` | observe-until-dead |
| `if (pid_reused === true)` → `if (false)` | the PID-reuse termination shape |
| reuse identity comparison inverted | PID reuse requires a real identity change |
| unknown-final guard → `if (false)` | `unknown` never terminates the poll |
| a real check forced false | the entry is fail-closed (0 tokens, nonzero exit) |

Two mutations must first arm an adversary. That is honest, not a trick: a
correctly-behaving owned child never drifts and is never `unknown`, so those
guards cannot be shown to bite by removing them alone. The hooks are passthroughs
on the real path.

Three of these gates exist because the first submission got them WRONG and the
mutations proved it:

- **The frozen identity was not bound.** The walk took only `identity.pid` and
  re-observed whatever identity was live at that pid, so a completely tampered
  frozen identity still produced `ok:true`. The walk now takes the frozen identity
  as a required argument and checks it on BOTH sides of the first bracket
  (`subject_identity_mismatch` / `subject_identity_mismatch_after`), and a
  separate `subject_identity_is_frozen` check re-asserts it at the call site.
- **`subject_was_live` was an unconditional `true`.** It is now DERIVED inside the
  walk from the first bracket's own before/after records, so a caller cannot claim
  liveness the walk never observed.
- **The two termination shapes were conflated.** `never_unknown_as_dead` required a
  final `dead` record in every case, which REJECTS a legitimate PID reuse (the last
  observation there is `live` with a different identity). It now accepts exactly
  two shapes, takes the frozen identity instead of trusting `pid_reused` as a
  boolean, and requires the poll never to END on `unknown` in either shape.
- **The entry was fail-open.** It printed `EVIDENCE`/`PASS` unconditionally, so a
  red result still exited 0 with a PASS line beside it. It now prints the envelope
  first and throws a named `LIVE_PROCESS_PREFLIGHT_FAILED:<checks>` unless `ok` is
  true.

The `unknown`-final gate needed two new scenes to bite at all: with
`proved_dead === true` the shape checks already reject an unknown record, so
removing the guard was invisible. The `proved_dead === false` pair is what makes it
observable.

### Orphan evidence

After three consecutive preflight runs and after the red gate (whose mutants can
abort early), no `/bin/sleep 10` survivor was present. Teardown is best-effort on
**every** path, including a partially constructed subject, and failures are
aggregated rather than abandoning the remaining steps — stopping at the first
failure is exactly how orphans are made.

### Honest limits

- **No browser.** The subject is a non-browser child; the live court's real
  subject is a browser, and launching one is out of scope here.
- **No ordinal, no ledger row, no stage publication.** This is the R1 blocker:
  the preflight cannot show that every throw site is preceded by a persisted
  stage. It narrows the gap; it does not close it.
- **A 2-node chain.** The chain is `subject → worker`. Deeper real chains are not
  exercised, and the ceiling/cycle cases remain fixture-proven only.
- The `DRIFT_MODE` / `UNKNOWN_MODE` hooks exist for the red gate. They are inert
  on the real path, but a reviewer should know the file contains them.

## Template consistency: latent contradiction found and fixed

A browser-free persisted-stage slice was attempted and **stopped** by a blocker,
because `identity-source` could not be published at all. The defect was in the
template data, not in the broker.

### The contradiction

`result-template.json` declared `identity-source` with **required** facts
`scanned_source_count` and `call_count`, while `stage_fact_whitelist` contained
neither. `validate_stage_facts` checks the whitelist **before** the shape, so the
stage was internally unsatisfiable. Every possible fact strategy was measured
against a real disposable broker, and each hit a different wall:

| facts supplied | result |
|---|---|
| its own required counts | `stage_fact_not_whitelisted` |
| `{}` | `stage_required_fact_missing` |
| only whitelisted keys | `stage_fact_not_in_stage_shape` |

All three fail, which is what makes this a contradiction rather than a missing
convenience. The mirror defect was `last_completed_stage`: whitelisted, but
admitted by no stage shape, so `stage_fact_not_in_stage_shape` refused it —
unreachable in both directions.

**Why nothing caught it earlier.** `identity-source` had never been staged
anywhere: zero occurrences in `broker-self-test.sh`, and no other call site. The
existing gates exercised the other six stages, so the contradiction stayed latent
from the moment the template was written and would have surfaced only at the
first runtime attempt — potentially *after* an ordinal was reserved.

### The fix

Data (`result-template.json`): `scanned_source_count` and `call_count` added to
`stage_fact_whitelist` (17 → 18), preserving `identity-source` semantics;
`last_completed_stage` removed. No receipt or ledger key changes.

Guard (`broker-spine.sh`): the template is validated **once at startup**, before
any operation and before any state root is created.

| Violation | Named code |
|---|---|
| a stage names a fact outside the whitelist | `template_stage_fact_not_whitelisted:<stage>:<fact>` |
| a whitelisted fact is consumed by no stage shape | `template_whitelist_fact_unused:<fact>` |
| a stage fact has no declared type | `template_stage_fact_type_missing:<stage>:<fact>` |
| a stage fact declares an unknown type | `template_stage_fact_type_unknown:<stage>:<fact>:<kind>` |

The type half **reuses** `%FACT_TYPE`; a second list of legal types would be a
second truth and would drift.

### Evidence

`BROKER_SELF_TEST_PASS requests=139 failures=0` at that leaf — nine new checks
over the previous 130 (the count rises further with the kill-terminal leaf):

| Check | What it proves |
|---|---|
| the shipped template passes the startup consistency gate | the positive case; the guard is not a blanket refusal |
| a stage fact missing from the whitelist is refused at startup by name | required ⊄ whitelist is caught **before** any operation |
| a whitelisted fact no stage shape consumes is refused at startup by name | the orphan case (re-introducing `last_completed_stage`) |
| a stage fact with an unknown declared type is refused at startup by name | the type half, with the specific kind in the code |
| identity-source R1 reserves in its own disposable root | the behavioural precondition |
| identity-source stages with exactly its required facts | **the case that was impossible before the fix** |
| the identity-source row is readable from disk with its exact facts | read-back, not just an `accepted` return |
| a stage missing a required fact is still refused by name | the fix did not weaken the shape check |
| a fact outside this stage's shape is still refused by name | the fix did not widen any single stage |

Each mutation was additionally verified to fire **before any state root is
created**, so an inconsistent template cannot reserve an ordinal at all.

### The kill terminals were a separate blocker; a later leaf fixes them

At the time of the template fix, **terminal honesty was a separate blocker**: no
legal terminal code existed for "V6 mechanism proven, browser criteria not-run",
because `NEW_INFORMATION_INSUFFICIENT` and `CLEANUP_NOT_INDEPENDENT` were both
refused by `terminal_criteria_not_implemented`. Closing the whitelist gap did not
open that path. That blocker is **now fixed** by the kill-terminal leaf described
at the end of this file.

### Still open: the browser-live mechanism

Terminal honesty was necessary but not sufficient. **No formal R1 reservation, no kill
criterion 4 closure and no V1–V7 claim is made here.** The browser-live path is
still unproven: no owned-browser ordinal has been reserved, and kill criterion 4
is not closed because the browser-live throw sites are not covered.

## Persisted-stage real-process preflight

`run-current-host.sh --live-staged-preflight` passes against a disposable root.
The authoritative journal contains eight read-back rows:

| Sequence | Stage | Code | Decisive fact |
|---:|---|---|---|
| 1 | `preflight` | `PREFLIGHT_OK` | before subject spawn |
| 2 | `identity-source` | `IDENTITY_SOURCE_PENDING` | six sources scanned; zero process calls yet |
| 3 | `identity-source` | `IDENTITY_SOURCE_PROVEN` | exact-key calls projected from the real trace |
| 4 | `ownership` | `OWNERSHIP_PROVEN` | two-node owned chain |
| 5 | `stop` | `STOP_REQUESTED` | before kill/wait/release |
| 6 | `termination-proof` | `TERMINATION_PENDING` | zero deaths proven before polling |
| 7 | `termination-proof` | `TERMINATION_PROVEN` | one frozen identity proven gone |
| 8 | `final-inventory` | `FINAL_INVENTORY_VACUOUS` | browser-free 0/0; not V5 evidence |

The disk audit succeeds, the formal root is byte-identical, and the disposable
ledger ends `abandoned` with no terminal code or bound receipt. Baseline
`finish_called` is false. Fourteen red controls bite by named failure.

This proves only persisted-stage mechanics for experimental subject operations.
Evidence-infrastructure broker spawns and browser-live throw sites are outside
its trace. It proves no V1–V7 criterion, reserves no formal ordinal, and leaves
kill criterion 4 open.

## Backfill (§8)

Intentionally empty until a terminal live result exists. A terminal result must
include the V1-V7 table, D1-D3 measurements, the exact decision-tree trace,
every deviation from the specification, the consumed ordinal, reproducible
commands, and an explicit statement that criteria were not changed after seeing
the result.

## Kill terminals and endpoint identity digests

A following leaf implements the two kill-criterion terminals the plan fixed, so an
attempt that must report a kill criterion can now close truthfully instead of
being stuck at `abandon`.

**Each terminal has exactly one legal criteria shape**, and every other shape is
refused by name:

| Terminal | Legal shape | Meaning |
|---|---|---|
| `NEW_INFORMATION_INSUFFICIENT` | V1, V2 `pass`; V3 `fail`; V4–V7 `not-run` | the mechanism could not be shown, and nothing downstream ran |
| `CLEANUP_NOT_INDEPENDENT` | V1–V3 `pass`; V7 `fail`; V4–V6 `not-run` | ownership was shown, but cleanup was not shown to be independent |

Refusals use `terminal_criteria_not_legal`. The check is a full-shape match, not a
spot test on one cell: a shape that differs from the legal one in **any** single
cell is refused, so "V3 is fail" or "V7 is fail" alone is not sufficient to close.
Every non-design terminal now has a mapping, so `terminal_criteria_not_implemented`
no longer fires for a kill criterion; a design selection (`A1_SELECTED`,
`B_SELECTED`) remains refused by its own gate
(`terminal_design_selection_not_implemented`), because the broker owns no design
receipt contract.

**Endpoint identities are persisted as stable sha256 digests, never raw pids or
paths.** Three facts were added to the whitelist (18 → 21), each owned by exactly
one stage:

| Fact | Stage | Purpose |
|---|---|---|
| `browser_identity_digest` | `ownership` (optional) | the browser endpoint |
| `bridge_host_identity_digest` | `ownership` (optional) | the bridge host endpoint |
| `connection_identity_digest` | `ownership` (optional) | the bridge connection |

All three are owned by `ownership` — the only stage where every endpoint is
known, since `identity-source` can run while the connection is still pending.
One fact, one owner: no digest has two stages that could disagree about it.

Because the declared type is `sha256`, a raw pid (`4242`), a raw filesystem path or
any non-sha256 string is refused as `stage_fact_type` — the "never persist a raw
pid or path" rule is enforced by the type itself, not by a separate validator.

**A V3-pass claim must be bound to its evidence.** A receipt recording
`criteria.V3 = pass` asserts the ownership chain was proven. That claim is only
supported when the attempt's own journal carries all three endpoint digests, so
the broker now re-reads them **from the `ownership` journal row** (never from the
receipt the caller supplied) and refuses, by name, when any is missing:

| Condition | Code |
|---|---|
| no `ownership` row at all | `v3_pass_ownership_evidence_missing` |
| a digest key absent from the row | `v3_pass_digest_missing:<fact>` |
| a digest present but not sha256 | `v3_pass_digest_not_sha256:<fact>` (defense in depth; the `sha256` type already rejects it at stage time, so no self-test mutation reaches it) |

The same check runs on `finish` **and** on the authoritative load, so a forged
attempt cannot become authoritative by being written to disk first. A terminal
recording V3 as `fail` (both kill terminals) makes no ownership claim and is not
required to carry the digests.

**Evidence.** `BROKER_SELF_TEST_PASS requests=168 failures=0`, twenty-eight new
checks over the previous 139: both legal shapes accepted; six one-cell-off shapes
refused by name; a design selection refused; the legal terminal staged **and
finished** with a receipt; a sha256 endpoint digest accepted on `ownership`; a raw
pid, a raw path and a non-sha256 string each refused; a V3-pass close with all
three digests finished; a V3-pass close missing each digest refused; a V3-fail
kill terminal closing with no digest required; and — for the load path — a
**coherent forgery** refused.

Stage names may repeat, so the binding uses the **latest** `ownership` row. A
regression case publishes a complete row followed by a newer row missing the
connection digest; the V3-pass finish is refused rather than accepting stale
endpoint evidence from the first row.

The forgery cases matter because a row that is merely edited is caught earlier by
the journal chain or the receipt digest, so those checks would not reach the
binding. The forgery cases instead drop a digest key and repair the journal chain,
the receipt's journal claims and the ledger's receipt digest, so the journal and
receipt are mutually consistent. Only the digest binding refuses them, each with
its specific `v3_pass_digest_missing:<fact>` code.

Two older checks used `NEW_INFORMATION_INSUFFICIENT` as their specimen of an
*unmapped* terminal. Since every non-design terminal is now mapped, no such
specimen remains; those checks were re-pointed at the design-selection gate rather
than left asserting something that is no longer true.

**This closes the terminal blocker only.** It does not reserve an ordinal, does not
launch a browser, and does not close kill criterion 4 or any of V1–V7. The
browser-live mechanism remains open.
