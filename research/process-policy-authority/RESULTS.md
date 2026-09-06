# Process-policy authority results

Status: **COMPLETE · arbitrary-process Mach task-policy route rejected**

Specification:
[`plan/design-process-policy-authority-experiment.md`](../../plan/design-process-policy-authority-experiment.md)

## Measurement conditions

- Source baseline: `6988f18d1f0196e421bee72527c39c93a5c23514` plus only
  this experiment's uncommitted files
- Host: macOS 26.5.1, arm64
- Compiler: Apple clang 21.0.0 (`clang-2100.1.1.101`)
- Probe identity: ordinary linker-signed ad-hoc Mach-O; no Team identifier,
  debugger attachment, root, entitlement or security-setting change
- Targets: two forked and blocked children owned by the probe

## Result

The ordinary process could observe both children with `proc_pidinfo`, but
`task_for_pid(mach_task_self(), owned_child, &task)` returned Mach code `5`
(`KERN_FAILURE`). The experiment stopped at the precommitted C1 gate; no policy
mutation function was called. A final independent read showed the sibling's
flags stayed exactly `20971536` before and after.

Normalized structured result (runtime PIDs deliberately omitted):

```json
{
  "task_for_pid": 5,
  "get_before": 5,
  "set_background": 5,
  "get_background": 5,
  "restore": 5,
  "get_restored": 5,
  "get_after_exit": 5,
  "target_flags_before": 20971536,
  "sibling_flags_before": 20971536,
  "sibling_flags_after": 20971536,
  "sibling_after_read": true
}
```

The later fields retain their initialized failure code because C1 failed and
the probe intentionally did not call them. They are not additional kernel
observations.

| Criterion | Result | Evidence |
|---|---:|---|
| C1 ordinary acquisition | **FAIL** | owned-child `task_for_pid` → `KERN_FAILURE(5)` |
| C2 exact mutation | not evaluated | C1 gate stopped before mutation |
| C3 read-back and rollback | not evaluated | C1 gate stopped before mutation |
| C4 control isolation | **PASS for the no-effect path** | sibling flags unchanged and sibling remained readable |
| C5 dead-target refusal | not evaluated | no task port existed to retain |
| C6 shippable identity | **FAIL** | ordinary delivery identity cannot acquire the required port |

## Decision trace

The decision tree begins with C1/C6. Both failed, so the exact Mach task-port
candidate is rejected for ordinary ACU delivery and C2/C3/C5 are intentionally
not manufactured from weaker evidence. ACU must not fall back to MCU's
`taskpolicy -p PID`: its identity reads can detect a replacement only after an
effect and cannot prevent mutation of that replacement.

Product consequence: publish exact macOS policy observation and a typed
pre-mutation limitation for arbitrary-process background/normal changes. A
separate owned managed-job design may have the child set its own policy before
exec; that does not satisfy this existing-process contract.

## Reproduction

Run the two commands in `README.md`. Expected decisive output is
`"task_for_pid":5` and a nonzero process exit. The exact numeric code is
recorded rather than inferred from stderr.

## Deviations and honesty

- Because C1 failed, the target was cleaned up rather than killed after a
  successful round trip; C5 had no retained port to test.
- No PID reuse loop was added: it could not rescue a route that never acquired
  exact authority and would only increase noise.
- No criterion or threshold changed after the result. The result contradicted
  the convenient hope that parent ownership of a child grants task-policy
  authority on current macOS.
