# Exact process-policy authority experiment

Status: **COMPLETE · exact arbitrary-process Mach route rejected**

Date: 2026-09-07
Purpose: decide whether ACU can change an already-running macOS process between
normal and Darwin-background policy with exact-object authority, instead of
inheriting MCU's bracketed-but-PID-racy `taskpolicy -p PID` mutation.
Implementation: `research/process-policy-authority/`
Parent: [`goal-acu-replaces-mcu.md`](goal-acu-replaces-mcu.md)
Source discipline: public macOS SDK contracts only; MCU supplies the behavior
to compare, never implementation code.

## 0. Frozen facts and question

1. ACU already observes stable process-start identity and current Darwin
   background flags.
2. Reading identity before and after a command that accepts only a numeric PID
   can detect some races, but cannot make the effect exact-object.
3. `taskpolicy -b|-B -p PID` accepts no retained task port or start identity.
4. A typed refusal is preferable to mutating a replacement process.
5. This experiment does not design privilege consent, Linux scheduling policy,
   Windows power throttling, or managed-job policy.

Question: **can an ordinary shipped AgenTerm process acquire and retain a Mach
task-policy port for one invocation-owned child, use that port to change and
read back Darwin background policy, and fail closed after the child exits?**

## 1. Hard constraints

- The effect target must be the retained Mach task port. A PID may select the
  initial child only; it must not be used for the mutation after authority is
  acquired.
- The probe must run without root, debugger attachment, private entitlement,
  shelling out to `taskpolicy`, or changing host security configuration.
- The fixture is an invocation-owned child and an unrelated sibling. The
  sibling's background flags must remain unchanged.
- The experiment must restore the child's original policy before cleanup.
- If task-port acquisition, policy mutation, read-back, restoration, or
  dead-target refusal is unavailable, report that exact stage. Do not add a
  PID fallback.
- Any urge to call `taskpolicy -p PID` after retaining only a start-identity
  string is the safety defect this experiment detects, not a workaround.

## 2. Minimal experiment

| Dimension | Frozen choice | Why |
|---|---|---|
| Baseline | MCU-shaped `taskpolicy -p PID`, source inspection only | establishes the weaker public behavior without repeating it |
| Candidate | `task_for_pid` then `task_policy_get/set` on the retained port | only public SDK route that can bind the effect to one task object |
| Target | one forked, blocked child | owned, deterministic, no foreign process |
| Control | one forked, blocked sibling | detects accidental broad mutation |
| Policy | original → Darwin background → original | independently observable round trip |
| Death test | terminate target, retain port, attempt read/set | proves a stale authority cannot reach a replacement PID |

## 3. Precommitted criteria

| ID | Criterion | Nature | Pass condition |
|---|---|---|---|
| C1 | Ordinary acquisition | Boolean | unprivileged probe obtains a send right for the owned child task |
| C2 | Exact mutation | Safety | set is issued only through that task port and child flags change as requested |
| C3 | Read-back and rollback | Boolean | port read-back and `proc_pidinfo` agree, then original flags are restored |
| C4 | Control isolation | Safety | sibling flags and liveness are unchanged throughout |
| C5 | Dead-target refusal | Safety | after target exit, the retained port cannot mutate or resolve another task |
| C6 | Shippable identity | Boolean | the mechanism needs no root, private entitlement, debugger mode, or helper compiled at runtime |

The source, compiler command, host architecture, return codes and observed
flag transitions go into `research/process-policy-authority/RESULTS.md`.

## 4. Decision tree, kill criterion and time box

1. If C1 or C6 fails, reject arbitrary-process policy mutation through Mach
   task authority for the ordinary ACU binary. Do not evaluate PID fallback as
   an equivalent candidate.
2. If C1/C6 pass but C2 or C4 fails, reject the candidate as unsafe.
3. If C1/C2/C4/C6 pass but C3 or C5 fails, keep the mechanism research-only.
4. Only all six passes permit a product implementation and public qjswasm
   round-trip court.

Kill criterion: the first need for a numeric-PID mutation after acquisition,
root, a private entitlement, debugger security relaxation, or target-name scan
immediately rejects the candidate for ordinary ACU delivery.

Time box: stop when C1 and C6 have an ordinary-binary result and, if they pass,
when one complete policy round trip plus dead-target attempt has produced C2–C5.

Every C1–C6 criterion appears in the tree. Safety gates precede convenience;
all pass/fail combinations exit as reject, research-only, or product-eligible.

## 5. Result layout

```text
research/process-policy-authority/
├── README.md
├── RESULTS.md
└── probe.c
```

## 6. Excluded alternatives

| Alternative | Reason excluded |
|---|---|
| Bracket `taskpolicy -p PID` with identity reads | detects a race after an effect; does not prevent it |
| Run the whole ACU CLI as root | changes the product trust and consent boundary |
| Require debugger/get-task-allow entitlement | not a viable authority for arbitrary customer processes |
| Apply policy only when spawning a new process | useful separate managed-job feature, not parity for an existing target |
| Rename nice/priority mutation as background policy | different observable OS semantics |

## 7. Not answered

- native consent and a privileged provider;
- owned managed-job policy and resource enforcement;
- Linux cgroup/scheduler or Windows power-throttling semantics;
- priority/nice changes, signals, or process adoption.

## 8. Result backfill

The ordinary linker-signed arm64 probe on macOS 26.5.1 could read both owned
children but `task_for_pid` returned `KERN_FAILURE(5)` even for its direct
child. The decision tree stopped at C1/C6: no task port meant no exact mutation,
rollback, or dead-port claim could be evaluated. The unrelated child remained
live with identical policy flags.

Verdict: reject the arbitrary-process Mach task-policy route for ordinary ACU,
and do not inherit MCU's bracketed `taskpolicy -p PID` mutation. The product
slice is exact observation plus typed pre-effect limitation. Owned managed-job
pre-exec policy is a separate question. Full commands, matrix, normalized
output and deviations are in
[`research/process-policy-authority/RESULTS.md`](../research/process-policy-authority/RESULTS.md).
