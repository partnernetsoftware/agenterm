# SkyLight window-local pointer probe

Research-only current-host probe for the precommitted experiment in
`plan/design-skylight-window-local-pointer-experiment.md`. It is not linked into
any AgenTerm product or release artifact.

The runner compiles two small Swift programs. `Fixture` owns all three windows:
target windows A and B live in one process, while a separate guard process owns
the foreground window. `Probe` resolves the private SkyLight symbols at runtime,
binds every action to an exact `CGWindowID` plus owner PID, and compares fixture
event counters and host state before and after each action.

From the repository root on macOS:

```sh
./research/skylight-window-local-pointer/run-current-host.sh
```

The probe never falls back to global posting, activation, AppleScript,
coordinate clicking, or physical cursor movement. Missing symbols, stale or
wrong-owner windows, invalid geometry, absent verification, and host-state drift
all fail closed. The private ABI is additionally pinned to the one measured OS
build and architecture; every other host returns `provider_unavailable` before
injection until its own controlled experiment deliberately extends the research
allowlist. Generated binaries stay under the ignored `.build/` directory.

The current runner first evaluates C1-C7. Only when that process exits green
does it launch a fresh fixture and run C8's 1,000 alternating actions. It emits
one JSON document for C1-C7 and one for C8. A current-host pass does not
establish C9 or authorize product migration.

## Chromium discriminator

The AppKit fixture did not distinguish public PID-targeted posting from the
private route. A separate runner owns the precommitted Chromium comparison in
section 9 of the experiment plan:

```sh
AGENTERM_CU_BROWSER_EXE=~/path/to/Chromium \
  ./research/skylight-window-local-pointer/run-chromium-current-host.sh
```

`AGENTERM_EXE` and `AGENTERM_CU_EXE` may name already-built binaries; by
default the runner uses their debug-profile locations. The runner compiles a
single-action native injector and the existing owned guard fixture, then runs a
qjswasm court with an invocation-owned temporary browser profile. It creates peer window A and
background target B, builds a fail-closed Chromium-window-to-`CGWindowID`
bijection, and compares 20 seeded triplets of private delivery, public delivery
at B's location, and public delivery away from B. Page counters are read back
independently through CDP.

The result is one JSON verdict with a repository SHA and path-independent
source digest. `PASS` means only that the private route discriminated on this
current host. `FAIL_NONDISTINGUISHING` is the useful negative result that the
public route was sufficient for this court. `FAIL_PRIVATE` rejects the private
route, while identity, dependency, cleanup or unstable outcomes remain
`INCONCLUSIVE`. This research runner registers no evidence and never changes
the capability ledger by itself.

The 1,000-action PRIVATE repeat keeps one parent-owned browser profile, two
window identities and one foreground guard for the whole run. Its parent court
executes consecutive 50-action blocks in fresh qjswasm top-level calls because
the engine's hard step ceiling is per call. Blocks cannot recreate or resolve
the fixture; they receive its frozen identities and repeat the same per-action
oracles. The parent requires contiguous coverage of all 1,000 indices, zero
failures and verified cleanup.

`probe_digest` is path-independent: it hashes a canonical stream of fixed
repo-relative source labels followed by each file's SHA-256. Moving an
identical clone therefore does not change the probe identity.
