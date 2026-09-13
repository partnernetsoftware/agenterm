# dyn typed-symbol folding results

Status: **decided — B killed at V0; C wins over A and is integrated**.

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

## Arm C: platform-private systemd loader

Arm C centralizes only the fixed `libsystemd.so.0` load, lookup, close and RAII
mechanism in `adapters/linux/systemd_library.rs`. Both consumers continue to own
their exact symbol literals, function-pointer typedefs, product errors and native
result ownership. No dyn, Cargo dependency, public API, cache or policy changed.

Production NCLOC used one meter for both arms:

```sh
awk '/^#\[cfg\(test\)\]/{exit} NF && $0 !~ /^[[:space:]]*\/\//{n++} END{print n+0}' FILE
```

`systemd_library.rs` required separate exclusion of its small leading and trailing
`cfg(test)` instrumentation; its production count is 36. Tests and documentation
are excluded throughout.

| production boundary | A retain | C shared seam | delta |
|---|---:|---:|---:|
| `login_session.rs` | 361 | 336 | -25 |
| shared module registration + loader after consumer 1 | 0 | 42 | +42 |
| first-consumer cumulative | 361 | 378 | **+17** |
| `current_target_binding.rs` | 451 | 426 | **-25 marginal** |
| two-consumer total | 812 | 804 | **-8** |

The first consumer alone would not justify the abstraction. The second real
consumer crosses the precommitted S1 gate. Direct loader/lookup/rollback/Drop
implementations fall from two to one; the two hand-written partial-construction
`dlclose` branches disappear because the local RAII owner drops on `?`.

### Behavior and mutation evidence

- Six cells passed `cargo check -p agenterm-platform --features
  current-target-binding --target <triplet>`; Linux used `cargo-zigbuild check`
  with glibc 2.28 and Windows used `cargo xwin check`. Three existing
  `current_target_binding.rs` dead-code warnings appeared identically in every
  cell and are outside this diff.
- Linux aarch64 native execution in the existing Linux guest passed 180/180
  feature-isolated tests. The real
  `native_inventory_proves_a_session_or_fails_closed` court passed.
- Both adapter-specific missing-symbol tests passed and retained their exact
  product kind/code/message. Each also observes one close after the failed
  lookup scope.
- Removing the shared `Drop` body made both tests fail with `left: 0, right: 1`.
- Mutating `login-session-provider-incomplete` made the binding test fail with
  the mutated code named.
- Adding a second `dlsym` declaration to the second consumer made the source
  inventory gate list both files instead of only `systemd_library.rs`.
- Every mutation was restored before the final 180/180 run.

Feature-isolated `login-session` without `current-target-binding` exposed a
pre-existing missing `dep:libc` feature edge. It reproduces on the parent source
and is not claimed as a regression or fixed in this experiment. The owning
`current-target-binding` feature includes libc through its existing dependencies
and is the like-for-like two-consumer court used here.

### Delivery footprint

Same-meter Linux x86_64 release outputs:

| boundary | tool/build/target/execution | A | C | delta |
|---|---|---:|---:|---:|
| L1 mechanism | whole-file identity implication; mechanism symbols not separately retained | no distinguishable bytes | no distinguishable bytes | 0 |
| L2 mechanism + Linux seam | whole-file identity implication; `cargo-zigbuild` release | no distinguishable bytes | no distinguishable bytes | 0 |
| L3 delivered `agenterm` | `llvm-size`, release, x86_64 Linux, build-only | text 9,075,521; data 347,560; bss 13,616 | same | 0 |
| L3 flat file | `stat`, release, x86_64 Linux, build-only | 9,426,208 | 9,426,208 | 0 |

Both complete files had SHA-256
`4f6895417149ffd413306a867042e822654a81dade01524c852387241407b0b5`.
Because the complete binaries are byte-identical, there is no sub-boundary byte
delta to attribute; L1/L2 absolute ownership remains not separately measurable.
The Linux aarch64 test binary, not either measured x86_64 release executable, was
the artifact actually run.

## Final decision trace

```text
B prototype question
└── V0 structural lifetime/raw-escape gate: FAIL
    └── kill B; agenterm-dyn remains unchanged

C platform-local seam
├── V1/D0/V2: PASS
├── S1: +17 first consumer, -25 second consumer, -8 total
├── S2: two loader implementations become one
└── F0: byte-identical Linux x86_64 release executable
    └── C wins; integrate C and retain dyn unchanged
```

## Partial criterion table

| criterion | A retain | B dyn seam | C platform seam |
|---|---:|---:|---:|
| V0 lifetime/raw safety | current conventional ownership | **FAIL** | PASS — crate-private seam only |
| V1 single mechanism | two local implementations | not run after V0 | PASS — one implementation |
| V2 two-consumer behavior | current baseline | not run | PASS — 180/180 Linux runtime |
| D0 dependency isolation | current baseline | not run | PASS — no dependency change, six cells |
| first / second / total NCLOC | 0 / 0 / 812 | not measured after kill | +17 / -25 / 804 |
| loader implementations | 2 | not measured after kill | 1 |
| L1/L2/L3 byte delta | baseline | not measured after kill | 0 / 0 / 0; whole file identical |

## Deviations and honest limits

- The specification proposed a compile-fail lifetime witness for B; static type
  analysis proved the witness necessarily compiles for every permitted generic
  callable exposure, so no knowingly invalid product prototype was added.
- C does not claim to attach a Rust lifetime to copied C function-pointer bits.
  It narrows the conventional proof to one private module and gives no public or
  crate-level raw/callable extraction API. The adapters retain the owner field.
- The L1/L2 byte rows use complete-file identity rather than separately retained
  mechanism symbols. They prove zero delta, not an absolute mechanism size.
- Full all-feature Linux Clippy is red on existing unrelated platform lints. The
  candidate introduced no diagnostic in feature-isolated checks; this experiment
  does not claim the existing full-crate lint debt is green.
- No metric or threshold was changed after observing the results.
