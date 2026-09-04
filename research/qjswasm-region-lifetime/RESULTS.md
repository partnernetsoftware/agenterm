# Results

Status: **Phase B partial; L0 not decided; no allocator change**.

## Exact source

- AgenTerm: `fa9455c0000ff75d364bae0074b647a53abfb450`
- tinyvm / tinyvm-qjs: `78442d966fe42e7c97e42b616359f31b1a7bea06`
- host: macOS aarch64
- budgets: 1,000,000,000 steps, 300,000 ms, 1 MiB output; memory default unchanged

## Product-journey waterlines

| journey | result | steps | host ops / bytes | start B | end B | allocated B | pages |
|---|---:|---:|---:|---:|---:|---:|---:|
| server-smoke | pass | 19,526,070 | 395 / 348,557 | 27,020 | 1,196,608 | 1,169,588 | 19 |
| wake-smoke | pass | 31,023,304 | 370 / 460,564 | 26,196 | 1,693,544 | 1,667,348 | 26 |
| workbench-smoke | pending Windows court | — | — | — | — | — | — |

Both completed journeys preserved their public STEP/EVIDENCE/PASS behavior.
The earlier server comparison measured 19,526,090 steps without the probe and
19,526,098 with it; the eight-step difference is far inside L5 but is not yet
the final same-source three-journey comparison.

## Reading

The result proves that page receipts had hidden useful byte-scale information:
server and wake allocate about 1.17 MiB and 1.67 MiB after their literal pools.
It does **not** yet prove those bytes form a recoverable suffix. These are
one-shot journeys, so every object becomes dead when the slot is destroyed;
counting that as a region win would answer the wrong question.

The next attribution slice must instrument allocations inside the call by
operation family and last-use class. L0 remains `not-decided` until at least
two real journeys show a closed, operation-boundary dead suffix of 25% or more.
