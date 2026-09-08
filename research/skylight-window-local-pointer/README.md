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

`probe_digest` is path-independent: it hashes a canonical stream of fixed
repo-relative source labels followed by each file's SHA-256. Moving an
identical clone therefore does not change the probe identity.
