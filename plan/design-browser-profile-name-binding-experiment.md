# Browser profile name binding decisive experiment

This is a research precommitment. It does not qualify a provider, change the
`browser.profile-name-binding` ledger cell, remove `acu.dynamic.075`, or turn a
human profile label into authority.

| Field | Value |
|---|---|
| Date | 2026-09-10 |
| Purpose | Decide whether a human Chromium Profile label can safely select one live MV3 `profile_instance_id`, and whether that requires an explicit durable binding |
| Implementation | `research/browser-profile-name-binding/` |
| Required reading | `prd/PRD_02_28_agenterm_cu.md`, `plan/acu-mcu-capability-ledger.json`, archived `browser/client.ts` selector |
| Source discipline | Invocation-owned synthetic HOME and browser Profiles only; no real Profile mutation, MCU runtime or fallback |

## 0. Fixed background and competing designs

The following facts are not reopened by this experiment.

1. A lowercase-hex `profile_instance_id` prefix is already exact when it
   resolves to one entry in a complete live connection inventory.
2. Archived MCU first tried that exact prefix. Its human-name fallback returned
   the only connection when there was exactly one matching installed Profile,
   one installed Profile overall and one live connection. It observed no
   directory-to-instance relation and called the name a safe alias only under
   uniqueness.
3. Current ACU `Local State` observation owns `{application, directory,
   display_name}`. `Preferences` or `Secure Preferences` can say that the fixed
   extension is recorded. Strict bridge status owns `{profile_instance_id,
   extension_id, protocol, version, build_id}`. None currently carries the
   other's Profile identity.
4. `profile_instance_id` is generated in `chrome.storage.local`; Chromium does
   not expose the human Profile directory through the accepted extension API.
5. Raw active LevelDB reads, browser command-line scans, account permissions,
   browser activation and window-title guessing are outside the bridge boundary.

Three designs are compared, not conflated:

- **A0 · archived cardinality alias control:** reproduce MCU's relation-free
  1-1-1 fallback exactly. The armed negative trap must select B, so A0 has no
  winning branch; refusal means the archived behavior was not reproduced.
- **A1 · verified implicit edge:** automatically select only if a Profile-unique
  identifier from one trusted issuer binds the candidate directory to the live
  instance, and the current name-to-directory snapshot is unique and frozen.
  A1 needs both a positive and a negative arm to win.
- **B · explicit durable binding:** require a caller-authorized,
  generation-bound `(application,directory,instance)` receipt and revalidate it
  against the complete live inventory on every use.

The experiment accepts any of these results. Cardinality alone is never an
identity edge, and a safe refusal is not implementation of the legacy selector.

## 1. Hard constraints

1. All browser, HOME, Local State, Preferences, registry and receipt data is
   invocation-owned and removed after the court.
2. The experiment reads only bounded direct regular files under the synthetic
   browser root. It rejects absolute, `.`/`..`, slash-containing,
   backslash-containing, symlinked or over-limit Profile directory keys.
3. It never emits a HOME/browser/extension path, Preferences payload,
   MAC/protection field, URL, title, email, PID or full instance id. Failure
   strings, command stdout/stderr, audit rows and retained cleanup bundles obey
   the same rule. Public high-entropy identifiers are 64 lowercase-hex SHA-256
   values over `agenterm-cu/profile-binding-experiment/v1\0` plus length-framed
   field name, fixture label and identifier. The reproducibility input digest
   instead uses the distinct `agenterm-cu/profile-binding-experiment/input/v1\0`
   domain followed by the length-framed repo-relative name and exact bytes of
   every frozen input. Executed program identities are conventional SHA-256 of
   their exact bytes and are reported separately; they identify what ran but do
   not claim those binaries were built from the source commit. Low-entropy human aliases,
   directories and paths are reported only as fixed fixture labels, never as
   raw hashes. The exact-prefix control uses 12 lowercase hex characters only
   inside the private invocation and never prints them.
4. A complete connection inventory is mandatory. Truncation, malformed rows or
   stale process identity is typed inconclusive. At a selector observation that
   requires one live connection, zero or multiple connections is also typed
   inconclusive, but must remain distinguishable as
   `selector_observation_no_connection` and
   `selector_observation_multiple_connections`. The negative-arm baseline and
   post-stop cleanup instead require zero connections as their explicit success
   condition.
5. The exact fixed extension id, protocol, version and build id pass the
   existing strict status validator. Those values are shared by multiple
   Profiles and explicitly fail the identity-edge test.
6. No product resolver, compat mapping, ledger state, evidence registration or
   PRD completion checkbox changes before the decision trace is reviewed.
7. No repair may add LevelDB parsing, command-line scanning, browser activation,
   a new browser permission, real Profile mutation or another heuristic. Wanting
   one is a detected missing edge, not a reason to widen A0 or A1.
8. One wrong implicit selection kills that design. Aggregate success rates and
   archived parity cannot outweigh it.

## 2. Minimal experiment

| Dimension | Frozen choice | Reason |
|---|---|---|
| Browser | One current-host Chromium-family fixture with the exact embedded extension | Exercises real strict connection/status identity without a user Profile |
| Candidate A | Synthetic catalog root with `Profile 1`, display name `Work`, and one bounded fixed-extension installation record; A is not launched in the negative arm | Arms archived N/I uniqueness |
| Live Profile B | Different invocation-owned user-data directory outside every candidate scan root, launched directly by the research harness | Produces one real connection whose owner is not A and cannot enter candidate sets |
| Positive A1 arm | Launch A from the exact candidate directory after resetting the negative arm | Gives verified-edge A1 a scenario in which it can select correctly |
| Observation | `N_all`: all normalized name/directory matches; `N`: matching installed candidates as MCU defined it; `I`: catalog-root installed candidates; `C`: strict live connections; `E`: possible identity edges | Separates collision safety, archived filtering and identity relation |
| Restart | Stop the browser without deleting its user-data directory, then relaunch the same directory | Tests durable instance/edge stability; `browser-session-remove` is not used because it deletes the Profile |
| Name mutation | Freeze one observation, rename A's display name, then add a second same-normalized-name Profile | Tests stale snapshot and collision behavior separately from directory identity |
| Control | B's 12-hex instance prefix privately resolves only to B | Proves the connection works while isolating name binding |

The negative-arm ground truth is frozen independently of the fields under test:

1. A exists before the baseline but is never launched.
2. Baseline C is empty.
3. Only B starts; exactly one new connection appears after that start.
4. Stopping B removes that exact connection.
5. B's directory is outside every enumerated scanner root, which proves that
   neither B nor a parent of B can be visited by this bounded scan, and B is
   absent from `N_all`, N and I.

Any failure here is `INCONCLUSIVE_FIXTURE`; the trap is not armed. `I` always
means the archived-observable installed set below the catalog roots, not every
Profile that may exist on the host.

The accepted-boundary edge inventory is exhaustive:

| Surface | Candidate side | Connection side | Edge decision |
|---|---|---|---|
| Local State | application, directory, display name, last-used and profile ordering | none | No edge unless the connection authenticates the same candidate-specific value; ordering and last-used state are mutable non-edges |
| Preferences | fixed extension recorded, disabled flag and state | extension id | Extension id is shared and fails injectivity; disabled/state are local mutable facts |
| Connection record | none | connection id, host process identity, protocol | Host identity is not browser Profile identity |
| Strict status | none | profile instance, extension id, protocol, version, build id | Pass only if a bounded candidate file independently carries a collision-resistant Profile-unique value from the same trusted issuer |
| Owned-session receipt | controlled Profile object identity | connection observed after start | Valid only for that owned object; it cannot generalize to an arbitrary default Profile name |

The court records one decision object for every field in the verified union of
these surfaces: presence on each side, same-issuer status, injectivity,
Profile uniqueness and final eligibility. On the currently accepted schemas,
the only field present on both sides is the fixed extension id; it has the same
issuer but is neither injective nor Profile-unique, so G4a is expected to reject
A1 without running its positive arm. `SELECT_VERIFIED_IMPLICIT_EDGE` remains a
model-locked counterfactual branch and may become executable only under a new
precommitment that explicitly extends and re-freezes the accepted surfaces.

Display name, directory, extension id, protocol, version, build id, connection
id and host PID are explicitly non-edges unless the opposite side carries an
authenticated equal Profile-specific value. If an observed E claims A equals B
while frozen ground truth says A differs from B, the result is
`INCONCLUSIVE_GROUND_TRUTH_CONFLICT`, never an A1 win.

## 3. Precommitted criteria

| ID | Criterion | Nature | Pass | Fail |
|---|---|---|---|---|
| G1 | Fixture identity and inventory | Boolean gate | Exact extension/status, complete C and frozen construction trace | `INCONCLUSIVE_DEPENDENCY` |
| G2 | Armed archived trap | Boolean gate | `|N|=1`, `|I|=1`, `|C|=1`, A is sole N/I member, and B is outside scan roots and all candidate sets | `INCONCLUSIVE_FIXTURE` |
| G3 | A0 archived-behavior control | Safety | No winning branch: exact A0 selects B and is rejected | Refusal means A0 was not reproduced; A→B permanently rejects the archived alias |
| G4a | A1 directory/instance edge eligibility | Checklist/Boolean | Candidate and connection share one authenticated, collision-resistant, injective, Profile-unique value from the same issuer | Missing, duplicate or ground-truth-conflicting value rejects A1 |
| G4b | A1 edge restart persistence | Boolean | The eligible G4a value survives a same-directory restart unchanged | Missing or changed value rejects A1 |
| G5 | A1 name/directory snapshot | Safety | Normalized name uniquely names one directory; frozen receipt goes stale on rename; old name stops; renamed value cannot inherit the frozen receipt; duplicate normalized name is typed ambiguous even if only one Profile records the extension | Any wrong, inherited or fallback selection rejects A1 |
| G6 | Exact-prefix control | Boolean | B's private 12-hex prefix resolves only to B | `INCONCLUSIVE_FIXTURE` |
| G7 | Explicit-binding model | Safety | All bounded B operations and authority/conflict invariants close | Otherwise `INCONCLUSIVE_BINDING_DESIGN` |

Order is validity G1/G2/G6, then pure edge eligibility inventory G4a, then the
A0 control G3. Only an eligible G4a edge permits the positive A-owned arm and
same-directory restart for G4b; G5 name safety follows, and B feasibility G7 is
last. Validity gates never vote for a design.

The result records:

- reachable source SHA and one digest covering this specification revision,
  runner, browser court, binding model, exact embedded extension assets and
  exact Local State/Preferences fixture bytes;
- `N_all`/N/I/C cardinalities and fixed labels;
- every possible E field and its injectivity/issuer decision;
- the A0 subdecision, separately from the terminal experiment verdict;
- baseline/start/stop construction trace, restart, rename, duplicate-name and
  exact-prefix controls;
- decision branch and excluded claims.

## 4. Decision tree, kill criterion and timebox

1. If G1, G2 or G6 fails, return its typed inconclusive result. One fixture-only
   repair and rerun is allowed from a new frozen reachable source. A second
   validity failure ends the experiment as `INCONCLUSIVE_FIXTURE_EXHAUSTED`.
2. Inventory G4a before running a selector. If E claims A equals B against frozen
   construction, return `INCONCLUSIVE_GROUND_TRUTH_CONFLICT` and stop.
3. Run A0 in the negative trap. If it selects B, record
   `REJECT_ARCHIVED_CARDINALITY_ALIAS` as the A0 subdecision; A0 can never be
   revived. If it refuses, record the A0 subdecision
   `REJECT_A0_NOT_REPRODUCED`: that result may establish that a stricter
   implementation refused safely, but it is neither legacy compatibility nor a
   reason to retain A0. In both cases continue to step 4; an A0 subdecision is
   not the terminal experiment verdict.
4. A1 can return `SELECT_VERIFIED_IMPLICIT_EDGE` only if G4a exists, the positive
   A-owned arm selects A, the A-idle/B-live trap refuses B, G4b proves that the
   edge survives same-directory restart, and every G5 rename/collision assertion
   passes. G4a absent, G4b failure or any wrong/refused positive selection rejects
   A1.
5. If neither implicit design wins, run G7. B returns
   `SELECT_EXPLICIT_DURABLE_BINDING` only if all operations and invariants below
   pass; otherwise keep the TODO as `INCONCLUSIVE_BINDING_DESIGN` and require a
   new precommitment.

G7 operations are create, resolve, restart-revalidate, target-stale, collision,
generation-conflict and revoke. Its invariants are:

- one `(application,directory)` has at most one active instance;
- one instance cannot back two active alias records, including across apps;
- duplicate normalized aliases are typed before selection;
- deleted/recreated Profile or extension-reinstall identity change is typed
  stale, never silently rebound;
- complete inventory with zero/two matching live connections and stale build is
  typed;
- create/replace/revoke require explicit request, runtime session and session
  lease authority plus generation CAS;
- rename within the same directory does not silently create or transfer an
  alias; an explicit CAS replace is required;
- revoke makes every later resolve typed not-found.

**Kill criteria:** one A-label→B-connection selection without complete
G4a/G4b/G5 proof permanently rejects that implicit design. One ground-truth
conflict stops the experiment rather than laundering a bad fixture into either
result.

**Timebox:** stop after one valid negative trap, one positive arm, one
same-directory restart, the rename and duplicate-name cases, the exhaustive
edge inventory and the seven-operation B model. Do not implement a product
resolver, Preferences inventory, compat route or persistent store before review
accepts the result.

Every criterion has an exit: G1/G2/G6 validate, G4a precedes selection, G3
rejects or fails to reproduce A0, G4a+G4b+G5 decide A1, and G7 decides whether B
is ready. G4a present or absent crossed with correct, wrong or refused selections
is covered, and G4b is evaluated only after the A-owned arm can restart.

## 5. Planned research layout

```text
research/browser-profile-name-binding/
├── README.md
├── court-current-host.qjs
├── binding-model.qjs
├── run-current-host.sh
├── fixtures/
│   ├── local-state.json
│   └── preferences.json
└── RESULTS.md
```

The runner requires a clean research directory, reachable `origin/main` source,
the exact extension build identity, bounded attempt counters and a synthetic
HOME. Every digest input must be tracked by that reachable source commit and its
working-tree bytes must equal the committed bytes; an untracked, missing or dirty
input is refused before the court starts. Only the primary agent runs the browser
court.

## 6. Excluded options

| Option | Reason excluded |
|---|---|
| Treat 1-1-1 cardinality as identity | Cardinality supplies no relation between singleton sets |
| Read active `chrome.storage.local` LevelDB | New parser, locking/corruption and privacy surface; MCU did not require it |
| Scan browser command lines | Existing bridge contract excludes it and it does not authenticate extension Profile identity |
| Use email/account identity | New permission and personal identity still do not equal Profile directory |
| Infer from title, last-used state or active tab | Mutable presentation, not identity |
| Launch/activate every candidate | Visible effectful search violates background and no-guess boundaries |
| Persist an absolute Profile path | Redaction, relocation and authority hazard; B stores catalog app plus validated relative directory |
| Keep the TODO after A0/A1 lose and G7 passes | Hides an implementable typed migration |

## 7. This experiment does not answer

1. Linux or Windows browser-profile qualification.
2. Whether aliases should be shared across browser applications.
3. Whether account synchronization preserves `profile_instance_id`.
4. Whether LevelDB access could be safe under another product/precommitment.
5. Whether explicit bindings later need a management UI.
6. How to redesign the separately flaky broad macOS focus fixture.

## 8. Result write-back

After a frozen run, append the decision trace here and write the reproducible
result to `research/browser-profile-name-binding/RESULTS.md`. Only then may
product implementation begin.

An A0 result must say **archived cardinality alias**, never verified binding.
An A1 result must name the exact two-part directory/instance and frozen
name/directory edges. A B result must say **explicit durable alias binding** and
identify the migration boundary. The result may not call a human name
cryptographic identity or generalize an owned-session edge to arbitrary Profiles.

### Attempt 1 and the single fixture repair

Attempt 1 at source `c576db84b7035b7cc353346ae936a49b53cf94ce`
returned `INCONCLUSIVE_DEPENDENCY` with
`selector_observation_no_connection` before any selection criterion ran. The
fixture had selected installed stable Google Chrome; no bridge connection
appeared within the bounded deadline. The run registered no evidence and made
no product-state change. Its complete source/input/build identities are in the
results file.

The one authorized fixture repair selects installed Brave Origin for attempt 2
and makes the failure path prove a 0/0 connection inventory after stopping the
browser. This does not change G1-G7, the A0/A1/B decision order or any kill
criterion. Attempt 2 is terminal for validity failures and may run only from a
new reviewed commit reachable from `origin/main`.

### Attempt 2 and terminal research disposition

Attempt 2 at source `6e675b05549cae3e69c94db16080c98a5720af4f`
and input digest
`bc816430bd53f079571128949c859486e944a82dccac8b707e3a364167956eb2`
returned `INCONCLUSIVE_FIXTURE_EXHAUSTED` with
`profile_binding_connection_cleanup_unverified`. The browser process was
stopped, but the post-stop inventory did not prove the required 0/0 state
within the bounded cleanup window. Cleanup is part of G1 validity, so values
computed before that failure are non-authoritative execution trace rather than
an A0, A1 or B decision. The pure G7 model self-test remains a separately
reproducible static fact; it is not a selection verdict or implementation
authorization.

Both authorized attempts are consumed and a third run is forbidden. The
experiment therefore ends without a profile-name-binding design verdict and
does not change compatibility, ledger, evidence or product state. A future
investigation requires a new precommitment that first proves the stopped
browser child owns the bridge connection process identity, preserves a bounded
failure stage in its receipt, and defines cleanup independently of provisional
selector results.
