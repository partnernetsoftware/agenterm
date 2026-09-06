# PTY process-control semantics decision experiment

Status: **COMPLETE · POSIX foreground semantics accepted; Windows typed-limited**

Date: 2026-09-06  
Purpose: decide whether a direct Windows ConPTY can support the same exact
foreground-process-group signal contract as POSIX, without weakening
`pty-signal` into byte injection or silently broadening it to whole-job kill.  
Implementation: `research/pty-process-control/`  
Parent: [`goal-acu-replaces-mcu.md`](goal-acu-replaces-mcu.md)  
Prerequisites: this specification, `docs/agenterm-rust-cheatsheet.md`, and the
platform PTY contract. The implementation is independent and uses only public
OS contracts; MCU is a behavior reference, never copied code.

This experiment is on the active MCU-retirement path, but it does not by itself
change the capability ledger. Only the owning public qjswasm court may promote
the row.

## 0. Frozen facts and question

The following are already decided and are not reopened here:

1. AgenTerm's existing server is the sole PTY owner; ACU does not create a
   second PTY daemon.
2. POSIX PTY spawn creates a session leader and controlling terminal, but the
   current forced close kills only its numeric root PID.
3. Windows creates the child suspended, assigns it to a private Job Object
   before resume, and can terminate that retained job exactly.
4. A byte `0x03` written to ConPTY is terminal input, not proof that an exact
   process set received an interrupt signal.
5. MCU's native Windows PTY signal path was never qualified. ACU must preserve
   the POSIX foreground-group behavior and may return a typed Windows
   limitation; it must not invent false parity.

The unresolved question is narrow: **can direct ConPTY identify and signal the
current foreground console process set with the same exactness as POSIX, or is
that operation necessarily a platform-limited extension beside a portable
owned-session termination contract?**

## 1. Hard constraints

- The experiment must target an owned PTY created by the ordinary product path.
- Target identity must be derived from retained native authority. A displayed
  PID, process-name scan, timing guess, terminal bytes, or focus state is not
  authority.
- The fixture contains a shell/leader, one foreground child, one same-session
  background child, and one unrelated sibling. Every process has an independent
  observable liveness marker.
- A foreground signal passes only if the foreground child changes as expected,
  the shell remains usable when the signal permits it, and both background and
  unrelated controls remain unchanged.
- Owned-session termination passes only if every contained member exits and the
  unrelated sibling survives.
- A signal that the backend cannot target exactly must fail typed before
  mutation. `delivered=true` is forbidden without native delivery evidence;
  `verified=true` additionally requires the declared postcondition.
- Any urge to add a process scan, Ctrl-C byte fallback, foreground activation,
  or whole-job kill under the name “foreground signal” is the disease this
  experiment detects. Record it as a failed mechanism; do not add the escape
  hatch.

## 2. Minimal experiment

| Dimension | Frozen choice | Why |
|---|---|---|
| Product owner | existing headless `agenterm server` + one exact `@tab` | exercises the real retained PTY authority |
| POSIX reference | macOS plus Linux, `tcgetpgrp` from the retained PTY master | isolates the established terminal foreground primitive |
| Windows subject | direct ConPTY and legacy console-agent recorded separately | prevents one backend from hiding the other's limitation |
| Signals | STOP, CONT, TERM; INT only after target identity is proven | these provide independently observable state/exit results |
| Cleanup | explicit owned-session forced termination | proves the portable safety floor independently of signal semantics |
| Controls | same-session background child + unrelated sibling | detects accidental whole-session or global delivery |
| Public layer | `pty-signal` and strengthened `pty-stop` through ACU/qjswasm | compilation or private-unit success is insufficient |

Implementation remains one instrumented product path. Backend selection yields
the variants; do not build two unrelated prototype daemons.

## 3. Precommitted criteria

| ID | Criterion | Nature | Pass condition |
|---|---|---|---|
| C1 | Retained target authority | Boolean/safety | target is derived from retained PTY/console authority; no scan or input fallback |
| C2 | Foreground isolation | Boolean/safety | STOP/CONT/TERM affect exactly the foreground set and preserve both controls |
| C3 | Postcondition honesty | Boolean | STOP/CONT state or TERM exit is independently observed; weaker signals remain `verified=false` |
| C4 | Owned cleanup | Boolean/safety | all contained members exit, unrelated sibling lives, overflow/instability fails incomplete |
| C5 | Backend coverage | Checklist | macOS, Linux, direct ConPTY, console-agent are reported separately |
| C6 | Public identity | Boolean | stale scope/epoch/tab request fails before effect; current request receipt binds all three |

All criteria are behavioral. No LOC, size, or timing comparison answers this
question. Commands, source SHA, backend report, target cell and process-state
receipts must be written to `research/pty-process-control/RESULTS.md`.

## 4. Decision tree, kill criterion and time box

1. If C1 fails for a backend, foreground signal is unavailable on that backend;
   do not evaluate C2/C3 as if a weaker mechanism were equivalent.
2. If C1 passes but C2 fails, reject foreground signaling for that backend.
3. If C1/C2 pass but C3 fails, keep the implementation private until truthful
   verification or `verified=false` semantics are demonstrated.
4. C4 and C6 are unconditional product gates. Failure blocks promotion even if
   foreground signaling works.
5. C5 determines the final matrix, never the least-common-denominator meaning:
   - POSIX and Windows pass C1–C3: `pty-signal` is portable.
   - POSIX passes and either Windows backend fails: `pty-signal` remains the
     exact foreground operation with typed Windows limitations; `pty-stop`
     supplies portable exact owned-session cleanup.
   - POSIX fails: reject the proposed facade and keep the ledger gap.

Kill criterion: the first need for PID/name scanning, Ctrl-C byte substitution,
foreground activation, or whole-job delivery under `pty-signal` immediately
rejects that backend's foreground claim.

Time box: stop once C1 has a reproducible answer for direct ConPTY and C4/C6
have public evidence on one POSIX cell. Do not add signals, policy, adoption,
resource limits, or daemon features before that result is written.

Criterion accounting: C1 → C2 → C3 is the foreground branch; C4 and C6 are
mandatory independent gates; C5 only records which backend branches passed.
Every pass/fail combination exits through one of the three matrix outcomes
above.

## 5. Result layout

```text
research/pty-process-control/
├── README.md          reproducible fixture and commands
├── RESULTS.md         criterion table, receipts, decision trace and deviations
└── fixtures/          bounded child/background/control helpers if required
```

## 6. Excluded alternatives

| Alternative | Reason excluded |
|---|---|
| Write Ctrl-C bytes | tests terminal input handling, not exact signal targeting |
| Export PID to generic `process-signal` | loses PTY foreground and Job Object authority |
| Rename whole-job termination to signal | destroys foreground semantics and can kill a shell MCU preserved |
| Introduce a CU PTY daemon | duplicates the settled AgenTerm server owner |
| Claim escaped `setsid` descendants | neither a POSIX PTY session nor a Windows Job with allowed breakaway owns that set |

## 7. Not answered

- process-group adoption, expiry detach, priority/policy, or resource limits;
- application-level acknowledgement of HUP/INT/USR signals;
- arbitrary escaped descendants outside the owned session/job;
- tmux multi-pane semantics;
- GUI terminal keyboard shortcuts.

## 8. Result backfill

The result is recorded in
[`research/pty-process-control/RESULTS.md`](../research/pty-process-control/RESULTS.md).
macOS passed retained-master foreground isolation and post-state through the
public qjswasm court. Linux shares the accepted POSIX implementation and
compiles, with native runtime qualification still pending. Direct ConPTY and
the console-agent both fail C1: neither exposes an exact retained foreground
process-set authority, and console control group zero would cross into the
background control. They therefore return a typed limitation before mutation.
The criteria and kill conditions above were not changed after measurement.
