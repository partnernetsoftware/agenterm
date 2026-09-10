# Browser Profile name binding owned-lifecycle experiment v2 results

Status: **R1 consumed; inconclusive; no design result**.

Formal rehearsal `R1` ran on 2026-09-11 from source
`f8e8799a4913807a205e300618fba997c1c5f91a`. It is finished, irrevocably
consumed, and recorded with:

- run id `22269f85c720e610b05071243230b211`;
- input digest
  `e4c2dc4b555ccf253fa977a5985ce06c0e4e1c5cd73088a609eddbbcf6b1fcb5`;
- terminal code and primary cause `INCONCLUSIVE_DEPENDENCY`;
- receipt SHA-256
  `a562b88104c7785d1ea2648834ff379903ed23c05d71db7d18a5cf3e578f7633`;
- eight journal rows ending at
  `4ea77d53b5f87c274e3b7ce3336fdb1a3b693e6d5c79aae7dcd183d334ef3677`.

The external ledger contains the matching `reserved` then `finished` rows.
The receipt contains all fifteen criteria keys: V1 and V4b are `pass`; V3,
V3a, and V3b are `fail`; every design criterion and every other unreached
criterion is the literal `not-run`. Cleanup failure did not replace the earlier
primary cause. `R2` and `D1` were not touched.

The frozen source permits a unique static diagnosis. It derived the public
session name as `profile-binding-v2-` plus the 32-character run id: 51 ASCII
bytes. The public session validator permits at most 32 bytes. The value was
non-empty, began with lowercase ASCII, and otherwise contained only lowercase
ASCII, digits, and hyphens, so length was the sole violated constraint. Control
flow and the journal agree: preflight and baseline were persisted, session
start failed before `session-ready`, and cleanup followed with no created
session identities.

That run also exposed a diagnostic gap. The terminal classification was
correct, but the journal did not persist the failed session-start action or its
redacted reason; later cleanup booleans could not distinguish “never created”
from “still present.” Before `R2`, the source repair therefore:

1. derives an exactly 32-byte name, `pbv2-` plus 27 run-id characters;
2. mirrors all four public name constraints in no-side-effect preflight;
3. persists a fixed, redacted `session-ready` failure class before returning
   `INCONCLUSIVE_DEPENDENCY`; and
4. marks cleanup stages as not applicable when their required identities were
   never created.

This repair changes the frozen input digest and therefore requires `R2`. It
does not reinterpret `R1`, select a design, unlock `D1`, register qualification
evidence, or change a PRD capability state.

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
