# Browser Profile name binding owned-lifecycle decisive experiment

This is a new research precommitment. It does not reopen or amend the exhausted
`research/browser-profile-name-binding/` budget, qualify a provider, change the
`browser.profile-name-binding` ledger cell, remove
`acu.dynamic.075.profile-name-binding`, or turn a human Profile label into
authority.

| Field | Value |
|---|---|
| Date | 2026-09-10 |
| Purpose | Recover a valid decision for human Chromium Profile-name binding after the first experiment exhausted both attempts without proving browser/native-host ownership or preserving its failure stages |
| Implementation | `research/browser-profile-name-binding-v2/` |
| Required reading | `plan/design-browser-profile-name-binding-experiment.md`, `research/browser-profile-name-binding/RESULTS.md`, `prd/PRD_02_28_agenterm_cu.md`, `plan/acu-mcu-capability-ledger.json` |
| Source discipline | Invocation-owned synthetic HOME and browser Profiles only; no real Profile mutation, MCU runtime or fallback |

## 0. Fixed background and competing designs

The following facts are frozen and are not reopened by this experiment.

1. The first precommitment consumed both attempts and ended
   `INCONCLUSIVE_FIXTURE_EXHAUSTED`. Its source, results and
   `budget-exhausted.json` remain immutable history. A v2 run is not a third v1
   attempt.
2. Attempt 1 proved only that no new live bridge connection appeared inside its
   deadline. Its failure receipt did not retain browser brand/version or a
   cleanup proof, so later explanations of that observation are not part of the
   receipt-level result.
3. Attempt 2 reached a live connection, strict status, exact-prefix control and
   provisional model adjudication, then failed a combined `visited=0` and live
   connection `returned=0` cleanup check. The receipt did not retain those
   stages and the court never proved that the connection's native-host process
   belonged to the browser child it stopped. No A0, A1 or B verdict survived.
4. A connection inventory's `visited` count names syntactically eligible
   registry files examined before liveness filtering. `connections` contains
   the identity-validated live rows. A killed contained process tree may leave a
   dead registry file until the invocation-owned synthetic HOME is removed.
   Registry-file hygiene and live-process cleanup are different facts.
5. Current public `browser-session-start --bridge` owns a named synthetic
   Profile, publishes exact owner/browser process identities and launches the
   browser inside native descendant containment. `browser-session-stop` stops
   and reaps that contained tree. V2 uses this lifecycle instead of a raw court
   child.
6. A lowercase-hex `profile_instance_id` prefix remains the only implemented
   exact live selector. No current accepted Profile/connection surface carries
   an authenticated directory-to-instance edge.
7. The three architectural designs remain unchanged:
   - **A0 · archived cardinality alias control:** the relation-free 1-1-1 MCU
     fallback. It has no winning branch; a wrong A-label to B-connection choice
     permanently rejects it.
   - **A1 · verified implicit edge:** automatic selection only when one trusted
     issuer authenticates a collision-resistant, Profile-unique value on both
     the candidate directory and live connection, plus restart and name-safety
     proof.
   - **B · explicit durable binding:** caller-authorized,
     generation-controlled `(application,directory,instance)` binding that is
     revalidated against the complete live inventory on every use.
8. V2 does not extend the accepted candidate/connection surfaces. A1 therefore
   remains a monitored counterfactual: if G4a unexpectedly finds an eligible
   two-sided schema, the frozen surface assumption is false and this experiment stops as
   `INCONCLUSIVE_ACCEPTED_SURFACE_CHANGED`. A later A1 experiment must freeze a
   same-file-object relaunch fixture before it can run G4b/G5. V2 must not use
   remove-plus-create as a counterfeit restart.
9. A result selecting B authorizes a product implementation leaf, not a ledger
   qualification. The existing in-memory G7 model does not prove durable bytes,
   crash recovery, locking, native publication or public black-box behavior.

The experiment accepts either a B implementation direction or a result that
keeps the TODO because ownership, cleanup, surface stability or the B model did
not close. A0 is a negative control, and this frozen V2 surface cannot select
A1. This is deliberate scope, not a post-result preference.

## 1. Hard constraints

1. The old specification, result files, exhausted marker and two historical
   attempt identities are read-only inputs. V2 has a distinct experiment id,
   state root, input digest and budget.
2. Before any decision attempt, the exact frozen V2 source and exact executable
   identities must pass a lifecycle rehearsal that runs no A0/A1/B selector and
   emits no provisional design facts. Rehearsal may use only connection count
   and process identity from the live registry row. It must not call strict
   status, inspect `profile_instance_id`, parse candidate Local State or
   Preferences, run the prefix selector, inventory an identity edge or execute
   the binding model. Other fields present in the registry reply are ignored:
   they cannot be asserted, recorded or used for a branch.
   For V4a/V4b, every registry row is reduced before normalization to the sole
   rehearsal-safe projection `{host_process_identity_digest}`, where that
   digest covers only PID plus start identity under a fixed domain. Completeness
   and row count accompany the set digest. `profile_instance_id`, status,
   protocol/build metadata and every other row field are deleted before hashing
   and cannot affect the rehearsal comparison. Decision-only G1/G2/G6 take
   fresh authorized observations and never reuse this projection.
3. Browser lifecycle uses public `browser-session-start --bridge`, status and
   stop/remove verbs. The session TTL is at least 30 seconds longer than the
   whole runner budget, so TTL expiry cannot impersonate an explicit stop.
4. Before stop, the parent freezes the public session owner PID/start identity,
   browser PID/start identity and the live connection's native-host PID/start
   identity. On macOS the fresh containment identifier is exactly the browser
   PID because the owned launch establishes `setpgid(0, 0)`. Two observations
   must agree that the host belongs to that exact browser containment:
   - the qjswasm host's bounded `process_tree(browser_pid)` contains the host;
   - an independent bounded `ps` projection over only PID, parent PID, process
     group and session reconstructs the transitive parent chain to the same
     browser and confirms the fresh containment group. The scan has a fixed
     row ceiling and rejects truncation.
   No argv, environment, title, URL or user Profile is read.
   The independent witness is one fresh snapshot of at most 4,096 rows. It must
   show `browser.pgid == browser.pid`, `host.pgid == browser.pid`, equal nonzero
   browser/host session identifiers and a cycle-free, unbroken parent chain from
   host to browser. The public `process-state` identities must still match on
   both sides of that snapshot, and the host must also occur in the bounded
   qjswasm tree. Missing, reparented, repeated, cyclic or truncated rows, an
   identity change or a containment mismatch is `INCONCLUSIVE_OWNERSHIP`.
   `process_tree` is a genealogy witness, not a substitute for the process-group
   containment predicate. Genealogy without the frozen process-group predicate
   remains `INCONCLUSIVE_OWNERSHIP`, not a browser-model exception, because the
   public stop owns and reaps that exact group; a host outside it is outside the
   lifecycle being tested.
5. Process identities are validated again with public `process-state`. A PID
   reuse with a different start identity means the frozen object is gone; an
   unknown observation is inconclusive, never dead.
6. Cleanup is one ordered pipeline: public stop; V3a
   `browser-owner-stopped` plus `frozen-host-gone`; V4b
   `live-inventory-baseline`; the non-voting `registry-hygiene-recorded`; then
   V3b removal and independent absence of the synthetic state root. V3 is the
   aggregate of V3a and V3b but does not own or reuse V4b's inventory fact. V3a,
   V4b and V3b are validity gates. Registry hygiene records the `visited` delta
   but does not confuse a dead invocation-owned file with a live connection.
   `browser-session-stop` proves contained process-group termination and reap.
   It does not prove a graceful browser exit or Native Messaging stdio EOF, and
   no V3 result may be cited as either.
7. Before the next side effect, every stage is added to a bounded, redacted
   journal that is atomically replaced outside the run lane and read back for
   exact equality. A failure receipt includes the last completed stage, current
   deadline, typed code and cleanup facts already proven. It never deletes its
   only copy before review. The experiment claims process-crash and lane-loss
   persistence, not power-loss durability unavailable through the frozen qjs
   file API.
8. The attempt ledger and authoritative stage/terminal receipt live under
   `.agenterm-research-state/browser-profile-name-binding-v2/`, outside the
   disposable Cargo/run lane. A corresponding ignore rule must exist before
   implementation. The external ledger is authoritative; a lane receipt is
   only a disposable mirror. A lane claim absent from the external ledger, or a
   same-attempt digest disagreement, is
   `profile_binding_v2_state_inconsistent`, never a fresh attempt. An external
   record with no lane mirror still prevents reuse of its budget. The runner
   opens the state root and its no-follow exclusive lock without following
   symlinks, then validates and compare-and-reserves under that lock. Each ledger
   row has exactly `{schema, experiment, kind, ordinal, run_id, source_sha,
   input_digest, status, receipt_sha256, terminal_code}`. `kind` is
   `rehearsal` or `decision`; ordinals are immutable `R1`, `R2` and `D1`; status
   is `reserved`, `finished` or `abandoned`. A `reserved` row has JSON null for
   both terminal fields. A `finished` row has a typed terminal code and follows
   the receipt-digest rules below. An independently audited `abandoned` row has
   JSON null `receipt_sha256` and terminal code
   `attempt_abandoned_after_independent_audit`; it records history but is never
   a design result.
9. Admission atomically publishes and reads back the external `reserved` row
   before any side effect; only that successful publication consumes the named
   attempt. Any `D1` reservation consumes the sole decision attempt. A residual
   reservation refuses every run until an independent audit appends an
   `abandoned` row for the same kind, ordinal and run id; the runner never heals,
   renames or reuses it automatically. Every later stage updates the external
   receipt first and the lane mirror second. A mirror failure leaves the
   authoritative last-complete stage intact with a typed failure. The runner
   prints each reserved/finished/abandoned ledger row and the SHA-256 of the
   validated external receipt. Ordinarily, `finished` is appended only after
   that receipt is readable and its digest matches. A journal, authoritative
   receipt or lane-mirror persistence failure is
   `INCONCLUSIVE_EVIDENCE_PERSISTENCE`. While the ledger remains writable, the
   runner appends a `finished` row with that terminal code; `receipt_sha256` is
   the validated external receipt digest when one exists and JSON null when it
   could not be persisted. This typed failure is the sole exception to the
   ordinary receipt-before-finished rule. If the ledger itself cannot append
   that terminal row, the reserved row remains authoritative and the runner
   emits one fixed JSON stdout record with the same experiment, kind, ordinal,
   run id, source/input digests and code, then exits nonzero; it never reports
   an unused attempt.
10. The synthetic browser root is canonicalized. Candidate keys reject
    absolute paths, empty components, `.`/`..`, separators, symlinks and
    over-limit files. Each root records its canonical path, ancestor symlink
    audit and platform file-object identity. “Outside scan roots” requires a
    distinct file-object ancestry as well as a non-descendant canonical path;
    lexical prefix comparison alone cannot pass.
11. Browser family and version come from the browser's bounded `--version`
    output and exact executable digest. A frozen prefix table accepts only
    `Chromium`, `Chrome for Testing`, `Google Chrome`, `Brave Browser`,
    `Brave Browser Beta` or `Microsoft Edge`; unknown, empty or timed-out output
    is typed dependency failure. Path substrings do not classify brand.
12. Complete connection inventories are mandatory. Truncation, malformed rows,
    unknown process identity or baseline drift is typed inconclusive. The final
    comparison preserves every baseline row rather than assuming the machine
    globally has zero connections. Before selection, current must equal all
    baseline identity rows unchanged plus exactly the owned B row. A
    missing/changed baseline row or any extra non-owned row is
    `INCONCLUSIVE_BASELINE_MUTATION`; malformed or truncated inventory is
    `INCONCLUSIVE_INVENTORY`. Thus G2b's one B connection is the exact owned
    delta, not an assertion that the global baseline was empty.
13. G2 is split by source. G2a takes application, directory, display name,
    last-used and ordering facts from the public `browser-profiles` reply and
    cross-checks their overlap against an independent bounded Local State
    parser. G2b derives the N/I installed-candidate rows from an independent
    bounded Preferences parser because the public reply does not expose those
    extension-installation facts. Each parser records an input digest and
    actual row count. Hand-authored candidate arrays cannot vote.
14. No repair may add LevelDB parsing, command-line scanning, browser
    activation, a new browser permission, account identity, real Profile
    mutation or title guessing. Wanting one is a detected missing edge, not a
    reason to widen A0 or A1.
15. One A-label to B-connection choice without the full A1 edge/restart/name
    proof kills that implicit design. Aggregate success and archived parity
    cannot outweigh it.
16. No resolver, compatibility mapping, persistent binding store, ledger state,
    evidence registration or PRD completion changes before an independently
    reviewed terminal receipt exists.

The disease detector for this experiment is any attempt to make the fixture
green by weakening exact process ownership, treating a stale registry file as a
live host, broadening accepted identity surfaces, or moving the only receipt
back under a disposable run directory. Such pressure is a finding, not a
requirement to satisfy.

## 2. Minimal experiment content

| Dimension | Frozen choice | Why |
|---|---|---|
| Browser | One explicitly supplied current-host Chromium-family test binary, exact digest and bounded version string | Separates the tested binary from a human-installed default and makes brand policy visible |
| Profile lifecycle | One public named `browser-session-start --bridge` session in a synthetic HOME | Reuses the product's native containment and identity-bearing owner instead of an unowned raw child |
| Candidate A | `Profile 1`, display name `Work`, fixed-extension record under the one synthetic catalog root; it remains idle in the negative trap | Arms the archived cardinality fallback without touching a user Profile |
| Live Profile B | The contained session's different owned profile, outside every canonical candidate root | Gives the only live connection a known non-A owner |
| Ownership witness | Public session/connection identities + qjswasm `process_tree` + independent bounded `ps` relation | Proves the exact native host stopped later is part of the browser containment |
| Candidate witness | G2a public `browser-profiles` + independent Local State parser; G2b independent bounded Preferences parser | Keeps public Profile facts and extension-installation eligibility in their real evidence domains |
| Cleanup | Explicit session stop, exact frozen-host death, live inventory restoration, separate registry hygiene, then raw synthetic-root removal | Makes each failure diagnosable and prevents stale files from masquerading as live processes |
| Decision | V1/V4a/V2/G1/G2a/G2b/G6 validate the run; G4a/G4c stop on a changed or contradictory surface; G3 rejects A0; G7 tests B; V3/V4b close every lifecycle branch | Changes the broken measurement without fabricating a restart path |
| Persistent evidence | Stage journal, receipt copy and attempt ledger outside the run lane | A cleanup or lane loss cannot erase that a budget was consumed |

### 2.1 Lifecycle rehearsal

The rehearsal executes only these stages:

1. freeze source, executable/browser identities and complete baseline inventory;
2. start one contained bridge session and observe its exact ready identities;
3. observe one identity-valid live registry row, using only count and process
   identity for the rehearsal;
4. prove browser-to-native-host containment with both witnesses;
5. stop the public session;
6. run V3a and prove the exact owner, browser, host and frozen group are gone;
7. run V4b's third fresh inventory and compare it directly with baseline;
8. record registry hygiene, then run V3b to remove the invocation-owned state
   and prove absence.

The rehearsal receipt contains no N/N_all/I/E values, no A0 selection and no G7
result. At most two rehearsal attempts are permitted. Each has its own numbered
receipt and `reserved`/`finished` ledger rows; a later rehearsal never overwrites
an earlier artifact. A successful rehearsal freezes its source SHA, input
digest and the exact `agenterm` qjswasm host, `agenterm-cu` and browser
executable digests; the single decision attempt must match them byte-for-byte.
Any source repair requires a new rehearsal and may not edit the decision
criteria.

G1 remains decision-only by design. Its full predicate requires the strict
status reply, which is excluded from rehearsal because it carries selector
fields. Rehearsal already freezes bounded browser identity, executable digest
and lifecycle readiness; moving only those G1 fragments earlier would not prove
G1 and must not be reported as a G1 pass. This preserves rehearsal blindness at
the accepted cost that a strict-status dependency failure can consume D1.

### 2.2 Decision attempt

The decision attempt repeats the same lifecycle and adds only:

- the independently cross-checked candidate observation;
- the complete edge inventory;
- the exact-prefix control;
- the A0 negative trap;
- a typed surface-change stop if an eligible two-sided schema unexpectedly exists;
- the frozen G7 explicit-binding model.

No result from the rehearsal is copied in as a decision fact. The decision run
must re-observe every validity fact.

## 3. Precommitted criteria

| ID | Criterion | Nature | Pass | Failure |
|---|---|---|---|---|
| V1 | Frozen identity and state chain | Boolean gate | A rehearsal independently agrees on and freezes its source/input/executable digests; a decision matches one successful rehearsal byte-for-byte; authoritative external state is valid and any lane mirror present agrees | `INCONCLUSIVE_SOURCE` |
| V2 | Browser/native-host ownership | Safety gate | Exact connection host is live with its recorded start identity and both independent relations place it inside the exact browser containment before stop | `INCONCLUSIVE_OWNERSHIP` |
| V3 | Contained cleanup envelope | Safety gate | V3a proves public stop succeeds, exact public owner/browser/host objects are gone and a fresh bounded process snapshot has no frozen-group member; after V4b and hygiene, V3b proves the owned synthetic root absent | `INCONCLUSIVE_CLEANUP` |
| V4a | Pre-selection observations | Boolean gate | Two fresh public baseline/current inventories use the frozen identity-only projection and are complete and well formed; current is exactly every unchanged baseline identity row plus the single owned B row, with no other delta; normalized projection digests are retained | Malformed/truncated input is `INCONCLUSIVE_INVENTORY`; a missing/changed baseline row or extra non-owned row is `INCONCLUSIVE_BASELINE_MUTATION` |
| V4b | Post-stop observation | Boolean gate | A third fresh public inventory uses the identical identity-only projection, is complete and well formed and equals baseline directly without reusing a provisional object | `INCONCLUSIVE_INVENTORY` |
| G1 | Fixture identity | Boolean gate | Exact extension/status identity, bounded browser identity and flushed construction trace | `INCONCLUSIVE_DEPENDENCY` |
| G2a | Public Profile observation | Boolean gate | Public application/directory/name/last-used/order facts agree with the independent Local State overlap and make A the sole N_all member | `INCONCLUSIVE_CANDIDATE_OBSERVER` |
| G2b | Installed-candidate observation | Boolean gate | Bounded Preferences installation rows intersected with G2a make A the sole N and I member; B is outside roots; the current inventory delta is exactly the one owned B connection | `INCONCLUSIVE_FIXTURE` |
| G3 | A0 control | Safety | Exact archived 1-1-1 algorithm selects B and is permanently rejected, or its refusal is recorded as not reproduced | `REJECT_ARCHIVED_CARDINALITY_ALIAS` or `REJECT_A0_NOT_REPRODUCED`; A0 has no winning branch |
| G4a | A1 edge-schema eligibility | Three-valued checklist | Inventory every accepted candidate and connection field schema; each pair/property is proved yes, proved no or unknown for shared issuer, authentication, collision resistance, injectivity and Profile uniqueness | After all pairs are classified, aggregate in fixed `INCONCLUSIVE_GROUND_TRUTH_CONFLICT` > `INCONCLUSIVE_ACCEPTED_SURFACE_CHANGED` > `INCONCLUSIVE_EDGE_ELIGIBILITY_UNKNOWN` > all-ineligible order |
| G4c | Frozen ground-truth conflict | Safety gate | The known-different A and B objects do not present equal values through an eligible two-sided schema | An eligible schema whose observed A/B values are equal is `INCONCLUSIVE_GROUND_TRUTH_CONFLICT` |
| G6 | Exact-prefix control | Boolean | The private 12-hex prefix resolves only the exact B connection | `INCONCLUSIVE_FIXTURE` |
| G7 | Explicit-binding architecture model | Safety | Seven operations and authority/conflict fault matrix close under the frozen model | `INCONCLUSIVE_BINDING_DESIGN` |

The stage journal records producer, monotonic sequence, elapsed time, typed code
and frozen redacted facts for every row. Process and instance identities appear only as
domain-separated digests. Cardinalities, fixed fixture labels, browser
family/version, executable digests and registry `visited` deltas may be
reported directly.

The validity producers are frozen independently:

| Gate | Producer and forced cross-check |
|---|---|
| V1 | Before compare-and-reserve, the outer runner computes source/input/executable digests and invokes the court's no-side-effect preflight to recompute and compare every digest independently; the outer runner then reacquires the state lock and refuses if state or frozen inputs changed before reservation |
| V2 | qjswasm `process_tree` supplies genealogy and one bounded numeric-only `ps` snapshot supplies the exact process-group/session predicate, bracketed by public `process-state` identity checks |
| V3 | V3a uses public stop/`process-state` plus a fresh post-stop `ps` snapshot for exact-object/group termination; after V4b and hygiene, V3b uses a fresh filesystem witness for owned-root absence |
| V4a | Two fresh public inventory calls first reduce every row to the frozen identity-only projection, then produce baseline/current projection sets and digests; a set-difference check permits only the exact owned B row and proves every baseline projected identity byte-stable |
| V4b | A third fresh post-stop inventory uses the identical identity-only projection and compares that projection set directly with baseline; it cannot reuse the current inventory object or a provisional selector result |

G4a first classifies every pair in the Cartesian product of accepted candidate
and connection field schemas against the five required properties. `eligible`
means all five are proved yes from a frozen source clause; `ineligible` means
at least one is proved no; every remaining combination is `unknown`. Only
after the complete table exists is it reduced with this fixed priority:
eligible plus equal observed A/B values returns G4c
`INCONCLUSIVE_GROUND_TRUTH_CONFLICT`; otherwise any eligible pair returns
`INCONCLUSIVE_ACCEPTED_SURFACE_CHANGED`; otherwise any unknown returns
`INCONCLUSIVE_EDGE_ELIGIBILITY_UNKNOWN`; only all-ineligible reaches A0. The
receipt records only schema names, three-valued property outcomes,
source-clause digests, an `observed_values_equal` boolean and a
domain-separated ground-truth-pair digest, never field values.

G7 freezes two separately digested inputs: the executable model and its fault
matrix. Its seven operations are `create`, `resolve`, `restart-revalidate`,
`target-stale`, `collision`, `generation-conflict` and `revoke`. The matrix must
prove active-pair uniqueness, at most one active alias per instance, typed
duplicate-alias rejection, identity/build staleness, typed zero/multiple live
inventory, explicit request/runtime-session/session-lease authority,
generation compare-and-swap, explicit replacement on rename and typed
not-found revocation. A model/matrix digest disagreement or any missing row is
`INCONCLUSIVE_BINDING_DESIGN`.

## 4. Decision tree, kill criteria and timebox

1. Refuse the decision run unless one lifecycle rehearsal on the same frozen
   bytes has V1, V2, V3, V4a and V4b all true. A rehearsal failure cannot
   select a design.
2. V1 is a no-side-effect pre-admission handshake. Its runner/court digest
   comparison happens before reservation; the runner reacquires the exclusive
   lock and revalidates the frozen inputs and state immediately before append.
   A V1 failure refuses before appending a
   decision reservation and consumes no decision attempt. After reservation,
   evaluate V4a, V2, G1, G2a, G2b and G6 before any selector. Any failure
   returns its typed inconclusive result, executes the ordered
   V3a → V4b → hygiene → V3b pipeline for every lifecycle resource already
   created and consumes the sole decision attempt.
3. Inventory and classify every G4a pair before A0, then apply the frozen
   aggregate priority. Any eligible pair with equal observed A/B values returns
   G4c `INCONCLUSIVE_GROUND_TRUTH_CONFLICT` with only its domain-separated
   ground-truth-pair digest and redacted-safe schema names. Otherwise any
   eligible pair returns `INCONCLUSIVE_ACCEPTED_SURFACE_CHANGED`; otherwise any
   unknown pair returns `INCONCLUSIVE_EDGE_ELIGIBILITY_UNKNOWN`. Only a
   complete all-ineligible inventory permits A0 to run.
4. Run the exact archived A0 algorithm. If it chooses B, record
   `REJECT_ARCHIVED_CARDINALITY_ALIAS`; if it refuses, record
   `REJECT_A0_NOT_REPRODUCED`. Continue either way; A0 cannot win.
5. After G4a proves every schema pair ineligible, record A1 as rejected on the
   frozen surface and continue. Do not improvise a restart or name test after
   observing the inventory.
6. Run G7. It returns
   `SELECT_EXPLICIT_DURABLE_BINDING` only when every operation and fault
   invariant passes; otherwise return `INCONCLUSIVE_BINDING_DESIGN`.
7. For every lifecycle resource created, always run public stop and V3a, then
   V4b, record hygiene, and finally run V3b. The receipt keeps the first failure
   as `primary_cause`, records V3a/V3b in `cleanup_status`, and records V4b in
   `final_inventory_status`. A later failure becomes terminal only when there
   was no earlier primary cause: `INCONCLUSIVE_CLEANUP` for V3a/V3b or
   `INCONCLUSIVE_INVENTORY` for V4b. Otherwise the earlier typed cause remains
   terminal and each later failure is a second explicit fact. A computed
   decision remains non-authoritative whenever V3a, V4b or V3b is not proved.

**Kill criteria:** one implicit wrong selection permanently rejects that design;
one ownership disagreement, process-identity unknown, baseline mutation or
ground-truth conflict ends the run inconclusive. A receipt-copy or ledger-write
failure is `INCONCLUSIVE_EVIDENCE_PERSISTENCE`, ends the run without a design
verdict and leaves at least the authoritative reserved row or fixed stdout
failure record visible.

**Timebox:** at most two lifecycle rehearsals and one decision attempt. Each
rehearsal is bounded to 180 seconds. The decision attempt is bounded to 240
seconds with explicit sub-deadlines: source/preflight 10 seconds, baseline 5,
session ready 30, connection 20, ownership 5, each of
G1/G2a/G2b/G4a/G3/G6/G7 once at 5 seconds, stop 20, host disappearance 15,
final live inventory 15 and final cleanup 20. There are no decision-stage
retries; bounded observation
polls remain inside their owning stage and outer deadline. At session start the
TTL must be at least the remaining outer deadline plus a 30-second cleanup
margin and never less than 300 seconds. Do not implement a product binding
store or resolver before the terminal decision is independently reviewed.

Every criterion occurs in the tree: V1/V2/V4a and G1/G2a/G2b/G6 validate the
run before selection; G4a/G4c check the frozen no-edge and different-object
assumptions; G3 rejects A0; G7 decides B readiness; V3/V4b are mandatory after
every lifecycle branch. G4b/G5 are explicitly not criteria in this
V2 experiment because there is no admitted edge or same-file-object relaunch
fixture. Registry hygiene is deliberately an inventory output rather than a
vote because it describes dead owned files, not live selector state.

## 5. Planned research layout

```text
research/browser-profile-name-binding-v2/
├── README.md
├── result-template.json
├── run-current-host.sh
├── court-current-host.qjs
├── binding-model.qjs
├── fixtures/
│   ├── local-state.json
│   └── preferences.json
└── RESULTS.md

.agenterm-research-state/browser-profile-name-binding-v2/
├── state.lock
├── attempt-ledger.jsonl
├── rehearsal-1-receipt.json
├── rehearsal-2-receipt.json
├── decision-1-receipt.json
└── stage-journal/
    ├── rehearsal-1.jsonl
    ├── rehearsal-2.jsonl
    └── decision-1.jsonl
```

The state directory is ignored but not disposable. The runner refuses a dirty
tracked input, an untracked research input, a source not reachable from
`origin/main`, a digest mismatch, a duplicate JSON key, a missing external
state counterpart or an attempt outside the frozen budget. Raw fixture data
stays in the disposable run lane and is removed after the redacted receipt copy
has been validated.

Every journal row has exactly `{schema, experiment, kind, ordinal, run_id, seq,
prev_sha256, stage, producer, deadline_ms, elapsed_ms, code, facts}` and uses
RFC 8785 canonical JSON bytes. It is chained from a domain-separated zero
predecessor through the SHA-256 of each canonical row. A journal is limited to
64 rows, 32 KiB per canonical row and 1 MiB total; reaching or exceeding any
limit is `INCONCLUSIVE_EVIDENCE_PERSISTENCE`, never truncation.
The matching terminal receipt names the journal's final sequence and digest;
both journal and receipt remain after `finished`. A stage update is complete
only after the external journal has been atomically replaced, read back and
matched. A rehearsal or decision can never share or overwrite another
ordinal's journal or receipt.

`stage` is a closed member of
`preflight`, `baseline`, `session-ready`, `connection-ready`, `fixture`,
`ownership`, `candidate`, `surface`, `a0-control`, `prefix-control`,
`binding-model`, `stop`, `termination-proof`, `final-inventory`,
`registry-hygiene`, `root-removal` or `terminal`. Each `code` is a typed
stage outcome.
`facts` is an object restricted by stage:

| Exact stage members | Allowed redacted facts |
|---|---|
| preflight | source/input/executable/browser digests, browser family/version and state-chain booleans |
| baseline, final-inventory, registry-hygiene | row count, normalized-row digest, completeness, baseline-preserved and owned-delta booleans, plus registry `visited` count only for registry-hygiene |
| session-ready, connection-ready | domain-separated owner/browser/host/connection identity digests and readiness/completeness booleans |
| fixture | fixed extension id, normalized strict-status digest, extension/status/build identity booleans, native-host build/identity digest and construction-trace digest/completeness |
| ownership | domain-separated owner/browser/host/connection identity digests, tree/ps row counts, exact PGID/SID/chain booleans and public identity-match booleans |
| candidate | Local State and Preferences input digests/counts and N/N_all/I/C cardinalities |
| surface | schema names, three-valued property outcomes, source-clause digests, `observed_values_equal` and domain-separated ground-truth-pair digest |
| a0-control, prefix-control, binding-model | typed A0/G6 outcomes, model/matrix digests and the seven operation/fault booleans |
| stop | public stop invocation result only |
| termination-proof | exact owner/browser/host/group absence booleans and post-stop ps/process-state counts |
| root-removal | root-removal result and independently observed root-absence boolean |
| terminal | `primary_cause`, `cleanup_status`, `final_inventory_status` and terminal code |

No other fact key is accepted. Identity digests use fixed domain strings named
by the result template; raw PID, start identity, instance value, path, title,
URL, argv, environment and Profile content cannot enter the external journal.
The frozen `result-template.json` is the sole machine schema for every facts
object's exact keys, scalar types and required/optional status. It is included
in V1's input digest, recursively compared by the runner and court, and rejects
unknown or missing required keys before a terminal receipt can be accepted.

## 6. Excluded options

| Option | Reason excluded |
|---|---|
| Run the old court a third time | Its budget is exhausted and its validity oracle is known broken |
| Treat one singleton candidate and connection as an edge | Cardinality supplies no relation |
| Treat owned-session identity as arbitrary human-Profile identity | It proves only the object the caller created and owns |
| Require registry `visited=0` before declaring the host dead | Dead record hygiene is not live process identity and recreated the old false inconclusive |
| Ignore dead registry files | Their bounded delta is still recorded and final owned-root removal is proved |
| Read LevelDB, argv, account, title or active tab | New privacy/locking/heuristic surfaces that do not authenticate directory identity |
| Relaunch by remove + fresh create for G4b | A new Profile file object is not a same-directory restart |
| Let the G7 in-memory model qualify B | It lacks durable/public implementation evidence |
| Keep receipt and budget only under `target/` | A cleanup can erase both evidence and attempt history |
| Repair after seeing the decision result | It breaks the precommitment; only pre-decision rehearsal repair exists |

## 7. This experiment does not answer

1. Linux or Windows browser-profile qualification.
2. Whether aliases should be shared across browser applications.
3. Whether account synchronization preserves `profile_instance_id`.
4. Whether an accepted future extension API could provide an A1 edge.
5. The persistent-store schema, UI or migration details after a B decision.
6. Whether dead registry files should be proactively garbage-collected outside
   invocation-owned synthetic state.
7. Browser brand compatibility beyond the exact binary named in the receipt.

## 8. Result write-back

After the frozen decision run, append the exact V1/V2/V3 (including V3a/V3b),
V4a/V4b and
G1/G2a/G2b/G3/G4a/G4c/G6/G7 trace here and write the reproducible result to
`research/browser-profile-name-binding-v2/RESULTS.md`. The result must include:

1. source, input, browser and executable digests plus browser brand/version;
2. the successful rehearsal receipt digest and the full external ledger;
3. every stage producer, deadline, elapsed time and typed outcome;
4. baseline/current/final live cardinalities and separate registry hygiene;
5. the ownership proof and exact-object cleanup booleans;
6. the decision-tree path, A0 subdecision, G4a/G4c surface decisions and G7
   result, using the literal `not-run` for every unreached criterion;
7. every deviation from this specification and whether it weakens the result;
8. a statement that no criterion was changed after observing decision data;
9. third-party reproduction commands and independently recomputed reference
   digests.

Every criterion key is present in a terminal receipt. A criterion not reached
by the selected branch is the literal string `not-run`, never omitted, null or
silently inferred.

A surface-change result must name the eligible field class without disclosing
its value and must not call it an A1 selection. A B result must say **explicit
durable alias binding** and identify the product implementation boundary; it may
not call a human name a cryptographic identity. An inconclusive result leaves
the stable TODO and gap unchanged. No result becomes product evidence until the
implementation and its own public black-box court ship.

## 9. Terminal record

The precommitment closed on 2026-09-11 without a design verdict. Both permitted
rehearsals finished inconclusive, neither produced `REHEARSAL_PASS`, and D1 is
machine-ineligible: the immutable R1 and R2 ordinals are both consumed, and no
successful rehearsal exists for a decision run to match. A0, A1 and B were not
selected. No product evidence was registered, and the stable TODO and
capability state remain unchanged.

The complete attempt identities, ledger, stage traces, unavailable result
items, deviations and reproduction boundary are recorded in
`research/browser-profile-name-binding-v2/RESULTS.md`. This section is
post-experiment history, not part of either completed run's frozen input. No
criterion changed after either run. This harness must not be repaired or rerun
in place; any further investigation requires a new precommitment and new
budget.
