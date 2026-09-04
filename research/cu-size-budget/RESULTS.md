# Results

Status: **A/B measured · Fat LTO rejected at S1 · Variant C next**.

## Measurement conditions

- Source: clean exact `b7ba020ba898f3ef2b871c274fbe2d055bbe8702`.
- Compiler: Rust 1.97.0, Windows x86_64 MSVC target through `cargo-xwin`.
- Boundary: L1 whole stripped `agenterm-cu.exe`; OS seam and cold metadata are
  inside, required `agenterm.dll` is outside. L2 and L3 are not measured yet.
- Tool: `wc -c` for the flat file and `objdump -h` for PE sections.
- Execution: byte measurement only. The matching feature already has three-OS
  runtime evidence, but these exact Thin/Fat files were not executed.
- Build: `opt-level=z`, `codegen-units=1`, `panic=abort`, `strip=true`; the only
  changed axis is Thin versus Fat LTO.
- Rebuild time: dependencies retained; `cargo clean -p agenterm-cu --release
  --target x86_64-pc-windows-msvc` forces the owning package through codegen and
  link before timing.

## Exact results

| variant | file bytes | delta from 2 MiB | `.text` | `.rdata` | `.pdata` | package rebuild | SHA-256 | state |
|---|---:|---:|---:|---:|---:|---:|---|---|
| A Thin | 2,376,704 | +279,552 | 1,890,502 | 404,980 | 66,708 | 18 s | `26fb9c323154bbc0b300e0ab29ce44f4c9c4b20e878b81d2e3979d3cfc4031ab` | byte-only |
| B Fat | 2,268,672 | +171,520 | 1,843,718 | 376,804 | 35,304 | 21 s | `88dc61fab27e390c0e5aee7275d6df13261f76c9899a74037c54e7512b455442` | byte-only |

Fat saves 108,032 bytes (4.5% of A), but still misses S1. Its 21-second package
rebuild is below 60 seconds and 16.7% slower than A, so S4 passes. Per the
precommitted order, S1 kills B before a behavior rerun; the court advances to
Variant C. L3 and the 0/16/32-row slope are intentionally still unmeasured.

## Decision trace

1. Reproduce A from exact clean source.
2. Change only LTO mode for B.
3. S1 fails by 171,520 bytes; stop B.
4. Do not accept the real latency pass as a substitute for size.
5. Enter Variant C; do not move bytes into the ABI or raise the ceiling.

## Reproduce

From repository root:

```bash
research/cu-size-budget/measure.sh thin
research/cu-size-budget/measure.sh fat
```

No measurement rule was changed to improve the result. The useful surprise is
that Fat LTO removes about half of `.pdata` yet only 4.5% of the whole file;
the remaining miss requires a structural source/data result, not more linker
flag search.
