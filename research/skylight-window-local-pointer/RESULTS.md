# SkyLight window-local pointer experiment results

## Current host · 2026-09-08

- Repository base SHA: `9e38fcf9814a1b2a347cb5acc22a0eda616bb4f4`
- Probe source digest: `e147a93f63f684442259f3aa27598d8c9765d2f0414a776c0f8a0e1098f61586`;
  the probe was uncommitted research layered on the base SHA when measured.
- Host: macOS 26.5.1 build 25F80, arm64.
- Resolved symbols: `SLEventPostToPid`, `SLEventSetIntegerValueField`, and
  `CGEventSetWindowLocation` all present.
- Fixture: two target windows in one owned process and one foreground guard
  window in a second owned process. Ephemeral PIDs and native handles are not
  persisted here.

| Criterion | Outcome | Measured evidence |
|---|---|---|
| C1 | PASS | all three required symbols resolved; forced missing-symbol route returned `provider_unavailable` with zero injection attempts |
| C2 | PASS | A move, B move, A vertical wheel and B horizontal wheel each advanced only the addressed target sequence by exactly one; guard and peer counters were unchanged |
| C3 | PASS | native `mouseMoved` callbacks reported the requested local point `(130, 106)` and a native timestamp on both windows |
| C4 | PASS | native `scrollWheel` callbacks reported exact bounded deltas `(0, 3)` and `(-2, 0)` |
| C5 | PASS | `NSEvent.mouseLocation` doubles were exactly equal before and after every action and all 1,000 repeat actions |
| C6 | PASS | foreground PID and topmost foreground CG window were exactly equal before and after every action; the owned guard remained foreground |
| C7 | PASS | forced missing symbols, wrong owner, nonexistent/zero handle, owner-only ambiguous resolution and a closed target all reached zero injection attempts |
| C8 | PASS | 1,000 alternating A/B move and wheel actions: zero loss, misdelivery, guard delivery or host-state drift; the complete C1-C8 runner passed twice consecutively |
| C9 | INCOMPLETE | only the current arm64 host is measured; no previous macOS generation or native Intel host result exists |

The public PID-targeted baseline **did deliver** a move to this owned AppKit
fixture (`baseline_public_pid_no_window=false`). Therefore this fixture does not
reproduce the earlier no-window failure and cannot prove that private SkyLight
is necessary for AppKit. It does prove that the runtime-resolved private route
can be exact and non-foreground on this one host.

Verdict: keep the provider research-only. C1-C8 are green on the current host,
but C9 is open and the nondiscriminating public baseline prevents a product
migration claim. A future discriminating owned Chromium/Electron fixture and a
previous-generation arm64 court are required before revisiting the decision.
