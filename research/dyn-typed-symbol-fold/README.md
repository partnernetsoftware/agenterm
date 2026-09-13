# dyn typed-symbol folding experiment

Status: **in progress**. Arm B was killed at the precommitted V0 safety gate;
arms A and C remain to be measured.

Specification: `plan/design-dyn-typed-symbol-fold-experiment.md`.

The experiment compares the current two adapter-local `libsystemd` loaders with
one platform-local shared seam. It does not add a dyn API, change a product
capability, or introduce a symbol policy.

## Identity

| item | value |
|---|---|
| specification commit | `a773e50db0cfba03d812a9894d3a5534b1da8393` |
| arm B audit | static Rust API/lifetime analysis; no prototype compiled |
| arm C | not started |

## Next boundary

Implement arm C only far enough to migrate both existing Linux systemd adapters,
then measure the first-consumer intercept and second-consumer marginal NCLOC and
unsafe-site change. If C fails a hard gate or is not net subtractive by consumer
2, retain arm A.
