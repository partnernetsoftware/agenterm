# Where the Candidate's Windows cell spends its time

Measured on run 35545108236 (2026-09-21), the windows-x86_64 cell's whole
45-minute budget is one step, `Run release quality gate`:

| gate | time | what it is doing |
| --- | --- | --- |
| unit tests | 14.3 min | compiling test binaries, then running 23 specs |
| current artifact | 11.1 min | building the shipped artifact |
| alternate GUI fixture build | 10.6 min | building a second variant for the upgrade smoke |
| Clippy | 2.4 min | another full type-check |
| public MCP conformance | 2.0 min | `cargo test --test mcp_stdio` |
| local preflight p95 benchmark | 1.3 min | running preflight five times to measure p95 |
| the other 25 gates | ~3.5 min | scripts, lint, policy, smokes |

**About 36 of the 45 minutes is compilation.** The 25 gates that actually
exercise the product take minutes between them; `remote-ui-smoke` is 0.7 min.

For contrast, the same cell took 14.9 minutes on 2026-09-03. Nothing about the
pipeline changed: the workspace grew (664 packages in `Cargo.lock`, 17 members)
and the compile-bound gates grew 6-8x with it while the execution-bound ones
grew about 2x.

## Why the same code is compiled three times

Cargo shares nothing between profiles, and this cell asks for two:

```toml
[release]       opt-level = "z"  lto = "thin"  codegen-units = 1   strip = true
[release-fast]  inherits = "release"  lto = false  codegen-units = 16  incremental = true
```

`current artifact` builds `release`; `alternate GUI fixture build` builds
`release-fast`; `unit tests` builds the `test` profile. Three separate
artifact sets from one source tree.

`codegen-units = 1` is the expensive half of `release`: it gives up
intra-crate parallel codegen for better optimisation. Published measurements
put fat LTO + `codegen-units = 1` at 4m16s against thin LTO +
`codegen-units = 16` at 1m08s for the same crate — roughly 4x, and thin LTO is
reported to retain about 95% of fat LTO's benefit for half the compile time.
`release` here is already thin, so the remaining lever is the codegen units.

## The single largest miss: the target cache never hits

```yaml
key: cargo-target-v3-windows-x86_64-candidate-…-${{ inputs.source_sha }}
# and no restore-keys
```

The key ends with the exact Candidate SHA and there is no prefix fallback, so
it can only hit when the *same* SHA is re-run. **Every new Candidate therefore
compiles from cold**, which is most of the 36 minutes.

The workflow records why: cross-SHA prefix restore twice produced a Windows
worker fast-fail (0xc0000409) before the first gate, so target/incremental
state was deliberately made exact-source-only. That hazard is real, and it is
specifically a hazard of *incremental* state — a stale `target/` directory
carrying partial fingerprints from another commit.

## Plan, in the order the payoff justifies

1. **Give the compile-bound gates a warm cache without reintroducing stale
   incremental state: `sccache`.** It is content-addressed — a cache entry is
   keyed by the compiler input, so nothing stale can be resurrected the way a
   restored `target/` can. That addresses the 0xc0000409 hazard head-on rather
   than by giving up caching. Expected to remove most of the cold-build cost
   on every round after the first.

2. **Take `unit-tests` and `Clippy` off the Candidate's critical path.**
   Neither needs the sealed bytes; both can run as a parallel job whose
   receipt the Candidate consumes. This does not reduce total CPU, it removes
   ~16.7 minutes of *wall clock* from a serial cell. MiniCon's line already
   has this shape: its `candidate.yml` runs `cargo` zero times.

3. **Stop rebuilding what the cell already built.** `current artifact` and
   `alternate GUI fixture build` are 21.7 minutes of compilation after the
   cell's own build step. Consuming the produced bytes instead changes what
   those gates assert — from "this builds" to "these bytes are what we
   expect" — so it needs a new gate id and a fresh reverse-verified contract,
   not a quiet swap.

4. **`cargo nextest` for the unit-test gate.** Reported around 40% faster in
   CI (less on CI's fewer cores than the 3x quoted for developer machines).
   Against 14.3 minutes that is worth roughly 5 minutes, and it also gives
   per-test timing, which this repository currently has to infer.

5. **`CARGO_PROFILE_TEST_DEBUG = 0`.** Test binaries here do not need debug
   info in CI, and dropping it both compiles faster and makes the cache
   smaller.

6. **Retire the `preflight p95 benchmark` from the release lane.** Running
   preflight five times to measure p95 is performance observation, not release
   qualification. 1.3 minutes, and the cheapest item on this page.

Items 1, 4, 5 and 6 change no assertion and can land independently. Item 2 is
a workflow restructure. Item 3 changes what two gates prove and should be done
last, deliberately.

## What not to do

Do not lower `release`'s optimisation to save gate time. `current artifact`
builds the bytes that ship; its profile is a product decision about the
artifact, not a CI knob. The fixture build is the one that can be cheap.

Sources: [Tips for Faster Rust CI Builds](https://corrode.dev/blog/tips-for-faster-ci-builds/),
[The Rust Performance Book — Build Configuration](https://nnethercote.github.io/perf-book/build-configuration.html),
[Cargo Profile Settings](https://www.stanza.dev/courses/rust-performance/profiles/rust-perf-cargo-profiles).
