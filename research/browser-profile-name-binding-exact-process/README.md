# Browser Profile name binding exact-process experiment

Decisive-experiment implementation for
`plan/design-browser-profile-name-binding-exact-process-experiment.md`.

**Status: in progress (first slice only).** No live ordinal is reserved, no
browser is launched, no design is selected or rejected. `RESULTS.md` carries no
conclusion.

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

## Provenance

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
