# Cut the Windows cell from ninety minutes

Agreed with the owner on 2026-09-21: do items 1 and 2 immediately after
v0.1.18 publishes. Not before — editing `candidate.yml` voids a Candidate in
flight and restarts its clock.

## The measurement that settles it

Candidate 35606274661, actual durations:

| cell | how | time |
| --- | --- | --- |
| `windows-aarch64` | cross-compiled on `ubuntu-24.04` | **6 min** |
| `linux-x86_64` | native | 6 min |
| `linux-aarch64` | native | 8 min |
| `macos-x86_64` | cross-compiled | 10 min |
| `macos-aarch64` | native | 13 min |
| `windows-x86_64` | native on `windows-2025` | **90 min budget** |

Producing Windows bytes is not slow: the aarch64 Windows artifact is
cross-compiled on Linux in six minutes. The x86_64 cell is slow because it
alone also carries the whole release quality gate.

## 1. Take the artifact build off the gate cell

Cross-compile `x86_64-pc-windows-msvc` on `ubuntu-24.04`, exactly as the
aarch64 Windows cell already does, and make the quality gate a separate
`windows-2025` job that consumes those bytes. The two then run in parallel
instead of in series.

## 2. sccache

About 36 of the cell's minutes are compilation, and all of it is cold: the
target cache key ends in `${{ inputs.source_sha }}` with no `restore-keys`, so
it can only hit when the same SHA is re-run. Cargo shares nothing between
profiles and the cell asks for three — `release` (opt-level "z", thin LTO,
codegen-units 1), `release-fast` for the upgrade fixture, and `test` — so one
664-package tree is compiled three times from scratch every round.

The exact-SHA key is deliberate: cross-SHA prefix restore twice produced a
Windows worker fast-fail (0xc0000409) before the first gate, so target and
incremental state were made exact-source-only. That hazard is real and it is
specifically a hazard of *incremental* state.

`sccache` is content-addressed — an entry is keyed by the compiler input — so
nothing stale can be resurrected the way a restored `target/` can. It answers
that hazard directly instead of answering it by giving up caching.

## Later, in this order

3. Move `unit-tests` and Clippy off the critical path as a parallel job whose
   receipt the Candidate consumes: ~16.7 minutes of wall clock, no less CPU.
   MiniCon's `candidate.yml` runs `cargo` zero times; that is the shape.
4. `cargo nextest` for the unit-test gate (~5 min).
5. `CARGO_PROFILE_TEST_DEBUG=0`.
6. Retire the preflight p95 benchmark from the release lane — it is performance
   observation, not release qualification.

Items 1, 2, 4, 5 and 6 change no assertion. Item 3 is a workflow restructure.
Do not lower `release`'s optimisation to save gate time: that profile is a
product decision about the bytes that ship, not a CI knob.

See `docs/candidate-gate-speed.md` for the per-gate breakdown.
