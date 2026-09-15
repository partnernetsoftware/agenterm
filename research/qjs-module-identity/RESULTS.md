# QJS module resolution identity results

Specification: `plan/design-qjs-module-identity-experiment.md`.

## Conditions

- Source state: `a23c6dfb` for the observed baseline.
- Compiler dependency: repository lockfile pin for `tinyvm-qjs`.
- Execution: current-host public CLI.
- Interface evidence: repository-visible Rust call sites only; upstream source
  was not opened.

## Results

| Criterion | Variant A | Variant B | Evidence state | Result |
|---|---:|---:|---|---|
| Canonical reads and charges for one file under two specifiers | 2 | 1 | Structure inspection | C1 passes |
| Top-level marker emissions | 2 | 2 | A: current-host execution; B: identical compiler inputs, structure inference | C2 fails |
| Identity-bearing fields in the visible resolver callback | 0 | 0 | Exhaustive call-site inspection | C3 empty |
| Product-side module parser required | no | no | Source inspection | C4 passes for the tested variants |

Variant B is represented structurally by the existing canonical source cache in
`crates/agenterm-qjswasm/src/check_many.rs`: a second canonical hit returns the
cached source without another read or charge, but the compiler still receives
the same two specifier/source pairs as Variant A. The second evaluation count is
therefore a structural inference from byte-identical compiler inputs, not a
second runtime execution claim.

## Reproduction

From repository root, create a temporary project containing `lib/side.qjs`
whose top level prints `MODULE-EXECUTED`, then import it from one entry as both
`lib/side` and `lib/side.qjs`. Run the entry through:

```text
target/debug/agenterm cli script run <temporary-entry.qjs>
```

The output contains two marker lines. Independently inspect every
`compile_qjs_*_with_modules` wrapper in `crates/agenterm-qjswasm/src/lib.rs` and
the caller in `src/script_engine.rs`; the resolver type is consistently a
specifier-to-source callback with no identity field.

## Decision trace

C2 failed and C3 was empty, so the specification's branch 2 applies. Ship the
canonical read/budget ledger independently. Do not claim or approximate
canonical single evaluation.

## Deviations and honesty

- No new prototype was needed: `check-many` is the cached variant and the
  single-file public CLI is the uncached behavioral baseline.
- Exact read counts come from resolver structure. Variant A's evaluation count
  comes from public marker output; Variant B's count is a structure inference
  from unchanged compiler inputs and is labelled as such above.
- The upstream repository was deliberately not inspected, so the result says
  the identity API is absent from this repository's visible interface, not that
  no such upstream API can exist.
- Measurement criteria were not changed to favor either result.
