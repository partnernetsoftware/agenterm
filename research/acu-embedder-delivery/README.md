# ACU embedder delivery prototype

This directory contains the measurement-only B-route prototype from
`plan/design-acu-embedder-delivery-experiment.md`.

- `provider` is a `cdylib` that calls the existing
  `agenterm_cu::embedder::execute_json_from_environment`; it owns no ACU schema.
- `loader-probe` loads that provider once, checks ABI version 1, performs
  bounded opaque request/reply calls, and emits typed boundary errors.
- `empty-probe` gives the same-profile standalone executable intercept needed
  to attribute the loader seam. It is not the product main-PE baseline.

Build from repository root with a repo-local isolated target:

```bash
CARGO_TARGET_DIR=target/acu-embedder-delivery cargo build \
  --manifest-path research/acu-embedder-delivery/Cargo.toml --release
```

Windows byte-only builds use the same manifest and
`--target x86_64-pc-windows-msvc` through `cargo xwin build`. Exact commands and
results are recorded in `RESULTS.md`.
