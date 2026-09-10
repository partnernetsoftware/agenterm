# Browser Profile name binding owned-lifecycle experiment v2 results

Status: **not run**.

No admitted lifecycle rehearsal (`R1`/`R2`) or decision (`D1`) has been
executed from this implementation. There is no terminal experiment receipt,
design selection, qualification evidence, or PRD capability change to report.

## Implementation self-test incident

During runner development, the first `--self-test` exposed a state-root
override capture bug. It wrote one 410-byte synthetic `R1` `reserved` row and
an empty lock into the formal broker namespace. The unmistakable test values
were `a` and `b` sentinel digests plus run id
`0123456789abcdef0123456789abcdef`. It created no journal row, receipt, browser
process, product state or GUI activity.

The row was read back before cleanup, then those two exact files, the empty
`stage-journal` directory and the empty experiment state directory were
removed. No sibling external state was touched. This was namespace pollution
and is recorded here rather than hidden. It was not a formal `R1`: the
implementation was not frozen, the values were sentinels, and no real V1
preflight or admission handshake occurred.

For durable audit, the deleted ledger line was exactly 410 bytes including its
trailing newline and had the following complete canonical content:

```json
{"experiment":"acu.dynamic.075.profile-name-binding-v2","input_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","kind":"rehearsal","ordinal":"R1","receipt_sha256":null,"run_id":"0123456789abcdef0123456789abcdef","schema":"agenterm.profile-binding-v2-attempt/v1","source_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","status":"reserved","terminal_code":null}
```

Cleanup removed that ledger, the exact zero-byte `state.lock`, the empty
`stage-journal` directory, and then the empty experiment directory. It did not
append `abandoned`: that transition belongs to an independently audited
admitted attempt, while this unfrozen synthetic write bypassed both real V1 and
admission. Preserving it in the formal ordinal namespace would therefore have
spent `R1` on an implementation self-test rather than an experiment attempt.

The capture bug is fixed. `--self-test` now asserts before and after the test
that the formal experiment directory does not exist, places all broker state
under a fresh temporary root, exercises reserve, journal/receipt publication,
finish, independently audited abandonment and state inspection there, and
removes the temporary root.

The frozen criteria remain unchanged in
`plan/design-browser-profile-name-binding-owned-lifecycle-experiment.md`.
