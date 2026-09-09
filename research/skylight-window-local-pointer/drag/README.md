# SkyLight exact-window drag oracle

This directory contains the owned Chromium page oracle precommitted by
section 10 of `plan/design-skylight-window-local-pointer-experiment.md` for
`acu.dynamic.004`.

It is **research only**. The runner, injector and parent court are wired for
the frozen G1-G8 current-host discriminator, but no result has been measured.
They must not be registered as public evidence or used to change the capability
ledger. The parent court owns the disposable Chromium profile, peer windows,
foreground guard, exact native identities, injection process and cleanup.

`page.html` has no external resources. A future court may load it twice as:

```text
file://.../page.html?label=A&nonce=<fixture-nonce>
file://.../page.html?label=B&nonce=<fixture-nonce>
```

From the repository root, an authorized owned-fixture run uses:

```sh
AGENTERM_CU_BROWSER_EXE=~/path/to/Chromium \
  ./research/skylight-window-local-pointer/run-drag-current-host.sh
```

The page exposes `window.__agentermDrag` for independent CDP read-back:

- `reset(trialNonce)` clears the bounded event log and business-effect state;
- `calibrate()` returns screen origin, outer/inner dimensions, device-pixel
  ratio, the owned stage rectangle, stable start/inside-end and viewport-edge
  markers, plus a point explicitly outside the right viewport edge;
- `snapshot()` returns the trial nonce, focus fact, ordered event records,
  stable target ids and ancestry, down/up nearest common ancestor, click target,
  final point and token movement/completion facts.

Capture listeners observe `mousedown`, `mousemove`, `mouseup` and `click`.
Each record includes a monotonic sequence, `button`, `buttons`, `clientX`,
`clientY`, screen coordinates, native browser timestamp, stable target id and
stable ancestor-id path. The page does not synthesize input, choose a provider
or declare a trial successful. The parent court remains responsible for
applying the frozen G1–G10
criteria, including exact down/held-move/up ordering, trajectory-specific click
rules, peer isolation, intermediate host-state preservation and release
closure.

The inside path begins at `#drag-start` and ends at `#drag-inside-end`. The
inside-to-outside path begins at the same marker, crosses
`#outside-exit-edge`, and ends at the returned point to the right of the
viewport. These are calibration facts only: the parent court must reconcile
them with CDP, exact `CGWindowID` and AX geometry before the first down.

Do not add global-event fallback, foreground activation, physical-pointer
movement, user-profile discovery or product linkage to this directory. Any
failure or uncertainty after a down retains the matching-release obligations
and verdict rules frozen in the experiment plan.
