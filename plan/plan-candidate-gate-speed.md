# Cut the Windows cell from ninety minutes

## 2026-09-21 measurement and recovery state

- `sccache` is wired in. Candidate `35626359893` on source `905e583ac`
  reported 1,587 hits, 117 misses, and a 93.13% hit rate in the Windows
  x86_64 job. The release quality step ran 35m39s, then failed in
  `powershell-migration-audit` with a qjswasm memory-access-out-of-bounds trap.
  All other five build cells passed; runtime and aggregate were skipped. The
  cache measurement is valid, but this is not a successful Candidate or a
  complete Windows wall-clock comparison.
- Failed-job rerun of Candidate `35619273184` did not repair its macOS x86_64
  runtime failure: attempt 2 requested `candidate-runtime-control-<run>-2`,
  while preflight was not rerun and had uploaded the control for attempt 1.
  Resolve this artifact identity before relying on failed-job reruns.
- Keep the quality/artifact job split below behind a trustworthy gate; do not
  count a skipped aggregate as speed evidence.

### G0 follow-up, 2026-09-22

- Rerun artifact identity is repaired in `ebcfb5ef1`: preflight
  exposes the artifact name it actually uploaded, and runtime consumes that
  job output. The owning policy suite reports 115 passing tests, including a
  negative fixture for the old attempt-derived consumer name; pre-push exited
  0. The commit is on `main`; it has not been qualified by a new Candidate.
- The audit script is not established as the trap root cause. An isolated
  macOS/Windows ARM64 guest campaign saw 0 traps in 20 runs, while a scan of
  60 recent Candidates found three guest OOB traps, all on native Windows
  x86_64, including two in the outer `check.qjs` rather than the nested audit.
  This narrows the investigation to qjswasm/tinyvm and host-input boundaries;
  it does not yet prove a mechanism or a fix.
- `b10585adc` adds bounded trap context: the last billed host door, its parked
  answer length and resource counts. It preserves the failure class and exit
  behavior. The door is a diagnostic lead, not proof of where the guest trapped
  or why. The native Windows x86_64 court is `BLOCKED` because its guest agent
  did not become ready within the court budget; Windows ARM64 Prism stress ran
  150 audit children with 14,988 tree/state samples and no trap, but does not
  qualify native x86_64.
- Next evidence: a bounded native Windows x86_64 stress reproduction with a
  positive attachment check; safe diagnostic context at the trap boundary;
  then a causal fix and negative control. Do not dispatch another Candidate
  merely to sample an intermittent failure.

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
