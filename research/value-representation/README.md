# Value-representation decisive experiment

Which universal value representation should the `.qjs → .wasm` compiler use?

- **V1** two-word `(tag: i32, payload: i64)` — `src/repr_pair.rs`
- **V2** single-`f64` NaN-boxing — `src/repr_nanbox.rs`

Specification (authoritative): [`plan/design-value-representation-experiment.md`](../../plan/design-value-representation-experiment.md).
Findings, numbers and decision trace: [`RESULTS.md`](RESULTS.md).

**Status: done. Verdict V1.** Not a product; nothing here ships and nothing here
changes a PRD capability state.

## Shape

```text
source ─lex→ tokens ─parse→ AST ─emit→ wasm IR ─encode→ bytes ─→ tinyvm
                                    │
                                    └── Repr ── the only layer with two implementations
```

One implementation packaged twice. Everything except `src/repr.rs` +
`src/repr_pair.rs` + `src/repr_nanbox.rs` is shared byte-for-byte between the
two products, which is what makes "which one got more effort" a non-variable.

| path | what |
|---|---|
| `src/repr.rs` | the trait both representations implement, and the shared helpers |
| `src/repr_pair.rs` | V1 |
| `src/repr_nanbox.rs` | V2 |
| `src/lex.rs` `src/parse.rs` `src/ast.rs` | shared front end |
| `src/emit.rs` | shared lowering; knows nothing about tags or NaNs |
| `src/runtime.rs` | the guest runtime the compiler emits (operators, bump allocator, string helpers) |
| `src/ir.rs` `src/encode.rs` | wasm IR and hand-written encoder, with per-instruction provenance for the size tiers |
| `src/harness.rs` | tinyvm load gate, execution, metric readout |
| `corpus/` | the shared `.qjs` corpus and the shared expected-value table |
| `measure.sh` | produces all four builds and every criterion |

## Reproducing

Requires a **sibling checkout of `tinyvm`**: the crate has a path dependency on
`../../../tinyvm/crates/tinyvm`, so `tinyvm` and `agenterm` must live in the
same parent directory. Toolchain: Rust 1.97.0.

```sh
cd research/value-representation
./measure.sh              # every criterion, with measurement conditions
cargo test                # independent-validator cross-check
```

`measure.sh` exits non-zero if any product fails to load, fails to run, or
returns something other than its expected value.
