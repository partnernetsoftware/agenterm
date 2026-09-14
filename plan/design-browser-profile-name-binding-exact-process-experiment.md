# Browser Profile name binding exact-process decisive experiment

This is a new research precommitment. It does not reopen or amend either
exhausted browser-profile-name-binding experiment, qualify a provider, change
the `browser.profile-name-binding` ledger cell, remove
`acu.dynamic.075.profile-name-binding`, or turn a human Profile label into
authority.

| Field | Value |
|---|---|
| Date | 2026-09-14 |
| Purpose | Decide whether a human Chromium Profile name can bind to one exact live bridge connection after exact-key process observation made invocation-owned ancestry independently provable |
| Implementation | `research/browser-profile-name-binding-exact-process/` |
| Required reading | `plan/design-browser-profile-name-binding-experiment.md`, `plan/design-browser-profile-name-binding-owned-lifecycle-experiment.md`, both corresponding `RESULTS.md` files, `prd/PRD_02_28_agenterm_cu.md`, `plan/acu-mcu-capability-ledger.json` |
| Source discipline | Invocation-owned synthetic HOME and browser Profiles only; no real Profile mutation, MCU runtime or fallback |

## 0. Fixed background and competing designs

The following facts are frozen and are not reopened by this experiment.

1. The first experiment consumed two attempts and ended
   `INCONCLUSIVE_FIXTURE_EXHAUSTED`. The owned-lifecycle replacement consumed
   rehearsals R1 and R2 and never made D1 eligible. Their sources, results,
   ledgers and exhausted markers remain immutable history. This experiment is
   not attempt 3, R3 or a repaired copy of either court.
2. Neither prior experiment selected or rejected A0, A1 or B. Values computed
   before a validity failure were not authoritative measurements.
3. Owned-lifecycle R2 failed because its independent `/bin/ps` witness required
   every session id to be positive. The host returned valid zero session ids,
   so the court stopped before persisting an ownership result. That is a court
   validity defect, not evidence for any binding design.
4. `process.observe(pid)` and `process.parent(pid)` shipped after both frozen
   experiments. Their exact-key records are genuinely new evidence: the old
   source revisions had neither operation.
5. `process.observe` is the liveness and start-identity authority.
   `process.parent` only reports that a native record supplied a parent id; its
   `live` state does not itself prove that the process is still running.
6. A bridge inventory row supplies the native host PID and start identity. The
   browser child handle supplies the invocation-owned browser PID. A bounded,
   cycle-free sequence of exact parent records bracketed by exact observations
   can prove that the host belongs to that browser tree without a process-table
   scan, argv inspection, title matching or selector result.
7. The competing designs remain unchanged:
   - **A0 · archived cardinality alias control:** reproduce the relation-free
     1-1-1 fallback only as a negative control; it has no winning branch.
   - **A1 · verified implicit edge:** select only when one trusted issuer
     authenticates a collision-resistant, Profile-unique value on both the
     candidate directory and live connection, followed by restart and
     name-safety proof.
   - **B · explicit durable binding:** caller-authorized,
     generation-controlled `(application,directory,instance)` binding,
     revalidated against the complete live inventory on every use.
8. A terminal result may authorize a later implementation leaf. It does not by
   itself qualify a platform, register evidence or close the compatibility
   ledger.

The new information under test is narrow: exact-key process facts may repair
the ownership and cleanup validity model. They do not create a directory to
profile-instance edge and therefore do not prejudge A1 or B.

## 1. Hard constraints

1. The two old specifications, result directories, ledgers, receipts and
   exhausted markers are read-only inputs. This experiment has a distinct id,
   state root, source digest, input digest and budget. Renaming an old ordinal
   or editing a marker is an invalid run.
2. The court must use `process.pid(handle)`, `process.observe(pid)` and
   `process.parent(pid)` for the live ownership chain. Production and research
   source for this experiment must contain no `/bin/ps`, `pgrep`, `ps -`,
   process-table parser, process-group inference or session-id predicate.
3. Every PID in the chain is bracketed by two `process.observe` records. Both
   observations must be `live`, must carry non-null equal `start_identity`
   values, and must surround the parent observation that advances the chain.
   `dead`, `unknown`, a null identity or an identity change is
   `INCONCLUSIVE_PROCESS_IDENTITY`, never absence.
4. The parent chain has a fixed ceiling of 64 edges, rejects a repeated PID and
   must terminate at the exact invocation-owned browser PID and start identity.
   Reaching PID 0, an unrelated root, a missing parent or the ceiling is
   `INCONCLUSIVE_OWNERSHIP`.
5. Ownership is frozen before any A0/A1/B selector runs. Selector inputs may
   refer to the frozen owned connection id, but ownership and cleanup code may
   not read `profile_instance_id`, candidate labels, selector output,
   provisional model output or a design verdict.
6. Cleanup is defined independently as: stop and reap the invocation-owned
   browser handle; observe every frozen tree identity until each is `dead`; then
   require the complete live bridge inventory to equal the preserved baseline.
   Registry-file hygiene is recorded separately and cannot turn a dead process
   into a live connection or invalidate an otherwise exact baseline restoration.
7. Before each side effect and before every possible throw, the court atomically
   publishes and reads back one bounded, redacted stage receipt outside the
   disposable run lane. The receipt includes the last completed stage, typed
   code, deadline, frozen identities already proven and cleanup facts already
   proven. It never contains argv, environment, titles, URLs or home paths.
   Persistence failure itself is the sole exception: it emits one fixed,
   redacted `INVALID_EVIDENCE` stdout record containing the experiment, kind,
   ordinal, run id, source and input digests, and code, then exits nonzero. It
   publishes no design fact and leaves any reserved ordinal authoritative for
   independent audit. A prior unrelated stage row cannot prove that the
   failing stage was persisted.
8. Admission atomically reserves one named ordinal in the external ledger
   before fixture creation. A reservation is never reused. A residual
   reservation requires independent audit; the runner does not heal it.
   The external ledger is authoritative and a lane receipt is only a disposable
   mirror. The `reserved` / `finished` / `abandoned` state machine, independently
   audited abandonment, state-lock reacquisition and frozen-input recheck before
   reservation, receipt-digest binding, and diagnostic printing of ledger rows
   plus the validated receipt digest are inherited unchanged from sections
   1.8-1.9 of the owned-lifecycle precommitment.
9. Complete connection inventories are mandatory. Truncation, malformed rows,
   identity disagreement or baseline mutation is typed inconclusive. The owned
   connection is exactly one identity-validated delta over the preserved
   baseline; the global machine need not start empty.
10. Candidate Profile inputs come from the same bounded public and independent
    parsers frozen by the owned-lifecycle experiment. No hand-authored candidate
    array may vote.
11. No repair may add LevelDB parsing, command-line scanning, browser
    activation, a new browser permission, account identity, real Profile
    mutation, title guessing, a new process host operation or a weaker
    liveness interpretation.
12. No resolver, compatibility mapping, persistent binding store, evidence
    registration, ledger-state change or PRD capability completion lands before
    an independently reviewed terminal receipt exists.

The disease detector is any attempt to make the court green by treating
`process.parent.live` as liveness, accepting an unbracketed PID, consulting a
selector during cleanup, reviving `/bin/ps`, weakening a stage receipt or
editing an exhausted marker. Such pressure is a finding, not a requirement.

## 2. Minimal experiment

| Dimension | Frozen choice | Why |
|---|---|---|
| Process root | Invocation-owned browser child handle and `process.pid` | Names the exact object the court may stop |
| Native-host identity | Bridge inventory PID plus start identity | Existing public connection fact, not a guessed name |
| Genealogy | At most 64 `process.parent` hops, each bracketed by `process.observe` | Avoids the invalid process-table/session witness and detects PID reuse |
| Cleanup | Owned-handle stop plus frozen-identity death and baseline inventory restoration | Does not depend on provisional selector results |
| Evidence | External bounded stage receipt before every side effect/throw | Makes validity failures authoritative instead of reconstructed |
| Designs | Existing A0 negative control, A1 verified edge, B explicit binding | Keeps the unresolved architectural question unchanged |
| Platforms | macOS decision court; model/self-tests platform-neutral | Matches the available owned browser fixture without claiming Linux/Windows qualification |

Implementation is limited to one new research directory, this specification
and the owning PRD sentence. Production code is out of scope until a verdict.
Before admission, a registered tool-profile capability preflight may exercise
the exact process, durable-file, lock and digest primitives against its own
worker and one owned non-browser child. It reserves no ordinal and produces no
V3-V5 or D1-D3 fact; its only purpose is to reject an unusable host boundary
before the irreversible rehearsal reservation.

## 3. Criteria and measurement discipline

Validity criteria run before any design criterion and cannot be outweighed.

| ID | Property | Pass | Failure |
|---|---|---|---|
| V1 | Boolean · preflight | Exact source/input/browser/extension identities validate without fixture side effects | `INVALID_INPUT` |
| V2 | Boolean · identity source | Static and runtime receipts prove only exact-key process ops were used; no process-table command or parser exists | `INCONCLUSIVE_IDENTITY_SOURCE` |
| V3 | Boolean · ownership | One cycle-free bracketed chain connects the live bridge host identity to the invocation-owned browser identity within 64 hops | `INCONCLUSIVE_OWNERSHIP` |
| V4 | Boolean · exact termination | Browser stop is followed by `dead` observations for every frozen owned identity; no step is skipped | `INCONCLUSIVE_CLEANUP` |
| V5 | Boolean · inventory restoration | Final complete live inventory equals the preserved baseline; stale registry hygiene is reported separately | `INCONCLUSIVE_CLEANUP` |
| V6 | Boolean · evidence persistence | Every possible failure has a read-back-equal bounded stage row written before it, except persistence failure itself, which emits the fixed non-evidence record from §1.7 | `INVALID_EVIDENCE` |
| V7 | Safety · selector independence | Ownership and cleanup consume no selector, candidate label, profile instance or provisional verdict field | `INVALID_EXPERIMENT` |
| D1 | Safety · A0 control | Armed alias trap selects the wrong connection or otherwise demonstrates relation-free ambiguity | Permanently reject A0 |
| D2 | Checklist/Boolean · A1 edge | Same trusted issuer supplies a unique two-sided edge, restart persistence and all name-safety arms | Select A1 only if every arm passes |
| D3 | Boolean · B model | Explicit binding passes create/use/rename/duplicate/stale-generation/crash-recovery cases with typed refusal and independent cleanup | Select B if A1 is ineligible and every B arm passes |

Every receipt records exact source/input/executable digests, browser family and
version, ordinal, wall deadline, process-chain length, stage sequence and the
boolean result for V1-V7. Every receipt that records V3 `pass` also records
stable digests of the frozen browser, bridge-host and connection identities.
Raw PIDs and paths are not durable stage facts. No
performance or size claim is made; therefore no cross-experiment byte or timing
ratio is permitted.

## 4. Decision tree, kill criteria and timebox

Decision order is validity, then the safety control, then eligible designs:

1. V1 fails: stop before fixture creation with `INVALID_INPUT`.
2. V2 fails: stop with `INCONCLUSIVE_IDENTITY_SOURCE`; the new process facts
   were insufficient and the typed TODO remains.
3. V3 fails: stop with `INCONCLUSIVE_OWNERSHIP`.
4. V4 or V5 fails: stop with `INCONCLUSIVE_CLEANUP`.
5. V6 fails: use only the fixed non-evidence record from §1.7, stop nonzero
   with `INVALID_EVIDENCE`, and cite no design fact.
6. V7 fails: stop with `INVALID_EXPERIMENT`; no design fact may be cited.
7. V1-V7 pass: D1 becomes eligible. Its trap permanently rejects A0; it never
   selects another design.
8. If D2's issuer edge exists, run every restart and name-safety arm. All pass
   selects A1; any failure rejects A1 without widening its surface and
   continues to D3.
9. If A1 is structurally ineligible or was rejected by D2, run D3. Every B arm
   passing selects B; otherwise return `INCONCLUSIVE_MECHANISM` and retain the
   TODO.

All criteria occur in the tree: V1-V7 are validity gates, D1 is a non-voting
negative control, D2 precedes D3 because an authenticated existing edge avoids
new durable state, and D3 is the explicit fallback. Every branch has an exit.

Kill criteria:

1. Exact-key process ops cannot prove V3 without `/bin/ps`, process inventory,
   argv/name inference or a new host operation: stop permanently with
   `NEW_INFORMATION_INSUFFICIENT`.
2. Any required exact observation is `unknown` or has no start identity: stop;
   do not reinterpret it as dead or unrelated.
3. Cleanup cannot be expressed without selector-derived data: stop with
   `CLEANUP_NOT_INDEPENDENT`.
4. The first implemented self-test cannot prove that every throw site is
   preceded by a persisted stage: do not reserve a live ordinal.
5. A live ordinal is consumed when its external reservation is published.
   There is one rehearsal ordinal and one decision ordinal; neither may be
   repaired, enlarged or rerun after a terminal result.

Kill terminals are early exits outside the ordinary validity-tree sequence and
have exact criteria shapes. `NEW_INFORMATION_INSUFFICIENT` records V1 and V2
`pass`, V3 `fail`, and V4-V7 `not-run`; its terminal code and primary cause
distinguish permanent mechanism insufficiency from an ordinary
`INCONCLUSIVE_OWNERSHIP` observation. `CLEANUP_NOT_INDEPENDENT` records V1-V3
`pass`, V7 `fail`, and V4-V6 `not-run`, because the structural selector leak is
detected before cleanup is allowed to execute. No other criteria shape may use
either terminal.

The implementation timebox ends when the platform-neutral self-test has
machine-checked V2, V6, V7 and every decision-tree combination. No browser is
launched before that reviewed result. The live timebox ends at the first
terminal rehearsal receipt; a failing rehearsal does not authorize D1.

## 5. Planned directory

```text
research/browser-profile-name-binding-exact-process/
├── README.md
├── RESULTS.md
├── run-current-host.sh
├── court-current-host.qjs
├── binding-model.qjs
├── result-template.json
└── fixtures/
    ├── local-state.json
    └── preferences.json
```

The implementation may reuse frozen fixture bytes and pure model logic by
copying them with provenance, but it must not import mutable result state or
attempt ledgers from either exhausted experiment.

## 6. Excluded options

| Option | Why excluded |
|---|---|
| Repair or rerun v1/v2 | Their budgets and input digests are exhausted history |
| Process table, pgid or session-id witness | Recreates the exact R2 validity defect and transports unrelated global state |
| Treat parent `live` as liveness | Contradicts the shipped contract; only `process.observe` decides liveness |
| Connection-id or profile-prefix ownership | Selector output cannot prove process ownership |
| Add parent fields to bridge inventory | Duplicates the exact-key process authority and changes production before a verdict |
| Parse argv, titles, LevelDB or real Profiles | Introduces heuristic identity or mutates user state |
| Add a process host op | The experiment exists to test whether the newly shipped exact-key ops suffice |
| Choose B because A1 is absent today | Absence of a frozen edge is not evidence that an explicit store passes its own model |

## 7. This experiment does not answer

- Linux or Windows browser-profile qualification.
- Whether a future browser protocol should publish an authenticated
  directory-to-instance edge.
- General browser discovery, activation or default-Profile policy.
- Whether `process.parent.live` implies liveness; it explicitly does not.
- Production schema, persistence location or migration details before a design
  verdict.
- Any tinyvm, dyn ABI or Script Runtime authorization question.

## 8. Result backfill

This section is intentionally empty until execution. A terminal result must
include the V1-V7 table, D1-D3 measurements, the exact decision-tree trace,
every deviation from this specification, the consumed ordinal, reproducible
commands and an explicit statement that criteria were not changed after seeing
the result. A surprising or adverse result is recorded as prominently as a
selected design.

Only after the reviewed terminal receipt may the owning CU PRD and capability
ledger record a selected design or retain the typed TODO with a new terminal
reason.
