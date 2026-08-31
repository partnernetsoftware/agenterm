# `agenterm-cu` targets and transports

Parent: [Computer-use foundation (`agenterm-cu`)](PRD_02_28_agenterm_cu.md)

Delivery truth: `agenterm-cu` is the sole executable and the first runtime
consumer of `libagenterm`. Target selection and transport policy remain product
semantics here; native window, accessibility, input and desktop-host mechanisms
remain behind the ABI/platform boundary.
This module owns the target family, transport selection, and the per-platform
backends that realize the abstract command set from
[29](PRD_02_29_cu_command_surface.md).

Legend: `[x]` shipped, `[~]` partial, `[ ]` planned.

## Target family

Branch status (cut 3.53 — macOS AX web content live: owned `WKWebView`
fixture, `AXWebArea` tree, `unlock` = real `AXManualAccessibility` poke
(ABI 1.15) reported from a re-read, positive `scroll` with an independent
extents readback; macOS AX semantic `send-keys` (`enter` → `AXConfirm`);
macOS AX node text / geometry live:
`get-extents` / `select` / `get-selection` / `set-caret` / `get-caret`
through `AXPosition`+`AXSize` and `AXSelectedTextRange`, `scroll` mapped to
`AXScrollToVisible`; 3.51 macOS AX background verbs live: `menu inspect` /
`menu invoke` / `focused` / `invoke --focused` / `observe` through ABI 1.14;
3.50 macOS AX semantic actuation live: `invoke` /
`verify` / `wait --expect` through ABI 1.13; 3.49 macOS AX observe live with
bounded walk, action names and typed permission denial; 3.48 Linux `tree`
cross-tier conformance;
3.47 per-target `capabilities`; 3.46 RDP placeholder):

| Target | Status | Notes |
|--------|--------|-------|
| `current` | **[x]** | Local in-process; Linux/Windows evidence held; macOS AX observe (3.49), semantic actuation `invoke` / `verify` (3.50) and background `menu` / `focused` / `observe` (3.51) proven (`cu-macos-smoke`); `capabilities` names `data.target:"current"` + live libagenterm status; Linux `tree` cross-tier proven (3.48) |
| `ssh` | **[x]** | OpenSSH exec of remote `--target current`; `capabilities` restores public/`data.target:"ssh"`; Linux `tree` cross-tier proven (3.48) |
| `vnc` | **[x]** | RFB + local `--target current` worker; `capabilities` restores public/`data.target:"vnc"`; Linux `tree` cross-tier proven (3.48) |
| `rdp` | **[~]** | PLACEHOLDER: parseable; `capabilities` declares transport placeholder/unavailable + `tree` unsupported with zero I/O; other authorized commands typed `rdp_unavailable`. Transport/session/live evidence empty |

- [x] `current`, `ssh`, and `vnc` are tiers of one family sharing one
  command set. `current` is the **local degenerate tier** — transport is
  in-process — not a temporary prototype to be replaced later. `ssh` first cut
  is OpenSSH `ssh` exec of a remote `agenterm-cu --target current` worker
  (`--ssh <user@host>`; same verbs including actuate; no new verb). `vnc`
  first cut is RFB handshake to `--vnc <host[:port]>` (security type None /
  `x11vnc -nopw`) then a local `agenterm-cu --target current` worker against
  the shared session (`DISPLAY` / AT-SPI env; same verbs; no new verb).
- [~] `rdp` (cut 3.46 PLACEHOLDER + cut 3.47 declaration) is parseable and
  fail-closed for live work. Purpose: remote Windows desktops (and other
  RDP endpoints). Public surface: `TargetRef::Rdp`, `as_str() == "rdp"`,
  `--rdp HOST[:PORT]` (implies `--target rdp`; default port 3389 is
  syntax-only), and explicit `--target rdp`. **`capabilities` (observe)**
  succeeds with a static declaration: `data.target:"rdp"`, transport
  `status:"placeholder"` / `available:false` / `reason:"rdp_unavailable"`,
  verb `capabilities` available, verb `tree` **unsupported** with the same
  reason — **no** socket connect, DNS, TLS/CredSSP, UIA, screenshot, or
  `--coords`. Every other authorized RDP command (reserved first *live*
  observe: `tree --window HANDLE`) returns `ok:false`, `target:"rdp"`, the
  original command name, and `error.code:"rdp_unavailable"` with **no**
  socket connect, TLS/CredSSP/NLA, credential flag, screenshot,
  `--coords`, or silent `ssh`/`vnc`/`current` reuse. `--target rdp`
  without `--rdp`: `capabilities` still declares the tier; other verbs
  return the same typed `rdp_unavailable` family (precise
  missing-endpoint message), not a generic usage/`current` fallback.
  Malformed ports remain `invalid_input`. Transport, session lifecycle,
  authentication, and live UIA-over-RDP evidence are **[ ]** — owned by a
  later Windows agent (see Evidence handoff below). Windows UIA on
  `current` is a separate line and is not promoted by this placeholder.
- [~] `current` ships first. Doing so is the cheapest way to pin the interface,
  because adding a remote transport afterwards changes transport only, not the
  commands above it. `ssh` get-selection evidence reuses the #50 con-publish
  selection observe path over loopback `sshd` against a second `agenterm-con`
  (never steal the resident control socket): host
  `cu --ssh send-text --name Command -- SEED` (payload after `--`; not
  `--text`) plants the seed, host `cu --ssh select --name Command --start N
  --end M` runs remote AT-SPI `Text.SetSelection`, then host independent
  `cu --ssh get-selection --name Command` returns that range
  (`via=get-selection`; start/end equal the selected slice of the seed, or
  the seed when the range is the whole field). Native AT-SPI
  `GetNSelections` + `GetSelection`. Never screenshot, `--coords`, or XTest.
  `vnc` first get-selection evidence reuses the #50 con-publish selection
  observe path over a **gate-owned** loopback `x11vnc` (not the resident
  `:2` listener alone) against a second `agenterm-con` with a unique
  title: `Command` holds a known ASCII seed and a known non-empty
  selection `START..END` (gate precondition via already-landed
  `send-text` + `select`; not this cut's verb), then host independent
  `cu --vnc 127.0.0.1:<port> get-selection --window HANDLE --name Command`
  returns that range (`via=get-selection`; native AT-SPI
  `GetNSelections` + `GetSelection(0)`; `n == 1` and integer `start` /
  `end` equal the precondition range so `seed[start:end] == expected`).
  Never screenshot, `--coords`, mouse-drag, RFB framebuffer OCR, cached
  setter reply, or steal the resident control socket. `get-extents`
  (3.43), `get-caret` (3.42), `tree` (3.41), `focus` (3.40), `scroll`
  (3.39), `click` (3.38), `set-caret` (3.37), `select` (3.36),
  `send-keys` (3.35), `copy` (3.34), `paste --text` (3.33), and
  `send-text` (3.32) over vnc still hold.
- [ ] a target reference is explicit, addressable and stable for the lifetime of
  its session. Enumerating targets remains planned; describing one target's
  declared capabilities is the existing `capabilities` observe verb (cut 3.47).
- [~] capability differences between tiers are **declared, not discovered by
  failure** (cut 3.47). A caller can ask `capabilities` what a target supports
  before acting: `current` reports live libagenterm status;
  `ssh`/`vnc` restore public/`data.target` to the requested tier while
  retaining worker mechanism facts; `rdp` declares transport placeholder and
  does **not** claim `tree` supported. Discovery still requires the observe
  grant and grants no actuation right. Target enumeration remains **[ ]**.
  Linux `current` / `ssh` / `vnc` **`tree` semantic cross-tier conformance is
  proven** (cut 3.48; harness `scripts/cu-linux-cross-tier-tree.sh`); RDP,
  macOS AX, Windows-over-RDP, and other verbs remain open. An unsupported
  command returns typed `Unsupported` / `rdp_unavailable` rather than a fake
  success or a silent coordinate fallback.

## Platform backends

- [ ] backends consume `agenterm-platform` contracts — screenshot, window,
  input, accessibility tree, process reference, clipboard, filesystem — and do
  not open raw OS APIs. A missing mechanism is added to the platform crate with
  typed `Available`/`Unsupported`/`Failed`, per
  [20 Native platform](PRD_02_20_native_platform.md).
- [ ] Windows, Linux and macOS each reach the same abstract command set through
  their own accessibility/input stacks. Product behavior does not move into the
  adapters: what a click *means* is one shared rule; how a click is delivered is
  per host.
- [ ] a platform whose control-tree access is unavailable or not yet wired
  returns typed `Unsupported` / `Failed`. Coordinate-only or screenshot-only
  operation is always visible in the command result ([29](PRD_02_29_cu_command_surface.md));
  it is never a silent substitute for structured success.
- [ ] first-platform delivery is explicit and does not imply the others. A tier
  or platform is claimed only with its own evidence.

- [x] macOS `current` observation is live (cut 3.49, 2026-08-30, slice 1 of
  `plan/design-mcu-absorption.md`): `windows` returns stable CGWindow handles
  with PID / app / title inventory filters, `tree --window H` and `query`
  return a real AX tree (`backend: ax`, `degraded: false`) through
  `libagenterm` ABI 1.12 with a traversal-time depth / node budget,
  `truncated` / `visited` / `returned` counts, per-node `AXActionNames`
  actions and `AXIdentifier`. Evidence is `scripts/qjs/cu-macos-smoke.qjs`
  against an owned Cocoa fixture (`examples/objc/agenterm_ax_fixture.m`):
  STEP "capabilities declare the AX tree and query available"
  (`cu.macos-ax-capabilities`), STEP "public wait and windows --pid prove
  PID, title and CGWindow identity" (`cu.macos-ax-window-identity`), STEP
  "query by role, identifier and exact text" (`cu.macos-ax-query`), STEP
  "tree --max-nodes 5 and --depth 0 report truncated"
  (`cu.macos-ax-tree-bounded`), STEP "full tree reports truncated false and
  AX action names" (`cu.macos-ax-tree-actions`), STEP "SIGTERM ends only the
  owned fixture" (`cu.macos-ax-fixture-cleanup`). Missing Accessibility
  permission is typed `denied` with the repair path (see the shared leaf
  below).
- [x] macOS `current` semantic actuation is live (cut 3.50, 2026-08-30,
  slice 2 of `plan/design-mcu-absorption.md`): `invoke --window H
  (--node | --index | --name [--role] | --identifier) <press | set-value |
  select-option | set-checked | set-expanded | increment | decrement>`
  runs through `libagenterm` ABI 1.13 `agt_a11y_node_invoke` into the AX
  adapter (`AXPress`, `AXValue` write with read-back, pop-up option chosen
  by pressing the matching `AXMenuItem`, desired-state `set-checked` /
  `set-expanded` that read before pressing and read back after,
  `AXIncrement` / `AXDecrement`); `verify --expect` and `wait --expect`
  read the same tree back. Nothing activates or raises the fixture
  (`AXRaise` is never sent), and the journey proves the focused window
  handle before and after the actuation section is the same one and never
  the fixture. Evidence is `scripts/qjs/cu-macos-smoke.qjs` against the
  extended fixture (text field `fixture-field`, check box `Fixture Check`,
  stepper `fixture-stepper`, pop-up `fixture-popup`, count label
  `fixture-press-count`, twin buttons `Fixture Twin`): STEP "invoke
  set-value writes the text field and verify --expect reads it back"
  (`cu.macos-ax-invoke-set-value`), STEP "invoke set-checked true twice:
  the first presses, the second is a verified no-op"
  (`cu.macos-ax-invoke-set-checked`), STEP "invoke press advances the
  count label; wait --expect and verify read it on another node"
  (`cu.macos-ax-invoke-press`), STEP "invoke increment / decrement on the
  stepper and select-option on the pop-up read back"
  (`cu.macos-ax-invoke-value-readback`), STEP "ambiguous --name, missing
  action, unobservable state, missing target and observe-only grant are
  typed refusals" (`cu.macos-ax-invoke-refusals`). Not claimed: `click` /
  `focus` on macOS (mapped to `AXPress` / `AXFocused` in the adapter but
  not in the journey), destructive actions (none is offered), Linux /
  Windows live evidence for the new verbs.
- [x] macOS `current` background verbs are live (cut 3.51, 2026-08-30, slice
  3 of `plan/design-mcu-absorption.md` — background menus, the App-local
  focused control and the observation stream):
  `menu inspect` / `menu invoke` read and press the application's
  `AXMenuBar` through `libagenterm` ABI 1.14 (`agt_a11y_menu_snapshot` /
  `agt_a11y_menu_invoke`) without opening a menu on screen or activating
  the app; `focused` / `invoke --focused` read and write the application's
  own `AXFocusedUIElement` (`agt_a11y_focused_snapshot`) as a node in the
  same window tree without requiring the foreground; `observe` is a
  poll-diff event stream over the bounded tree (no AXObserver is wired in
  the platform crate — the reply says `mode: "poll-diff"`). Evidence is
  `scripts/qjs/cu-macos-smoke.qjs` against the fixture extended with a
  never-shown main menu (`File` → `Do Thing` / `Disabled Thing` / two `Twin
  Thing` / `More` → `Deeper Thing`, label `fixture-menu-label`): STEP "menu
  inspect reads the background menu bar and finds File/Do Thing without
  opening it" (`cu.macos-ax-menu-inspect`), STEP "menu invoke presses
  File/Do Thing in the background; the label changes and the focused window
  does not" (`cu.macos-ax-menu-invoke`), STEP "focus moves the first
  responder to the text field and focused / invoke --focused bind it"
  (`cu.macos-ax-focused` — also the first journey proof of `focus` on
  macOS), STEP "observe --duration 1.5 captures the ValueChanged of a
  set-value issued while it runs" (`cu.macos-ax-observe`; the observer is a
  second `agenterm-cu` spawned through the door). The focused window handle
  is the same before and after every actuating section and never the
  fixture. Not claimed: AX notifications (`observe` is poll-diff), Linux /
  Windows for the background verbs (their adapters answer typed
  `unsupported`); macOS `click` is now proven in slice 4 below.
- [x] macOS `current` slice 4 is live (cut 3.52, 2026-08-31, slice 4 of
  `plan/design-mcu-absorption.md` — the destructive gate, crash-persistent
  receipts, and click / focus / pointer / frame journey-proven): `click`
  and `focus` by `--node` and by `--name` press / focus the fixture
  through the a11y backend (verified by tree-diff / focused-readback), the
  read-only `pointer-position` (a macOS `CGEvent` sample that posts no
  event) is read before and after every click and close and is unchanged
  (PRD 31's pointer invariant), `window-place --action frame` sets an
  arbitrary rect in one transaction read back from the inventory, and the
  destructive `close --window H [--pid N] [--title T] --snapshot --expect
  gone` closes one window through macOS `AXCloseButton` + `AXPress` behind
  the three-part gate, every actuation appending a `reserved` / `completed`
  line to a per-target receipt file read back by `receipts`. Evidence is
  `scripts/qjs/cu-macos-smoke.qjs` STEPs `cu.macos-ax-click`,
  `cu.macos-ax-frame`, `cu.macos-ax-destructive-refusals`,
  `cu.macos-ax-destructive-close`, `cu.macos-ax-receipts`. The focused
  window handle is the same before and after the whole section and never
  the fixture (the adapter now confirms the focused application is
  genuinely frontmost via `AXFrontmost`, so a background key panel is never
  mislabeled as the foreground). Not claimed: `close` on Linux / Windows
  (their adapters answer typed `unsupported` / use `WM_CLOSE` untested by a
  journey), pointer *injection* on macOS (only the read is wired), and
  remote tiers.
## Platform accessibility backends

This branch is the **native accessibility stack** that backs structured
`tree` observation and `click` / `focus` by node identity. It lives in
`agenterm-platform` (`a11y-tree` and related contracts); `agenterm-cu` selects the stack
for the host OS and target tier. Screenshot capture and coordinate pointer
injection are separate platform mechanisms and remain **degraded fallbacks**
only — never silent replacements when the a11y tree is unavailable.

```text
targets / transports (30)                    legend: [x] shipped  [~] partial  [ ] planned
├── target family
│   ├── current  [x]  local in-process (Linux/Windows live evidence held)
│   ├── ssh      [x]  OpenSSH exec → remote --target current
│   ├── vnc      [x]  RFB handshake → local --target current worker
│   └── rdp      [~]  PLACEHOLDER: capabilities declares unavailable; other verbs rdp_unavailable; live evidence [ ]
└── platform a11y backends (agenterm-platform)
    ├── Linux AT-SPI2                 [x] live evidence (cu-linux-smoke + named journeys); desired-state / option / range invoke and focused mapped (cut 3.53), not yet journey-proven
    ├── Windows UIA                   [~] existing tree evidence (cu-windows-smoke); expand/range/scroll-item/extents/focused mapped (cut 3.53), not yet journey-proven; separate from rdp
    ├── macOS AX current tree         [x] observe live (cut 3.49, cu-macos-smoke): bounded walk, actions, typed denied
    ├── macOS AX actuation            [x] invoke / verify live (cut 3.50, cu-macos-smoke): AXPress, AXValue write, option press, desired-state checked/expanded, increment/decrement; focus journey-proven in 3.51, click mapped only
    ├── macOS AX background           [x] menu inspect / invoke, focused / invoke --focused, observe (poll-diff) live (cut 3.51, cu-macos-smoke): AXMenuBar walk + AXPress, AXFocusedUIElement; no AXObserver
    ├── macOS AX node text/geometry   [x] get-extents / select / get-selection / set-caret / get-caret live (cut 3.53, cu-macos-smoke): AXPosition+AXSize, AXSelectedTextRange; scroll maps to AXScrollToVisible, which AppKit does not publish
    ├── macOS AX semantic send-keys   [x] enter -> AXConfirm, escape -> AXCancel live (cut 3.53, cu-macos-smoke); every other chord typed: macOS delivers keys only to the active app (measured), and cu never activates
    ├── macOS input injection         [x] pointer-move / click / type-text / send-keys on the HID tap live (cut 3.53, cu-macos-pointer-smoke); no window-local route exists (measured), so --to <handle> is refused, not approximated
    ├── macOS AX web content          [x] AXWebArea tree, unlock (AXManualAccessibility, ABI 1.15), positive scroll with extents readback, web invoke / verify live (cut 3.53, cu-macos-web-smoke) against an owned WKWebView fixture
    └── RDP remote desktop transport  [ ] not started (session + UIA-over-RDP later Windows cut)
```

**Cut 3.49 / 3.50 boundary:** the macOS AX **observe** path (`windows`,
`tree`, `query`) and the **semantic actuation** path (`invoke`, `verify`,
`wait --expect`) are live in `agenterm-platform`
(`adapters/macos/accessibility_tree.rs`, backend string `"ax"`) and proven
by `scripts/qjs/cu-macos-smoke.qjs`. A caller's depth / node budget ends
the walk with `truncated: true` instead of failing; typed failures remain
`a11y_permission_denied` (surfaced as cu `denied` with the repair path),
`unsupported`, `a11y_tree_timeout`, `a11y_action_no_effect` (a write whose
read-back does not match), `a11y_option_not_found` / `a11y_option_ambiguous`,
and the adapter's own string / sibling limits. No screenshot, no
`--coords`, no CGEvent fallback, no `AXRaise` / activation, no silent
AT-SPI/UIA reuse.

Canonical host mapping (approved product vocabulary):

| Host | Native accessibility stack | Structured `tree` source | Structured actuation |
|------|---------------------------|--------------------------|----------------------|
| Windows | native API + **UIA** | `IUIAutomation` control tree | UIA patterns / legacy accessible (`Invoke`, `LegacyIAccessible`) |
| macOS | **AX** (`NSAccessibility`) | accessibility element tree — **live** (bounded walk, `AXActionNames`, `AXIdentifier`) | `AXPress` / `AXValue` write / option `AXPress` / desired-state checked & expanded / `AXIncrement` `AXDecrement` — **live** (`invoke`, cut 3.50); `AXRaise` is never sent |
| Linux | **AT-SPI2** (`at-spi2-core` / `org.a11y.atspi.*`) | AT-SPI accessible hierarchy — **live** | AT-SPI `Action` / `Component` / `EditableText` — **live** |

### Requirements by stack

**Shared (all hosts)**

- [ ] `agenterm-platform` exposes one typed `accessibility_tree` contract:
  flattened nodes with path id, role, name, states, exact bounds, and action
  names. `agenterm-cu` maps this contract to its public JSON without host-specific
  fields leaking upward.
- [~] when the a11y bus / API is missing (headless without a11y, no registry,
  denied permission), `tree` and node actuation return typed `Unsupported` or
  `Failed` — never coordinate guessing while reporting structured success.
  macOS (cut 3.49): a stack the OS refuses is not "unsupported" — with
  Accessibility permission absent, `capability_status()` answers
  `Failed { a11y_permission_denied }`, ABI 1.12 `agt_capability_query` /
  every `agt_a11y_*` export answer `AGT_FAILED{a11y_permission_denied}` with
  the repair path in the message, and cu replies `error.code = "denied"`
  with `detail.repair` ("System Settings > Privacy & Security >
  Accessibility ...") while `capabilities` reports `tree: "Denied"` — never
  an empty tree. Pure evidence only (`permission_denial_is_typed_denied_with_repair_path`);
  the journey host holds the permission, so the denied branch is not
  exercised live. Linux / Windows unchanged.
- [ ] screenshot and coordinate pointer paths are explicit degraded modes with
  observable markers in the reply; they do not satisfy a caller that requested
  structured node identity.

**Linux — AT-SPI2 (`current` first evidence)**

- [~] `current` on Linux/X11 enumerates a control tree through AT-SPI2:
  `agenterm-platform` (`a11y-tree`) implements the host stack; `agenterm-cu` consumes
  libagenterm milestone 6 (`agt_a11y_tree_snapshot` / `agt_a11y_node_perform`)
  rather than calling platform accessibility APIs directly. Nodes carry role,
  name, states, screen bounds, and action names; node ids are child-index paths
  from each application root (for example `/3/0/0/1/0`).
  A `--window` snapshot matches AT-SPI application roots by the X11
  `_NET_WM_PID`, that process's descendants (WebKit web process), and
  exact title / `WM_CLASS` / `comm` equality — not PID equality alone.
  Child walks read raw `(bus name, path)` pairs so well-known embed
  destinations (WebKit `org.webkit.app-*.Sandboxed.WebProcess-*`) are
  not dropped by unique-name-only `ObjectRef` parsing. The walker talks
  to the a11y bus only (no atspi P2P handshake — that hangs WebKit/Wails
  sockets), skips dests with no owner, maps empty WebKit `GetRoleName`
  via `GetRole`, and snapshots Accessible name/role/state so a Reasonix
  / MiniBrowser document tree exposes named inner widgets (buttons,
  text, tabs). `agenterm-con` on
  Linux registers as an AT-SPI toolkit (`a11y-publish`) and exposes the
  painted chrome as children (tabs, session, Command input, SEND). A
  toolkit that still never registered (stock `xfce4-terminal` without
  atk-bridge) returns a one-node showing `frame` from the X11 window
  title and bounds so named `wait` / `focus` / `send-keys` can address
  that window; `focus`/`click` on that node raise it. The one-node frame
  is not the success path for `agenterm-con` and is not a screenshot or
  coordinate substitute.
- [~] `click --node <path>` and `focus --node <path>` invoke AT-SPI `Action`
  / `Component` for the resolved node via `agt_a11y_node_perform`. A
  node-addressed click uses a named `click`/`press` when present, otherwise
  the AT-SPI default action (`DoAction(0)`) when the node exposes Action —
  including Chrome controls whose `GetActions` names are empty. A showing
  named node with no Action interface uses the AT-SPI Component path
  (`GetExtents` + registry `GenerateMouseEvent`) and still reports
  `addressing=accessibility-tree`. It never silently becomes `--coords` /
  `--degraded`. Focus stays named-`focus` then `Component::grab_focus`.
  Invalid paths return typed `a11y_node_not_found`.
- [~] `click --window HANDLE --name PAT [--role ROLE]` and the matching
  `focus` form resolve one showing node with the same matcher as
  `wait --node-name-contains`, then call the node-path AT-SPI action above.
  `--name` cannot be combined with `--node` or `--coords`. A miss is typed
  `a11y_node_not_found`. Two or more showing matches are typed
  `a11y_node_ambiguous` with the match count; the command does not pick
  the first. There is no screenshot or degraded-coordinate substitute.
- [~] the desired-state, option and range half of `invoke` on Linux
  (cut 3.53), mapped but not journey-proven: `set-checked` and
  `set-expanded` read the node's `StateSet`, act only on a difference and
  poll the state back (already being in the requested state is success
  with no action performed); `select-option` resolves the child named
  exactly the option through AT-SPI `Selection.SelectChild`, refusing a
  duplicate name as `a11y_option_ambiguous` and a miss as
  `a11y_option_not_found`; `increment` / `decrement` step the `Value`
  interface by the backend's own `MinimumIncrement`, clamped to the
  published range and read back. A node whose backend publishes no such
  state or interface answers `a11y_action_unavailable`; nothing here falls
  back to a synthetic click, a keystroke or `--coords`.
- [x] AT-SPI publishes only the states that are set, so this cut names both
  directions of a state it can read: `checkable` yields `checked` /
  `unchecked` / `mixed` and `expandable` yields `expanded` / `collapsed`,
  the same vocabulary the macOS AX and Windows UIA adapters report. A
  control that is neither checkable nor expandable gains no words, so
  `verify --expect checked:false` still fails closed against a node with
  no such state.
- [~] `focused --window HANDLE` on Linux (cut 3.53), mapped but not
  journey-proven: the App-local focused control is the deepest node
  carrying `STATE_FOCUSED` in a bounded walk of the window's own tree
  (depth 24 / 4000 nodes), so no event subscription is needed and nothing
  is activated or raised. A truncated walk that found no marked node says
  so rather than reporting "no focus". `capabilities` declares
  `focused.mode = "state-search"`: a search is a weaker claim than a
  toolkit naming its own focus, and the declaration must not hide that.
- [x] `send-text --window HANDLE --name PAT [--role ROLE] [--] <text...>`
  resolves through that same path, then writes via AT-SPI `EditableText`
  (`SetTextContents` / `InsertText`, `agt_a11y_node_set_text`) or, when
  the node exposes `Text` + `editable` but not `EditableText` (Chrome,
  WebKitGTK/Reasonix `<textarea>`), via AT-SPI `Text` plus the toolkit
  set-value, confirmed by
  `GetText`. A named showing node with no writeable text interface
  typed-fails (`a11y_text_unavailable`) and never silently uses XTest /
  `input_inject::type_text`. Resolution failure (miss or ambiguous name)
  aborts before any write. Without `--window`, `send-text` still injects
  into whatever is focused.
- [x] `send-text --window HANDLE` without `--name` writes that same
  AT-SPI `EditableText` / `Text` + toolkit set-value path on the
  showing focused node (innermost Text candidate). Never XTest /
  `input_inject::type_text` when `--window` is set. Independent
  `get-text --window HANDLE` (no `--name`) must equal the typed
  string. Live hosts: agenterm-con named `Command` (native
  `EditableText` on a second con; never steal the resident control
  socket), Chrome `GetTextField` after `focus --name` (renderer on the
  same host AT-SPI bus: `AT_SPI_BUS` / `AT_SPI_BUS_ADDRESS`), and
  Reasonix composer `Message Reasonix…` under
  `scripts/reasonix-desktop-a11y.sh` (AT-SPI `Text` plus the
  eval-helper set-value; no protocol change). Do not mark this leaf
  shipped on worker JSON.
- [x] `copy --window HANDLE --name PAT [--role ROLE]` resolves through
  that same path, then publishes AT-SPI `Text.GetText`
  (`agt_a11y_node_get_text`) onto the native clipboard
  (`agt_clipboard_set_text`) and reports `via=gettext`. On Linux X11 the
  seed is a native CLIPBOARD selection owner, not `xclip`. A named
  showing node with no Text interface typed-fails
  (`a11y_text_unavailable`) and never silently uses XTest / `--coords` /
  screenshot. Resolution failure (miss or ambiguous name) aborts before
  any clipboard write. Live close-the-circuit includes Chrome fixture
  fields and the Reasonix composer (`Message Reasonix…`): after
  `copy --name`, a different `send-text`, then `paste --name` with no
  `--text`, `wait --text-equals` sees independent GetText equal the
  copied source (paste write still uses the WebKit eval-helper set-value
  path).
- [x] `copy --window HANDLE` without `--name` publishes GetText from the
  showing focused node (innermost Text candidate) onto native CLIPBOARD
  (`via=gettext`). Never XTest / `--coords` when `--window` is set. Proof
  is independent seed → focused `copy` → clear → focused `paste` →
  `get-text --window HANDLE` (no `--name`) equal to the seeded string.
  Live hosts: agenterm-con named `Command` after `focus --name`
  (`via=gettext` on copy; paste restore `via=editable-text` on a second
  con; never steal the resident control socket); Chrome `GetTextField`
  after `focus --name` on the host AT-SPI bus (`AT_SPI_BUS` /
  `AT_SPI_BUS_ADDRESS`); Reasonix composer `Message Reasonix…` after
  `focus --name` under `scripts/reasonix-desktop-a11y.sh` (`via=gettext`;
  paste restore uses eval-helper set-value, `via=text`). Without
  `--window` copy is invalid.
- [x] `paste --window HANDLE --name PAT [--role ROLE] [--text TEXT]`
  resolves through that same path, then writes the clipboard via the same
  AT-SPI `EditableText` / `Text` + toolkit set-value path as named
  `send-text` (`agt_a11y_node_set_text`). `--text` only seeds the clipboard
  (`agt_clipboard_set_text`); the field write always reads
  `agt_clipboard_get_text`. On Linux X11 the seed is a native CLIPBOARD
  selection owner, not `xclip`. A named showing node with no writeable
  text interface typed-fails (`a11y_text_unavailable`) and never silently
  uses XTest / `--coords` / screenshot. Resolution failure (miss or
  ambiguous name) aborts before any write or clipboard seed. Reasonix
  composer (`Message Reasonix…`) writes through the same WebKit
  eval-helper set-value path as named `send-text`. A prior `copy --name`
  may seed the clipboard instead of `--text`.
- [x] `paste --window HANDLE` without `--name` writes that same clipboard
  path on the showing focused node (innermost Text candidate). Never
  XTest / `--coords` when `--window` is set. Proof is independent
  `get-text --window HANDLE` (no `--name`) equal to the clipboard string.
  Live hosts: agenterm-con named `Command` (native `EditableText`,
  `via=editable-text` on a second con; never steal the resident control
  socket); Chrome `GetTextField` after `focus --name` on the host AT-SPI
  bus (`AT_SPI_BUS` / `AT_SPI_BUS_ADDRESS`); Reasonix composer
  `Message Reasonix…` after `focus --name` under
  `scripts/reasonix-desktop-a11y.sh` (eval-helper set-value, `via=text`).
  Without `--window` paste is invalid.
- [x] `send-keys --window HANDLE --name PAT [--role ROLE] [--] <keys...>`
  resolves through that same path, then delivers the chord via AT-SPI
  `DeviceEventListener` (`NotifyEvent`, `agt_a11y_node_send_keys`). A named
  showing node with no Device/key interface typed-fails
  (`a11y_key_unavailable`) and never silently uses XTest /
  `input_inject::send_keys`. Resolution failure (miss or ambiguous name)
  aborts before any keystroke. After a successful named `send-keys`,
  the same window's AT-SPI tree must still be there for a second named
  command (one process-wide a11y-bus connection; do not drop the bus).
- [x] `send-keys --window HANDLE` without `--name` targets the showing
  focused node (innermost Text candidate). Prefers
  `DeviceEventListener.NotifyEvent` (`via=device-event`). When that
  interface is absent (con `Command`; Chrome renderer entry; WebKitGTK
  textarea) and the payload is plain typeable text, writes through
  AT-SPI `EditableText` / `Text` + toolkit set-value (same path as
  focused `send-text`). Never XTest / `input_inject::send_keys` when
  `--window` is set. Independent `get-text --window HANDLE` (no
  `--name`) must equal the typed string. Live hosts: agenterm-con named
  `Command` (native `EditableText`, `via=editable-text` on a second con;
  never steal the resident control socket), Chrome `GetTextField` after
  `focus --name` on the host AT-SPI bus (`AT_SPI_BUS` /
  `AT_SPI_BUS_ADDRESS`, `via=text`); Reasonix composer
  `Message Reasonix…` after `focus --name` under
  `scripts/reasonix-desktop-a11y.sh` (`via=text`). Special chords
  without a key interface still typed-fail. Do not mark this leaf
  shipped on worker JSON.
- [x] `wait --window HANDLE --name PAT [--role ROLE] --text-equals TEXT`
  (alias `--node-text-equals`) polls `agt_a11y_node_get_text` (`Text.GetText`)
  on that unique showing node until the independent text equals `TEXT`.
  Timeout is typed `timeout`. This is not `send-text` / `paste` / `copy`
  `matched.text`, not a sidecar walk of `agenterm-cu tree` snapshot `text` fields,
  and not the WebKit eval helper's queued-job `OK` (Reasonix composer
  `Message Reasonix…`).
  Never screenshot, XTest, or `--coords`.
- [x] `wait --window HANDLE --name PAT [--role ROLE] --text-contains SUB`
  (alias `--node-text-contains`) polls that same `agt_a11y_node_get_text`
  until the independent GetText contains `SUB`. Success reports
  `via=gettext` and the full GetText. Timeout is typed `timeout` and
  reports the last GetText. `send-text` / `paste` / `copy` `matched.text`
  do not count. Never screenshot, XTest, or `--coords`.
- [x] `scroll --window HANDLE --name PAT [--role ROLE]` resolves through
  that same path, then one-shot AT-SPI `Component.ScrollTo(TopEdge)`
  (`agt_a11y_node_scroll`). Success is `via=scroll-to`. Missing / false /
  `UnknownMethod` typed-fails (`a11y_scroll_unavailable`). Never Action
  `scroll*`, XTest wheel, `--coords`, or screenshot. Geometric proof is
  independent `get-extents`, not `matched.extents`. WebKitGTK
  `Component.GetExtents(Screen)` is already the independent observe
  sibling; `ScrollTo` is a no-op true, so Reasonix launched via
  `scripts/reasonix-desktop-a11y.sh` applies `scrollIntoView` through
  the same eval helper as named set-value (hello `A11YSCROLL1`, no ABI
  change). Linux `agenterm-con` publishes a real `ScrollTo` that moves
  named `OffscreenField` (Session child); same verbs, no ABI change.
- [x] `get-extents --window HANDLE --name PAT [--role ROLE]` resolves
  through that same path, then independent AT-SPI
  `Component.GetExtents(Screen)` (`agt_a11y_node_get_extents`). Snapshot
  `node.bounds` do not count. Empty extents typed-fail
  (`a11y_extents_unavailable`).
- [x] `select --window HANDLE --name PAT --start N --end M [--role ROLE]`
  resolves through that same path, then one-shot AT-SPI
  `Text.SetSelection(0, start, end)` (`agt_a11y_node_set_selection`).
  Success is `via=set-selection`. Missing Text / `UnknownMethod`
  typed-fails (`a11y_selection_unavailable`). SetSelection false
  typed-fails (`a11y_selection_no_effect`). Never XTest, mouse-drag,
  `--coords`, or screenshot. Proof is independent `get-selection`, not
  the `select` reply.
- [x] `get-selection --window HANDLE --name PAT [--role ROLE]` resolves
  through that same path, then independent AT-SPI `Text.GetNSelections`
  + `GetSelection(0)` (`agt_a11y_node_get_selection`). The `select`
  reply payload does not count. Missing Text typed-fails
  (`a11y_selection_unavailable`). `n == 0` is empty success.
  Reasonix composer (`Message Reasonix…` under
  `scripts/reasonix-desktop-a11y.sh`) uses that same native Text
  path; WebKit 2.52 already implements SetSelection/GetSelection
  (no `A11YSELECT1` eval helper — unlike ScrollTo). Linux
  `agenterm-con` composer `Command` publishes real
  `SetSelection` / `GetNSelections` / `GetSelection` (same verbs,
  no ABI change).
- [x] `set-caret --window HANDLE --name PAT --offset N [--role ROLE]`
  resolves through that same path, then one-shot AT-SPI
  `Text.SetCaretOffset` (`agt_a11y_node_set_caret_offset`).
  Success is `via=set-caret-offset`. Missing Text / `UnknownMethod`
  typed-fails (`a11y_caret_unavailable`). SetCaretOffset false
  typed-fails (`a11y_caret_no_effect`). Never XTest, `--coords`, or
  screenshot. Proof is independent `get-caret`, not the `set-caret`
  reply. Live Chrome fixture field `CaretField`
  (`fixtures/cu/310-chrome-caret.html`) uses that same native Text
  path (no ABI / eval-helper change). Reasonix composer
  (`Message Reasonix…` under `scripts/reasonix-desktop-a11y.sh`)
  uses that same native path; WebKit 2.52 already implements
  SetCaretOffset / CaretOffset (no `A11YCARET1` eval helper).
- [x] `get-caret --window HANDLE --name PAT [--role ROLE]` resolves
  through that same path, then independent AT-SPI `Text.CaretOffset`
  / `GetCaretOffset` (`agt_a11y_node_get_caret_offset`). The
  `set-caret` reply payload does not count. Missing Text typed-fails
  (`a11y_caret_unavailable`). Chrome `CaretField` unfocused
  `CaretOffset` may be `-1`; after `set-caret --offset N` independent
  readback equals `N`. Reasonix composer after `send-text HELLO`
  reports `CaretOffset=5`; after `set-caret --offset 2` independent
  `get-caret` is `2`. Linux `agenterm-con` composer `Command`
  publishes real `SetCaretOffset` / `CaretOffset` (ABI 1.9 verbs).
- [x] `get-text --window HANDLE --name PAT [--role ROLE]` resolves
  through that same path, then one-shot independent AT-SPI
  `Text.GetText` (`agt_a11y_node_get_text`) — the same authority
  `wait --text-equals` polls, without a timeout. `send-text` /
  `paste` / `copy` `matched.text` and tree snapshot `text` do not
  count. Missing Text typed-fails (`a11y_text_unavailable`). Never
  XTest / `--coords` / screenshot. Live Chrome fixture field
  `GetTextField` (`fixtures/cu/311b-chrome-gettext.html`) and
  Reasonix composer (`Message Reasonix…` under
  `scripts/reasonix-desktop-a11y.sh`) both use that same native Text
  path; WebKit 2.52 already implements GetText on the composer
  `<textarea>` (no `A11YGETTEXT1` eval helper). Linux `agenterm-con`
  composer `Command` publishes real `Text.GetText` (same verb, no
  ABI change).
- [~] `get-text --window HANDLE` without `--name` uses that same
  GetText authority on the showing focused node (innermost Text
  candidate). Linux connect prefers `AT_SPI_BUS_ADDRESS` then
  `AT_SPI_BUS`, strips a `GetAddress` `,guid=` suffix, and only then
  asks `org.a11y.Bus`. `scripts/box-chrome-a11y.sh` writes the
  standard `$XDG_RUNTIME_DIR/at-spi/bus` file after box-chrome's
  XDG rewrite so the renderer joins that same host socket. A
  one-node synthetic X11 `frame` is not a Chrome document tree.
- [~] `windows` / `screenshot` / coordinate-degraded input on `current` still
  use `agenterm-platform` until `agt_window_enumerate` / unified screenshot /
  `agt_input_inject` milestones ship; capability JSON documents the gap.
- [ ] AT-SPI unavailable at runtime (no session bus, registry absent) → typed
  `Unsupported` / `Failed`; no silent fallback to XTest coordinates.
- [ ] black-box evidence: `scripts/cu-linux-smoke.sh` against the real `agenterm-cu`
  binary on a host with `DISPLAY` and `at-spi2-registryd`.

**Windows — native API + UIA**

- [~] Windows `current` now reaches the UIA accessibility facade through the
  runtime `agenterm.dll` boundary: `agenterm-cu` `Command`/`Executor` owns
  target resolution and product meaning, while the ABI and
  `agenterm-platform` own UIA tree, Value, Invoke and Focus mechanisms. The
  owning evidence is five pure tests plus two real Win32 UIA fixture tests.
  The staged public `cu-windows-smoke` also passes all seven declared evidence
  checks through the colocated `agenterm-cu.exe` + `agenterm.dll`; Candidate
  qualification and release remain open.
- [x] `tree` uses the UIA Control View and returns bounded node identity,
  parent relationships, role, name, text, state, bounds and actions. Node IDs
  encode UIA RuntimeId paths, but every Value, Invoke, Focus or key operation
  resolves that path again from the requested HWND (or the bounded desktop
  root for `None`). A RuntimeId is never treated as a retained COM object, and
  no COM interface pointer is cached across calls or apartments.
- [x] Each UIA operation initializes an MTA-capable COM session with owned RAII
  for interfaces, BSTR, SAFEARRAY and VARIANT values, configures
  `SetAutoSetFocus(FALSE)`, a 500 ms connection timeout and a 250 ms transaction
  timeout, and also enforces 5 s snapshot / 2 s action wall-clock budgets plus
  strict node, depth and string limits. Window loss, access denial, timeout and
  recycled nodes are typed failures.
- [x] Structured Focus calls UIA `SetFocus`; text writes use Value and reads use
  Value/Text patterns; click prefers Invoke, SelectionItem, Toggle and the
  legacy default action. Missing patterns fail typed. No UIA node operation
  silently degrades to coordinates; node key delivery is explicitly reported
  as `uia-focus+send-input` after UIA focus.
- [~] the desired-state, option and range half of `invoke` on Windows
  (cut 3.53), mapped but not journey-proven: `set-expanded` reads the
  ExpandCollapse pattern, acts only on a difference and reads back (a node
  reporting `LeafNode` has nothing to expand and says so);
  `select-option` expands a collapsed combo box, selects the descendant
  named exactly the option through SelectionItem, restores the control's
  own expansion state and refuses a duplicate name as
  `a11y_option_ambiguous`; `increment` / `decrement` step the RangeValue
  pattern by the control's own `CurrentSmallChange`, clamped to the
  published range and read back. The node snapshot now also carries
  `expanded` / `collapsed`, so the UIA vocabulary matches AX and AT-SPI.
- [~] `scroll` and `get-extents` on Windows (cut 3.53), mapped but not
  journey-proven: `scroll` is the ScrollItem pattern's `ScrollIntoView`
  (UIA's spelling of AT-SPI `Component.ScrollTo`; a node whose container
  does not scroll exposes no such pattern and is refused typed) and
  `get-extents` re-reads the live element's `BoundingRectangle`,
  answering `a11y_extents_unavailable` for an empty rect rather than
  passing zeros off as geometry. Both were declared unavailable through
  UIA before this cut, which was wrong: the patterns were there.
- [~] `focused --window HANDLE` on Windows (cut 3.53), mapped but not
  journey-proven: the deepest node reporting `HasKeyboardFocus` in a
  bounded walk of the window's own tree.
  `IUIAutomation::GetFocusedElement` is deliberately not used -- it
  answers with the *desktop's* focus, so it would report another
  application's control as this window's. `capabilities` declares
  `focused.mode = "state-search"`.
- [~] `select` / `get-selection` / `set-caret` / `get-caret` on Windows
  (cut 3.53), mapped but not journey-proven: the Text pattern's
  `GetSelection` gives the control's own range, and an offset is measured
  by cloning the document range and moving its end onto the endpoint --
  UIA ranges are opaque, so an offset is the length of the text before it,
  in UTF-16 code units. A write builds a degenerate range at `start`,
  extends it by the length and calls `Select()`, then reads the selection
  back and refuses on a mismatch. A control whose `SupportedTextSelection`
  is `None` is refused typed, and a control that cannot move the endpoint
  as far as asked says how far it got -- never a silently shorter
  selection, never a mouse drag or shift-arrow keystrokes. A degenerate
  selection is reported as the AT-SPI "no selection" shape (`n == 0`) with
  the position still readable through `get-caret`, matching AX.
- [ ] macOS `screenshot` is not wired and this is a platform decision, not
  an omission: `CGWindowListCreateImage` was **obsoleted in macOS 15.0 and
  removed from the SDK** (measured on this toolchain -- the compiler
  refuses it outright). Its replacement, ScreenCaptureKit, is a
  block-based async API behind the Screen Recording TCC grant, which is a
  different permission from the Accessibility one the rest of the adapter
  holds. The refusal now names that constraint instead of saying
  "unavailable", and nothing degrades to a full-screen grab: a screenshot
  of the wrong thing is worse than a typed refusal.
- [x] every UIA pattern this adapter calls through a hand-written vtable
  has its slot offsets pinned by a test against the SDK's IDL order
  (Invoke, SelectionItem, Toggle, Legacy, Value, Text, TextRange, and the
  ExpandCollapse / RangeValue / ScrollItem patterns added in cut 3.53). A
  wrong slot is a call into the wrong method on a machine this repository
  cannot run, so the layout is pinned rather than trusted.
- [x] Win32 window enumeration uses the runtime library's two-stage
  required-size/fill contract. Desktop churn can increase `required` after the
  caller allocated `capacity`; `required > capacity` triggers a bounded retry
  with a fresh capacity instead of truncation, out-of-bounds writes, false
  success or an unbounded loop. Exhaustion is typed failure.
- [~] Screenshot and coordinate/input injection remain separate platform
  contracts consumed through the runtime library; they do not replace UIA
  structured success.

**macOS — AX (NSAccessibility)**

- [x] AX-backed `tree` / `query` on `current` through `agenterm-platform`
  (`adapters/macos/accessibility_tree.rs`). The adapter resolves a
  `CGWindowID` handle to an application AX element, walks `AXChildren`
  breadth-first under the caller's depth / node budget (defaults 32 / 1000,
  ceilings 64 / 20000) plus string / sibling / wall-clock bounds, and returns
  `backend: "ax"` with path ids, role, name, states, bounds, text,
  `AXIdentifier`, and the element's `AXUIElementCopyActionNames`
  (normalized: `click`, `focus`, `show-menu`, `scroll-to-visible`, ...).
  Reaching the budget is `truncated: true` with the nodes read so far;
  `kAXErrorFailure` on an optional attribute is an absent value, not a walk
  failure. Missing Accessibility permission fails typed
  (`a11y_permission_denied` → cu `denied` + repair path); timeout and the
  adapter's own limits fail typed. Live evidence: `cu-macos-smoke` STEPs
  "query by role, identifier and exact text", "tree --max-nodes 5 and
  --depth 0 report truncated", "full tree reports truncated false and AX
  action names" against the owned fixture (2026-08-30). No screenshot /
  `--coords` / CGEvent fallback and no silent AT-SPI or UIA reuse.
- [x] AX semantic actuation on `current` (cut 3.50): `invoke` resolves the
  child-index path at call time (`a11y_node_not_found` when stale), checks
  the element's `AXActionNames` before performing (`AXPress`, `AXIncrement`,
  `AXDecrement`; a missing action is typed `Unsupported`), writes `AXValue`
  only when `AXUIElementIsAttributeSettable` says so and reads it back
  (`a11y_action_no_effect` on mismatch; a numeric value is compared as a
  number), chooses a pop-up option by opening the pop-up with `AXPress` and
  pressing the unique `AXMenuItem` titled exactly the option (closing the
  menu again with `AXCancel` when none matches), and treats `set-checked` /
  `set-expanded` as desired states (read `AXValue` 0/1/2 or `AXExpanded`,
  press only when different, read back). `click` maps to `AXPress` and
  `focus` sets `AXFocused` on the element (first responder inside the app;
  no activation) — both compiled, neither journey-proven. `get-text` /
  `send-text --name` use the same `AXValue` read / write. Live evidence:
  `cu-macos-smoke` STEPs "invoke set-value ...", "invoke set-checked true
  twice ...", "invoke press advances the count label ...", "invoke
  increment / decrement ... select-option ...", "ambiguous --name, missing
  action, unobservable state, missing target and observe-only grant are
  typed refusals" (2026-08-30). Never `AXRaise`, never a CGEvent, never a
  screenshot or `--coords` fallback.
- [x] AX background verbs on `current` (cut 3.51): `menu inspect` walks the
  application's `AXMenuBar` (`AXMenuBarItem` → `AXMenu` → `AXMenuItem`)
  under a menu-level / node budget — AppKit publishes a closed menu's items
  through AX, so nothing opens on screen and the app is never activated —
  reporting `enabled` / `disabled` and `checked` (`AXMenuItemMarkChar`) per
  item; `menu invoke` resolves the title path segment by segment
  (`AXTitle`, exact), refuses a missing / duplicate / disabled segment and
  a non-leaf item before `AXPress`, presses the leaf and re-resolves it to
  read the mark back. `focused` reads the application's
  `AXFocusedUIElement`, walks `AXParent` up to the window and locates each
  hop in its parent's `AXChildren` (`CFEqual`) so the node carries its
  window-tree path id; a focused element outside the window is typed
  (`a11y_focus_outside_window`). `observe` is cu-side poll-diff over
  `tree_for_window_bounded`; no `AXObserver` is registered. Live evidence:
  `cu-macos-smoke` STEPs "menu inspect ...", "menu invoke ...", "focus
  moves the first responder ...", "observe --duration 1.5 ..." (2026-08-30).
  Never `AXRaise`, never a CGEvent, never activation.
- [x] AX node text and geometry on `current` (cut 3.53): the verbs Linux
  AT-SPI2 and Windows UIA already carried, which macOS answered as a
  `PLACEHOLDER cut` refusal until this cut. `get-extents` re-reads the live
  element's `AXPosition` + `AXSize` (independent of the snapshot's
  `bounds`; an element with no rect is `a11y_extents_unavailable`, never a
  zero rect); `select` / `get-selection` and `set-caret` / `get-caret` read
  and write `AXSelectedTextRange` as a `CFRange`, requiring
  `AXUIElementIsAttributeSettable` before every write and reading the range
  back after it (`a11y_selection_no_effect` / `a11y_caret_no_effect` on
  mismatch, `a11y_selection_unavailable` / `a11y_caret_unavailable` on a
  node that publishes no selected range). AX carries at most one range and
  spells "nothing selected" as a zero-length range at the insertion point,
  so `get-selection` reports the AT-SPI shape (`n == 0`, endpoints zero)
  and the position stays readable through `get-caret` — one vocabulary on
  all three backends. `scroll` maps to `AXScrollToVisible`; a node that
  does not offer it is `a11y_scroll_unavailable` naming the actions it does
  offer. Live evidence: `cu-macos-smoke` STEP "get-extents, select /
  get-selection and set-caret / get-caret read and write the text area
  through AX; scroll is a typed refusal that names the actions the node
  does offer" (2026-08-31). Never a mouse drag, never shift-arrow
  keystrokes, never a `--coords` or screenshot fallback.
- [ ] macOS `scroll` has no positive journey. AppKit publishes
  `AXScrollToVisible` on nothing the owned fixture can hold (measured on
  `NSButton`, a plain `NSView` overriding both the modern and the legacy
  action API, and `NSTableView` rows); Chromium and WebKit web content do
  publish it (measured: 130 nodes of one Brave window). Positive evidence
  therefore needs a web target the journey owns, which it does not have
  yet.
- [x] AX `send-keys` to a node on `current` (cut 3.53) is **semantic, not
  synthetic**. macOS has no way to hand a keystroke to an application that
  is not the active one; this was measured rather than assumed -- an
  accessory app whose window is ordered front reports `keyWindow = no`, and
  key events posted to its pid with `CGEventPostToPid` never reach its
  `sendEvent:`. Activating the app would break the background invariant and
  a global `CGEventPost` would land the chord in whatever the user is
  typing in, so the adapter maps the chords that have an AX action
  equivalent -- `enter` / `return` to `AXConfirm`, `esc` / `escape` to
  `AXCancel` -- and refuses every other chord typed
  (`a11y_key_unavailable`, naming the constraint). A modifier rules a chord
  out: `cmd+enter` is a different command, not a confirm. The reply says
  `via: "ax-action"`, never `device-event`, so it cannot read as "a key was
  delivered". Live evidence: `cu-macos-smoke` STEP "send-keys enter
  performs the AX action the chord means and its postcondition advances"
  (2026-08-31) -- the fixture's `NSTextField` action fires and its label
  advances, with the frontmost window and the real pointer unchanged.
- [x] each `windows` row names the managed Spaces it sits on (cut 3.58,
  macOS SkyLight `SLSCopySpacesForWindows`, read-only). The Space
  *inventory* says which Spaces exist; this says where a given window
  lives among them, which is the attribution an agent needs: a window on
  another Space is present but not on screen, and that is neither
  minimized nor closed. An absent `spaces` field means the host has no
  such notion or no SPI for it -- never a default. No ABI change: the
  SkyLight reader already lives in `agenterm-cu`. Live evidence:
  `cu-macos-smoke` asserts every window's Space ids are ids the `spaces`
  inventory lists, which is the cross-check that catches an attribution
  drifting away from the inventory.
- [x] `clipboard-read` reports what the clipboard is *carrying*, not only
  what it can read as text (cut 3.58, ABI 1.19 `agt_clipboard_types`).
  The verb still reads Unicode text and nothing else, but the reply now
  carries `types`: the host's own spelling of every representation on the
  clipboard -- macOS class names, X11 TARGETS atoms, Windows clipboard
  format names -- with no normalization, so a caller matching on one is
  matching on what the platform said. `types_available: false` means the
  host cannot enumerate them, which is a different fact from an empty
  list.
  This is the difference between "the clipboard is empty" and "the
  clipboard holds something I cannot read". Measured on this desktop with
  a PNG on the clipboard: `text: ""`, `bytes: 0`, and `types`
  `["«class PNGf»", "«class AVIF»", "«class 8BPS»", "GIF picture", ...]`.
  Without the list an agent reads an empty string and concludes the copy
  failed.
  No journey step: a hermetic one would have to write to the user's real
  clipboard, which the test lane should not do for a read-shape assertion.
  macOS reads the list from AppleScript's `clipboard info` (the one route
  that does not require linking AppKit into this adapter); Linux reuses
  the TARGETS probe the text check already runs; Windows enumerates
  clipboard formats in the order the system offers them, which is a
  preference ranking.
- [~] `close`, `orderwin` and topmost on Linux (cut 3.57), mapped but not
  journey-proven: `close` sends the EWMH `_NET_CLOSE_WINDOW` client
  message to the root window, which asks the window manager to close the
  window the way its own close button would -- so the application still
  runs its shutdown path and can show a "save your work?" dialog. It is a
  request, not a kill, which is exactly why cu's destructive gate reads
  the handle back afterwards instead of trusting the call. `orderwin`
  raises with `ConfigureWindow(stack_mode = Above)`, the X11 primitive
  that brings a window forward **without touching keyboard focus**, and
  topmost adds or removes `_NET_WM_STATE_ABOVE`. The other show states
  (iconify, maximize, restore) stay typed `unsupported`: they are
  window-manager policy rather than a stacking operation, and guessing at
  `_NET_WM_STATE` transitions a given WM may ignore would report success
  for nothing. `capabilities` now declares `close` from the window-op
  capability on every host instead of hard-coding a Linux refusal.
- [~] background menus on Linux and Windows (cut 3.57), mapped but not
  journey-proven: both find the `menu bar` node in the window's own
  bounded tree -- AT-SPI publishes a frame's menu bar and UIA publishes a
  `MenuBar` element (including the classic Win32 menus the MSAA bridge
  exposes), so reading one is a walk and a search, with no menu opened on
  screen and no activation. `menu invoke` resolves every segment of the
  title path before pressing anything, refusing a missing, duplicated or
  disabled segment and a non-leaf item with nothing performed, then
  presses the item's own action -- never a click at its coordinates. A
  menu-bar item that owns a `menu` child holding the items is matched one
  level down, because that is how both toolkits nest them. The check mark
  comes back as the state word the tree already speaks (`checked` /
  `unchecked` / `mixed`) rather than as a macOS mark character.
  `capabilities` declares `mode: "tree-search"` on these two: a toolkit
  that populates a closed menu lazily publishes nothing to find, which is
  a weaker claim than macOS asking the application for its menu bar, and
  the declaration must not hide that.
- [x] `observe --mode notifications` on `current` (cut 3.56, ABI 1.18
  `agt_a11y_observe_window`): the events come from the application's own
  `AXObserver` subscription rather than from the difference between two
  tree walks. The observer is registered on the application element, its
  run-loop source runs on the calling thread in short slices until the
  duration ends or `max_events` arrive, and it is removed before the call
  returns -- nothing outlives it.
  **It does not replace poll-diff, because neither mode subsumes the
  other.** Polling compares two walks, so every event carries `before` and
  `after`, but a change that reverts between walks is invisible and an idle
  interface still costs a walk per interval. Notifications carry the order
  and arrival time of every change, including ones that revert, but say
  "this changed", not what it changed from. Defaulting to notifications
  would silently drop `before` / `after` from every reply, so `poll-diff`
  stays the default, `--mode notifications` is the explicit ask, the reply
  always says which mode ran, and the notification reply carries no `polls`
  or `interval_ms` because there were none. `capabilities` declares
  `observe.default_mode` and a status per mode; Linux and Windows report
  `notifications: unsupported` (their backends have event mechanisms; this
  cut does not wire them).
  Live evidence: `cu-macos-smoke` STEP "observe --mode notifications takes
  the events from the backend itself ..." (2026-09-01) -- a value is
  written and written back while the observer runs, and both writes are
  seen, which a poll-diff between the same two instants could not have
  reported at all.
- [x] `windows` reports the desktop's front-to-back order and how much of
  each window is covered (cut 3.55, ABI 1.17
  `agt_window_stacking_list`): `z_index` 0 is frontmost and
  `occluded_percent` is computed by subtracting the rectangles of the
  windows in front, exactly -- no screen sampling, no screenshot, no grid
  approximation. Both numbers describe one observation instant. A host
  that cannot report a real stacking order answers typed `unsupported`
  and the rows carry neither field, because an absent number is honest
  while `z_index: 0` would claim "frontmost". Linux deliberately refuses
  the `_NET_CLIENT_LIST` fallback its enumeration accepts: that list is in
  creation order, which is indistinguishable from stacking order right up
  to the moment it is wrong. The rectangle arithmetic lives in the
  platform contract with its own tests (overlapping covers counted once, a
  single visible pixel never rounding up to "fully hidden", an empty
  window not dividing by zero), so the hard part is verified on any host.
  Live evidence: `cu-macos-smoke` STEP "the inventory reports a
  front-to-back z-order and how much of each window is covered ..."
  (2026-09-01) -- both owned windows start visible with distinct indices,
  the whole desktop ranks densely from 0, and framing the front window
  onto the back one takes the back one to 100.
- [x] the last three MCU `invoke` spellings on `current` (cut 3.54, ABI
  1.16): `set-selected` is a desired state over macOS `AXSelected` (read,
  act only on a difference, read back; already selected is success with
  nothing performed), `cancel` is `AXCancel` and `show-default-ui` is
  `AXShowDefaultUI`. They were parseable-but-typed before, which is the
  honest state for a spelling with no mechanism -- but the mechanisms were
  there.
  Making them verifiable needed the state vocabulary finished first: macOS
  published `selected` only when true and Linux only when set, so a caller
  could not tell "not selected" from "no selection state here". Both now
  name the negative (`unselected`, from `AXSelected` false and from the
  AT-SPI `Selectable` state), matching what Windows already reported, and
  `invoke set-selected` verifies through `selected-readback`. A node with
  no selection state of its own is refused before the mechanism is touched
  (`state_unobservable`), the same guard `set-checked` uses.
  Live evidence: `cu-macos-smoke` STEP "invoke set-selected selects a
  table row and repeats as a verified no-op; cancel and show-default-ui
  are typed refusals that name the actions the node does offer"
  (2026-09-01) -- one of three rows ends selected, the other two report
  `unselected`, and the second call performs nothing while still
  verifying.
- [x] macOS input injection (cut 3.53), the one path here that is global
  by design. Two measurements decided its shape. **Keys cannot reach an
  application that is not active**: an accessory app whose window is
  ordered front reports `keyWindow = no`, and key events posted to its pid
  never arrive at its `sendEvent:`. **Mouse events posted to a pid do
  arrive but carry no window**: the same probe sees `LeftMouseDown` /
  `LeftMouseUp` with the real pointer unmoved, and its button never fires,
  because AppKit has no window to route through (setting
  `kCGMouseEventWindowUnderMousePointer` does not change that). There is
  therefore **no window-local pointer route on macOS**, so
  `pointer-move --to <handle>` is refused typed rather than approximated
  by a global move that would aim somewhere else. What is wired is the HID
  tap: `pointer_move`, `pointer_click` (with the click-state field, so a
  double click reads as one), `type_text` (Unicode attached to the key
  event, so it does not depend on the user's layout) and `send_keys`
  (ANSI physical key positions; a character with no physical key is
  refused, never guessed). None of the semantic verbs reach this code:
  `click --node`, `invoke`, `focus` and the rest stay on the accessibility
  tree and never move the cursor. `capabilities` separates the two --
  `pointer-position` is `available` with `mode: "read-only"` on every
  host, and `pointer-move` carries `scope: "desktop"` so a caller cannot
  read a window-local promise into it. Live evidence:
  `cu-macos-pointer-smoke` (2026-08-31), 4 STEP / 4 EVIDENCE -- the cursor
  is moved, read back independently, restored to exactly where the user
  left it, and that restore is read back too; three typed refusals
  (window scope, missing `--to`, observe-only grant) each leave it
  untouched. Nothing is clicked or typed in the journey: a global click
  lands in whatever is frontmost, which a hermetic journey does not own.
- [x] web content on `current` through the system AX tree (cut 3.53), with
  its own owned page rather than the user's browser:
  `examples/objc/agenterm_web_fixture.m` is an accessory-policy window
  holding a `WKWebView` with a hermetic `loadHTMLString` document (no
  network, no profile). The ordinary bounded walk reaches the `AXWebArea`
  and the page's own heading, button, field and link; `invoke press` acts
  on a web button and its postcondition is read on another node; a web
  text field takes an `AXValue` write only once focused, and the unfocused
  attempt fails closed (`a11y_action_no_effect`, `verified: false`)
  instead of reporting a write that did not land. Live evidence:
  `cu-macos-web-smoke` (2026-08-31), 6 STEP / 6 EVIDENCE.
- [x] `unlock` performs the real `AXManualAccessibility` poke (ABI 1.15
  `agt_a11y_manual_accessibility_poke`) instead of only re-reading and
  classifying. A browser engine leaves its web tree unbuilt until an
  assistive client asks, so "empty chrome is not an empty page" describes
  a tree that has not been built. **The call's own status is not the
  outcome**: AppKit reports `kAXErrorAttributeUnsupported` for this
  attribute even when the poke lands (measured on the fixture: three nodes
  before, fourteen after, the same AXError both times), so the reply
  separates `poked` (the request was delivered) from `grew` and
  `returned_before` (what two reads actually found). Second measurement:
  once any assistive client has enabled accessibility in a session, WebKit
  publishes eagerly for processes started afterwards, so `grew: false` on
  an already-built tree is a correct report, not a failed poke. Linux and
  Windows answer typed `unsupported` -- neither backend gates a browser's
  tree behind a per-application attribute.
- [x] `scroll` has positive macOS evidence at last, and only a web target
  could give it: AppKit publishes `AXScrollToVisible` on nothing an owned
  Cocoa fixture can hold, while every WebKit node offers it. The journey
  scrolls a link sitting 1200 px below the fold and reads the movement
  back with an independent `get-extents` (y 1955 -> 905); the `scroll`
  reply itself is not the proof.

### Degraded fallbacks (never silent)

- [ ] `screenshot` may exist as an observation command but does not replace
  `tree`. When native window capture is unavailable, the command returns typed
  `unsupported`.
- [ ] coordinate `click` requires an explicit degraded marker in the command
  and reply (`addressing: degraded-coordinates`). It is audited separately from
  AT-SPI / UIA / AX actuation.

## Process and session model

- [ ] a session's ownership, lifetime and teardown are explicit. Closing a
  session releases its native resources and its target reference within a
  bounded deadline, and reports incomplete teardown as a typed error rather than
  pretending success.
- [ ] one session's failure, flood or resource exhaustion cannot corrupt another
  session or abort the host.
- [ ] if a backend requires a helper process, its lifecycle, identity and
  failure semantics are owned here and it is never an undeclared background
  authority. Binary-role registration belongs to
  [02 Executable family](PRD_02_02_executable_family.md).

## Reference assets

- [ ] existing reference implementations (the sibling monorepo
  `skills/computer-use/` Windows UIA/CDP and RDP work, the macOS AX/CGEvent
  helper split, the Linux AT-SPI2 bridge) are **design input only**. They inform
  the command set and backend shape; no code, runtime or dependency from them
  enters the product graph. Source review, licensing and independent
  implementation are governed by
  [14 Research provenance](PRD_02_14_research_provenance.md).

## Evidence

- [ ] each tier is proven by a public black-box journey against a real target of
  that tier. A tier proven only in simulation is not claimed.
- [~] Linux `vnc` first cut: host `agenterm-cu --vnc 127.0.0.1:<port>` against
  a gate-owned loopback `x11vnc` (RFB security type None / `-nopw`; not the
  resident `:2` listener alone) handshakes RFB then runs a local
  `agenterm-cu --target current` session worker. Get-selection observe path:
  second `agenterm-con` `Command` field holds a known ASCII seed and a known
  non-empty selection `START..END` (gate precondition via already-landed
  `send-text` + `select`; not this cut's verb; unique title; never steal
  `unix:/tmp/run-box/agenterm-con.sock`); host independent
  `get-selection --window HANDLE --name Command` returns that range
  (`via=get-selection`; native AT-SPI `GetNSelections` + `GetSelection(0)`;
  `n == 1` and integer `start` / `end` equal the precondition range so
  `seed[start:end] == expected`). Never screenshot / `--coords` /
  mouse-drag / RFB framebuffer OCR / cached setter reply. Missing Text
  typed-fails `a11y_selection_unavailable` on the session worker the same
  as local `current`. `get-extents` / `get-caret` / `tree` / `focus` /
  `scroll` / `click` / `set-caret` / `select` / `send-keys` / `copy` /
  `paste --text` / `send-text` over vnc and observe-only `windows` /
  `get-text` / `wait --text-equals` still hold. Worker JSON does not
  count; CEO owns the official gate. Connect / protocol / auth failures
  are typed (`vnc_unavailable` / `vnc_transport_failed` / `vnc_auth_failed` /
  `invalid_input`).
- [~] Linux `ssh` first cut: host `agenterm-cu --ssh` against loopback OpenSSH
  runs remote `agenterm-cu --target current`. Get-selection observe path: host
  `send-text --window HANDLE --name Command -- SEED` (payload after `--`; not
  `--text`) plants a seed on a second `agenterm-con` `Command` field; host
  `select --window HANDLE --name Command --start N --end M` runs remote
  AT-SPI `Text.SetSelection`; host independent
  `get-selection --window HANDLE --name Command` returns that range
  (`via=get-selection`; start/end equal the selected slice of the seed, or
  the seed when the range is the whole field). Native AT-SPI
  `GetNSelections` + `GetSelection`. Never screenshot / `--coords` /
  mouse-drag / XTest. Missing Text typed-fails `a11y_selection_unavailable`
  on the remote worker the same as local `current`. `get-extents` /
  `get-caret` / `tree` / `focus` / `scroll` / `click` / `set-caret` /
  `select` / `send-keys` / `copy` / `paste --text` / `send-text` over ssh
  and observe-only `wait` / `get-text` still hold. Worker JSON does not
  count; CEO owns the official gate. Auth failure and missing destination
  are typed (`ssh_unavailable` / `ssh_transport_failed` / `invalid_input`).
- [~] Linux `current` / AT-SPI2: `scripts/cu-linux-smoke.sh` (real `agenterm-cu`, X11
  `DISPLAY`, running `at-spi2-registryd`) proves `tree`, refused unauthorized
  actuation, audited degraded coordinate click, invalid node path failure, and
  structured AT-SPI click when a clickable node exists.
- [x] Windows `current` staged public `cu-windows-smoke` passes its seven
  declared receipts: `cu.windows-host-self-test`,
  `cu.libagenterm-load-cleanup`, `cu.windows-uia-window-identity`,
  `cu.windows-uia-tree`, `cu.windows-uia-name-actuation`,
  `cu.windows-uia-value-wait`, and `cu.windows-uia-cleanup`. This proves the
  staged host/DLL load and cleanup, exact window identity, public UIA tree,
  name-addressed Value/GetText/Invoke journeys and bounded fixture cleanup; it
  does not prove Candidate qualification or release.
- [x] **macOS AX `current` actuation (cut 3.50) — live evidence held.**
  The same journey, same fixture, same run (2026-08-30) emits
  `cu.macos-ax-invoke-set-value`, `cu.macos-ax-invoke-set-checked`,
  `cu.macos-ax-invoke-press`, `cu.macos-ax-invoke-value-readback`,
  `cu.macos-ax-invoke-refusals`. Canonical argv:

  ```sh
  agenterm-cu --target current --grant actuate invoke --window "$HANDLE" --identifier fixture-field set-value "written by cu"
  agenterm-cu --target current --grant observe verify --window "$HANDLE" --expect '[{"identifier":"fixture-field","value":"written by cu"}]'
  agenterm-cu --target current --grant actuate invoke --window "$HANDLE" --name "Fixture Check" --role AXCheckBox set-checked true
  agenterm-cu --target current --grant actuate invoke --window "$HANDLE" --identifier fixture-press press
  agenterm-cu --target current --grant observe wait --timeout-ms 3000 --window "$HANDLE" --expect '[{"identifier":"fixture-press-count","value":"pressed 1"}]'
  agenterm-cu --target current --grant actuate invoke --window "$HANDLE" --identifier fixture-stepper increment
  agenterm-cu --target current --grant actuate invoke --window "$HANDLE" --identifier fixture-popup select-option Beta
  ```

  Required replies: every accepted `invoke` is `verified: true` with a
  `before` / `after` receipt (the second `set-checked true` is
  `performed: false`); `verify` answers `unverified` for a wrong value,
  `unsupported` (`state_unobservable`) for `checked` on the text field and
  `usage` for a misspelled key; `wait --expect` on `pressed 99` is
  `timeout` carrying the last observed `pressed 1`; `--name "Fixture
  Twin"` is `ambiguous` with `count: 2`; `increment` on the check box is
  `unsupported` (`node_action_missing`); `--grant observe` is `refused`;
  the focused window handle is unchanged across the section and is never
  the fixture.
- [x] **macOS AX `current` background verbs (cut 3.51) — live evidence held.**
  The same journey, same fixture (extended with a never-shown main menu),
  same run (2026-08-30) emits `cu.macos-ax-menu-inspect`,
  `cu.macos-ax-menu-invoke`, `cu.macos-ax-focused`, `cu.macos-ax-observe`.
  Canonical argv:

  ```sh
  agenterm-cu --target current --grant observe menu inspect --window "$HANDLE" --depth 2 --title "Do Thing" --exact
  agenterm-cu --target current --grant actuate menu invoke --window "$HANDLE" --path 'File/Do Thing'
  agenterm-cu --target current --grant observe verify --window "$HANDLE" --expect '[{"identifier":"fixture-menu-label","value":"did thing 1"}]'
  agenterm-cu --target current --grant observe focused --window "$HANDLE" --role AXTextField --max-value-bytes 2
  agenterm-cu --target current --grant actuate invoke --window "$HANDLE" --focused --role AXTextField set-value "typed into focus"
  agenterm-cu --target current --grant observe observe --window "$HANDLE" --duration 1.5 --notification ValueChanged --max-events 50
  ```

  Required replies: `menu inspect` lists `File/Do Thing` enabled and
  `File/Disabled Thing` disabled with counts and `truncated` under
  `--max-nodes 3`; `menu invoke` is `verified: true` and the label reads
  `did thing 1` on an independent `verify`, while `File/Twin Thing` →
  `a11y_menu_item_ambiguous`, `File/Disabled Thing` →
  `a11y_menu_item_disabled`, `File/Nowhere` → `a11y_menu_item_not_found`,
  `File/More` → `a11y_menu_item_not_leaf`, `File` → `usage`, `--grant
  observe` → `refused`; `focused` names the field with `value_bytes` and
  `value_truncated`, `--role AXButton` is `unverified`; `invoke --focused`
  is verified on that field; the `observe` reply (`mode: "poll-diff"`)
  holds the `ValueChanged` of the `set-value` issued while it ran with
  monotonic `seq` / `t_ms`, and `--max-events 1` stops early with
  `truncated: true`; the focused window handle is unchanged across every
  section and is never the fixture.
- [x] **macOS AX `current` observe (cut 3.49) — live evidence held.**
  `scripts/qjs/cu-macos-smoke.qjs` (task `cu-macos-smoke`, host-native gate,
  `--profile tool`) passed on a real macOS desktop on 2026-08-30 and emits
  `cu.macos-ax-capabilities`, `cu.macos-ax-window-identity`,
  `cu.macos-ax-query`, `cu.macos-ax-tree-bounded`,
  `cu.macos-ax-tree-actions`, `cu.macos-ax-fixture-cleanup`. The recipe it
  realizes (the cut-3.45 handoff, now executable):

  1. The process that spawns the journey holds Accessibility
     (`AXIsProcessTrusted()` true for `agenterm-cu`).
  2. The journey compiles and spawns the owned fixture
     `examples/objc/agenterm_ax_fixture.m` (accessory policy, ordered front
     without activating, unique title `agenterm-ax-fixture-<pid>`, an
     `AXTextArea` seeded with **`345AXTREE`** and identifier `fixture-text`,
     a **`Fixture Press`** button with identifier `fixture-press`); a
     system app cannot stand in (launch constraints kill a directly exec'd
     system binary; `open -a` hands the pid to LaunchServices).
  3. `wait --window-title-contains` then `windows --pid PID` resolve
     `HANDLE` with inventory counts.
  4. Canonical argv:

     ```sh
     agenterm-cu --target current --grant observe query --window "$HANDLE" --role AXTextArea
     agenterm-cu --target current --grant observe tree --window "$HANDLE" --max-nodes 5
     agenterm-cu --target current --grant observe tree --window "$HANDLE" --flat
     ```

  5. Required replies: `backend: "ax"`, exactly one `text-area` whose text
     is `345AXTREE`, `--max-nodes 5` → `truncated: true` / `returned: 5`,
     the full tree → `truncated: false` with the `Fixture Press` button
     carrying `click`, and the fixture ends with exit 0 on SIGTERM with no
     orphan. Never screenshot, OCR, CGEvent, or AT-SPI/UIA reuse.

- [~] **RDP target PLACEHOLDER (cut 3.46) + declared `capabilities` (cut
  3.47) — live RDP / Windows evidence NOT claimed.** Linux static proof:
  `TargetRef::parse("rdp") == Some(Rdp)`, `as_str() == "rdp"`, `--rdp` /
  `--target rdp` CLI; `capabilities` returns `ok:true` with transport
  placeholder/`rdp_unavailable` and `tree` unsupported without dialing
  (sentinel receives zero connections); every other authorized RDP command
  returns `error.code:"rdp_unavailable"` without dialing. Worker JSON does
  not count; CEO owns the official check. Transport, authentication,
  session lifecycle, and UIA-over-RDP remain empty. **Handoff to a later
  Windows agent** (new cut, not 3.46/3.47):

  1. Choose and document the real boundary: native RDP client/session API
     or an explicitly managed external client; NLA/CredSSP and certificate
     validation; username/domain/secret acquisition without argv leakage;
     desktop/session lifecycle and teardown; cancellation/timeouts; and
     whether a remote session worker is deployed or UIA runs inside the
     logged-on Windows session. Do **not** silently map RDP to SSH, VNC,
     local `current`, screenshots, or coordinates. Do not invent password
     flags until secret-input policy is designed for that cut.
  2. Controlled Windows host/VM with RDP enabled, dedicated test
     account/session, and a cut-owned native fixture (unique title/PID;
     seeded editable text e.g. `346RDPTREE`; named button e.g.
     `Fixture Press`) visible to the remote interactive desktop's UIA.
  3. Resolve `HANDLE` through the RDP target's own window enumeration, then
     reuse the reserved first-observe argv:

     ```sh
     agenterm-cu --rdp "WINDOWS_HOST:3389" \
       --grant observe tree --window "$HANDLE"
     ```

  4. Positive live acceptance: `ok:true`, `target:"rdp"`, `command:"tree"`,
     `backend:"uia"`, unique native fixture nodes; session uniquely owned
     by the gate; no screenshot/OCR/`--coords`/SSH/VNC/`current` fallback.
  5. Negative typed failures (distinct codes as the surface lands): closed
     endpoint, bad credentials, certificate rejection, lost session,
     missing UIA, bogus handle, timeout, bound exhaustion. Until live
     evidence lands, every RDP command **except** the static
     `capabilities` declaration remains `rdp_unavailable` and the PRD
     branch stays `[~]` placeholder.

- [x] **Linux `tree` cross-tier conformance (cut 3.48).** Same abstract
  `tree --window HANDLE` on one cut-owned second `agenterm-con` through
  `current`, loopback SSH, and dedicated loopback VNC. Semantic compare
  (not byte-for-byte): public target restored per tier; `backend:"at-spi2"`;
  same window / root / node count; exactly one showing `Command`, `SEND`,
  and `OffscreenField`; same role/name/text/actions/parent/stable path;
  bounds equal absent movement (bounded full-set retry on desktop churn);
  volatile focus states may differ by observation timing. Harness:
  `scripts/cu-linux-cross-tier-tree.sh` → `live/348-cross-tier-tree.json`.
  **Not claimed:** RDP tree, macOS AX live, Windows UIA-over-RDP, other
  verbs, screenshots/`--coords`. A mismatch is
  `cross_tier_conformance_failed`.
- [~] **Per-target `capabilities` declaration (cut 3.47).** Existing observe
  verb only (no new verb, no target enumeration). Load-bearing:
  `--rdp … capabilities` → `ok:true` / `target:"rdp"` / transport
  placeholder unavailable / `tree` not supported / zero sentinel
  connections; `--rdp … tree` still `rdp_unavailable`; `current` /
  loopback `ssh` / dedicated loopback `vnc` each name their public tier
  on both reply and `data.target` (no `data.target:"current"` leak for
  ssh/vnc requests); no tier declares unproven macOS AX or live RDP as
  available; missing observe grant remains `refused`. Full
  declare-vs-reality conformance across every verb remains **[ ]**: a
  target that declares support and then fails the command is a defect,
  not a runtime condition. Linux `tree` is the first verb closed under
  cut 3.48.
