# PTY process-control results

Status: **IN PROGRESS · POSIX forced-cleanup floor green**

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
| Linux unix-pty | pending | pending | pending | pending | pending | pending |
| Windows direct ConPTY | pending | pending | pending | pending | pending | pending |
| Windows console-agent | pending | pending | pending | pending | pending | pending |

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
4. C1 for direct ConPTY remains the time-box endpoint. No conclusion about
   portable foreground signaling has been drawn.

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

- C1–C3 were not attempted because the mandatory cleanup floor failed first.
- The failure baseline was first reproduced with a bounded repository-root
  shell sequence. Its fixed C4 behavior is now in the registered
  `cu-pty-smoke.qjs` public qjswasm court and records the native containment
  receipt. C1–C3 and the non-macOS runtime cells remain deliberately open.
