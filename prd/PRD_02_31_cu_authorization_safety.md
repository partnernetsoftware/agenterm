# `agenterm-cu` authorization, safety and audit

Parent: [Computer-use foundation (`agenterm-cu`)](PRD_02_28_agenterm_cu.md)

Computer-use is a high-risk capability face: full desktop actuation plus remote
transports is exactly the shape used for lateral movement. This module owns the
authorization model, the audit record, and the refusal semantics. It exists so
that the capability face cannot ship before its control face.

Legend: `[x]` shipped, `[~]` partial, `[ ]` planned.

## Why this is not the script-engine posture

- [ ] `agenterm-cu` does **not** inherit the unrestricted local runtime posture
  of [10 script engines](PRD_02_10_rhai_scripting.md). That posture is
  deliberate for a local automation runtime the invoking user already fully
  controls. It is not appropriate for a surface whose defining feature is
  actuating other machines.
- [ ] the difference is the target, not the trust level of the user: a command
  set that reaches beyond the invoking machine needs an explicit grant per
  target, not ambient authority inherited from the process that started it.

## Authorization model

- [ ] every action is authorized before execution. There is no code path that
  actuates a target without passing the authorization decision.
- [ ] authority is granted per target and is explicit, bounded and revocable.
  Possession of a target reference is not by itself authority to act on it.
- [ ] remote credentials, secrets and session material are isolated from
  command payloads, logs, snapshots, screenshots and error text. Redaction is a
  property of the evidence path, not something each call site remembers.
- [ ] the default posture is least capability. A newly reachable target grants
  observation before actuation, and actuation requires a distinct explicit
  grant.
- [ ] a denied action fails typed and locally. It never partially executes, and
  it never falls back to a lower-fidelity path that achieves the same effect.

- [x] a background action never steals the foreground application, its
  keyboard focus or the real pointer position; an explicit `focus` may only
  move focus *inside* the addressed application. A target that cannot be
  reached without doing so fails closed with a typed refusal -- there is no
  silent fallback to global input or implicit privilege. **Proven for
  macOS `current` `invoke` only** (cut 3.50, `scripts/qjs/cu-macos-smoke.qjs`
  STEP "ambiguous --name, missing action, unobservable state, missing
  target and observe-only grant are typed refusals", 2026-08-30): the
  macOS adapter never sends `AXRaise` or activates the application, the
  fixture is an accessory-policy app ordered front without activation, and
  the journey reads `windows --focused true` before the actuation section
  (set-value, set-checked, press, increment, decrement, select-option and
  every refusal) and after it, requiring the same focused window handle
  and never the fixture's. Cut 3.51 (slice 3, 2026-08-30) extends the same
  proof to `menu invoke` (a background `AXPress` on the application's menu
  bar; the focused window handle is read before and after the menu STEP),
  to `focus` / `invoke --focused` (the STEP "focus moves the first
  responder to the text field ..." moves focus *inside* the fixture only —
  `focused` reads it back — while the user's focused window stays the
  same) and to the observation STEP. Cut 3.52 (slice 4, 2026-08-31) adds
  the **pointer invariant**: the journey reads `pointer-position` (a
  read-only macOS `CGEvent` sample, no event posted) immediately before
  and after every `click` and every `close` and requires the same
  coordinates, proving the real pointer never moved; `close` (the
  destructive verb) runs a background `AXPress` on the window's own
  `AXCloseButton`, and the focused window handle is read before and after
  the whole click / frame / close section and is never the fixture (the
  fixture is proven non-frontmost: its window can be the app's key window
  for the `focused` read while `AXFocusedApplication` stays the user's
  real frontmost app, which the adapter now checks via `AXFrontmost`).
  Not proven here: keyboard focus of the user's application (the journey
  reads the focused *window*, not the keystroke sink), Linux / Windows,
  and remote tiers.
- [x] refusals use one typed vocabulary across every tier: `unsupported`
  (backend lacks the capability), `degraded` (a weaker path was used and says
  so), `denied` (authorization or OS permission), `needs-privilege` (an
  elevation would be required and was not attempted). **Proven for macOS
  `current` `invoke` / `verify` / `wait --expect`** (cut 3.50, same
  journey STEP): `unsupported` for an action the node does not list
  (`detail.reason = node_action_missing`, offered actions in
  `detail.offered`), for a desired-state verb on a node with no readable
  state (`state_unobservable`) and for a `verify` state the node does not
  expose; `refused` for an actuation under an observe-only grant;
  `ambiguous` (with `count`) for two showing matches; `a11y_node_not_found`
  for none; `unverified` for a `verify` mismatch; `timeout` (carrying the
  last observation) for `wait --expect` (identity-only
  `name`/`titleIncludes` is a legal expect item; it does not change
  grant: still `observe`); `usage` / `invalid_input` for a
  malformed action or value. Cut 3.51 adds the background vocabulary, all
  journey-proven: `a11y_menu_item_not_found` / `a11y_menu_item_ambiguous`
  / `a11y_menu_item_disabled` / `a11y_menu_item_not_leaf` (every one
  refused before anything is pressed), `refused` for `menu invoke` under
  an observe-only grant, `unverified` for a `focused` / `invoke --focused`
  role binding the focused control does not meet, `usage` for `--focused`
  mixed with another selector or an unknown `observe` notification, and
  `invalid_input` for an out-of-range menu depth. `denied` for a missing Accessibility
  permission is pure-tested (cut 3.49), not live; `degraded` and
  `needs-privilege` are not exercised by this journey (no coordinate path
  and no elevation exists on the macOS actuation path); Linux / Windows /
  remote tiers are not claimed.
- [x] delivery is not success: every actuation result carries `verified`
  (the postcondition was read back) or `unverified`, and a receipt naming
  target, node, action and observed state survives the process. **Proven
  for macOS `current` `invoke`** (cut 3.50, `cu-macos-smoke` STEPs "invoke
  set-value ...", "invoke set-checked true twice ...", "invoke press
  advances the count label ...", "invoke increment / decrement ...
  select-option ..."): each accepted reply carries `verified: true|false`
  with `verification.method` (value / checked / expanded read-back, tree
  diff for `press`) and `reason` when false, plus the receipt (`target`,
  `node` id / role / name / identifier / index, `action`, `value`,
  `performed`, `before` / `after` node state); a mechanism failure returns
  the receipt in `error.detail.receipt` (the `select-option Omega` case).
  Cut 3.51: `menu invoke` answers the same way (`verified` by mark
  read-back or a whole-window tree diff, `mark_before` / `mark_after`,
  `no_observable_change` when neither moved) and `invoke --focused`
  carries the focused identity it bound in `target` / `node`. Cut 3.52
  (slice 4) builds the **crash-persistent effect receipt**: every
  actuation (`invoke`, `menu invoke`, `click`, `focus`, `close`) opens a
  per-target JSONL file under the audit dir and appends a `reserved` line
  (target, node, action, value, before / snapshot) **before** the
  mechanism is touched and a `completed` / `failed` line (after,
  verified, verification method / reason) after the read-back, each
  flushed; a `reserved` line with no partner is the crash signature
  (uncertain, never "did not happen"), and `receipts --window H --max N`
  reads them back in order. Failure to reserve is failure to act
  (`receipt_unavailable`). `click` and `focus` now also carry `verified`
  (tree-diff for click, focused-readback for focus). Proven by
  `cu-macos-smoke` STEPs "click --node and click --name ..."
  (`cu.macos-ax-click`) and "receipts lists every actuation of this run
  in order ..." (`cu.macos-ax-receipts`: the receipt file lists the
  run's invoke / menu / focus / click / close lines, reserved before and
  completed after each, the close snapshot inside its reserved line).
  `send-text` and the older verbs still answer without `verified`.
- [x] a destructive action (close, quit, delete, overwrite) requires an exact
  target reference, a prior snapshot of the state it changes and a checkable
  postcondition; without all three it is refused typed. **Proven for macOS
  `current` `close`** (cut 3.52, slice 4, `scripts/qjs/cu-macos-smoke.qjs`
  STEPs "close without a snapshot, a postcondition or a target ..."
  (`cu.macos-ax-destructive-refusals`) and "close --pid --title --snapshot
  --expect gone closes the second window ..." (`cu.macos-ax-destructive-close`)):
  `close --window H [--pid N] [--title T] --snapshot --expect gone` closes
  one top-level window through the platform's own close control (macOS
  `AXCloseButton` + `AXPress`, never activating the app). The three-part
  gate is checked before anything is touched — an exact target (`--window`,
  bound to `--pid` / exact `--title` in one inventory read), a prior
  snapshot (`--snapshot`: the bounded tree of the target written to the
  reserved receipt) and a checkable postcondition (`--expect gone`: the
  handle read back as absent) — and any missing part is typed `refused`
  (`detail.reason = destructive_gate`, `missing` naming which, `effect:
  not_performed`) with nothing performed; a wrong `--pid` / `--title` is
  `window_identity_mismatch`, an unknown handle `window_not_found`, an
  observe grant `refused`, a bad postcondition `invalid_input`. The journey
  closes the fixture's second window with all three, reads it gone from the
  inventory (pointer and focused window unchanged, the main window and
  process still alive), and a second close of the same handle is
  `window_not_found`. `invoke` still offers no destructive action (no
  quit / delete). Linux / Windows / remote tiers are not claimed.
## Audit

- [~] every authorized action produces an observable record identifying target,
  command, decision, outcome and time, sufficient to reconstruct what was done
  to which machine. `audit-query` now exposes newest-first bounded observation
  with independent returned-row, scanned-row and byte ceilings, malformed-row
  counts, truncation flags and a continuation offset when only the result
  window truncates. Retention/compaction, an older-byte cursor and
  universal request correlation remain open.
- [~] the versioned JSONL audit is machine-readable, cross-process append
  serialized, durably flushed and survives the session that produced it.
- [x] failure to record is failure to act: if the audit path is unavailable, the
  action does not execute.

## Stable macOS consent identity

- [ ] Routine CU execution must come from one fixed installed app-bundle path
  carrying a stable signed identity. Worktree binaries are development-only:
  changing path or code identity invalidates prior Camera/Accessibility TCC
  consent and can make a host-permission failure resemble a device fault. The
  Candidate pipeline must ship and verify the original signed
  `AgentermCu.app` bytes, include separate Camera and Accessibility usage
  descriptions, and install shims only toward that verified executable. It
  must never locally re-sign promoted bytes. Camera, Accessibility and Screen
  Recording remain distinct permissions; a DAL capture verb must not demand
  Screen Recording or treat Accessibility denial as a phone trust failure.

## Refusal semantics

- [ ] refusal is typed and distinguishable from mechanism failure. A caller can
  always tell "you are not allowed" from "this target cannot do that" from
  "this attempt failed".
- [ ] no refusal is silently retried through another tier, transport or
  coordinate fallback.

## Delivery gate

- [ ] no tier of [30](PRD_02_30_cu_targets_transports.md) may be claimed shipped
  before this module's authorization, audit and refusal requirements are proven
  for that tier. The `current` tier is included: local actuation is not exempt
  because it is local.
- [ ] the evidence is a public black-box journey proving an unauthorized action
  is refused, a revoked grant stops taking effect, an authorized action is
  recorded, and credential material is absent from every published artifact.
- [ ] a security review of this surface is required before any remote transport
  tier is claimed, and it belongs to the release gate rather than to the
  authoring agent's own judgment.

## Windows current checkpoint

- [~] Legacy `--grant` / `AGENTERM_CU_GRANT` selection is now strict and
  single-source: a present CLI value wins without union or fallback, and an
  empty or unknown scope returns typed `invalid_authorization` without echoing
  the supplied token. The Windows smoke supplies an environment `actuate`
  scope behind an explicit CLI `observe` placement attempt and proves the
  action remains refused with unchanged bounds. These remain ephemeral
  process inputs, not persisted or target-bound grants.
- [~] SSH and VNC worker environment forwarding now reserves the complete
  case-insensitive `AGENTERM_CU_GRANT*` and `AGENTERM_CU_AUTH*` namespaces.
  Caller-supplied matches fail typed before SSH spawn or VNC handshake, the
  value is not echoed, and inherited selectors are removed from child process
  environments. Existing raw scope forwarding is still legacy ephemeral
  behavior, not a target-bound delegation protocol.
- [~] A persisted bounded grant-store core now owns validated create/list,
  pre-effect attempt reservation, revocation, expiry/not-before, exact
  target+tier+session matching, generation conflict detection, and corrupt
  store refusal. Observe grants are capped at 24 hours / 10,000 uses; any
  grant containing Actuate is capped at 1 hour / 100 uses, and one-shot means
  exactly one attempt even when the downstream mechanism fails. RDP cannot
  receive a grant while its transport is unavailable. Publication uses a
  flushed same-directory replacement snapshot. Cooperating writers now take a
  zero-wait platform lock on a stable sibling sidecar, re-read generation while
  holding it, and retain the guard through replacement and durability handling;
  contention is typed and publishes nothing. This closes the local
  compare-to-rename race without claiming protection from non-cooperating
  writers, hostile sidecar replacement, cross-session Windows callers or
  filesystems without coherent locking. Store schema 2 now accepts grant specs
  and attempts only through the sealed verified binding type, persists an
  explicit binding version, and requires exact fixed-prefix lowercase target
  and session identifiers. Legacy schema 1 contained caller-provided identity;
  it fails typed and remains byte-for-byte unchanged instead of being silently
  reinterpreted as trusted. A separate production open path resolves the
  machine-local product-data location, refuses bare filenames and link-like
  stores, protects the parent directory, and publishes every replacement from
  an exclusively created private temporary. The raw `open_at` remains only an
  injected-path seam. A current-only `grant create/list/revoke` management CLI
  now uses that production path, generates opaque IDs from platform entropy,
  explicitly enrolls and resolves the sealed binding, and projects records
  without session identity. It rejects ambient authorization selectors and
  sanitizes store failures. Current-target `--grant-id` execution now opens the
  audit first, resolves the verified binding, durably reserves one attempt,
  flushes an authorized attempt record, re-resolves the exact binding before
  dispatch, and writes the outcome with the same opaque decision ID. Denials
  are recorded without consuming uses; a failed downstream command does not
  refund a reserved use. Session identity/key material is absent from the
  audit. The public Windows smoke now owns an isolated one-shot observe grant:
  its first `capabilities` command succeeds, the second is refused as
  exhausted, and a separately revoked grant is refused before dispatch. Four
  JSONL records prove that the authorized attempt/outcome share one decision
  ID, exhausted and revoked denials have distinct decisions, and no session,
  install-key or credential-like field reaches the audit. That journey has
  passed against a current-source CU binary; the complete staged smoke still
  needs a same-source ABI artifact rerun because the available older staged
  library failed the unrelated clipboard-read contract later in the task.
  Remote delegation and session-nonce invalidation remain open.
- [~] A sealed `TargetBinding` contract now separates opaque provider identity
  and exact desktop-session identity from routing material. Current, SSH and
  VNC fail typed when no crate-owned verified provider is available; RDP stays
  `unsupported` even if a provider is offered. The public resolver accepts no
  hostname, IP, user, port, `DISPLAY` or arbitrary binding-string constructor.
  `agenterm-platform` now provides the first Windows `current` identity
  mechanism: enrollment creates one locked, current-user-only 32-byte install
  key, while load/query never rotate missing or corrupt state. The session
  digest binds the opaque provider to token SID, authentication and terminal
  session identifiers, positive WTS active/logon evidence, and the current
  input desktop. Session zero, a disconnected or changed desktop, unsafe key
  state, and unavailable proof fail typed. The digest is only an opaque
  equality identifier, not an authenticator. CU now adapts those two opaque
  32-byte identities into its sealed, fixed-prefix target/session binding;
  resolution never creates state, while a separate explicit enrollment call
  owns first installation. Linux and macOS providers remain explicitly
  unsupported. Store schema 2, the management CLI and the current-target
  executor now consume this verified binding; remote tiers still have no
  verified provider or delegation path.
- [x] The staged Windows x86_64 `cu-windows-smoke` proves an observe-only
  `window-place` is typed `refused` and leaves the owned fixture bounds
  unchanged. The authorized call writes exactly one `attempt` and one `ok`
  JSONL record through one retained audit handle; each record names the
  `authorized` decision and process-bounded authority scope. Independent
  window enumeration confirms the reported placement, and the smoke rejects
  credential-like fields from the published audit records.
- [ ] This checkpoint proves current-target one-shot exhaustion and revocation,
  but does not yet prove bounded expiry through the public gate, every
  actuation verb, credential absence from every published artifact, Windows
  ARM64, another OS, or the required remote-transport security review. The
  module therefore remains planned/partial rather than shipped.
