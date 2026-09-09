# SkyLight exact-window drag oracle

This directory contains the owned Chromium page oracle precommitted by
section 10 of `plan/design-skylight-window-local-pointer-experiment.md` for
`acu.dynamic.004`.

It is **research only**. The runner, injector and parent court implement the
frozen G1-G8 current-host discriminator. The corrected run recorded in
`../RESULTS.md` returned `FAIL_PRIVATE` on its first PRIVATE down because the
target application's AX main/focused window changed. The result must not be
registered as public evidence or used to change the capability ledger. The
parent court owns the disposable Chromium profile, peer windows,
foreground guard, exact native identities, injection process and cleanup.
Section 10.5 forbids rerunning that measured protocol without a new explicit
precommitment. Section 10.6 is that new precommitment. This directory now
implements its lease-only dry cycle, G6a/G6b receipts and one-way decision tree,
but it must not run until the integrated source has passed the section 10.6
static review and been frozen at one reachable commit.

`page.html` has no external resources. The parent court loads it twice as:

```text
file://.../page.html?label=A&nonce=<fixture-nonce>
file://.../page.html?label=B&nonce=<fixture-nonce>
```

After that implementation review, an authorized owned-fixture run from the
repository root uses:

```sh
AGENTERM_CU_BROWSER_EXE=~/path/to/Chromium \
  ./research/skylight-window-local-pointer/run-drag-current-host.sh
```

The runner refuses dirty research sources. It records
`ACU004_DRY_ATTEMPT` (1..6), `ACU004_REPAIR_COUNT` (0..2), and
`ACU004_RUN_NUMBER` (1..2), whose defaults are 1, 0, and 1. A second run or a
dependency repair must set the corresponding values explicitly; these counters
do not authorize another run beyond the section 10.6 time box.

The page exposes `window.__agentermDrag` for independent CDP read-back:

- `reset(trialNonce)` clears the bounded pointer/focus logs and business-effect
  state;
- `calibrate()` returns screen origin, outer/inner dimensions, device-pixel
  ratio, the owned stage rectangle, stable start/inside-end and viewport-edge
  markers, plus a point explicitly outside the right viewport edge;
- `snapshot()` returns the trial nonce, current focus fact, bounded focus
  transitions, ordered event records,
  stable target ids and ancestry, down/up nearest common ancestor, click target,
  final point and token movement/completion facts.

Capture listeners observe `mousedown`, `mousemove`, `mouseup`, `click`, `focus`
and `blur`.
Each record includes a monotonic sequence, `button`, `buttons`, `clientX`,
`clientY`, screen coordinates, native browser timestamp, stable target id and
stable ancestor-id path. The page does not synthesize input, choose a provider
or declare a trial successful. The parent court remains responsible for
applying the frozen G1–G10
criteria, including exact down/held-move/up ordering, trajectory-specific click
rules, peer isolation, the dry-cycle-frozen A/B page focus phases, G6a/G6b
preservation and release-before-focus-restore closure.

The inside path begins at `#drag-start` and ends at `#drag-inside-end`. The
inside-to-outside path begins at the same marker, crosses
`#outside-exit-edge`, and ends at the returned point to the right of the
viewport. These are calibration facts only: the parent court must reconcile
them with CDP, exact `CGWindowID` and AX geometry before the first down.

Do not add global-event fallback, foreground activation, physical-pointer
movement, user-profile discovery or product linkage to this directory. Any
failure or uncertainty after a down retains the matching-release obligations
and verdict rules frozen in the experiment plan.
