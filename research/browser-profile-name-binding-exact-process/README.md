# Browser Profile name binding exact-process experiment

Decisive-experiment implementation for
`plan/design-browser-profile-name-binding-exact-process-experiment.md`.

**Status: in progress (admission-free slices only).** No live ordinal is
reserved, no browser is launched, no design is selected or rejected.
`RESULTS.md` carries no conclusion.

This is a new precommitment. It does not reopen, amend or repair
`research/browser-profile-name-binding/` or
`research/browser-profile-name-binding-v2/`; their frozen sources, results,
ledgers, receipts and exhausted markers are read-only inputs. This experiment
has a distinct id, state root, source digest, input digest and budget.

## What exists today

| Path | Role | State |
|---|---|---|
| `binding-model.qjs` | Platform-neutral pure model: process chain, ownership freeze, cleanup, stage receipts, V1-V7/D1-D3 decision tree | Implemented, self-tested |
| `court-current-host.qjs` | Platform-neutral model checks for the runtime half of V2, the V6/V7 contracts and every decision-tree combination | Implemented |
| `capability-preflight.qjs` | Registered tool-profile host preflight: real child PID, exact process observations, durable read-back, lock contention and digest | Implemented, admission-free |
| `admission-dry-run.qjs` | Registered tool-profile admission dry-run: drives the external broker through one complete rehearsal transaction in a disposable root | Implemented, disposable-only |
| `court-current-host.qjs` live path | macOS live court (browser launch, real `process.observe`/`parent`) | **Not implemented, fail-closed** |
| `result-template.json` | Receipt/stage field contract | Implemented |
| `fixtures/` | Synthetic Profile fixtures | Reused bytes with provenance |

## Run it

```sh
research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --self-test
```

The runner emits two JSON records: the V2 static source-scan result, then the
platform-neutral model result. Both are required. Running the court directly
proves only its model half; it cannot inspect its own source through the plain
qjswasm `run` embedder. The self-test launches no browser, reads no process
table, and calls no `process.*` host operation. Its live-path sibling refuses
with a typed code.

The separate host preflight uses the registered `profile: "tool"` entry because
the plain `script run` embedder intentionally has no process or filesystem
doors:

```sh
research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --capability-preflight
```

It must emit `IDENTITY_SOURCE_PROVEN`, one all-true capability record, the
declared evidence id and `PASS`. It starts only an owned sleeping shell child,
removes its scratch directory, launches no browser and reserves no ordinal.
This proves host primitives, not V3-V5 or D1-D3.

## Admission dry-run

`admission-dry-run.qjs` is a **separate** entry from `court-current-host.qjs`,
on purpose. It uses `arg`, `env.*` and `process.spawn`, which the plain
`cli script run` embedder does not declare. Adding those calls to the model
court would make the ordinary `--self-test` path fail to compile, so the
dry-run is its own file and is reachable only as a registered
`profile: "tool"` task.

It drives the real broker through one complete rehearsal transaction — reserve,
a legal `preflight` stage, a legal terminal and `finish`, plus an independent
`abandon` path — inside a disposable root. It launches **no** browser, reaches
**no** design verdict, and reserves **no** formal ordinal.

```sh
research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --admission-dry-run
research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --admission-red-gate       # needs nothing registered
```

**Registered task entry.** `agenterm.tasks.json` provides a task with
`"entry": "research/browser-profile-name-binding-exact-process/admission-dry-run.qjs"`,
`"profile": "tool"` and `"platforms": ["macos"]`. That manifest is a declared
hot file, so it is maintained by the owning agent rather than by this
directory's author.

**Why the ordinal is still named `R1`.** The broker accepts only `R1`/`D1`; a
reservation with any other ordinal fails `reserve_ordinal`. The dry-run
therefore cannot avoid the name. Isolation comes from the **root**: the runner
creates a `mktemp -d` directory, points the broker at it, and asserts the formal
root is byte-for-byte unchanged afterwards.

**Disposable roots must be fully resolved.** `fs.create_new_regular_durable`
deliberately refuses to traverse a link-like ancestor: it opens each path
component as a real directory. On macOS `TMPDIR` sits under `/var`, which is a
symlink to `/private/var`, so an unresolved `mktemp` path fails with the
misleading `Not a directory` even though `fs.exists` reports it. `TMPDIR` also
ends in a separator, which doubles the slash. The runner therefore resolves the
root with `pwd -P` before handing it over.

**What it does not prove.** This is not a side-effect-free admission check. It
really writes a ledger, a journal and receipts — into the disposable root. It
proves that the admission state machine round-trips through the real broker and
that the refusal surface behaves as documented. It proves nothing about V3-V5
or D1-D3, and it does not reserve, consume or replace the formal `R1`.

Facts carrying `argv`, `env`, titles, URLs or home paths are refused by the
template's forbidden-key list (`stage_forbidden_key`), and a key that is merely
unlisted fails the whitelist (`stage_fact_not_whitelisted`). Both are exercised
from the runner by `--admission-red-gate`, which also asserts the dry-run entry
is unreachable from the plain run path and proves the formal-root snapshot
detects an in-place byte change even when the path set is unchanged.

## Broker self-test

`broker-spine.sh` is the external attempt ledger, admission and stage
persistence broker (`wc -l`: 1216 lines). Its ledger/journal/receipt discipline is copied **with
provenance** from the v2 court's broker (`run-current-host.sh`, its `run_broker`
heredoc); it is not a rerun, repair or rename of that court. It has its own
experiment id, schemas, state root, ordinals (`R1`, `D1`, not reusable) and
input-digest domain, and it never reads or writes v2 state.

`broker-self-test.sh` (`wc -l`: 1548 lines) exercises that discipline entirely
inside a disposable `mktemp` root and reserves **no formal ordinal**:

```sh
sh research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --broker-self-test
```

It must emit `BROKER_SELF_TEST_PASS` with zero failures. It proves the admission
discipline only, never a design fact: the external ledger is authoritative while
a receipt is a disposable mirror; the state lock is exclusive; an ordinal is
never reused; `reserved` closes to `finished` only through `finish`, against a
re-read receipt whose digest is re-derived from the journal on disk, and to
`abandoned` with no receipt; the closing row must be a `terminal` stage whose
code, `facts.terminal_code`, receipt code and ledger `terminal_code` all agree
and whose criteria match one of that code's legal decision-tree shapes; a
attempt may record V1-V7 but never a design fact, and no attempt may publish a
design-selection terminal until its receipt contract exists; every simulated side effect is preceded by a published stage with an
enforced read-back; a persistence failure emits the fixed seven-key record,
forbids a design fact, exits nonzero and leaves the attempt `reserved` for
audit; and the disposable root has a defined disposition.

### Template consistency is checked at startup (latent contradiction fixed)

The stage shapes and the fact whitelist are two halves of one contract, and they
had silently disagreed. `identity-source` **required** `scanned_source_count` and
`call_count`, but **neither was in `stage_fact_whitelist`**. The broker checks the
whitelist before the shape, so that stage was internally unsatisfiable: no fact
set could ever publish it.

```text
facts = its own required counts  -> stage_fact_not_whitelisted
facts = {}                       -> stage_required_fact_missing
facts = only whitelisted keys    -> stage_fact_not_in_stage_shape
```

All three paths fail, which is what makes it a contradiction rather than a
missing convenience. The defect was **latent from the start**: `identity-source`
was never staged anywhere, so no existing gate could see it, and it would have
surfaced only at the first runtime attempt — potentially *after* a live ordinal
was reserved. The mirror defect was `last_completed_stage`: whitelisted but
admitted by no stage shape, so `stage_fact_not_in_stage_shape` refused it and it
was unreachable in both directions.

The fix has two halves:

1. **Data**: `scanned_source_count` and `call_count` are added to
   `stage_fact_whitelist` (17 → 18 facts), preserving `identity-source`
   semantics; take no other meaning. `last_completed_stage` is removed from it.
   No receipt or ledger key is affected.
2. **Guard**: the broker now validates the template **once at startup**, before
   any operation and before any state root is created, with a named code per
   violation:

| Violation | Code |
|-----------|------|
| a stage names a fact outside the whitelist | `template_stage_fact_not_whitelisted:<stage>:<fact>` |
| a whitelisted fact is consumed by no stage shape | `template_whitelist_fact_unused:<fact>` |
| a stage fact has no declared type | `template_stage_fact_type_missing:<stage>:<fact>` |
| a stage fact declares an unknown type | `template_stage_fact_type_unknown:<stage>:<fact>:<kind>` |

The type half **reuses** `%FACT_TYPE` rather than restating the legal type list,
because a second list would be a second truth and would drift. A mutated template
is refused before any state directory exists, so an inconsistent template cannot
reserve an ordinal at all. `broker-self-test.sh` covers the positive case, all
three mutations above, the now-publishable `identity-source` stage against a real
disposable broker, and the two shape violations that must still be refused.

`inspect` is genuinely read-only: it opens an existing lock **without**
`O_CREAT`, creates no state root and no `stage-journal` directory, and reports a
typed `state_absent` on a host that has never run an attempt.

An authoritative read re-derives every `finished` transition from disk. The
ledger's structural check can only confirm a row's shape and vocabulary, so
`inspect`, `reserve`, `stage`, `finish` and `abandon` all additionally require
that each finished row's receipt exists and hashes to the recorded digest, that
the receipt validates against its reservation, that the journal's real bytes
reproduce the receipt's digest and sequence, that the journal's last row is the
terminal stage, and that the terminal code survives the same kind and criteria
gates the write path applies. A hand-forged `finished` row is therefore refused
and never presented as a legitimate completion.

The summary is **fail-closed**: a run with any failed assertion prints
`BROKER_SELF_TEST_FAILED` and exits nonzero, and prints no pass token anywhere.
A harness proves that property rather than assuming it:

```sh
sh research/browser-profile-name-binding-exact-process/run-current-host.sh \
  --broker-harness
```

It must emit `BROKER_SELF_TEST_HARNESS_PASS`. It runs the real entry point twice
— clean, and with a deliberately injected failure — and asserts the exit code,
the summary token, and the **absence** of any pass token in the failing run. A
self-test that counted failures but still printed `PASS` would be worse than no
self-test, so the aggregator is itself gated.

### Precise claims (do not overstate)

| This broker does | This broker does **not** claim |
|---|---|
| **re-reads the frozen inputs.** For each path a manifest names, it opens the real file under the manifest's own `root` (no-follow, escape-checked) and hashes the bytes, rejecting a path that is absolute, contains `..`, resolves outside the root, is a symlink, or is missing. A forged `files[].sha256` or `input_digest` therefore fails, because the recomputed value would disagree | — |
| reads the pinned source revision from an **independent** readable source when the manifest declares a repository, rather than trusting the claim | — |
| reports an omitted manifest as `not-supplied`, never as `verified` | — |
| enforces the template's journal bounds on every read **and on the candidate journal before publication**, so an over-limit row is refused without touching the authoritative journal: `<64` rows, `<32768` bytes per row, `<1048576` bytes total | — |
| atomic publication: temp write, `fsync`, `rename`, then an enforced read-back | — |
| — | **power-loss** durability. The parent directory is not fsynced, so a power loss may lose the rename. This is **process-crash evidence**, deliberately weaker than the live runner's contract |

## Provenance

The broker shares its discipline with v2 but shares no state: the v2 broker's
`reserve`/`stage`/`finish`/`abandon` state machine, its atomic-replace plus
read-back publication, its exclusive `state.lock`, its chained stage journal and
its receipt-binding rules were read as the reference and re-expressed here. The
v2 heredoc is not imported, not executed and not modified.

`binding-model.qjs` reuses fixture bytes and the pure layout/parse discipline of
`research/browser-profile-name-binding-v2/`. Reuse is by **copying with
provenance**; this directory imports no mutable result state and no attempt
ledger from either exhausted experiment. The v2 model's binding-operation
semantics (A1 evidence shape and the B create/use/rename/duplicate/
stale-generation/crash-recovery model) are re-expressed here as *decision-tree
arms*, not as a copied verdict: neither exhausted court reached a verdict, so no
v2 value is authoritative.

## Live court (`live-rehearsal.qjs`)

The live court implements the §1.3 chain walk, the §1.5 frozen-ownership rule
and the §1.6 observe-to-dead cleanup. It is registered as a **tool-profile**
task, because it must call `process.observe`, `process.parent`, `process.kill`,
`process.wait` and `process.release`. That is also why it is a separate file
rather than a mode of `court-current-host.qjs`: adding those host calls to the
model court would break its ordinary `cli script run` path, which declares no
tool door.

Run it:

```sh
./research/browser-profile-name-binding-exact-process/run-current-host.sh --live-self-test
./research/browser-profile-name-binding-exact-process/run-current-host.sh --live-red-gate
```

`--live-self-test` drives the entire court against an **injected fixture**: a
fake observation source and a fake inventory. No browser is launched, no owned
process is observed for ownership and no ordinal is reserved. It machine-proves
the door adapter, the chain walk and the cleanup classifier (56 named checks).
The registered gate exercises injected variants. A separate one-off manual
implementation audit also probed the real tool door; that probe is not part of
the reproducible self-test evidence.

`--live-red-gate` proves those claims bite. Each of ten mutations removes one
guard and must make the self-test fail; a mutation that cannot be applied, that
fails to compile, or that crashes the suite instead of failing a named check is
rejected rather than counted as red. Two further controls assert that the court
refuses a live mode itself (printing `LIVE_COURT_NOT_ENABLED` with
`browser_launched:false`, `ordinal_reserved:false`) and that the region handed to
V7 is non-empty and selector-free.

### The door emits a PER-STATE shape

```
live    -> {state:"live",    start_identity:<string|null>} (parent: parent_id)
dead    -> {state:"dead",    reason:<string>}
unknown -> {state:"unknown", reason:<string>}
```

The adapter therefore parses by **state**, not by one fixed key set, and
preserves `reason` so a reviewer can tell "gone" from "unreadable". Demanding
`start_identity` on every record would kill a `dead` or `unknown` read at the
shape check — before the chain could classify it, which is backwards for the one
state (`unknown`) that must reach the identity logic.

### The chain-reading rule (§1.3)

`process.parent` reports only a parent id, so the successor's identity cannot be
carried forward. Each hop is therefore one bracket around the **current** node:

```
before  = observe(current)     establishes current identity
record  = parent(current)      the relation, read between the brackets
after   = observe(current)     closes the bracket
```

The successor's identity is established by the **first observation of the next
iteration**, and a node whose identity cannot be established fails in `observe`
rather than being assumed.

The bracket is applied to the terminus too, but the **advancement** requirement
(a `live` parent record with a numeric id) applies only when the walk must move
to a successor. The owned browser is the root of the owned tree, so its parent
relation is never used to advance: a `live` parent id, PID 0, a self-parent or
even a non-`live` parent record must all be able to close the chain. The terminus
is proven by the owned pid plus the frozen start identity, both bracketed.
`binding-model.classify_chain` agrees.

### Cleanup is a classifier with an injected clock

Cleanup takes an observation source plus an explicit `{now, sleep}` clock and
decides `TERMINATION_PROVEN` or `INCONCLUSIVE_CLEANUP`. It is **not** a live
wall-clock wait, and nothing here claims one exists. The poll ceiling is not
redundant with the deadline: a clock that fails to advance makes a deadline-only
loop non-terminating, so the bound cannot rest solely on something the
classifier cannot verify.

## Real-process preflight (`--live-process-preflight`)

This is the **first** code in this experiment that touches real host processes for
ownership. It is deliberately not the live court: the owned subject is a
non-browser, short-lived child, no ordinal is reserved, no ledger row is written
and no design verdict is reached.

```sh
./research/browser-profile-name-binding-exact-process/run-current-host.sh --live-process-preflight
./research/browser-profile-name-binding-exact-process/run-current-host.sh --live-process-red-gate
```

The unmutated preflight runs through the registered tool-profile task
`profile-binding-exact-process-live-preflight`. Temporary probe manifests are
used only for adversarial source mutations.

What it does, using the **real** door and the **same** `walk_ownership_chain` and
cleanup classifiers the live court uses:

1. Reads the real host clock and takes a real bounded sleep.
2. Spawns one owned, non-browser, short-lived child with `process.spawn_frozen`,
   which is the only spawn that yields a frozen `{pid, start_identity}`.
3. Cross-checks `process.pid(handle)` against that frozen identity: two
   independent doors describing the same owned child.
4. Walks the ownership chain from the subject up to **this worker**, so the chain
   proves the subject is owned by this worker rather than by something else.
5. Tears down best-effort as `kill → wait → release`, **aggregating** failures
   instead of abandoning the remaining steps.
6. Observes the frozen identity until it reaches `dead`, treating `unknown` as
   never-dead, and records every observation.
7. Asserts no orphan remains and the handle was released.

The worker is **never** part of the termination cleanup: its liveness cannot be a
precondition of a check that runs inside it.

### The clock is the door's, not a counter

`time.now_ms` is answer-style (status `0`, value in `tool_result`) and reads real
epoch milliseconds; `time.sleep_ms` performs a real bounded sleep. Neither is a
step counter: the preflight asserts an epoch-scale floor and that a requested
sleep actually elapsed. A counter that only advances on sleep is exactly the fake
clock this leaf forbids, and the epoch floor rejects it.

### `--live-process-red-gate`

Baseline plus fourteen mutations and a fail-closed token gate; each mutation flips
exactly one guard, and a mutation that cannot be applied, fails to compile, crashes
or hangs is rejected as a red.

The entry itself is **fail-closed** and the gate asserts that directly: it does not
read the envelope's `ok` field, because a grep of the envelope would pass even if the
entry printed `PASS` beside `"ok":false` and exited 0. It asserts the two observable
facts — zero `PASS`/`EVIDENCE` tokens and a nonzero exit code — plus a control that the
unmutated entry really does print both tokens once with `rc=0`.

Two mutations must first **arm an adversary** through the `DRIFT_MODE` /
`UNKNOWN_MODE` hooks. That is not a trick: a correctly-behaving owned child never
drifts its identity and is never `unknown`, so a guard against those cases cannot
be shown to bite by removing the guard alone. Arming the adversary and then
removing the guard is what makes the red attributable; the hooks are pure
passthroughs on the real path.

| Mutation | Guard |
|----------|-------|
| arm `DRIFT_MODE` | the bracket really reads a new record |
| arm `DRIFT_MODE` + binding off | the frozen subject identity is bound (closing side) |
| arm `DRIFT_MODE` + binding off | the frozen subject identity is bound (opening side) |
| arm `DRIFT_MODE` + neutralise the comparison | the identity comparison |
| arm `UNKNOWN_MODE` | `unknown` is never death |
| epoch read → constant `1` | the real clock (epoch floor) |
| `kill_status = -1` | teardown aggregation |
| `subject_was_live = false` | the subject was live during the chain |
| derived bracket liveness → `false` | subject liveness comes from the first bracket |
| `release_status = -1` | the handle is released |
| deadline loop → `while (false)` | observe-until-dead |
| `if (pid_reused === true)` → `if (false)` | the PID-reuse termination shape |
| reuse identity comparison inverted | PID reuse requires a real identity change |
| unknown-final guard → `if (false)` | `unknown` never terminates the poll |
| false check → unconditional tokens | the entry is fail-closed |

### What is still not implemented

`live`, `rehearsal` and `decision` are refused by **both** the runner and the
court (`LIVE_COURT_NOT_IMPLEMENTED` / `LIVE_COURT_NOT_ENABLED`). Two conditions
are unmet: no live ordinal may be reserved while the court cannot prove every
throw site is preceded by a persisted stage (§4 kill criterion 4), and no
browser may be launched before that proof is reviewed (§2). The live court has
**no browser-spawn code path at all** in this slice.

The court's stage-publication wiring against the broker is consequently
**untested live**: the self-test proves the ownership/cleanup classification, not
that each side effect is bracketed by a persisted stage.

**This is the R1 blocker.** Stage publication and the formal broker are
**deliberately out of scope** for the real-process preflight: it writes no ledger
row and reserves no ordinal, so it cannot show that every throw site is preceded
by a persisted stage. The preflight narrows the gap — the ownership, identity,
clock, cleanup and orphan mechanisms are now proven against real host processes
rather than only against a fixture — but it does **not** close it. A live R1
rehearsal still requires the stage-publication proof that §4 kill criterion 4
demands.

`court-current-host.qjs` still reports `live_path: "unimplemented-fail-closed"`.
That field is now imprecise — the live path exists and is gated — but this
slice is not authorized to edit the model court, so the stale field is recorded
here rather than silently rewritten.

### Persisted-stage real-process preflight

`run-current-host.sh --live-staged-preflight` runs a registered tool-profile
task against a fully resolved disposable state root. It uses the name `R1` only
inside that root, publishes and reads back eight rows, audits the chained
journal from disk, and records an independently audited `abandoned` attempt.
The baseline stages no terminal and never calls `finish`, so it creates no
receipt-bound result or design fact.

Repeated `identity-source` rows record zero process calls before the guarded
reads and the trace-derived count afterward. Repeated `termination-proof` rows
record zero proven deaths before polling and one afterward. The other rows are
`preflight`, `ownership`, `stop`, and a browser-free `final-inventory`; its 0/0
count is explicitly vacuous and is not V5 evidence.

`--live-staged-red-gate` runs fourteen controls. They introduce real ordering,
durability, truncation, terminal-attempt, trace-coverage, death-count, and
call-count defects and require named failures. Compilation failures, crashes,
hangs, and mutations that do not apply do not count as red.

The trace covers experimental subject operations only. Broker processes used
to publish evidence cannot be guarded by the row they are themselves
publishing. Browser-only throw sites remain unexercised, so
`kill_criterion_4_closed` stays false and no formal ordinal is eligible.

### Two independent blockers were open; one is now fixed

The persisted-stage work surfaced **two separate** defects. They are not the same
bug, and fixing one does not open the other:

1. **Template unsatisfiability — FIXED here.** `identity-source` required two
   facts that were not whitelisted, so no fact set could publish it. The
   whitelist now carries them (17 → 18 facts), the orphaned
   `last_completed_stage` is gone, and the broker validates the template at
   startup with a named code per violation. See "Template consistency is checked
   at startup" above.
2. **Terminal honesty — STILL OPEN.** There is no legal terminal code for
   "V6 mechanism proven, browser criteria not-run":

   | Terminal | Behaviour |
   |----------|-----------|
   | `NEW_INFORMATION_INSUFFICIENT` | refused — `terminal_criteria_not_implemented` |
   | `CLEANUP_NOT_INDEPENDENT` | refused — `terminal_criteria_not_implemented` |
   | `INCONCLUSIVE_MECHANISM` | accepted **only** by claiming V1–V7 all pass (unprovable browser-free) |
   | `INVALID_EVIDENCE` | accepted, but **means V6 failed** and also claims V1–V5 pass |

   Closing the whitelist gap does not open this path, and resolving this would
   not have made `identity-source` publishable. Until it is ruled on, no attempt can
   close truthfully, so a browser-free slice must **`abandon`** rather than
   `finish` — and reserving a live ordinal requires a truthful close.

Neither blocker is closed by this leaf. **No R1, no kill criterion 4, and no
V1–V7 claim is made anywhere in this directory.**

## What this directory must never do

- Treat `process.parent.live` as liveness. Only `process.observe` decides
  liveness and start identity.
- Accept an unbracketed PID, or a null/unequal `start_identity`.
- Consult a selector, candidate label, `profile_instance_id` or provisional
  verdict during ownership or cleanup.
- Revive `/bin/ps`, `pgrep`, a process-table parser, process-group inference or a
  session-id predicate.
- Weaken a stage receipt, or edit an exhausted marker.

Pressure to do any of these is a finding, not a requirement. See
`plan/design-browser-profile-name-binding-exact-process-experiment.md` §1.
