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
  or why. Windows ARM64 Prism stress ran 150 audit children with 14,988
  tree/state samples and no trap, but does not qualify native x86_64.
- The Windows x86_64 court (an x86_64 guest emulated by QEMU TCG on the
  ARM64 operator Mac, not native x86_64 hardware) was blocked by a `planned`
  registry state dated 2026-09-07; its historical Guest Agent timeout was not a
  failed boot on 2026-09-22. Operator utm-court commit `e13ded4` selected the installed VM
  by UUID. A supported lease, 120-second wait-ready, command probe and release
  completed on 2026-09-22, leaving the VM stopped. The court is now available
  for a bounded emulated-x86_64 reproduction; this does not repair the trap.
- The bounded heap-limit check below found no trap. Stop exploratory stress
  sampling and return to the v0.1.19 product and release path. Candidate is
  dispatched for qualification after local validation, never merely to sample
  an intermittent failure. A repeated trap still needs a causal local repair
  with a negative control before another qualification run.

### G0 T1 bounded reproduction, 2026-09-22 (closed)

Every run below used the `win-x86_64-desktop` court: an x86_64 Windows guest
emulated by QEMU TCG on the ARM64 host. The effective QEMU arguments were
`-accel tcg` without `thread=multi`, so both vCPUs execute serially. None of it
is native x86_64 hardware evidence, and none of it qualifies a Candidate.

- Trap context on `main` now also names the guest function: tinyvm `f476cd2`
  records `Instance::last_trap_site()` at the call boundary on failure only,
  and `7182894b8` prints it as `fn#N` ahead of the last billed door and the
  bill. `op#` was dropped: wrapping the interpreter loop to read the program
  counter cost 1-8% ns/instruction on `interpreter_throughput`.
- The driver copies the outer shape of `check.qjs` `execute()`: spawn one
  child, poll `process_tree` and `process_state` every 1000 ms until it
  exits, repeat. It records numbers only. The guest ran `agenterm.exe` built
  with `cargo xwin --release` from `6283e6ccc` (`dbcd8463f` for the first
  probe). The guest-side `certutil` SHA-256 matched the host build in the
  probes and segments 3 and 6:
  `eb4683a00380f7642ea4a279f52e00ff6e6c13ce0af96ecb9a41df1781999b62`
  (`4056ffa8...` for the `dbcd8463f` probe).

| Run | Shape | Result | Valid |
| --- | --- | --- | --- |
| probe (dbcd8463f) | `ping -n 3`, 1 min | 9 children, 43 tree/state samples, rc 0, `arch=AMD64` | yes, positive attachment only |
| long run, 14:28 | 3-level `cmd`/`ping`, planned 40 min | host disk ran low; guest agent returned `Timed out waiting for RPC`; output never retrieved | **no** |
| segment 1 | - | the host exe had been deleted while freeing disk; nothing ran | **no** |
| segment 2 | 3-level tree | driver died after 1 child: the host watchdog's `pull` of the progress file collided with the driver's append (`os error 32`); fixed by bounded retries and a skipped-line counter | **no** |
| segment 3 | 3-level tree, 10 min | 18 children, 520 samples, `tree_bytes_max` 233, 0 retries, rc 0, no trap | yes |
| segment 4 | 12 background pings, 1 min | 2 children, 54 samples, `tree_bytes_max` 873, rc 0 | yes, positive probe |
| segment 5 | 12 background pings, 10 min | 13 children, 345 samples, `tree_bytes_max` 10,653 (children 11-13; 873 before), mean 1,544 bytes/sample, rc 0, no trap; guest SHA pull failed, host SHA logged | yes; the 10,653-byte jump is unexplained |
| segment 6 | 12 background pings, 2 min | 3 children, 70 samples; every child `ping_max` 13, `root_bad` 0; max 1,168 bytes at 20 processes, then 15 processes and 873 bytes; rc 0 | yes, diagnostic |

- Court and disk incident: after the long run the VM stayed `stopping`; its
  QEMU process was still alive. `utmctl stop --kill` on the exact UUID stopped
  it, and a read-only `qemu-img check` found no errors. Segments 1-6 each
  checked host free space against a 20 GiB floor, used
  `lease --disposable`, and were released; free space stayed at 34-35 GiB.
- Diagnostic fields added in segment 6, numbers only: per child `n_max`,
  `ping_max` (design ceiling 13), `root_bad` (a non-empty tree whose root is
  not a child of the driver, meaning a reused PID), `bytes_max_child`, and the
  process and ping counts at that maximum.
- The CI history cannot be matched sample for sample. `timing.json` holds gate
  durations only, and no log records `process_tree` lengths. At one poll per
  second, the gates before `migration-audit` in `35626359893` imply about
  2,050 polls. Segments 3-6 total 989 samples with no trap. That is evidence
  of non-reproduction at this scale, not of absence.
- Stopped by the owner: no more T1/T2 stress sampling, and the one-off
  10,653-byte jump is not pursued.

### G0 A controlled heap-limit check, 2026-09-22 (closed)

On macOS ARM64 at `cf9c8dc50`, a debug build ran the migration audit and an
outer `check.qjs`-shaped spawn/poll driver with `AGENTERM_QJS_MAX_MEMORY_PAGES`
set to 57 values, including the failure boundaries. The unset-variable
controls passed. Low limits produced only `Budget(max_memory_pages)` failures:
the audit failed through 109 pages and passed from 110; the driver failed
through 22 and passed from 23. There were zero guest OOB traps. The driver
corrected an initial exit-code capture error and repeated the affected cases.
Evidence is in `scratchpad/evidence/g0-a/` (local, not a release artifact).

This excludes the tested path from a deliberately low page limit through
`memory.grow` failure to an OOB trap. It does not exclude other heap or
host-input-dependent address calculations. A qualification run containing
`7182894b8` would supply `fn#N` and the last billed door if the trap recurs;
these probes alone cannot declare that risk resolved.

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
