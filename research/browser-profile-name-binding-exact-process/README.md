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

## Broker self-test

`broker-spine.sh` is the external attempt ledger, admission and stage
persistence broker (`wc -l`: 1163 lines). Its ledger/journal/receipt discipline is copied **with
provenance** from the v2 court's broker (`run-current-host.sh`, its `run_broker`
heredoc); it is not a rerun, repair or rename of that court. It has its own
experiment id, schemas, state root, ordinals (`R1`, `D1`, not reusable) and
input-digest domain, and it never reads or writes v2 state.

`broker-self-test.sh` (`wc -l`: 1400 lines) exercises that discipline entirely
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
