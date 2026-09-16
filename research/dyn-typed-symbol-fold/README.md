# dyn typed-symbol folding experiment

Status: **decided**. Arm B was killed at the precommitted V0 safety gate; arm C
beat A after the second real consumer and was integrated.

Archived specification: `plan/archive/design-dyn-typed-symbol-fold-experiment.md`.

The experiment compares the current two adapter-local `libsystemd` loaders with
one platform-local shared seam. It does not add a dyn API, change a product
capability, or introduce a symbol policy.

## Identity

| item | value |
|---|---|
| specification commit | `a773e50db0cfba03d812a9894d3a5534b1da8393` |
| arm B audit | static Rust API/lifetime analysis; no prototype compiled |
| arm C | two consumers integrated; six-cell compile and Linux aarch64 runtime passed |

## Decision

Keep dyn unchanged. `agenterm-platform` now has one crate-private systemd loader;
both adapters retain their symbol/prototype and product-error ownership. The
first consumer pays a 17-NCLOC shared-mechanism intercept, the second deletes 25
NCLOC, so the two-consumer result is net -8 production NCLOC. Baseline and
candidate Linux x86_64 release executables were byte-identical.
