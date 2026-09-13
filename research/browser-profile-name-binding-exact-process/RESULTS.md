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

Last run on this host: `BROKER_SELF_TEST_PASS requests=130 failures=0`, and
`BROKER_SELF_TEST_HARNESS_PASS`.

Source sizes (`wc -l`): `broker-spine.sh` 1163, `broker-self-test.sh` 1400,
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
classifier against injected fixtures. All 43 checks pass; no browser is launched,
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

## Backfill (§8)

Intentionally empty until a terminal live result exists. A terminal result must
include the V1-V7 table, D1-D3 measurements, the exact decision-tree trace,
every deviation from the specification, the consumed ordinal, reproducible
commands, and an explicit statement that criteria were not changed after seeing
the result.
