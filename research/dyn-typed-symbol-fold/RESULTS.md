# dyn typed-symbol folding results

Status: **in progress — B killed at V0; A/C not yet decided**.

## Arm B: dyn lifetime-bound typed-symbol seam

Verdict: **KILLED** before implementation, exactly at specification criterion
V0. The joint requirements “generic over arbitrary typed C function signatures”,
“callable”, and “ordinary Rust cannot copy the callable beyond the library
lifetime” cannot be met by the allowed stable-Rust API shapes.

### Deciding witness

For `F = unsafe extern "C" fn() -> i32`, returning a
`libloading::Symbol<'lib, F>` does not retain the lifetime after dereference:
`F: Copy`, so `let copied: F = *symbol` produces a value with no `'lib` in its
type. A closure API has the same failure because the closure can return or store
`*symbol`; a higher-ranked closure lifetime does not become part of `F`.

A private non-`Copy` wrapper could hide the value, but stable Rust cannot give it
a generic call operator for arbitrary `extern "C" fn(A...) -> R`. It must either:

- expose `T`, `&T`, `Symbol<T>`, a raw address or an extraction method, reopening
  the copy escape;
- enumerate arities/signatures with sealed implementations or macros, creating a
  second ABI-shape family; or
- accept runtime values and dispatch through `invoke_abi`, which the experiment
  excluded because these platform adapters already possess exact compile-time C
  prototypes.

Each alternative triggers the specification's §1.2, §1.3 or §1.10 kill rule.
No dyn source was changed and no favorable footprint/LOC measurement was taken
after the safety failure.

The decisive compile-time witness for any future reopening must attempt the
actual copy, not merely return the wrapper:

```rust,ignore
type F = unsafe extern "C" fn() -> i32;

fn leak(handle: agenterm_dyn::LibraryHandle) -> F {
    let symbol = unsafe { handle.typed_symbol::<F>(b"getpid\0") }.unwrap();
    let copied: F = *symbol;
    drop(symbol);
    drop(handle);
    copied
}
```

An equivalent `with_symbol(..., |symbol| *symbol)` witness must also fail. If
either form compiles, the lifetime gate remains failed.

## Current decision trace

```text
B prototype question
└── V0 structural lifetime/raw-escape gate: FAIL
    └── kill B; agenterm-dyn remains unchanged

C platform-local seam: NOT STARTED
└── no final A-versus-C verdict yet
```

## Partial criterion table

| criterion | A retain | B dyn seam | C platform seam |
|---|---:|---:|---:|
| V0 lifetime/raw safety | current conventional ownership | **FAIL** | not run |
| V1 single mechanism | two local implementations | not run after V0 | not run |
| V2 two-consumer behavior | current baseline | not run | not run |
| D0 dependency isolation | current baseline | not run | not run |
| shared/marginal/total NCLOC | not measured | not measured after kill | not measured |
| independent unsafe sites | not measured | not measured after kill | not measured |
| L1/L2/L3 bytes | not measured | not measured after kill | not measured |

## Reproduction discipline for arm C

The final result must add exact baseline and candidate source identities, the
NCLOC meter and commands, feature-isolated and six-cell compile commands, Linux
runtime owner, negative mutations, deviations, and the full §4 decision-tree
trace. Until then this file is not a final architectural verdict.
