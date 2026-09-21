# v0.1.18 — the release record and what it cost

Published 2026-09-21T14:51:22Z, eighteen days after the previous green
Candidate (2026-09-03).

```
version            0.1.18
source_sha         eaaec807d99c6aea8c9dcb1d7290dd1f956d7ec0
candidate_run      35606274661
reputation_run     35614307176   local UTM Defender court, CLEAN, 2 assets
release dry-run    35614420387
release publish    35614774445
signing policy     windows off · linux off · macos unsigned-preview
```

Promotion was byte-for-byte: no rebuild, no re-sign.

## The seven defects that had held it

Six of the seven were invisible in CI because the assertions named a step
rather than a cause. The seventh was a one-line argument order.

1. **`debug_assert_eq!(state.submit(request), SubmitResult::Queued)`** in
   `src/mcp_stdio.rs`. The macro compiles out entirely in a release build, so
   the submit never ran: every *shipped* binary accepted a mutating
   `tools/call` and answered it never, while every debug test passed. Work
   does not belong inside an assertion.
2. **Stdin EOF discarded a request accepted while the ACU session was
   starting**, and suppressed the answer to one already dispatched. A client
   that has finished writing can still read. A broken *stdout* is the opposite
   case and still cancels and ends once.
3. **After dispatching pre-EOF work the connection was never told stdin had
   closed**, so it never entered session-ending and the process stayed up.
4. **The power-action TTL range was enforced twice**, and the generic copy ran
   first, making the typed `privilege_plan_ttl_invalid` that the gate and its
   callers match on unreachable behind a generic `invalid_command`.
5. **`getpid` cannot resolve on Windows.** The MSVC CRT spells it `_getpid`,
   and Rust links that CRT statically, so `GetProcAddress` on the process
   module cannot see it where `dlsym` finds libc for free. Naming `ucrtbase`
   gives the Windows guest the same shared C runtime. An unresolvable symbol
   ends the guest rather than raising, so it had to be named, not discovered.
6. **`candidate.yml` passed the four smoke scripts in the wrong order**, so
   `acu-mcp-provider-smoke.qjs` received the composition smoke's position and
   refused with "expected: no arguments". This alone reddened **all six**
   runtime cells, on every platform, with one message that named a step and
   not a cause. One 45-minute round per platform, for an argument order.
7. **The runtime cell declared no engine budgets.** Survivable on a native
   court; under x86 emulation it was cancelled mid-flight and said only
   "cancelled by the host while waiting".

## The two gates that cost two more rounds

`rustfmt` and `Clippy` — the checks the Windows leg runs *first*, and it stops
at its first failure, so they were two separate ninety-minute rounds. The
survey run (35604236354) settled the whole lane in one pass: **those two were
the only red gates in about thirty.** `scripts/pre-push-check.sh` now runs
both here, the lint against the Windows target in the release profile, which
is what the gate uses and what a host-target debug Clippy does not report.

## Invariants added, so these classes cannot return

- No gate script may judge a child by its exit code without carrying that
  child's stdout/stderr (`no_gate_script_judges_a_child_by_exit_code_alone`).
- Every workflow `cli script run` declares both engine budgets
  (`every_workflow_script_run_declares_both_engine_budgets`), matching the
  rule that already held inside gate scripts.
- A Linux cell may be blocked or run in a court, never a hard-coded
  host/port/key.
- A surveyed run remains structurally incapable of writing a receipt.

## Two mistakes of method, both mine

**A local reproduction has to reproduce the build, not just the directory.**
`cargo build -p agenterm -p agenterm-cu` in one invocation feature-unifies
`agenterm-platform`'s `device-capture` into an image the product ships without
it; on macOS that registers the same Objective-C class twice, dyld warns on
stderr, and a gate asserting the server says nothing there goes red. I reported
that to the owner as an inherent defect of the fixed-sibling provider design
and a release blocker. It was neither: `scripts/qjs/build.qjs` splits those
invocations on purpose and says why. Built that way the macOS runtime cell
passes locally.

**A probe stops being valid when you change what it measures.** `cli-smoke` was
used to conclude a host granted job breakaway after it had been changed to own
its own server, so it no longer depended on an autostarted one surviving.

Both times the instrument had changed, not the product.

## Environment facts worth keeping

- **`gh run download` hangs on the direct route from this network.** The
  sealed Candidate (118 MB) sat at zero bytes for 67 minutes; the same
  artifact fetched through `--proxy http://127.0.0.1:8888` in 59 seconds. Same
  class as the cargo-xwin MSVC CRT fetch in `docs/macos-local-build.md`.
- **A `utm-court lease` failure naming a court you did not ask for** is one
  unreachable peer blocking everyone: peer memory-reclaim is treated as a
  precondition rather than housekeeping. Stop that peer by hand and lease
  again. Three of the six cells read as "court broken" when only one was.

See [`windows-court-loop.md`](windows-court-loop.md) for the loop itself and
[`candidate-gate-speed.md`](candidate-gate-speed.md) for where the Windows
leg's time goes.
