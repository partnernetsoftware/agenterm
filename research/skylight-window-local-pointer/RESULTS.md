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

## Chromium discriminator attempt 1 · 2026-09-09

- Repository source SHA: `b86171422553500d3623abee3d6a9956ef9d07a1`.
- Discriminator source digest:
  `b033be4a3e0699f50812cc1e0a69cf76a210ac8a5a4ab3bc63686e0d3948cc65`.
- Host: macOS 26.5.1 build 25F80, arm64.
- Verdict: `INCONCLUSIVE` at `bridge-connection`.
- Injection attempts: zero; triplets completed: zero.
- Cleanup: the owned browser session reached `stopped` and was removed with
  `verified=true`.

The owned browser and its CDP endpoint reached `ready`, but the caller-selected
Google Chrome build did not publish a new Native Messaging bridge connection
within the frozen 15-second deadline. No window was opened and neither public
nor private delivery ran. This is not evidence for or against the provider.

The fixture owner is revised for attempt 2: use a direct temporary Chromium
profile with a loopback CDP endpoint and two explicit `--new-window`
launches, while retaining the exact same PRIVATE / PUBLIC_LOC / PUBLIC_OFF
criteria. Because attempt 1 reached zero injection, no delivery result is
carried forward. Attempt 2 receives a new source digest and starts the full
20-triplet comparison from zero.

## Chromium discriminator attempt 2 · 2026-09-09

- Repository source SHA: `61d81adcc30f7e20e332d3ab9fdc32c930dc285b`.
- Discriminator source digest:
  `65ce4b77a003f8e74f355eefd57918443cfd339a0590aa838a589303dffa07f9`.
- Host: macOS 26.5.1 build 25F80, arm64.
- Verdict: `INCONCLUSIVE` at `browser-launch-b`.
- Injection attempts: zero; triplets completed: zero.
- Cleanup: the owned Chromium process exited within the cleanup bound.

The direct profile was created, but the caller-selected Google Chrome build
did not expose the preselected fixed CDP port before the 30-second readiness
deadline. A separate bounded diagnostic with the same binary and another fresh
profile immediately published `DevToolsActivePort` when Chromium selected the
port itself. Attempt 3 therefore changes only this fixture mechanism to
`--remote-debugging-port=0` plus a bounded read of that exact profile record.
The discriminator criteria remain frozen and no delivery count carries
forward.

## Chromium discriminator attempt 3 · 2026-09-09

- Repository source SHA: `5464517f51beb9769032a04dda6a0af1569bd280`.
- Discriminator source digest:
  `4640ef687a4cbcc709912d348df0dea040cc30d42edbdc00137fdd587604210b`.
- Host: macOS 26.5.1 build 25F80, arm64.
- Verdict: `INCONCLUSIVE` at `browser-launch-b`.
- Injection attempts: zero; triplets completed: zero.
- Cleanup: the owned Chromium process exited within the cleanup bound.

Chrome did publish a valid two-line `DevToolsActivePort` record and listen on
its selected loopback port. The readiness curl nevertheless received the
host's configured HTTP proxy response because that environment did not exempt
loopback. Attempt 4 changes only the readiness probe to bypass proxies, retries
a partially written port record, and binds `/json/version` to the record's
browser WebSocket path. No delivery result carries forward.

Before the first injection attempt, review also made criterion D5 executable:
`PASS` requires `public_off_target_count == 0`. This is a clarification of the
precommitted statement that PUBLIC_OFF cannot satisfy exact target delivery,
not a conclusion drawn from arm results; attempts 1-3 all completed zero
triplets.
