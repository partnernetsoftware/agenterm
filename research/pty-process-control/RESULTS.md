# PTY process-control results

Status: **COMPLETE · POSIX foreground signal accepted; Windows typed-limited**

Specification:
[`plan/design-pty-process-control-experiment.md`](../../plan/design-pty-process-control-experiment.md)

## Measurement conditions

- Pre-fix source: `51172ef09bf211b5703f20deb4a0c1fbc7af1595`;
  post-fix evidence is owned by the commit containing this result
- Host/backend: macOS arm64, `unix-pty`
- Build: repository-root `cargo build -p agenterm -p agenterm-cu`
- Execution: real local `agenterm` headless server and public `agenterm-cu`
  `pty-*` commands; no mock, GUI, foreign process, or external service
- Fixture: one owned `/bin/sh` root and two child shells. Both children ignore
  HUP, TERM and INT and remain in their inherited owned session. Their loop is
  `sleep 60`; every PID is emitted through the PTY before the action.
- Cleanup: the court reaps every fixture PID with KILL after observation and
  prunes the named PTY record. A separate `kill -0` check confirmed all three
  fixture PIDs were absent after cleanup.

## Pre-fix result

The public action was:

```text
target/debug/agenterm-cu --target current --grant actuate \
  pty-stop <OWNED_FIXTURE_NAME> --expect stopped
```

The command returned `ok=true`, `state=stopped`, `performed=true` and
`verified=true`. After a 200 ms observation window:

| Fixture role | State after reported success |
|---|---|
| owned PTY root | exited |
| owned child A | **alive** |
| owned child B | **alive** |

This is a decisive C4 failure: endpoint disappearance currently proves only
that the server/tab authority disappeared. It does not prove that the owned
process tree is empty. Ordinary children often die from terminal HUP and can
hide the defect, so the resistant fixture is mandatory for the regression.

## Criterion matrix

| Backend | C1 retained target | C2 foreground isolation | C3 honest post-state | C4 owned cleanup | C5 recorded | C6 public identity |
|---|---:|---:|---:|---:|---:|---:|
| macOS unix-pty, pre-fix | not measured | not measured | not measured | **FAIL** | yes | existing tab identity only; native cleanup missing |
| macOS unix-pty, fixed | **PASS** retained master | **PASS** foreground changed; both controls live | **PASS** STOP/CONT state, TERM exit; INT delivery-only | **PASS** | yes | **PASS** job + scope + epoch + tab |
| Linux unix-pty | same accepted source; runtime pending | runtime pending | runtime pending | compiled; runtime pending | yes | wire/compile green; runtime pending |
| Windows direct ConPTY | **FAIL** no foreground-set authority | not evaluated | typed before mutation | Job cleanup contract compiled | yes | control identity retained in typed refusal |
| Windows console-agent | **FAIL** control group zero is whole console | rejected by kill criterion | not evaluated | Job cleanup contract compiled | yes | no foreground route published |

## Post-fix forced-cleanup result

The platform now treats native containment as the authority rather than the
root PID or vanished endpoint:

- POSIX retains an exact root observer, enumerates the bounded session, opens
  and rechecks exact member references, freezes until membership is stable,
  force-terminates the set and observes every reference exited.
- Windows terminates the retained Job Object and requires its accounting query
  to report `ActiveProcesses == 0`.
- `kill-window` returns a structured native cleanup receipt. `agenterm-cu`
  requires that receipt, worker completion and same-scope/epoch tab absence;
  any missing component is `terminal_close_unverified`.

The public qjswasm `cu-pty-smoke` starts a root plus two children that ignore
HUP, TERM and INT, verifies all three are live, performs `pty-stop`, then proves:

| Assertion | Result |
|---|---|
| native containment | `posix-session` |
| bounded members observed | at least 3 |
| containment empty | true |
| terminal workers complete | true |
| three retained fixture processes absent | true |
| unrelated sibling remains alive | true |

The macOS arm64 owning journey is green. Both Windows and Linux ISAs compile
the platform contract. The resistant-session native regression also passed 100
consecutive macOS runs after making the session leader last and reacquiring a
mutation token across an observed `exec` identity transition. Windows and
Linux runtime courts remain pending.

## Decision trace so far

1. C4 is unconditional; the pre-fix baseline failed and the post-fix macOS
   public court now passes.
2. The failure matches the source audit: Unix forced termination sends KILL to
   only the PTY root PID while Windows already terminates its retained Job.
3. `pty-stop` now reports native containment cleanup separately from endpoint
   cleanup and fails closed on incomplete or unstable membership.
4. macOS C1–C3 now pass. The product obtains the foreground process group only
   from `tcgetpgrp` on its retained PTY master, rechecks session membership,
   sends the native group signal and observes STOP/CONT state or TERM exit.
5. The public qjswasm journey creates a separate interactive-shell background
   job and an unrelated sibling. STOP and TERM change only the foreground job;
   both controls remain live, and the shell remains usable after STOP.
6. Direct ConPTY fails C1 because its retained pseudoconsole and Job Object do
   not expose the current foreground process set. The console-agent also fails:
   `GenerateConsoleCtrlEvent` with group zero reaches background processes.
7. The precommitted kill criterion therefore selects the mixed outcome: exact
   POSIX `pty-signal`, typed Windows limitation, and portable full-session
   `pty-stop`. No byte, scan, activation or whole-job fallback was admitted.

## Foreground-signal receipts

The public command is:

```text
target/debug/agenterm-cu --target current --grant actuate \
  pty-signal <OWNED_JOB> --signal stop --expect stopped
```

The receipt binds the durable job name, server scope, server epoch and stable
tab, then nests the native containment, signal, bounded member counts, delivery
fact, verification fact and postcondition. No PID is returned as reusable
mutation authority.

Measured on macOS arm64 through `cu-pty-smoke`:

| Action | Native postcondition | Foreground | Same-session background | Unrelated sibling |
|---|---|---|---|---|
| STOP | `stopped`, verified | stopped | alive | alive |
| shell-owned resume | shell prompt usable | resumed in background | alive | alive |
| TERM on a new foreground job | `exited`, verified | exited | alive | alive |
| final `pty-stop` | session empty | exited | exited | alive |

The platform regression independently executes STOP → CONT → TERM against the
retained master. INT returns `delivered=true`, `verified=false` at the native
layer because only the application can acknowledge its meaning; ACU verifies
only the caller's explicit `--expect delivered` for that signal.

## Reproduction recipe

From the repository root:

1. Build `agenterm` and `agenterm-cu` with the command above.
2. Start a unique PTY job whose typed argv is `/bin/sh -c <FIXTURE>`, where
   `<FIXTURE>` starts two child shells that ignore HUP/TERM/INT, prints the root
   and child PIDs, then waits.
3. Wait for the PID line with `pty-wait`, decode `pty-read.data_base64`, and
   retain all three numeric identities.
4. Run the displayed `pty-stop` command.
5. After 200 ms, use `kill -0` only on the retained fixture PIDs. Record each
   result, then KILL any survivor and prune the PTY job.

The exact fixture body used was:

```sh
trap "" HUP TERM INT
/bin/sh -c 'trap "" HUP TERM INT; while :; do sleep 60; done' & bg=$!
/bin/sh -c 'trap "" HUP TERM INT; while :; do sleep 60; done' & fg=$!
printf 'TREE ROOT=%s BG=%s FG=%s\n' "$$" "$bg" "$fg"
wait "$fg"
```

No criterion was changed after seeing this result. The expected convenience
path—children disappearing with the terminal—was deliberately rejected after
the resistant control exposed the false positive.

## Deviations

- The failure baseline was first reproduced with a bounded repository-root
  shell sequence. Its fixed C4 behavior and C1–C3 foreground branch now share
  the registered `cu-pty-smoke.qjs` public qjswasm court.
- Linux runtime and Windows typed-refusal runtime reruns remain delivery
  qualification, not an unanswered architecture question. Both target families
  compile the same public contract.
- No criterion, threshold, fixture control or kill condition was changed after
  observing the result.
