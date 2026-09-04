# Results

Status: **Phase B complete; L0 failed; no allocator change**.

## Exact source

- AgenTerm: `7860e04da0b9397e4d4fedb5abfaa66154c07bce`
- tinyvm / tinyvm-qjs: `1bf632bc423dd8d31469bbedc29928647af94295`
- host: macOS aarch64
- budgets: 1,000,000,000 steps, 300,000 ms, 1 MiB output; memory default unchanged

## Product-journey waterlines

| journey | result | steps | host ops / bytes | allocated B | parse B | stringify B | JSON gross share | proven dead suffix |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| server-smoke | pass | 19,524,216 | 395 / 348,492 | 1,169,456 | 452,020 | 210,984 | 56.69% | 0 B |
| wake-smoke | pass | 31,024,700 | 370 / 460,564 | 1,667,348 | 714,704 | 272,364 | 59.20% | 0 B |
| workbench-smoke | not required after L0 became impossible | — | — | — | — | — | — | — |

Both completed journeys preserved their public STEP/EVIDENCE/PASS behavior.
Their exact waterlines remained 27,020 → 1,196,476 bytes and 26,196 →
1,693,544 bytes, respectively.

## Reading

The result proves that page receipts had hidden useful byte-scale information:
server and wake allocate about 1.17 MiB and 1.67 MiB after their literal pools.
It does **not** yet prove those bytes form a recoverable suffix. These are
one-shot journeys, so every object becomes dead when the slot is destroyed;
counting that as a region win would answer the wrong question.

## L0 verdict

The gross counters found the right hot families, but the proposed recovery
boundary is structurally wrong. `JSON.stringify` creates its builder header and
buffers first, then `__jb_take` allocates the returned live string record at
the heap tail. `JSON.parse` allocates parser state first, then places the
returned object/array/string records after it. At either operation return, the
dead temporaries are a prefix below a live tail, so a bump rewind has a proven
safe suffix of **0 bytes** in both measured product journeys.

L0 required at least 25% in two of server/wake/workbench. Server and wake both
have zero; workbench alone cannot make two. The experiment therefore stops
before variant C. A future experiment may test a larger compiler-proven
last-use region around an immediately consumed host-call argument, but that is
a different recovery boundary and must receive its own frozen criteria.
