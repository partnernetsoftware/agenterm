# RESULTS — browser profile-name binding exact-process experiment

**Status: in progress. This file deliberately contains no conclusion.**

The first implementation slice (platform-neutral self-test) has landed. No live
ordinal has been reserved, no browser has been launched, no A0/A1/B design has
been selected or rejected, and no evidence has been registered. The owning PRD
leaf `acu.dynamic.075.profile-name-binding` and the capability ledger are
unchanged.

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
- No `process.observe` / `process.parent` / `process.pid` call was made. Every
  process fact used by the model is caller-supplied data.
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

## Backfill (§8)

Intentionally empty until a terminal live result exists. A terminal result must
include the V1-V7 table, D1-D3 measurements, the exact decision-tree trace,
every deviation from the specification, the consumed ordinal, reproducible
commands, and an explicit statement that criteria were not changed after seeing
the result.
