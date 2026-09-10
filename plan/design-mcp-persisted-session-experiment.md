# MCP persisted authorization/session experiment

> Research decision only. It does not advertise an MCP mutation tool, change
> the public catalog, or relax any authorization gate.

| Field | Value |
|---|---|
| Date | 2026-09-10 |
| Purpose | Decide whether the first MCP shell mutation retains a bounded scoped session or dispatches persisted-authorized requests directly |
| Implementation | `research/mcp-persisted-session/` |
| Read first | `prd/PRD_02_31_cu_authorization_safety.md`, `docs/agenterm-rust-cheatsheet.md` |
| Source discipline | One shared fake effect mechanism; no GUI, credential, network or real shell payload |

## 0. Fixed facts and alternatives

1. Production MCP mutation remains undiscoverable and unreachable throughout
   the experiment.
2. A human starts the sidecar with one explicit persisted grant id. MCP cannot
   create, select, list, revoke or replace grants.
3. Ambient `AGENTERM_CU_GRANT*` / `AGENTERM_CU_AUTH*` never authorizes this
   path.
4. JSON-RPC id owns transport cancellation. Caller `idempotency_key` owns
   durable effect identity across reconnects.
5. Grant expiry, revocation, target/session binding, operation binding and use
   accounting remain owned by the existing persisted grant store.
6. A fresh durable caller request is reserved before a matched persisted-grant
   attempt is consumed. Final replay and retained uncertainty consume no later
   grant use.
7. Runtime-session job/device ownership is out of scope and may not be weakened
   to make a shell-only model pass.

The experiment evaluates two real alternatives and one negative control:

```text
MCP persisted shell mutation
├─ A0 · same-grant lifecycle control (expected rejection)
│  └─ tries to authorize session-start/shell/renew/end with a shell-only grant
├─ A1 · bounded scoped-session exchange
│  ├─ a first fresh request verifies the named grant and current binding
│  │  without consuming it, then lazily creates the connection session
│  ├─ creates in-memory session state bound to grant digest, exact shell
│  │  operation, current binding, deadline and configured call ceiling
│  ├─ lifecycle manages only that derived object and consumes zero grant uses
│  └─ every shell effect still reserves its caller request and then consumes
│     exactly one persisted-grant attempt
└─ B · request-direct
   ├─ connection nonce is transport state, not authorization
   ├─ no provider session-start/renew/end exists for this shell-only path
   └─ every shell effect reserves its caller request and then consumes exactly
      one persisted-grant attempt
```

A1 is not free internal Actuate: it cannot add an operation, target, use,
deadline or authority source; it cannot dispatch an effect without a fresh
persisted-grant attempt. A0 proves why applying the shell grant directly to
lifecycle operations is invalid, but A0 never votes for A1 or B.

An A1 scoped session belongs to exactly one MCP transport connection. Its
connection nonce is routing state, not authority. Connection admission creates
no session and performs no grant check. Every request first queries durable
caller-request state: retained final, uncertainty or fingerprint conflict
returns immediately without startup verification or an effect attempt. Only a
key with no durable request may verify the named grant and current binding,
then lazily create a new session without consuming a use. Disconnect destroys
that session; a later connection creates a new nonce and repeats the same
request-first ordering. The experiment has no renewal: the fixed 60-second
session deadline exceeds one bounded case, and expiry or disconnect requires
teardown and a later lazy exchange.

## 1. Hard constraints

- A one-shot exact-`shell-exec` grant must execute one shell effect. Startup,
  teardown and transport bookkeeping must consume zero uses. A0 separately
  proves that each lifecycle operation, including renew, is rejected rather
  than authorized by that grant.
- Same grant + idempotency key + payload after reconnect must never repeat an
  effect or consume another use, whether the retained request is final or
  uncertain.
- A revoked, expired, not-yet-valid, wrong-target, wrong-session,
  wrong-operation or exhausted grant causes zero mechanism attempts. Mismatch
  denials consume zero uses.
- Cancellation after dispatch never changes an authoritative result into a
  refusal and never redispatches an uncertain effect.
- Broken stdout while stdin remains open must enter bounded teardown.
- No lease, target/session binding, installation key, command output,
  credential-like value, store path or complete grant id enters stdout, stderr,
  audit or result projections.
- Existing session-bound job/device ownership semantics remain byte- and
  behavior-unchanged.
- Any unscoped internal authority, ambient fallback or caller-constructible
  trusted identity kills that variant instead of widening the prototype.

## 2. Identical fixtures and execution order

The harness variants consume byte-identical initial fixtures. The runner writes
one canonical fixture bundle containing the fake current binding, grant store,
durable request store and failure schedule. Every case copies that bundle to a
fresh invocation-owned directory before starting either variant.

Fixed values are identical between A0, A1 and B: grant id, exact current
binding, exact `shell-exec` operation, issued/not-before/expiry times,
generation, maximum uses, idempotency key, payload digest, marker bytes and
failure phase. A case runs A0, A1 and B in that order against separate copies;
order is recorded and cannot share state. The clock is fixed within a case;
only X2 advances it, by one frozen offset beyond expiry that is recorded in the
receipt.

The fake effect appends one opaque fixed marker through the shared mechanism.
On every invocation, that mechanism increments and durably records its own
attempt counter before any injected check or marker append. Variants have no
write access to that counter; every reported `mechanism_attempts` value comes
from the independent mechanism record and never from a variant claim. The
mechanism returns no shell output. No case invokes a real command, GUI, network
endpoint or product MCP descriptor.

The shared grant-store fixture likewise owns two bounded records. A read-only,
zero-use session check appends `startup-verification`; every grant-attempt
reservation appends one `effect-attempt` authorization or denial code before it
returns. Variants can call the store API but cannot write either record. Results
take counts and codes only from these store-owned observations. This
distinguishes startup validation, a real persisted-grant effect decision and an
over-strict local/session prefilter that happens to refuse with similar text.

## 3. Durable phase matrix

The same five failure phases are applied to the ordinary one-shot success case
for A1 and B. A0 runs only the ordinary one-shot case because its purpose is the
lifecycle-operation rejection. The seven fresh C3 denial fixtures each run once
and do not multiply across this phase matrix because their required refusal
precedes effect dispatch. A1 also runs the post-exchange attacks in section 4;
those are distinct from the phase matrix.

| Phase | Injected stop | Required retained state | Marker / uses | A1 startup, initial / reconnect | Effect decisions, initial / reconnect | Same-key reconnect |
|---|---|---|---|---|---|---|
| F-1 | after connection admission, before caller-request reservation | no caller-request or grant-attempt/use state; one startup-verification observation only | 0 / 0 | 1 / 1 | 0 / 1 authorized | dispatch once, consume one use and append one marker |
| F0 | after fresh caller-request reservation, before grant attempt | request uncertain; grant untouched | 0 / 0 | 1 / 0 | 0 / 0 | refuse retained uncertainty; no dispatch |
| F1 | after grant attempt reservation, before effect | request uncertain; grant attempt retained | 0 / 1 | 1 / 0 | 1 authorized / 0 | refuse retained uncertainty; no second use or dispatch |
| F2 | after effect, before durable completion | request uncertain; effect may already exist | 1 / 1 | 1 / 0 | 1 authorized / 0 | refuse retained uncertainty; no second use or effect |
| F3 | after durable completion, before stdout delivery | exact final reply cached | 1 / 1 | 1 / 0 | 1 authorized / 0 | return byte-equal authoritative reply; no second use or effect |

`uncertain` is not a replayable result. A different payload under the same key
is a typed conflict in every phase and never dispatches. Every phase also runs
one stdout-loss case with the input writer retained open until the tested server
returns.

For F-1, A1 records one successful `startup-verification` on the initial fresh
request and one on the reconnect because no durable request was created: 1/1.
For F0-F3, A1 records 1/0 because the reconnect finds retained request state
before any exchange. B records 0/0 for every row. These startup counts are
separate from the effect-decision column and consume no use. C2 requires both
sets of exact deltas, so a retained request stops A1 before startup verification
and any new `effect-attempt`.

## 4. Predetermined criteria

| ID | Nature | Criterion |
|---|---|---|
| C1 | Boolean/security | an ordinary one-shot exact-shell grant produces one marker, one consumed use, exactly one authorized `effect-attempt` decision and one authoritative reply; lifecycle-use deltas are zero |
| C2 | Boolean/replay | every F-1 through F3 row matches the exact retained-state, marker/use and reconnect result in section 3 |
| C3 | Boolean/refusal | every fresh denial makes A1 record one matching `startup-verification` denial and zero effect decisions, while B records one matching `effect-attempt` denial; every A1 post-exchange attack records one matching `effect-attempt` denial; all have zero tested mechanism attempts and the specified zero marker/use delta |
| C4 | Boolean/cleanup | with stdin still open, stdout loss returns within 5 seconds, cancels queued work, reconciles dispatched work and leaves zero owned child, session, queue and connection records |
| C5 | Slope/accounting | independent ceilings exercise 0, 1 and 2 successful calls; effect-use and authorized `effect-attempt` deltas are each exactly 0, 1 and 2 while A1 startup/end grant-use deltas and every A0 lifecycle-rejection use delta remain zero |
| C6 | Inventory/privacy | enumerate every trusted input, in-memory authority field, durable field, public field and cleanup state; forbidden values and path/credential patterns have zero matches in all projections |
| C7 | Compatibility | a source and protocol inventory proves the experiment changes no product source, descriptor or catalog; job/device commands are not constructible through either prototype |

Each use-count sample records four values: before lifecycle, after lifecycle,
before effect and after effect. Startup-verification and effect-decision counts
are sampled at the same four points. Cleanup receives no authority to consume
or refund the tested grant.

A1 must run these post-exchange attacks after it has created the derived session
but before the tested request reaches its persisted-grant attempt:

| Attack | Mutation after exchange | Required tested-request result |
|---|---|---|
| X1 | revoke the named grant | distinct revoked denial; marker delta 0; use delta 0; mechanism attempts 0 |
| X2 | advance the injected clock beyond grant expiry | distinct expired denial; marker delta 0; use delta 0; mechanism attempts 0 |
| X3 | replace the current target/session binding | distinct binding-mismatch denial; marker delta 0; use delta 0; mechanism attempts 0 |
| X4 | a separate completed ordinary exact-`shell-exec` request under a different idempotency key, with the same canonical operation and current binding, consumes the last use | distinct exhausted denial; tested marker delta 0; tested use delta 0; tested mechanism attempts 0 |
| X5 | submit a different canonical operation through the established session | distinct operation-mismatch denial; marker delta 0; use delta 0; mechanism attempts 0 |

The X4 setup request's marker, mechanism attempt and one grant use are recorded
separately before the tested request baseline; they cannot be hidden inside its
deltas. For X1-X5, the store-owned effect-decision record must gain exactly one
`effect-attempt` carrying the corresponding revoked, expired, binding-mismatch,
exhausted or operation-mismatch denial. Passing only the startup-time fresh
denials is insufficient: every X arm must prove that A1 re-enters the
persisted-grant decision at the effect boundary and does not trust cached
session authority.

The C6 inventory uses these closed classes:

- allowed startup inputs: opaque grant selector, opaque store selector and
  fixed current target;
- allowed derived A1 state: domain-separated grant-selector digest, sealed
  current binding, exact operation, deadline, call ceiling and connection
  nonce;
- allowed durable state: existing grant-attempt and caller-request records plus
  the research fixture's bounded non-secret authorization-decision observation;
- allowed public inputs: idempotency key plus bounded fake command shape;
- forbidden everywhere public: raw grant/store selectors, session lease,
  installation key, sealed binding bytes, filesystem paths, command output and
  credentials.

Every result records source SHA, input digest, executable digest, target,
variant, case, failure phase, store digests before/after, all four use counts,
separate startup-verification and effect-attempt counts with bounded ordered
codes, mechanism attempts, marker count, owned-state counts, typed result and
elapsed milliseconds. Timing is diagnostic only.

## 5. Decision tree, ordering and kill criteria

1. Run A0 against fresh copies for session-start, renew and session-end. If any
   lifecycle call is not rejected as wrong-operation before use consumption,
   kill the experiment as a fixture/contract failure. Record A0 as a negative
   control only; its separate ordinary shell call must still satisfy C1.
2. For A1 and B, evaluate C1, then C2/C3/C4. Failure of any Boolean gate kills
   that variant.
3. A surviving variant must then pass C5/C6/C7. Any authority bypass, privacy
   leak or product-contract change kills it.
4. If exactly one variant survives, select it.
5. If both survive, compare the following lexicographic tuple, lower wins:
   new durable authority states; retained in-memory authority fields; new
   trusted inputs; new public protocol fields; lifecycle grant uses; cleanup
   states. Counts and canonical field-path member lists must both be recorded;
   an aggregate struct counts as its independently meaningful bounded fields,
   not as one item. The lifecycle-use dimension is forced to zero for every
   survivor by the hard constraints and is retained only as an explicit
   completeness check.
6. If the tuples are identical, select B because it introduces no retained
   session. If any dimension cannot be totally counted or the variants are
   incomparable under the frozen classes, return `INCONCLUSIVE_MODEL` and
   select neither.
7. If neither survives, return `REJECT_BOTH` and keep mutation private.

No performance score or implementation convenience may override this order.
If A1 ever dispatches with only its derived state and no fresh persisted-grant
attempt, kill A1. If B introduces an ambient/internal authority or weakens
job/device ownership, kill B.

## 6. Timebox and repair budget

- One frozen execution covers all cases for all variants and has a 15-minute
  outer deadline; each case has a 10-second deadline and C4 has its stricter
  5-second deadline.
- At most one fixture-only repair and one second frozen execution are allowed.
  A repair may correct harness construction or observation but cannot change a
  criterion, failure phase, tie-break, variant or expected result.
- Any fixture/contract failure on the second frozen execution returns
  `INCONCLUSIVE_FIXTURE_EXHAUSTED`, whether or not it equals the first failure.
  A Boolean/security failure with a complete trace needs no repeat.
- The runner records attempt 1 or 2 and rejects all later attempts before
  creating a store or process.

## 7. Frozen inputs and executable preflight

The runner must, before creating any fixture, store, mechanism or server child:

1. require the source SHA to be reachable from `origin/main`;
2. require every digest input to be tracked by that SHA and byte-equal in the
   worktree;
3. after the first implementation commit creates it, require the research
   directory to exist, have no untracked or dirty file, and have exactly the
   tracked file set listed in section 8;
4. compute a domain-separated, length-framed SHA-256 over the specification and
   every section-8 file except `RESULTS.md`, whose bytes are an output record;
5. record a separate SHA-256 of the executable actually run without claiming
   it was built from the source SHA;
6. run the research executable in a no-effect `--self-test` mode that covers
   every decision-tree branch, F-1 through F3 row and X1-X5 attack. This is the
   only process allowed before the run creates its invocation-owned fixture;
   it creates no store, fixture, mechanism or server child.

The public result contains only bounded digests and repo-relative input names,
never an expanded repository or store path.

## 8. Expected files

```text
research/mcp-persisted-session/
├─ Cargo.toml
├─ Cargo.lock
├─ README.md
├─ harness.rs
├─ fixture.json
├─ result-template.json
├─ run.sh
└─ RESULTS.md
```

The research crate is outside the product workspace and depends only on the
minimum existing store/contract crates needed for the fake mechanism. It may
not add a product feature, binary, descriptor or exported seam merely to make
the experiment easier.

## 9. Write-back discipline

The running harness writes one bounded receipt only to its invocation-owned
output and stdout. It never edits the specification or `RESULTS.md`. After the
process exits, the primary agent verifies that receipt and writes the reviewed
facts back in this order:

- `src/`, `crates/`, MCP descriptors, catalog, ledger, qualification gates,
  alignment contract and public evidence remain unchanged;
- append the trace to `research/mcp-persisted-session/RESULTS.md` without
  changing the frozen receipt or its input digest;
- an independent read-only review verifies the input digest, phase matrix,
  counts, privacy scan and unique decision-tree branch;
- only then append the design verdict below and update the owning PRD/goal;
- product implementation and public evidence land in a later coherent leaf.

## 10. Excluded choices

| Choice | Reason |
|---|---|
| Ambient `actuate` environment | not persisted, target-bound or independently revocable |
| Auto-select the only/latest grant | lets the runtime choose authority the human did not name |
| Put grant id in MCP tool arguments | lets each untrusted caller switch authority |
| Temporary process-environment mutation | races in the multithreaded sidecar and crosses the worker boundary |
| Give lifecycle calls free internal Actuate | creates an unscoped bypass around the authorization model |
| Treat A1 derived state as effect authority | skips the required fresh persisted-grant attempt |

## 11. Not answered here

- Linux verified current-session binding runtime qualification.
- Native six-cell packaged qualification.
- Which non-shell mutation verbs MCP should eventually expose.
- Human UX for issuing or backing up grants.

## 12. Result

Attempt 1 ran against frozen commit
`f7046fa2d6330b3d7e2e1ff4e72253d6cd5787bd` and exited zero, but its invocation
lane disappeared before independent review. The original receipt, metadata and
attempt ledger are unrecoverable, and the deletion source could not be
identified. The frozen procedure therefore consumes and voids the attempt. No
option is selected and no evidence is registered.

The terminal observation was seen before the artifacts were lost, so the
blind condition is no longer intact and this precommitment cannot be rerun as
written. Any further experiment must begin with a new precommitment, new input
digest and new attempt budget; it must disclose this void run and the prior
observation without treating either as evidence. That future design must also
place budget state outside the disposable build lane and preserve a validated
receipt outside that lane before reporting success.
