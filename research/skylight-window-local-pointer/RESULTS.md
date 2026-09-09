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

## Chromium discriminator attempt 4 · 2026-09-09

- Repository source SHA: `ec94d66f123a666de32a5a7723c568ea186edf4b`.
- Discriminator source digest:
  `06ab44cfb2970b9efd93de47e2c45beeaf955c6b284b44563947e8ecae04431b`.
- Host: macOS 26.5.1 build 25F80, arm64.
- Verdict: `PASS`.
- Completed comparison: 20 seeded triplets / 60 injection attempts.
- Counts: PRIVATE exact 20/20; PUBLIC_LOC exact 0/20; PUBLIC_OFF target
  delivery 0/20; peer-window delivery 0 in every arm.
- Cleanup: required and independently completed within the guard and Chromium
  process bounds.

Every PRIVATE action delivered exactly one DOM `wheel` event to background
target B. Chromium reported `deltaX=0`, `deltaY=-120`, `deltaMode=0`; the
requested line detent was intentionally judged by axis and nonzero delivery,
not by assuming a browser pixel-scale value. Both public PID-posted arms
delivered no DOM wheel event. Across all arms the exact physical-pointer
doubles, foreground PID and foreground native window were unchanged, while the
owned guard remained foreground and target B remained non-key. The page
oracles were independently read over the exact profile's CDP endpoint.

This establishes the current-host Boolean discriminator only. It does not
qualify a product provider or close acu.dynamic.074. The next precommitted gate
is a separate 1,000-action Chromium PRIVATE repeat with per-action target,
peer, pointer and foreground checks; only a zero-loss result may spend a
previous-generation arm64 host on C9.

The complete discriminator was then rerun from repository SHA
`cebfb00279cf74b4b3724a6a5aef6ab6a0a7532a` with the same
`06ab44cfb2970b9efd93de47e2c45beeaf955c6b284b44563947e8ecae04431b`
probe digest. It produced the same `PASS` counts and invariants: PRIVATE 20/20,
PUBLIC_LOC 0/20, PUBLIC_OFF 0/20, no peer delivery, no host-state change, and
verified cleanup. Thus the two required complete current-host discriminator
runs agree; this still does not substitute for the separate 1,000-action
PRIVATE repeat.

## Chromium PRIVATE repeat attempt 1 · 2026-09-09

- Repository source SHA: `05671a5051083385aaaaa6d768935d1f5973369d`.
- Repeat source digest:
  `6448014f69c7dbca8c12a24747f3162e676a3b773a07886e7cc4f3355cd8a95f`.
- Verdict: `INCONCLUSIVE` before a structured behavioral verdict.
- Runtime failure: qjswasm exhausted its hard `max_steps` ceiling even though
  the runner requested the maximum one-billion-operation budget.
- Cleanup audit: the backend trap prevented the court's cleanup report, but a
  bounded process inventory immediately afterward found neither the owned
  Chromium profile process nor the guard process alive. The abandoned ignored
  run directory was moved to the system trash.

This is a specification/runner-budget failure, not evidence for or against the
PRIVATE route. No partial action count is claimed because the trapped guest
could not emit an authenticated final report. Two explicitly non-qualifying
detector runs then measured the same source: 10/10 exact in 10,562 ms and 50/50
exact in 34,718 ms, both with zero loss, duplicate, peer delivery or host drift
and with cleanup verified. They establish that 50 actions fit one fresh step
budget; they do not add to the required 1,000.

The experiment protocol now isolates only the incidental qjswasm step budget:
one parent retains the same live profile, A/B window identities and guard while
20 fresh top-level calls cover contiguous 50-action blocks. The per-action
criteria and first-failure kill rule are unchanged. This correction was frozen
before any qualifying chunked run; no behavioral result was available when the
protocol changed.

## Chromium PRIVATE repeat attempt 2 · 2026-09-09

- Repository source SHA: `67685196101c72bdcc81cf7493387876d484fab0`.
- Repeat source digest:
  `fc30f730a1678c381a35f439b82a69cbc87d227a476eb1426baae17f6a287c61`.
- Runner output: 1,000/1,000 exact across 20 contiguous 50-action blocks;
  zero loss, duplicate, peer delivery or host drift; cleanup reported complete.
- Elapsed time: 610,130 ms.
- Protocol adjudication: `INCONCLUSIVE_REPORT_CONTRACT`.

The action path completed cleanly, but a post-run read-only audit found that a
short detector could still print `PASS`, rejected-worker diagnostics could put
host paths in the public JSON, a failure reply's global action index was not
independently checked, and the run directory was removed after rather than
before printing `PASS`. None changed a successful action's delivery oracle, but
they were defects in the frozen result contract. The source was corrected, so
this run is not carried forward as the qualifying result.

## Chromium PRIVATE repeat attempt 3 · 2026-09-09

- Repository source SHA: `ee478c2abfcb001315343833ad6f262cd752fabf`.
- Repeat source digest:
  `c27769903ba69056ab171ff7297a69f4fa981a821a5af3b1e81c8e9a8e066e3e`.
- Host: macOS 26.5.1 build 25F80, arm64.
- Verdict: `PASS`.
- Completed repeat: 1,000/1,000 exact PRIVATE actions in 20 contiguous
  50-action workers under one parent-owned profile, A/B identity and guard.
- Failure counts: host drift 0, misdelivery 0, duplicate 0, loss 0.
- Elapsed time: 609,655 ms.
- Cleanup: required, completed and verified before the final verdict was
  printed.

Every worker echoed the frozen source identity, global range and timing
contract; the parent rejected nonzero, timed-out, truncated, malformed or
noncontiguous replies and accepted all ranges from 1 through 1,000 exactly
once. Each action reset and independently read both DOM oracles, injected one
PRIVATE wheel action at the fixed background target, and preserved the guard's
foreground ownership. This satisfies the precommitted current-host Chromium
repeat gate. It does not qualify a product provider or close
`acu.dynamic.074`; it only permits spending a previous-generation arm64 host
on the unchanged C9 run.

## Chromium exact-window drag attempt 1 · 2026-09-10

- Repository source SHA: `bc13ef1c2604909b21bc1439048a3e33427718f4`.
- Drag source digest:
  `25c01e88cf2b695ae509e9d486d60de3106d89c804263d0d80d95378b826500e`.
- Runner output: `FAIL_PRIVATE` after the first PRIVATE down; one down attempt,
  zero move attempts, one up attempt, no outcome uncertainty, and verified
  cleanup.
- Protocol adjudication: `INCONCLUSIVE_REPORT_CONTRACT`.

The observed focus drift was behavioral, but a post-run read-only audit found
two unexercised release-reporting defects: an up-post error could still be
reported as a proved release, and a page-observed down without exactly one up
did not take the highest-priority `FAIL_RELEASE` branch. The protocol was
corrected without changing its precommitted criteria. This attempt is retained
as an invalidated report-contract run and is not the final behavioral verdict.

## Chromium exact-window drag attempt 2 · 2026-09-10

- Repository source SHA: `69eabd558a6535a652b2ad58fce2220cea19c741`.
- Drag source digest:
  `7837190688d4db3e6e0adee37fcd3a3fc45aa81cbd69468b8a1a36b854e05c3e`.
- Host: macOS build 25F80, arm64; Google Chrome 152.0.7977.83.
- Verdict: `FAIL_PRIVATE` on the first PRIVATE down of the inside trajectory.
- Seed: `20260910`; the recorded PRIVATE-first order came from the seeded arm
  shuffle rather than a fixed first-arm rule.
- Counts: zero completed triplets; one down attempt, zero move attempts and one
  same-route up attempt; `outcome_unknown=false`.
- Cleanup: required, completed and verified.

The zero-injection calibration uniquely bound the owned background target B,
peer A and foreground guard. The first PRIVATE button-down left the physical
pointer, foreground PID and foreground native window unchanged, but changed
Chromium's AX main and focused window from peer A to target B. The injector
therefore stopped before posting any dragged move, still posted the prebuilt
same-target button-up, and the parent completed cleanup. No target or peer DOM
sequence was accepted as an exact gesture.

The likely mechanism is application-local key/main-window handling when
AppKit delivers a mouse-down to B. This is an interpretation, not an additional
measured fact. The archived MCU helper checked the physical pointer and
frontmost process but did not sample this application-local AX state, so the
new court exposes a side effect the legacy success check could not see.

This is the precommitted host-preservation failure in G6 and the
`FAIL_PRIVATE` kill branch in section 10. The measured private route is rejected
as a product basis on this host. A second qualifying 20-triplet run, the
1,000-gesture G9 repeat and G10 host-matrix spending are forbidden after this
behavioral failure. The result remains research only: it registers no public
evidence, changes no capability-ledger state, qualifies no provider and does
not close or remove `acu.dynamic.004`.

A future decision to accept application-local key/main-window change would
alter G6 and the background-local contract; it requires a new precommitment and
resets these drag results rather than reclassifying this run.
