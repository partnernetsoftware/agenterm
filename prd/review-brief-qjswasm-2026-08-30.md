# Review brief: the `.qjs` + `.wasm` script line (2026-08-30)

For a reviewer: this is the structure, where the evidence is, and the questions.
Write findings to `prd/review-qjswasm-2026-08-30-grok.md` (create it).

## The structure (three layers, dependencies only point down)

| layer | crate | depends on | size |
|---|---|---|---|
| core | `tinyvm` (`~/repos/tinyvm/crates/tinyvm`) | `libm` only (everything else optional) | MVP wasm interpreter, no JIT |
| compiler | `tinyvm-qjs` (`~/repos/tinyvm/crates/tinyvm-qjs`) | `tinyvm` only | 24 405 lines: lex → parse → AST → lower to V1 boxed values (tag:i32 / payload:i64) → runtime prelude (bump heap, object/array records, per-method prefabs, gated: unused code is not emitted) → encode `.wasm` |
| product engine | `agenterm-qjswasm` (`~/repos/agenterm/crates/agenterm-qjswasm`) | tinyvm + tinyvm-qjs (git pin `e8f1686`), serde/serde_json, sha2, png, agenterm-platform (window/process features), agenterm-script-common | the `tool.*` door (42 host ops: fs/process/env/crypto/time/image/window), slots, budgets, fault legibility |

Above it: `agenterm` CLI (`script run/check/task run`, worker process, `--profile tool`, `--max-operations`), 71 `.qjs` scripts in `~/repos/agenterm/scripts/qjs` (all ported from the retired rh language; 7 of 10 GUI journeys PASS on macOS; the quick qualification lane `bootstrap.sh --quick` passes 8/8 gates).

## Where the evidence is

- `~/repos/tinyvm/prd/PRD.md` — capability tree (268 [x] / 36 [~] / 26 [ ] / 14 [–]), todo list A1–A10, 验证口径 (three commands), memory palace.
- `~/repos/agenterm/prd/PRD_02_36_agenterm_qjswasm.md` — product-side tree (190/10/15/9), todo A/B/C, journey scoreboard, version chain of pins, self-check.
- `~/repos/tinyvm/plan/design-*.md` — one note per engine decision (value representation experiment, JSON fast paths, concat/length/indexOf window skips, closures, unwind channel…).
- Tests: `cargo test -p tinyvm-qjs` (1048/0), `cargo test -p tinyvm` (315/0), `cargo test -p agenterm-qjswasm` (197/0). Byte-size pins on `"return 1;"` etc. lock the zero-cost gating.

## Questions for the review (be adversarial; numbers over opinions)

1. **Layering**: is anything in `tinyvm-qjs` actually product vocabulary in disguise? Is the door (`tool.rs`) carrying logic that belongs in the platform crate or the compiler?
2. **Value representation**: V1 boxed pairs (tag i32 / payload i64) were chosen over NaN-boxing by an experiment (`plan/design-value-representation-experiment.md` if present, else the PRD's 机制 section). Given today's per-character prices (concat ~2, `.length` ~3, `includes` ~7, `split` ~26, `toLowerCase` ~38, JSON.parse ~29/byte, JSON.stringify ~700/property), where is the next 10× and is it the representation, the interpreter's step model, or the prelude?
3. **Memory**: bump heap, no GC, 64 MiB cap per invocation. Long journeys survive only because invocations are short. What is the smallest honest GC (or arena/region reset) for this design, and what does it cost the zero-cost-gating discipline?
4. **Budget model**: steps = wasm instructions; default 128M, cap 1G; wall-clock timeout beside it. Is this the right unit for an agent-facing script engine, or should the budget be host-op-aware?
5. **Legibility**: faults are named (heap, throw, capability, host-argument, property-of-non-object…) except "call of a non-function" (A10). What else can trap unnamed?
6. **Door surface**: 42 ops, JSON in/out, two-copy result parking. Is the door the right abstraction for the next capabilities (network is deliberately absent; clipboard absent), or should it become a typed ABI?
7. **What is missing from the tree that a script engine for agents needs and we have not listed?**

Constraints on the review: do not edit any source; read only. Cite file paths and line numbers. Keep it under ~200 lines.
