# Results

Status: **court complete · C retained for bounded growth · D rejected · 2 MiB remains red**.

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
  Variant C.

## Variant C — hot route + compressed cold catalog

- Source: clean exact `a345e436139cbc92b0dde34fe1e57c1ead1a9bbe`.
- Change: `name + aliases + parser family` stay in the generated hot table;
  complete help, argument and JSON metadata live in one validated immutable
  zlib stream and are decoded only by help/catalog commands.
- Behavior: all 56 binary unit tests, all-target Clippy and generated verb
  documentation parity passed before the exact measurements.
- Build: Fat LTO, otherwise the same Release settings and toolchain as A/B.

| cold rows | file bytes | delta from 2 MiB | `.text` | `.rdata` | `.pdata` | rebuild | SHA-256 |
|---:|---:|---:|---:|---:|---:|---:|---|
| 0 | 2,221,056 | +123,904 | 1,850,694 | 325,108 | 35,472 | 22 s | `471d617c6d07240641e6e00a2a9c9b763cdd7e21d6af8786bf87fa64bd664006` |
| 16 | 2,222,592 | +125,440 | 1,850,694 | 326,388 | 35,472 | 20 s | `7280c66f602c5ae37b69988cbb40a6dd06ab5ffeee9a9572c251cc47029b2e71` |
| 32 | 2,223,104 | +125,952 | 1,850,694 | 327,028 | 35,472 | 20 s | `09742eda84b01acba9f5c3f39de400d945d2bb6f05c96e22e3d24d2796ca8c34` |

C saves 47,616 bytes versus B. S3 passes decisively: 0→16 is 96 bytes per
row, 0→32 averages 64 bytes per row, both below the 256-byte ceiling. S4 also
passes. S1 still fails by 123,904 bytes, so C is retained as the structurally
bounded catalog implementation but cannot close the court. The precommitted
tree now permits D to inspect only genuinely reusable ABI mechanism; moving
CLI wording or metadata across the DLL seam remains forbidden.

## Decision trace

1. Reproduce A from exact clean source.
2. Change only LTO mode for B.
3. S1 fails by 171,520 bytes; stop B.
4. Do not accept the real latency pass as a substitute for size.
5. C passes S3 and S4 but fails S1 by 123,904 bytes.
6. Retain C's bounded growth, enter D, and do not move CLI wording into the ABI
   or raise the ceiling.

## Reproduce

From repository root:

```bash
research/cu-size-budget/measure.sh thin
research/cu-size-budget/measure.sh fat
research/cu-size-budget/measure.sh cold0
research/cu-size-budget/measure.sh cold16
research/cu-size-budget/measure.sh cold32
```

No measurement rule was changed to improve the result. The useful surprise is
that Fat LTO removes about half of `.pdata` yet only 4.5% of the whole file;
the remaining miss requires a structural source/data result, not more linker
flag search.

## Variant D — resolver ABI relocation rejected and rolled back

A Darwin Fat-LTO symbol profile suggested about 130.8 KiB in the standard DNS
resolver path. That was explicitly treated as direction-only evidence. ABI
1.27 was prototyped as a neutral two-stage resolver while CU retained the
owned child, validation, TCP attempts and product JSON.

| source | CU bytes | delta from C0 | delta from 2 MiB | verdict |
|---|---:|---:|---:|---|
| `1f0b92cf` | 2,220,032 | −1,024 | +122,880 | S1 fail |
| `39bc8abf` (numeric fixture bind) | 2,219,520 | −1,536 | +122,368 | S1 fail |

Because D still failed S1 by more than 122 KiB, the court stopped before an L3
claim and reverted the ABI/platform expansion in `f5f93f4e`. The independent
numeric loopback fixture remains, but the exact post-rollback C binary is again
2,221,056 bytes. No threshold changed and no unproven Darwin attribution was
promoted into product architecture.

The no-raise tranche is complete: retain C's 64–96 B/row cold-catalog slope,
keep the 2 MiB release gate red, and return the 123,904-byte gap as an explicit
product-budget decision rather than continuing unbounded micro-optimization.

Post-rollback Variant C compiled at exact `4d524426` for all six delivery
targets: OSX arm64/x86_64 with Cargo, Linux arm64/x86_64 with cargo-zigbuild,
and Windows arm64/x86_64 with cargo-xwin. This is compile evidence, not a claim
that those six exact artifacts all ran.
