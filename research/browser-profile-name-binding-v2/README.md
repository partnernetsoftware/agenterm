# Browser Profile name binding owned-lifecycle experiment v2

This directory implements the frozen experiment in
`plan/design-browser-profile-name-binding-owned-lifecycle-experiment.md`. It is
a research court, not product evidence and not permission to change the
`browser.profile-name-binding` ledger cell.

## Entry point

From repository root, invoke exactly one budget kind:

```sh
AGENTERM_CU_BROWSER_EXE=~/path/to/chromium \
  AGENTERM_CU_BROWSER_APP='Brave Browser' \
  research/browser-profile-name-binding-v2/run-current-host.sh rehearsal

AGENTERM_CU_BROWSER_EXE=~/path/to/chromium \
  AGENTERM_CU_BROWSER_APP='Brave Browser' \
  research/browser-profile-name-binding-v2/run-current-host.sh decision
```

The caller cannot choose an ordinal. The external ledger assigns `R1`, then
`R2`, and permits `D1` only after a byte-identical successful rehearsal. A
reservation consumes that ordinal even if the process or disposable lane is
later lost. Residual reservations require an independent audit; the runner has
no automatic abandon, repair or reuse operation. After completing that audit,
the human auditor closes only the named reservation with:

```sh
research/browser-profile-name-binding-v2/run-current-host.sh \
  --audit-abandon rehearsal R1 <RUN_ID>
```

The command accepts only `rehearsal R1`, `rehearsal R2`, or `decision D1`,
requires the exact reserved run id, and appends the frozen `abandoned` row
under the same no-follow exclusive lock. It does not inspect, heal, rename,
delete, or reuse a reservation on its own.

The runner first requires clean, tracked frozen inputs reachable from
`origin/main`. Its outer digest calculation and the court's no-side-effect
`--preflight` calculation must emit the same canonical reservation candidate.
Only then does the runner reacquire the state lock and compare-and-reserve.

## Persistent state protocol

Authoritative state is outside the run lane at
`.agenterm-research-state/browser-profile-name-binding-v2/`. The embedded,
tracked Perl broker is the only writer. It opens a no-follow regular lock,
uses a non-blocking exclusive advisory lock, validates the complete ledger,
writes through a unique no-follow sibling temporary file, atomically replaces
the destination, and reads exact bytes back before reporting success. Initial
directory creation is idempotent under concurrent starters: an `EEXIST` race is
accepted only after the winning object is re-read and proved to be the expected
plain directory. The experiment state root and its journal directory must also
remain owned by the current user with mode `0700`; ordinary ancestor directories
are checked only for directory type and absence of symbolic links.

The court is invoked with this fixed positional interface (the mode is
`preflight`, `rehearsal`, or `decision`):

```text
MODE REPO AGENTERM_EXE CU_EXE BROWSER_EXE APP SESSION STATE_EXE CANDIDATE_DIR SOURCE_SHA INPUT_DIGEST AGENTERM_SHA CU_SHA BROWSER_SHA EXPECTED_BUILD_ID KIND ORDINAL RUN_ID
```

`APP` is explicitly supplied and the preflight must prove that the bounded
browser-version family maps to it and that public `browser-profiles` supports
it. Both sides trim only boundary ASCII whitespace from the bounded version
reply before comparing it; this preserves browsers whose `--version` command
prints a space before its newline. The no-side-effect preflight independently computes and returns exactly
`source_sha`, `input_digest`, `agenterm_sha256`, `agenterm_cu_sha256`,
`browser_sha256`, `result_template_sha256`, `browser_family`, and
`browser_version`. The runner compares all eight fields, then the broker-held
reservation recomputes the tracked input and three executable digests once
more before appending. `EXPECTED_BUILD_ID` is not counted as V1; it belongs to
decision-only G1. `STATE_EXE` is this tracked runner. Broker JSON is never placed in argv;
the court writes a direct, single-link regular file inside `CANDIDATE_DIR` and
calls one of:

```text
STATE_EXE __state stage KIND ORDINAL RUN_ID CANDIDATE_PATH
STATE_EXE __state finish KIND ORDINAL RUN_ID CANDIDATE_PATH
```

The session name is deterministically derived as `pbv2-` plus the first 27
hexadecimal characters of the 32-character run id. It is therefore exactly 32
ASCII bytes and preserves the public session-name contract: non-empty, at most
32 bytes, lowercase ASCII first, then only lowercase ASCII, digits, or hyphen.
The court mirrors all four checks during its no-side-effect preflight, before
reservation. Any future change to the public validator or this mirror must
update both sides together.

`APP` uses the public catalog spelling, not a bundle identifier. This
implementation admits `Google Chrome` for the frozen Google Chrome prefix and
`Brave Browser` for the frozen Brave Browser prefix. It recognizes the other
frozen version prefixes but refuses them before reservation because
`browser-profiles` has no corresponding public catalog entry.

A stage candidate has exactly `stage`, `producer`, `deadline_ms`,
`elapsed_ms`, `code`, `facts`, `criteria`, and `previous_receipt_sha256`.
The previous digest is JSON null for the first stage and otherwise is the
digest independently computed after the court atomically mirrored and read
back the prior `receipt_text`. A finish candidate has exactly
`receipt_sha256`, computed from that same read-back terminal mirror.

Every broker response is canonical JSON with exactly `ok`, `data`, and
`error`. On success, `data` is exactly `accepted`, `receipt_sha256`, and
`receipt_text`, and `error` is null. On failure, `accepted` is false, both
receipt fields are null, and `error` is the fixed redacted persistence record.

`result-template.json` is the sole key/type schema for stage facts and
criteria. Strings are printable ASCII, numbers are non-negative JSON safe
integers, floating-point values are forbidden, and unknown or missing fields
fail closed. This restricted domain has identical bytes under the broker's
canonical encoder and RFC 8785.

Rehearsal receipts use the template's rehearsal variants for
`connection-ready` and `ownership`: they contain only connection counts and
domain-separated process-identity digests. The court ignores connection ids,
endpoints, protocols, and all other connection-row fields until `D1`.
Decision-only G2b records the canonical candidate and live-root witnesses as
domain-separated digests plus the no-symlink, outside-root, and distinct-file-
object booleans; raw canonical paths and file identities never enter the
journal. Its `N` and `I` counts come from Preferences installation rows
intersected with the G2a Local State/public-profile overlap, not from the
public reply alone.

For each accepted request the broker validates the prior lane mirror, appends
and hash-chains the external journal, atomically publishes and reads it back,
then publishes and reads back the authoritative receipt. The court must copy
the returned `receipt_text` bytes atomically and read them back before its next
candidate. `finish` accepts only the digest of that exact terminal mirror. It
then appends the `finished` ledger row and returns the validated receipt bytes
and digest.

Connection and process snapshots permit at most 4,096 rows, including exactly
4,096. Journal limits are deliberately different and exclusive: the journal
stays below 64 rows, 32 KiB per canonical row, and 1 MiB total. Reaching any
of those journal boundaries fails; data is never truncated. State or mirror failure
after reservation is `INCONCLUSIVE_EVIDENCE_PERSISTENCE`; if the ledger cannot
record its terminal row, the reserved row remains the authoritative budget
fact and the caller must preserve the runner's fixed JSON failure output.

`--schema-check` is fully read-only.
`--self-test` instead points the broker at a fresh temporary state root and
exercises schema validation, no-follow state creation, atomic ledger
publication/readback, finished and independently audited abandoned transitions,
and inspection. It does not touch the formal state root or consume `R1`, `R2`,
or `D1`.

### Persistent implementation incident record

The first development invocation of `--self-test` revealed that the shell
function had captured the default state root before applying its temporary
override. It polluted the formal namespace with exactly a 410-byte canonical
ledger row and a 0-byte lock; `stage-journal` was empty and there was no
receipt. The row was `kind=rehearsal`, `ordinal=R1`,
`run_id=0123456789abcdef0123456789abcdef`, `source_sha=b` repeated 64 times,
`input_digest=a` repeated 64 times, `status=reserved`, with
`receipt_sha256=null` and `terminal_code=null`.

The exact 410-byte ledger line, including its trailing newline, was:

```json
{"experiment":"acu.dynamic.075.profile-name-binding-v2","input_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","kind":"rehearsal","ordinal":"R1","receipt_sha256":null,"run_id":"0123456789abcdef0123456789abcdef","schema":"agenterm.profile-binding-v2-attempt/v1","source_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","status":"reserved","terminal_code":null}
```

Those exact bytes were read back before cleanup. The implementation was not
frozen, this was a synthetic self-test that bypassed real V1 and the admission
handshake, and its sentinel identities could not describe a formal attempt.
After independent read-only confirmation, the two exact files, empty journal
directory and then-empty experiment directory were explicitly removed; no
sibling state was touched. An `abandoned` row was intentionally not fabricated:
that transition is reserved for an independently audited admitted attempt, and
keeping this pre-admission implementation defect would have consumed a formal
`R1` slot out of proportion to what occurred. `RESULTS.md` also records the
incident. The corrected test hard-fails if the formal experiment directory
exists before it begins or appears after its temporary-root state-machine test.
Once a formal attempt has created that directory, this E1 guard intentionally
makes the built-in `--self-test` unavailable for the rest of the experiment.
Later source repairs use read-only `--schema-check` plus independent review;
restoring state-machine self-test execution would require a separately reviewed
source change with an explicit fresh test root, never an ad-hoc bypass or
removal of formal state.

The separate court owns browser lifecycle, stage ordering, cleanup, and exact
receipt mirroring. The separate binding model owns only G7. Neither component
may write the external ledger or journal directly.

If public session start fails after reservation, the court first persists a
`session-ready` stage with terminal-neutral `SESSION_START_FAILED` and one of a
fixed set of redacted failure classes. The mapping covers command failure,
timeout or truncation, malformed replies, public refusal, non-ready state, and
malformed process identities. Raw product errors never become receipt terminal
codes. Cleanup stages also record whether stop, termination proof, and session
removal were applicable, so `false` no longer ambiguously means both “still
present” and “never created.”

## Result status

`R1` was consumed and finished as `INCONCLUSIVE_DEPENDENCY`; it produced no
design decision. Its source repair is recorded in `RESULTS.md`. `R2` remains
available and `D1` remains locked pending a successful rehearsal.
