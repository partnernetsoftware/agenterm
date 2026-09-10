# Browser profile open singleton-safety decisive experiment

This is a safety precommitment, not qualification evidence. It does not enter
must-ship scope, change the `browser.profile.open` ledger cell, amend the
existing Linux court, or authorize product implementation unless the decision
attempt finishes as `OWNED_HANDOFF_PROVED`.

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Purpose | Decide whether one macOS Launch Services request can hand a profile-window request to an already-owned Chromium session without leaving an uncontained process tree |
| Implementation | `research/browser-profile-open-singleton-safety/` |
| Required reading | `prd/PRD_02_28_agenterm_cu.md`, `plan/acu-mcu-capability-ledger.json`, `crates/agenterm-cu/src/browser_session.rs`, `crates/agenterm-cu/src/browser_session_owner.rs` |
| Source discipline | Dedicated disposable macOS GUI login, invocation-owned OS home, session root, `Default` profile and `about:blank` only; the selected browser bundle has no pre-existing instance; no real user Chromium Profile is read, copied, opened or removed |

## 0. Fixed background and decision

The current `browser-open` implementation reads `Local State` under one user
data root but passes only `--profile-directory` to `open -na`. The request can
therefore reach an instance whose user-data root differs from the root that
gave the profile its meaning. This is a product identity ambiguity independent
of the court: a profile directory name is meaningful only within its exact
user-data root. Any eventual product change must be justified by restoring that
identity invariant, not by making this experiment convenient.

This experiment does not run in a human user's GUI login and does not test the
unsafe case in which the selected browser bundle already has a user instance.
Its dedicated disposable macOS login has an experiment-owned OS home, not an
environment-only `HOME` override, so a negative singleton result has no real
user Profile to reach. Admission proves that the selected bundle has zero live
instances and refuses otherwise, without waiting, closing or cleaning anything.
It then creates one public, contained browser session over an invocation-owned
root and asks Launch Services to open one window in that exact root.

The two semantic outcomes are:

- **Owned handoff:** the directly spawned `/usr/bin/open` request and exactly
  one Launch Services relay exit within five seconds, leave no live outside
  descendant, and the contained browser creates exactly one owned window.
- **Escape or ambiguity:** an outside process persists, more than one relay is
  observed, an outside descendant survives, the returned window cannot be
  bound to the contained browser PID, or an observation is incomplete.

The first outcome is `OWNED_HANDOFF_PROVED` and authorizes only a separate
product design and review. Every other outcome is typed inconclusive or unsafe
and leaves `browser.profile.open` pending. This experiment never qualifies the
capability and does not answer whether handoff is safe while a real same-bundle
user instance exists.

## 1. Hard constraints

### 1.1 Frozen inputs and safe admission

1. The runner runs only as a freshly provisioned account whose short name has
   the frozen form `agenterm-court-<eight lowercase hex>`. The explicit expected
   short name and numeric uid are frozen into the input digest and remain
   identical across `R1`, `R2` and `D1`. Admission obtains the current account
   through `getpwuid(getuid())` and requires its canonical `pw_dir` bytes to
   equal `$HOME`, its name/uid to equal the frozen values, and the macOS console
   user to equal the same name/uid. Merely overriding `HOME`, running through
   SSH, or using a human login therefore fails mechanically. This guards
   accidental/misconfigured execution; a privileged actor deliberately forging
   account and console-user records is outside the threat model.
2. A bounded no-follow census reads only the account home's first level. Its
   frozen allowlist contains the ordinary macOS account skeleton,
   `Library`, and `.agenterm-research-state`; at admission the per-attempt
   scratch root must be absent, while authoritative state may be absent for
   fresh `R1` or present in its validated ledger shape thereafter. The census
   never recursively freezes the live `Library` tree.
3. Separately, every accepted-family default Chromium data root under that
   account is checked component by component without following symlinks and
   must be absent. This exact absence check, not the top-level census, carries
   the user-profile safety claim. Any unexpected top-level entry, browser root,
   symlink or unreadable component is `INCONCLUSIVE_ADMISSION`. Fixture bundles
   live only in frozen repository build output outside the account home.
   Ordinary login services are allowed as a frozen unrelated process/window
   baseline, but no unrelated selected-bundle or owned-root process is allowed;
   that separate process predicate has its own typed fact. Admission never
   cleans an unexpected entry or process and continues.
4. The runner receives one exact signed Chromium-family application bundle and
   one exact executable. Both paths are canonicalized without following an
   unexpected symlink; the executable must be inside that bundle. A bounded
   `Info.plist` read supplies the exact bundle identifier. The bundle identity,
   executable digest, browser version and accepted family are frozen. A name,
   title or path substring is never an identity witness. Throughout this
   specification, a `selected-bundle process` means a main process whose
   canonical executable file identity and digest equal that frozen executable.
   The bundle identifier selects and cross-checks public application facts but
   does not make helper executables members of that process set.
5. A build-only preparation phase may write ordinary repository build outputs,
   but it does not create an experiment root, external state or product object.
   After that phase, the frozen input digest covers `/usr/bin/open`, the
   AgenTerm qjswasm host, `agenterm-cu`, the independent process and foreground
   helpers, the redacted account-provisioning manifest, this specification, the
   runner, court, result template and all their direct fixture inputs. Helpers
   are then immutable frozen inputs. No compilation occurs during preflight or
   after an attempt is reserved.
6. Before any attempt is reserved, public application facts and an independent
   bounded bundle-process probe, both scoped to the frozen current console uid,
   must agree that the selected bundle has zero live main-process instances.
   Public facts use the frozen bundle identifier to enumerate candidate PIDs;
   the independent probe resolves each candidate to the frozen main executable
   identity defined above, and any unresolved or nonmatching candidate is a
   disagreement rather than a helper implicitly joining the set. After the
   owned session starts, that same pair of witnesses must continuously classify
   the selected-bundle process set. In
   the last complete sample immediately before the direct `open` spawn, the set
   must equal exactly the frozen owned browser main identity; otherwise the request is
   not spawned. From session start through final-inventory completion, any
   newly observed selected-bundle process outside that owned main identity that lacks the
   owned token is highest-priority `INCONCLUSIVE_BASELINE_MUTATION`. A
   token-bearing process is
   instead classified exclusively by G1/G2 as a relay, topology-inconclusive
   candidate or escape; G5 does not duplicate that fact. Any outside descendant
   of the direct request, relay or candidate also belongs exclusively to G2,
   regardless of its own token or executable. Independently, any process whose
   canonical executable file object lies inside the frozen bundle tree belongs
   to G2 whenever it is outside the frozen contained process group, lacks a
   retained observed lineage to the controlled browser root, and lacks a live
   G1 exemption as defined in section 1.3 within its frozen deadline.
   An identity with that retained controlled-browser lineage stays contained
   after a group breakaway, while the breakaway is recorded as a deviation.
   This bundle-tree projection does not
   depend on argv or a previously observed parent edge. Token and bundle
   projections are distinct witnesses: the former proves use of the owned root,
   while the latter detects a same-bundle process that lacks the token. Any
   disagreement, unknown identity, incomplete inventory or unexpected member
   refuses forward work. A pre-reservation one-shot inventory failure belongs
   to V1, a post-start one-shot public/independent inventory failure belongs to
   V2, and a scanner sample/sequence failure belongs only to V5. The runner
   never waits for, closes, activates or
   cleans a pre-existing or concurrent user instance. Other macOS login
   sessions are out of this per-console-user process set; uid-separated private
   home modes prevent this account and its Launch Services session from reading
   another account's Chromium roots.
7. The invocation scratch root, browser-session path and owned Chromium root
   must not exist before the run. Their parent chain must be canonical, private
   where owned, and free of symlinks. Any existing owned path or token-matching
   process refuses the run; the runner never cleans it and continues.
8. Once preflight begins, lock-free work is read-only and has no experiment,
   external-state or product side effect. Under the external no-follow
   exclusive state lock, admission revalidates every frozen input, the
   zero-instance result and external state, then atomically
   compare-and-reserves an attempt. Only after the reserved row is published
   and read back may the runner create the focus fixture or browser session.

### 1.2 Durable budget, journal and receipt

1. Authoritative state lives inside the dedicated account home but outside the
   disposable invocation scratch root, at
   `.agenterm-research-state/browser-profile-open-singleton-safety/`.
   The runner creates each missing private directory with `mkdir`; `EEXIST`
   requires immediate no-follow revalidation rather than failure or trust. It
   atomically opens `state.lock` with create and no-follow flags, then verifies
   the opened object is one regular, private, current-user-owned file before
   taking a nonblocking exclusive lock on that same descriptor. Failure never
   creates a second lock object or degrades to an unlocked run.
2. The immutable ordinals are rehearsal `R1`, rehearsal `R2` and decision
   `D1`. The external `attempt-ledger.jsonl` has exactly these keys per row:
   `schema`, `experiment`, `kind`, `ordinal`, `run_id`, `source_sha256`,
   `input_digest`, `status`, `receipt_sha256`, `terminal_code`. Reserved rows
   have JSON `null` for the last two fields. Finished rows have a frozen typed
   terminal code and the accepted receipt digest, except the persistence
   terminal described below. An independently audited abandoned attempt appends
   a row for the same identity with a null receipt and
   `attempt_abandoned_after_independent_audit`; it records history, never a
   design result, and never frees an ordinal.
3. A residual reservation blocks every run until that independent abandoned
   row exists. The implementation never heals, renames, removes or reuses a
   reservation. Every published ledger row is also printed as one canonical
   JSON line to stdout.
4. Rehearsal uses the same frozen bytes, exact browser and external
   persistence. It exercises input admission and an owned foreground fixture,
   then starts the bounded process scanner before creating a second owned
   fixture application with a distinct frozen bundle identifier. That second
   application takes foreground, closes its only ordinary onscreen window, is
   proven by public and independent witnesses to remain the zero-window
   app-level frontmost process, and is then terminated and reaped by the same
   native contained-owner primitive and deadlines used by
   `browser-session-stop`. The first fixture must return naturally. Rehearsal
   then exercises public browser-session start/status/stop/remove, exact cleanup
   and natural restoration of the original foreground. The scanner runs from
   immediately before the second fixture is created through final inventory at
   the same frozen cadence and row limits as `D1`. Rehearsal does not invoke
   `/usr/bin/open`, create a browser window, classify a relay, or record any
   singleton/window decision fact. Those decision criteria remain literal
   `not-run` in its receipt. `D1` is admissible only after at least one rehearsal
   finishes as `REHEARSAL_PASS`. Any source repair requires a new successful
   rehearsal; after `R1` and `R2` are consumed, further repair needs a new
   precommitment and budget.
5. The fixed artifact mapping is `R1` to `stage-journal/R1.jsonl` and
   `rehearsal-1-receipt.json`, `R2` to `stage-journal/R2.jsonl` and
   `rehearsal-2-receipt.json`, and `D1` to `stage-journal/D1.jsonl` and
   `decision-1-receipt.json`. Journal rows are RFC 8785 canonical JSON and form
   a domain-separated SHA-256 chain from a frozen zero predecessor. A stage is
   complete only after atomic replacement, readback and byte equality. The
   corresponding external receipt is then atomically published, read back and
   hashed before any lane mirror is written. A mirror failure never rolls back
   or modifies authoritative state.
6. Candidate journal append fails with
   `INCONCLUSIVE_EVIDENCE_PERSISTENCE` when its resulting row count is at least
   64, one canonical row is at least 32 KiB, or total canonical bytes are at
   least 1 MiB. It never truncates. If the ledger remains writable, the runner
   finishes the attempt with that terminal and a JSON-null receipt digest. If
   the ledger is also unwritable, the reserved row remains authoritative; the
   runner emits one fixed JSON stdout failure record and exits nonzero without
   claiming an unused attempt.
7. The closed stage set is `preflight`, `focus-baseline`, `fixture-ready`,
   `rehearsal-foreground`, `rehearsal-window-close`, `rehearsal-terminate`,
   `session-ready`, `ownership`,
   `open-request`, `relay`, `window-ready`, `window-close`, `session-stop`,
   `termination-proof`,
   `session-remove`, `root-removal`, `fixture-restore`, `original-restore`,
   `final-inventory`, `scan-coverage` and `terminal`. Every stage catches all
   producer failures,
   persists its own frozen typed code before propagating the terminal, and
   never relies on a later cleanup row to explain an earlier failure.
   A journal contains only reached stages. The receipt contains a status entry
   for every stage; any stage outside that attempt kind's frozen subsequence is
   literal `not-run`. Rehearsal excludes `open-request`, `relay`,
   `window-ready`, `window-close` and `fixture-restore`; decision excludes
   `rehearsal-foreground`, `rehearsal-window-close` and
   `rehearsal-terminate`. In rehearsal, `rehearsal-foreground` records the
   second fixture's creation and foreground proof, `rehearsal-window-close`
   records its exact close and zero-window-frontmost proof, then
   `rehearsal-terminate` records its contained termination and the first
   fixture's restoration before `session-ready`. The `fixture-restore` stage
   belongs only to `D1` and carries G4's post-browser-stop restoration; V4
   facts never enter it or become comparable decision evidence. In `D1`, the
   `open-request` stage owns direct-request exit and persists
   `INCONCLUSIVE_DIRECT_REQUEST` on timeout before relay classification.
8. The closed terminal set is `REHEARSAL_PASS`, `OWNED_HANDOFF_PROVED`,
   `UNSAFE_ESCAPED_PROCESS`, `INCONCLUSIVE_ADMISSION`,
   `INCONCLUSIVE_FIXTURE`, `INCONCLUSIVE_DIRECT_REQUEST`, `INCONCLUSIVE_RELAY`,
   `INCONCLUSIVE_RELAY_TOPOLOGY`, `INCONCLUSIVE_SCAN_COVERAGE`,
   `INCONCLUSIVE_HANDOFF`, `INCONCLUSIVE_FOCUS_RESTORE`,
   `INCONCLUSIVE_REHEARSAL_FOCUS`,
   `INCONCLUSIVE_CLEANUP`, `INCONCLUSIVE_BASELINE_MUTATION` and
   `INCONCLUSIVE_EVIDENCE_PERSISTENCE`. The abandoned-history code is not a
   result terminal. No internal helper or operating-system error crosses into
   the ledger without mapping to this set.
9. The receipt template freezes every stage, criterion, typed terminal and
   exact fact-key whitelist before runner or court implementation is accepted.
   Every terminal receipt has the complete criterion shape; an unreached
   criterion is the string `not-run`, never omitted, null or inferred. Raw
   paths, argv, environment, titles, URLs, Profile contents and unfrozen process
   identifiers never enter the external journal or receipt.

### 1.3 Launch Services relay and process ownership

1. Public `browser-session-start` starts the exact frozen executable without a
   bridge and retains its existing `--no-startup-window` behavior. Public status
   must return exact owner and browser PID/start identities. An independent
   bounded process snapshot must prove the browser process-group membership
   before `D1` continues. That exact browser identity is the root of the
   **contained browser closure**. The closure is dynamic, not a frozen PID set.
   The independently proven browser process group is the **frozen contained
   process group**. Each complete sample adds every matching PID/start identity
   in that group and every identity connected to the exact browser root by an
   observed transitive PPID chain, recording group-membership and
   controlled-root-lineage provenance separately. Lineage is established
   incrementally across samples: if a sample observes that identity `X` has a
   parent `Y` whose exact PID/start identity is then live in the retained
   controlled-root-lineage subset, `X` permanently inherits that provenance.
   Mere group membership does not manufacture lineage. The group-membership enumeration has a frozen bound
   of 4,096 identities; exceeding it is a hard incomplete-enumeration V5
   failure, never truncation. Its retained identity ledger and its observed
   transitive-lineage subset, seeded by the exact browser-root identity, are
   sticky through final inventory. Its live
   contained view includes retained identities proven live and either still in
   the frozen contained process group or in the retained observed lineage from
   the controlled browser root. A lineage-proven live identity that leaves the
   group remains contained and records a breakaway deviation. A live identity
   known only from former group membership and now outside the group is a G2
   escape because controlled ancestry was never established. The
   product's Unix containment provider is the process group, but its published
   `breakaway_prevented=false` fact means that group is an inventory witness,
   not an enforced boundary. Kernel session is not used as a containment
   predicate. This specification uses only
   `contained browser closure` for that object;
   `owned tree`, `contained tree` and `frozen contained tree` are not synonyms.
2. `D1` directly spawns, without a shell, exactly:

   ```text
   /usr/bin/open -n <canonical bundle path> --args --user-data-dir=<exact owned session root> --profile-directory=Default about:blank
   ```

   The runner freezes the direct `open` PID and start identity. The root is the
   exact file object already owned by the public session, contains an
   unpredictable per-run token, and is never reconstructed from a display
   string. This deliberately strengthens today's `open -na <application name>`
   product mechanism: a passing result applies only to canonical bundle-path
   selection plus the exact root argument and does not validate the current
   name-based command. The controlled navigation has no network and no URL
   nonce.
3. A frozen native scanner targets a 20 ms cadence and samples at most 4,096
   process rows. Its observation window begins at the first sample start. In a
   rehearsal that sample starts immediately before the second fixture
   application is created, and fixture creation is forbidden until the sample
   completes; the window therefore covers the zero-window termination and
   restoration exercise under scanner load. In `D1` the first sample starts
   immediately before session start, and session start is forbidden until that
   sample completes. The rehearsal window runs through root removal and final
   inventory. The `D1` window runs through exact
   window close, cleanup and final inventory. At every sample it projects both
   exact selected-bundle executable identity, every executable file object
   canonically contained by the frozen bundle tree, and the unpredictable
   owned-root token; public `ps --command <token>` snapshots at the pre-open, relay-exit
   and post-stop boundaries cross-check its token set. The independent
   projection records PID, PPID, PGID, kernel start time and canonical
   executable file identity. Raw command lines remain in memory and are not
   persisted.
4. The scanner freezes observation-window start and end monotonic timestamps.
   Every sample records its own start/end, completeness, bounded row count,
   interval from the preceding sample start and redacted identity digests. The
   receipt records sample count, maximum start interval `M`, maximum enumeration
   duration `E`, and resolution `R=M+E`. The observation-window start is
   exactly the first sample start, so there is no unmeasured head interval. A
   final sentinel sample must start at or after final-inventory completion; its
   start is the observation-window end, and the preceding-to-sentinel interval
   participates in `M`. Every internal interval, including that final interval,
   and every enumeration duration must be at most 200 ms. Sample count must be
   at least the floor of window duration divided by 200 ms; this count is a
   redundant cross-check on the bounded interval sequence, not an independent
   temporal claim. Any missing first
   or sentinel sample, early scanner stop, interval/count/row-bound failure, a
   contained-group member count above its 4,096-identity hard bound, or
   incomplete enumeration is `INCONCLUSIVE_SCAN_COVERAGE`. Forward adjudication
   stops at the first such fact, but exact cleanup, final inventory and terminal
   persistence continue. After final inventory, the `scan-coverage` stage
   persists either the completed sentinel proof or the typed coverage failure;
   a stopped scanner and missing sentinel are failure facts, not prerequisites
   that prevent that stage from being written. Canonical bundle-tree resolution
   is charged to `E`: broader executable coverage may increase `R` and therefore
   weaken the shortest excluded lifetime, while any `E` above 200 ms fails V5
   rather than silently widening the claim further.
5. Within that proven span, G2's negative claim is explicitly bounded: it
   excludes an unobserved non-exempt token process or outside descendant whose
   lifetime is greater than `R`. A shorter transient is outside the claim. A
   created owned window with no observed relay is `INCONCLUSIVE_RELAY`, never
   evidence that no relay existed.
6. The frozen direct `open` identity is the only unconditional group-external
   classification exemption. A **live G1 exemption** means exactly that direct
   request before its five-second exit deadline, or the single frozen relay or
   topology-inconclusive candidate before its own five-second exit deadline.
   No other identity qualifies by resemblance or inference. Exactly one other group-external token process must be observed
   for a handoff pass. It is classified as the Launch Services relay only when
   all frozen predicates hold: its kernel start time is after direct spawn and
   no more than two seconds later; PPID is PID 1; its canonical executable file
   identity and digest equal the frozen browser executable; it carries the
   unpredictable owned-root token; and its process group remains outside the
   frozen contained process group. Token, executable identity and the kernel-start window are
   the primary anti-impersonation predicates. Failure of a primary predicate is
   `UNSAFE_ESCAPED_PROCESS` and does not consume the relay slot. PPID and group
   are auxiliary topology checks; their mismatch freezes that identity as a
   topology-inconclusive candidate and yields `INCONCLUSIVE_RELAY_TOPOLOGY`, not
   an unsafe claim and not a pass. Its initial outside presence is explicitly
   excluded from G2's non-exempt set, while a distinct outside process, survival
   past the relay deadline or a live outside descendant remains a G2 unsafe
   fact. The direct
   request and relay must each exit within five seconds.
   The scanner maintains a transitive, sticky descendant closure rooted at the
   direct request, relay and topology-inconclusive candidate. Once a PID/start
   identity enters that closure it remains a member through final inventory;
   parent exit, reparenting or a later missing PPID edge never erases it. Each
   root's exit boundary and final inventory must show no live outside member of
   its retained closure. The `open-request` stage records the direct-request
   boundary, `relay` records relay/candidate boundaries, and `final-inventory`
   records separate retained/live digests and counts for those closures and the
   contained browser closure. At each root exit boundary and again at final
   inventory, the same complete sample cross-checks that every bundle-tree
   executable it enumerates in the frozen contained process group was added to
   the live contained browser closure. This is a redundant consistency check on
   the sample's insertion rule, not an independent temporal claim. Every
   outside-group bundle-tree match that lacks both retained controlled-browser
   lineage and a live G1 exemption must be absent. After the direct
   request and whichever relay or topology candidate was observed have exited,
   the complete token-matching set must be a subset of the live contained
   browser closure, and the
   contained browser identity itself must match the token.
7. The token set is an ownership cross-check, not a complete tree inventory:
   helpers that omit the root argument remain governed by the sticky descendant
   closure when their edge was observed and, independently, by the bundle-tree
   executable projection even when it was not. More than one fully classified relay, a persistent relay or
   topology-inconclusive candidate, any other non-exempt token match outside
   the group, any bundle-tree executable outside the frozen contained process
   group that lacks both retained controlled-browser lineage and a live G1
   exemption, or any live outside
   descendant is `UNSAFE_ESCAPED_PROCESS`. Scan incompleteness belongs only to
   V5 and is `INCONCLUSIVE_SCAN_COVERAGE`, never an unsafe fact. A later exit never turns
   an observed escape into a pass.
8. No identity in the live contained browser closure may ever be an unsafe
   cleanup target, regardless of witness. An escaped identity is eligible only
   after its independent positive G2 fact has been persisted with its exact
   PID/start identity and at least one G2 witness at observation time: token membership,
   retained sticky-descendant membership, or a bundle-tree executable outside
   the frozen contained process group that lacks both retained
   controlled-browser lineage and a live G1 exemption. Immediately before signalling, the same
   PID/start identity and qualifying witness must still match. At most one
   ordinary public `process-kill` is allowed per frozen target. Force, signal
   escalation, name/bundle kills, `pkill` and newly discovered cleanup targets
   are forbidden. Failure to remove an exact escaped identity preserves the
   unsafe result and records the residual identity digest.

### 1.4 Windows, foreground and baseline

1. Admission freezes an empty selected-bundle process and window baseline. From
   session start through final-inventory completion, sticky descendant relation
   and bundle-tree executable membership have first precedence: every outside
   descendant of the direct request, relay or topology-inconclusive candidate,
   and every process outside the frozen contained process group that executes a
   file object inside the frozen bundle and lacks both retained
   controlled-browser lineage and a live G1 exemption, belongs exclusively to G2
   regardless of its token. A lineage-proven browser descendant remains in the
   contained view after breakaway and records a deviation instead. Every
   remaining selected-bundle main-process identity then belongs to exactly one
   of three disjoint classes: the frozen owned browser main identity; a
   token-bearing outside identity classified exclusively by
   G1/G2; or a tokenless outside identity, which is a G5 baseline mutation.
   Thus G5's selected-bundle view must equal the owned browser main identity until
   `session-stop` proves the owned browser group absent and return empty from
   that boundary through final inventory; it excludes token-bearing relay,
   topology-candidate, escape, bundle-tree and descendant facts rather than
   adjudicating them twice. Candidate persistence or any additional
   token-bearing outside member are G2 facts. The
   direct `/usr/bin/open` request belongs
   only to the token projection, not the selected-bundle set. The contained
   browser must have zero ordinary windows with native `onscreen=true` before
   the decision request. Minimized, off-Space or otherwise hidden windows do
   not silently change that predicate.
2. An invocation-owned AgenTerm fixture becomes the exact frontmost window
   before browser work. Public focused state freezes bundle id, PID/start
   identity and native handle; an independent probe freezes the same PID and
   handle. It must be the unique frontmost window and in the active Space.
3. `D1` requires exactly one newly observed ordinary `onscreen` window whose
   PID is the frozen browser PID at the root of the contained browser closure.
   The quantifier ranges over new
   windows owned by that PID, not over unrelated user windows. The window must
   be in the same Space as the fixture and become the exact frontmost window by
   public and independent witnesses. Failure to prove Space or foreground is a
   typed inconclusive result, not a handoff pass.
4. Exact public close binds the returned handle, PID, title snapshot and
   expected-gone postcondition. After close, the exact owned window must be
   absent. The browser application may temporarily remain frontmost with no
   window; that application-level state is neither natural restoration nor a
   failure by itself.
5. From exact close until all restoration checks finish, the court sets
   `AGENTERM_NO_ACTIVATE=1` and makes no activation or foreground-setting call.
   After public browser-session stop proves the browser process gone, the owned
   fixture must naturally return as the exact frontmost window within five
   seconds. After session removal and fixture exit, the original pre-fixture
   foreground bundle id, PID/start identity and window handle must naturally
   return within five seconds. Each restoration requires matching public and
   independent observations.
6. Cleanup is ordered: exact window close and absence; continuous process
   classification; public browser-session stop; frozen owner/browser absence
   and empty token set; natural fixture restoration; public browser-session
   remove; owned session/root absence; fixture exit; original foreground
   restoration. Directory removal occurs only after every owned or escaped
   process target is proven absent and only for a canonical path beneath the
   invocation scratch root.
7. Final selected-bundle process and window baselines must again be empty.
   Other user baselines are compared as the exact sets
   `{pid,start_identity}` and `{handle,pid,start_identity}`. Titles, geometry,
   z-order and unrelated concurrent window deltas are excluded. A mutation of
   either frozen identity set is `INCONCLUSIVE_BASELINE_MUTATION`; it is never
   treated as owned or cleaned.
8. The experiment does not install or use Native Messaging. Custom user-data
   roots can change Native Messaging host lookup, but that subsystem is not
   exercised and supplies no fact to this verdict.
9. Any impulse to pass a real profile path, admit an existing selected-bundle
   instance, weaken PID/start ownership to a label, delete an unexplained
   process, or activate a user window is the safety failure this experiment
   detects, not an implementation shortcut.

## 2. Minimal experiment content

| Dimension | Frozen choice | Reason |
|---|---|---|
| Host | native macOS, dedicated disposable GUI login only | Tests Launch Services and native foreground semantics without exposing a human user's browser profile |
| Browser | one exact signed Chromium-family bundle/binary pair with zero pre-existing instance | Avoids name inference and any path into a real running Profile |
| Browser lifecycle | public `browser-session-start/status/stop/remove`, no bridge | Reuses exact contained ownership without changing product launch behavior |
| Profile/navigation | exact owned session root, `Default`, `about:blank` | Contains no user data, network or data-URL compatibility variable |
| Focus baseline | two owned fixture applications with distinct bundle ids in the active Space | Rehearses cross-application foreground restoration without activating a user window |
| Independent witnesses | continuous bounded process projection plus native foreground probe | Do not reuse product inventory as its own proof |
| Product calls excluded | `browser-open` and every evidence emitter | Tests a prerequisite, not the capability or its registration |

## 3. Criteria and measurement discipline

| ID | Role | PASS condition | Typed failure |
|---|---|---|---|
| V1 | Validity | Frozen inputs, zero selected-bundle instance, clean owned roots, external state and reserved attempt all validate under the lock | `INCONCLUSIVE_ADMISSION` |
| V2 | Validity | Exact focus fixture and public browser session reach their frozen identities; bounded public and independent inventories are complete | `INCONCLUSIVE_FIXTURE` |
| V3 | Validity | Every owned window/process/session object created by this attempt is cleaned exactly and owned roots are absent; it does not own focus-restoration or baseline facts | `INCONCLUSIVE_CLEANUP` |
| V4 | Rehearsal validity | A distinct contained fixture application takes foreground, closes its only onscreen window, remains app-level frontmost with zero windows under both witnesses, is terminated/reaped with browser-session-stop-equivalent semantics, and naturally restores the first fixture under both witnesses | `INCONCLUSIVE_REHEARSAL_FOCUS` |
| V5 | Validity | The scanner's first/internal/final samples, count, maximum interval and enumeration duration prove complete temporal coverage | `INCONCLUSIVE_SCAN_COVERAGE` |
| G1 | Decision | Direct `open` exits within five seconds and at least one fully classified relay is observed; direct-request timeout, zero candidates and auxiliary-topology mismatch have distinct inconclusive codes | `INCONCLUSIVE_DIRECT_REQUEST`, `INCONCLUSIVE_RELAY` or `INCONCLUSIVE_RELAY_TOPOLOGY` |
| G2 | Safety | The fully classified relay set has cardinality at most one; every classified relay exits within five seconds; no other non-exempt token process or outside descendant is observed, and no outside-group bundle-tree executable lacks both retained controlled-browser lineage and a live G1 exemption; no such unobserved process can have lived longer than `R=M+E`; after exemptions exit every token and bundle-tree match is in the live contained browser closure | `UNSAFE_ESCAPED_PROCESS` |
| G3 | Decision | Exactly one new ordinary `onscreen` window owned by the frozen browser PID appears | `INCONCLUSIVE_HANDOFF` |
| G4 | Decision | That exact window becomes frontmost in the fixture's Space; after its process stops the fixture restores, and after fixture exit the original foreground restores | `INCONCLUSIVE_FOCUS_RESTORE` |
| G5 | Safety | The selected-bundle set follows the exact allowed state sequence and returns empty; every other frozen baseline identity set is unchanged | `INCONCLUSIVE_BASELINE_MUTATION` |

Rehearsal must pass V1 through V5; G1 through G5 remain `not-run`. Decision
must pass V1, V2, V3, V5 and G1 through G5; V4 remains `not-run` because its
fixture-only foreground exercise is a rehearsal fact, not decision evidence.
Each observable fact has exactly one criterion owner: V3 owns removal and root
absence, V4 owns rehearsal focus restoration, V5 owns scanner coverage, G1
owns direct-request exit (`INCONCLUSIVE_DIRECT_REQUEST` on timeout) plus relay
candidate existence and classification, G2
owns safe relay cardinality, relay persistence and every
outside-process/descendant fact, G4
owns decision focus and both decision restorations, and G5 owns bundle/user
baselines only for identities without the owned token. A token-bearing
group-external process that fails a primary relay predicate is not classified
as a relay: G1 records that classification fact, while G2 owns the resulting
non-exempt outside-process fact and `UNSAFE_ESCAPED_PROCESS` terminal. A
candidate that satisfies all primary predicates but fails an auxiliary check
remains wholly G1-owned as `INCONCLUSIVE_RELAY_TOPOLOGY`; G2 and G5 exclude its
initial presence, while G2 still owns later persistence, descendants or an
additional outside process. Two or more fully
classified relays satisfy G1's existence test but fail G2's cardinality safety
test. Sticky descendant relation and bundle-tree executable membership take
precedence over token and selected-bundle membership: every outside descendant
belongs only to G2, as does an outside-group bundle-tree executable that lacks
both retained controlled-browser lineage and a live G1 exemption, so a tokenless helper
cannot also become a G5 baseline fact. A lineage-proven controlled-browser
descendant that breaks away remains contained and records a deviation rather
than becoming a G2 or G5 fact. Every stage records
producer, deadline, elapsed time, typed code and a whitelist-bounded redacted
fact object. Facts include
scan completeness and counts, domain-separated identity/token/set digests,
relay cardinality and exit predicates, owned window/focus predicates and exact
cleanup/restoration predicates. They never contain raw paths, argv,
environment, titles, URLs or Profile contents.

## 4. Decision tree, kill criteria and time box

### 4.1 Admission and rehearsal

1. Lock-free preflight is read-only. A successful compare-and-reserve and
   readback consumes the selected ordinal before the fixture or session starts.
2. `D1` is rejected unless `R1` or `R2` has a finished `REHEARSAL_PASS` receipt
   on the same frozen source and input digest. A rehearsal never supplies a
   singleton, relay, browser-window or handoff fact; V4 records only the two
   distinct owned fixture applications used to prove cross-application
   restoration.
3. V1 or V2 failure stops forward work. Cleanup and persistence still run. V3
   is evaluated independently and never hides the earlier primary cause.

### 4.2 Decision priority

1. Any observed G2 escape or G5 baseline mutation has highest priority. Later
   process exit, successful window creation or successful cleanup cannot turn
   either result into a pass.
2. Within G1, direct-request timeout has priority over an observed auxiliary
   topology mismatch, which has priority over absence of a relay candidate.
   V5 failure reports `INCONCLUSIVE_SCAN_COVERAGE`; unless G1, G2 or G5 already
   failed on a positive observation, those criteria are then literal `not-run`,
   not a third criterion state and not unsafe findings. Zero observed relay is
   absence rather than a positive G1 fact and does not override V5.
   Otherwise G1, G3 and G4 are reduced in that order after their complete fact
   tables exist. A missing or incomplete observation yields its typed
   inconclusive result; absence of evidence is never a negative fact.
3. V3 is then reported independently. If V3 is not proven, any computed design
   outcome is non-authoritative and the terminal is the earlier primary cause
   or `INCONCLUSIVE_CLEANUP` when no earlier cause exists.
4. Only V1, V2, V3, V5 and G1 through G5 all passing yields
   `OWNED_HANDOFF_PROVED`.

Kill criteria: stop forward adjudication immediately on an unclassifiable or
second relay, outside token process or descendant, PID/start mismatch, more
than one owned-PID new `onscreen` window, selected-bundle/user-baseline
mutation, loss of the focus fixture, or failure to persist the last completed
stage. V2 owns an incomplete or truncated one-shot public/independent inventory;
V5 owns an incomplete enumeration or temporal-coverage sequence. Either stops
forward adjudication, but neither stops the run: exact cleanup, final inventory,
the sentinel attempt, `scan-coverage` and terminal persistence still execute.
Cleanup remains limited to already-frozen identities and never broadens its
target set.

### 4.3 Time boxes

One rehearsal has a 165-second outer deadline. Its serial stage budget is 140
seconds:

- admission and reservation: 20 seconds;
- two fixture applications, contained termination and restoration: 35 seconds;
- public session start/status: 20 seconds;
- independent ownership witnesses: 15 seconds;
- session stop/remove and absence: 25 seconds;
- original foreground restoration: 10 seconds;
- persistence and terminal publication: 15 seconds.

The remaining 25 seconds is outer scheduling slack, not borrowable stage time.

The decision has a 180-second outer deadline. Its serial budget is 155 seconds:

- admission and reservation: 20 seconds;
- focus fixture: 15 seconds;
- public session start/status: 20 seconds;
- direct open and relay classification: 10 seconds;
- owned window, Space and focus proof: 20 seconds;
- exact window close: 10 seconds;
- session stop/remove and absence: 25 seconds;
- both natural restorations: 10 seconds;
- persistence and terminal publication: 25 seconds.

The remaining 25 seconds is outer scheduling slack, not borrowable stage time.
Rehearsal and decision therefore use the same rule: the outer deadline exceeds
the sum of frozen serial stage ceilings, and a stage cannot borrow that slack.
There are at most two rehearsals and exactly one decision attempt. A different
browser, root, command shape or source byte requires a new admissible rehearsal;
there is no in-place retry of `D1`.

## 5. Frozen layout

```text
.agenterm-research-state/browser-profile-open-singleton-safety/
├── state.lock
├── attempt-ledger.jsonl
├── rehearsal-1-receipt.json
├── rehearsal-2-receipt.json
├── decision-1-receipt.json
└── stage-journal/
    ├── R1.jsonl
    ├── R2.jsonl
    └── D1.jsonl

research/browser-profile-open-singleton-safety/
├── README.md
├── RESULTS.md
├── account-provisioning.json
├── run-current-host.sh
├── singleton-safety-court.qjs
├── result-template.json
└── fixtures/
```

The external state is ignored but not disposable. A run-local mirror is
disposable only after its authoritative external journal and receipt are
published and read back. No file under an invocation-owned HOME is evidence by
itself.

## 6. Excluded options

| Option | Why excluded |
|---|---|
| Admit a pre-existing selected-bundle browser | A negative singleton result could open a real user Profile before detection |
| Run today's `browser-open` against a synthetic HOME | It does not bind the parsed `Local State` root to the launched instance |
| Use a real browser Profile as the baseline | It risks user data and cannot prove owned cleanup |
| Remove `--no-startup-window` from the shared browser-session product path | It changes an established no-focus lifecycle instead of isolating this probe |
| Accept bundle/title/profile matching as ownership | Same-bundle processes and windows can satisfy those labels |
| Treat an arbitrary before/after delta as owned | Concurrent user activity can enter the delta |
| Kill by executable name, bundle or broad command pattern | It can terminate user work and destroys the baseline needed for proof |
| Use Native Messaging as a witness | It adds an unrelated subsystem and is unnecessary for singleton ownership |
| Retry the decision after changing source or inputs | It violates the frozen decision budget |

## 7. Not answered

- The final `browser-open` CLI shape or whether it should expose an explicit
  user-data-root argument.
- Today's name-selected `open -na <application name>` mechanism. A passing
  result applies only to `open -n <canonical bundle path>` and therefore
  requires a separately reviewed product mechanism change before use.
- Whether a handoff is safe when the same browser bundle already has a user
  instance. This precommitment deliberately rejects that case.
- Whether normal default-root invocations remain singleton-key equivalent
  across symlinks, case aliases, trailing separators or data-volume aliases.
- Whether a non-exempt outside process whose entire lifetime is shorter than
  the recorded scanner resolution `R=M+E` exists. The decision is expressly
  bounded to that measured temporal resolution.
- Whether an outside-group bundle helper whose entire intermediate ancestry was
  never sampled descended from the controlled browser or from an escaped
  Launch Services root. Without retained lineage the experiment conservatively
  treats that observed outside-group identity as G2 unsafe; it never invents
  controlled ancestry from former process-group membership.
- Whether the exact browser-window close mechanism works before `D1`; rehearsal
  deliberately never creates a browser window, so its first execution is the
  single decision attempt.
- Linux or Windows launch behavior.
- Profile display-name binding, tab semantics, Native Messaging, or evidence
  registration.
- Whether a successful safety result is sufficient to qualify
  `browser.profile.open`; it is only a prerequisite for a later owned court.

## 8. Result fill-in

Not run. Record the full external ledger; successful rehearsal receipt digest;
source, input, browser and executable digests; browser version and acquisition
method; every stage producer/deadline/elapsed/code/fact object; baseline,
current and final cardinalities; relay and ownership proof; decision-tree path;
cleanup and both foreground-restoration facts; all deviations and whether they
weaken the result; and third-party digest/rerun commands. State explicitly that
no criterion changed after observing any rehearsal or decision data.
