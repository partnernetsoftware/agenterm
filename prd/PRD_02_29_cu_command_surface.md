# `agenterm-cu` command surface and layering

Parent: [Computer-use foundation (`agenterm-cu`)](PRD_02_28_agenterm_cu.md)

Delivery truth: the command surface and desktop host are modes of the single
`agenterm-cu` executable. CU is the first runtime `libagenterm` consumer; this
module owns abstract command and action meaning, while the dynamic ABI and
platform adapters own native mechanism. Formal `dist`/Candidate qualification
is still in progress, so implemented command leaves do not make the root
product shipped.
This module owns the abstract command set every target must honor, the layering
contract that keeps it from rotting, the structured control-tree observation
model, and the determinism rules. It does not own transports
([30](PRD_02_30_cu_targets_transports.md)) or authorization
([31](PRD_02_31_cu_authorization_safety.md)).

Legend: `[x]` shipped, `[~]` partial, `[ ]` planned.

## Layering contract

- [ ] the surface is layered and **outer may depend on inner only**:

  ```text
  native primitive   平台机制（agenterm-platform 拥有，cu 不实现）
     ↑
  abstract command   目标无关命令集（本模块拥有）
     ↑
  target selector    选目标/transport（30 拥有）
     ↑
  workflow           组合动作、重试、等待编排
     ↑
  shell command      公开 CLI 入口
  ```

- [ ] a layer never reaches past its neighbor. A workflow may not call a native
  primitive; a shell command may not encode target-specific behavior. A
  violation is a structural defect, not a style preference.
- [ ] the abstract command set is target-agnostic by construction: a command
  whose semantics only make sense for one transport does not belong in it.

## Abstract command set

- [x] the concrete verb surface is one static table,
  `crates/agenterm-cu/src/bin/cli/verbs.rs` (name, aliases, scope, family,
  usage, reference). `agenterm-cu --help` renders it grouped by family;
  `agenterm-cu help <verb>` / `<verb> --help` carry the reference prose;
  `agenterm-cu verbs --json` emits the table; `scripts/gen-cu-verbs-doc.sh`
  regenerates `docs/agenterm-cu-verbs.md` from it. That generated file, not
  this PRD, is the exhaustive per-verb reference; this PRD lists the abstract
  set and layering. Verbs the table carries beyond the abstract set below:
  `windows-watch`, `exec --json`, `receipts`, `verbs`, `help`, and the MCU
  aliases (`shot`, `type`, `key`, `move`, `dclick`, `rclick`, `frame`,
  `movewin`, `resize`, `maximize`, `cursor`, `clip`, `caps`, `elements`,
  `inspect`, `find`, `read`).
- [x] focused-node text writers (`send-text`, `paste`, `send-keys` with
  `--window` and no `--name`) refuse `focused_node_is_browser_chrome` when the
  window is a browser and focus sits in its chrome (omnibox, toolbar, tab
  strip); `--allow-browser-chrome` is the explicit override. A `--name` miss
  performs no write (write-ledger tests).
- [ ] the initial set covers observation and actuation:
  screenshot; window enumeration; control-tree enumeration; pointer
  press/release/move/click/drag; wheel; keyboard text and named keys; clipboard
  read/write; file transfer in both directions; **named window placement**
  (`window-place`, owned by [32](PRD_02_32_cu_window_placement.md)).
- [x] the default control loop is `windows` -> bounded `query` / `tree` ->
  `invoke`, with `verify --expect` closing the loop (absorbed from
  `moltbaby/skills/mcu`, 2026-08-30). `elements`-style flat numbering is a
  secondary path (`tree --flat`, the same flatten index `invoke --index`
  uses); screenshots are the last resort, never the default. The whole loop
  is live on macOS `current` in `scripts/qjs/cu-macos-smoke.qjs`: observe
  STEPs "windows --pid", "query by role", "tree --max-nodes 5" (slice 1),
  then actuation STEPs "invoke set-value writes the text field and verify
  --expect reads it back", "invoke set-checked true twice", "invoke press
  advances the count label; wait --expect and verify read it on another
  node", "invoke increment / decrement on the stepper and select-option on
  the pop-up read back" (slice 2, 2026-08-30). Linux / Windows run the same
  verbs on their own backends with partial mappings (see PRD 30); live
  evidence there is not claimed.
- [x] Chromium idle chrome is not an empty page: `tree` / `query` JSON
  carries `ax` plus `next_actions` that name a deeper
  `query --role WebArea` (never screenshot, never “install extension”).
  `verify` / `wait --expect` accept identity-only `name` /
  `titleIncludes`; Heading↔WebArea alias applies only when a title
  predicate is present. Evidence:
  `crates/agenterm-cu/src/observe.rs`
  `empty_chrome_next_action_is_deeper_query_not_screenshot_or_extension`,
  `heading_title_includes_matches_webarea_title`.
- [~] MCU selector and recovery spellings are now explicit product inputs:
  `query --selector` scopes results beneath one `Role[idx] / Role@title /
  *@title / #desc` match, and `invoke --selector` binds the same walk to one
  action target. Extra MCU invoke spellings parse but
  fail typed `unsupported` until the platform ABI maps them. Unit/CLI evidence
  owns parsing, scope, ambiguity, and typed refusal; a new real-app journey is
  still required before this leaf becomes shipped.
- [x] `capabilities` carries a first-class `permissions` block (cut 3.58):
  each permission the host gates mechanisms behind, its grant status, the
  repair path when it is denied, and **the list of verbs that stop working
  without it**. The repair path used to live inside the `tree` verb's own
  declaration, which was where a caller would look for it only if they
  already knew the tree was the thing being denied -- while on macOS the
  same Accessibility grant also gates every input verb. A host with no
  per-application permission model says so (`model: "none"`) rather than
  reporting an empty set that reads like "nothing is granted".
  `setup` / `doctor` / `permissions` stay typed `unsupported`: the TCC
  wizard is deliberately MCU's, but the *reporting* it stands in for has
  to be complete, and now is. Live evidence: `cu-macos-smoke` asserts the
  grant status, that the gate list names both `tree` and `pointer-move`,
  and that screen recording reports `not_required` on a host with no
  capture path.
- [x] `unlock --window HANDLE` (Actuate grant) reads the window's bounded
  tree, asks the owning application to build its full accessibility tree
  (macOS `AXManualAccessibility`, ABI 1.15
  `agt_a11y_manual_accessibility_poke`), reads it again, and reports
  `poked` (the request was delivered), `grew` and `returned_before` (what
  the two reads found) plus `ax` and `next_actions`. The three fields are
  separate because **the poke's own status is not the outcome**: AppKit
  reports the attribute as unsupported even when the poke lands, so only
  the re-read can claim anything about the tree. A host with no such
  mechanism reports `poked: false` with the backend's reason and still
  returns the classification. Live evidence: `cu-macos-web-smoke` STEP
  "unlock asks the engine to build the web tree and reports what the
  re-read found, not what the call returned" (2026-08-31).
- [x] `page-js` is a second knife after AX: `Runtime.evaluate` over CDP on
  `127.0.0.1:<--port>` (default 9222) when the browser was started with
  `--remote-debugging-port`; with no listener the verb is typed
  `unsupported` with `detail.backend = debugger-runtime-evaluate`.
  Ordinary AX web control needs no browser extension. MAIN-world
  `eval` / `new Function` is never the backend (chatgpt.com CSP).
  Evidence: `Command::PageJs`, `capabilities.verbs["page-js"]`,
  `observe::page_js_backend()`.
- [x] `page-js` addresses one tab: at most one of `--target-id ID` (exact
  CDP id), `--target-url SUB` / `--target-title SUB` (case-insensitive
  substring), or MCU-compatible `--match SUB` across title + URL +
  description filters the `/json` page/webview/other targets; none keeps the
  first eligible page.
  No match is typed `cdp_target_not_found`, more than one
  `cdp_target_ambiguous`, both with the candidates (id, url, title) in
  `error.detail`; the reply echoes the chosen `target`. `Runtime.evaluate`
  runs in a background tab, so no focus changes. `page targets` /
  `page-targets [--port N | --pid PID]` lists the inventory (id, url, title, type,
  attached, websocket); no listener is typed `unsupported` like `page-js`.
  Every CDP page verb accepts the same exclusive endpoint pair. `--pid`
  observes one stable process identity before and after a bounded native
  command-line read, accepts only its explicit valid debugging-port flag, and
  never scans ports or publishes the credential-bearing command line.
  Evidence: `cdp::targets` selector unit tests; `scripts/cu-cdp-smoke.sh`
  PASS 2026-09-03, strengthened 2026-09-04 with real PID endpoint discovery
  (headless Brave Origin, fresh profile, two `data:`
  tabs: `page targets`, `page-js --target-title` on the background tab,
  not-found, ambiguous) once the `/json` reader honoured
  `Content-Length` / chunked framing (`src/cdp/http.rs`; Chromium keeps
  the DevTools socket open, so a read-to-EOF only returned on timeout).
- [x] a background tab in a background window is read AND acted on over
  CDP without changing which tab or window is active. The AX tree carries
  only the active tab's web-area on macOS, so these verbs take the
  `page-js` target selector, run on that target's own websocket
  (`src/cdp/ws.rs`: one session, numbered methods, buffered events, a
  `Transport` trait the tests script with fake transcripts), and never
  call `Target.activateTarget` / `Page.bringToFront`; every reply carries
  `focus_changed: false` and the target `{id, url, title}`:
  `page text --target-*` (observe; the AX-verb row shape `{id, role,
  text}` from `Accessibility.getFullAXTree`, fallback a DOM `innerText`
  walk, `backend: "cdp"`, `id` = backend DOM node id), `page find
  (--selector CSS | --text SUB | --role R [--name SUB])` (observe; `{node,
  path, tag, role, name, text, value, editable, box}`, a text hit inside a
  button / link lifted to the control, zero -> `cdp_node_not_found`),
  `page click ((--selector | --text | --node) | --x X --y Y) [--button]
  [--clicks]` (actuate; one node or `cdp_node_ambiguous` with candidates,
  or one frozen rendered viewport point; node clicks use document/node
  read-back, point clicks require trusted down/up at the point and attempt
  release after every accepted press), `page hover
  --x X --y Y` (actuate; bounded viewport CSS coordinates, one trusted
  `mousemove` probe installed before dispatch and removed after read-back;
  headless Chromium may truthfully deliver the event without maintaining CSS
  `:hover`), `page scroll --x X --y Y [--dx DX] [--dy DY]` (actuate; nearest
  scrollable container frozen during planning, wheel dispatched there, then an
  event-driven bounded wait reads its exact offsets; a boundary is performed
  but `no_observable_scroll_change`), `page drag --x1 X --y1 Y --x2 X --y2 Y`
  (actuate; two distinct rendered viewport points frozen before effect, left
  down + held move + up, release attempted after every accepted press,
  verified from the target page's trusted event sequence), `page files (--selector | --node)
  FILE...` (actuate; 1..16 absolute browser-host paths, regular non-symlink
  files only, exact enabled `input[type=file]`, `multiple` preflight,
  `DOM.setFileInputFiles`, then exact FileList basename/size read-back; public
  results and receipts never retain the local paths), `page dialog [--dismiss]
  [--text T] [--wait-ms N]` (actuate; waits for
  `Page.javascriptDialogOpening`, then accepts/dismisses and verifies
  `Page.javascriptDialogClosed`; message/default/response/userInput contents
  are represented only by byte counts), `page fill
  (--selector | --node) --text T [--clear] [--submit]` (actuate;
  `DOM.focus`, select-all, `Input.insertText`, `.value` read back ==
  text, Enter key events; focus emulation on for the write and off
  after), `page type TEXT` (actuate; freezes the already-focused editable
  element, inserts without changing focus, then requires the same element and
  exact value growth; text and field values are absent from public/persistent
  evidence), `page nav --url U [--wait-ms N]` (actuate; `Page.navigate`,
  `Page.loadEventFired`, final url / title), `page screenshot --out P`
  (observe; Chromium may refuse an unpainted background tab ->
  `cdp_screenshot_unavailable`, never activated; `--activate` is the one
  explicit actuate opt-in and replies `focus_changed: true`). Actuators
  reserve a receipt between the read-only plan and the dispatch. Evidence:
  `cdp::page` / `cdp::ax` unit tests on scripted transcripts (message
  shaping, ambiguity, verification, no activation method ever sent);
  `scripts/cu-cdp-actuate-smoke.sh` PASS 2026-09-03 (headless Brave
  Origin, tab A active, every verb on tab B whose button `onclick` and
  form `onsubmit` mutate the DOM, read back through `page-js`; after each
  verb `/json` still listed A first and `windows --focused` was
  unchanged). The expanded 2026-09-04 throwaway headless Google Chrome court
  verified `page-hover` by the received event target, `page-scroll` by a
  `scrollTop` change from 360 to 480, and `page-files` by exact FileList
  basename/size while its absolute fixture path was absent from public output;
  The same court then verified `page-drag` from two live element boxes by both
  the trusted held-event sequence and a page-owned business state change, and
  verified `page-type` against the existing input focus while proving the
  inserted text was absent from its reply and receipt,
  accepted a real prompt through `page-dialog`, reading its page-owned result
  back while proving the response was absent from reply/receipt. All retained
  `focus_changed: false`. MCU-shaped positional read/nav/hover/scroll/drag/dialog/files
  now route through the sibling compatibility shim instead of staying on MCU.
  The court also proves a unique `--match` selects B and a pattern spanning A+B
  is typed `cdp_target_ambiguous` rather than silently choosing the first. Still open:
  the same run on the owner's real instance needs
  it relaunched with `--remote-debugging-port=9222`.
- [x] `page text --window H [--max-bytes N] [--within X,Y,W,H] [--depth N]
  [--max-nodes N]` returns the visible words in reading order (child-index
  path = document order, not the breadth-first walk order) as compact rows
  `{id, role, text, bounds}` (+ `name` when it differs, `focused`,
  `actionable`), merging a link's / button's inner text into one row, so
  the next step is `invoke --node` / `click --node` and never a screenshot.
  Chromium keeps a web node's words in `AXValue` (`text`), which is why a
  `name`-only reading shows an empty page. Bounded by bytes (default 16 KiB,
  max 1 MiB, `truncated`) and by the walk budget, which defaults to depth
  64 / 6000 nodes: the platform's 1000-node breadth-first default is spent
  on browser chrome before deep web content. `query` / `tree` report the
  same fact in `next_actions` when their walk truncates. Evidence:
  `page_text` unit tests on a fake Chromium-shaped tree; live Brave (94
  rows, 472 nodes, 2026-09-03).
- [x] `unlock` sets `AXManualAccessibility` and `AXEnhancedUserInterface`
  on the application plus `AXManualAccessibility` on the window, wakes the
  renderer as an assistive client would, re-reads bounded (5 x 200 ms) and
  reports `web_nodes_before` / `web_nodes_after`, `rereads` and the
  `poke` description (plus `reason` when the poke was refused) with a
  depth-64 / 6000-node comparison read; `grew` is true when either the
  returned count or the web-node count rose. The previous depth-12
  comparison could not see a web-area's children, so `grew` was false
  regardless of the poke.
- [x] A typed miss performs no write: `send-text` / `send-keys` /
  `paste --name` on a missing node leave the mechanism write ledger
  (`mechanism::write_ledger`) untouched. Evidence:
  `name_and_role_send_text_miss_performs_no_write_on_any_path`; live
  reproduction of the reported omnibox write did not reproduce (omnibox
  unchanged after `a11y_node_not_found`).
- [x] `tab list --window H` / `tab select --window H (--title SUB |
  --index N)` are the a11y fallback for the same problem: macOS Chromium
  lists background tabs only as `radio-button` rows of the tab-strip
  `tab-group` (no `web-area`), so `tab select` presses that row in the
  background and verifies by `selected` read-back (already selected is a
  verified no-op; receipts `reserved` / `completed`). No such tab is
  `a11y_tab_not_found`, two title hits `a11y_tab_ambiguous`. Never raises
  or activates the window. Evidence: `tab_strip` matcher unit tests; live
  Brave background window 2026-09-03 (focused window unchanged before and
  after the switch).
- [x] Profiles of the **real running** Chromium-family browser (Brave
  Origin / Brave Browser / Google Chrome; anything else typed
  `unsupported`), 2026-09-03. `browser profiles [--app SUB]` reads the
  application's `Local State` (`profile.info_cache` directory -> display
  name, `profile.last_used`) and joins each profile to the inventory
  windows whose `browser_profile` equals its name: rows `{name,
  directory, last_used, windows: [handles]}`; without `--app` the one
  running catalog application is used (`browser_app_not_found` /
  `browser_app_ambiguous`). `browser open --profile NAME [--url URL]
  [--app SUB] [--timeout-ms N]` resolves NAME (exact, then unique
  case-insensitive substring; `browser_profile_not_found` /
  `browser_profile_ambiguous` with candidates), runs macOS `open -na
  <app> --args --profile-directory=<dir> [URL]` -- the process singleton
  hands it to the running instance, which is never quit or restarted --
  and polls the inventory (default 8000 ms) until a window of that
  profile appears that was not in the before snapshot or, with a URL
  into a profile that already had a window, until that window's title
  changes; reply `{handle, browser_profile, title, created}` with a
  receipt, timeout `browser_window_not_found`. `windows
  --browser-profile SUB` is one more inventory filter row. `page targets
  --browser-profile SUB` keeps only the targets whose title equals a tab
  title of that profile's window (`profile_match: "title"`) and says it
  is a heuristic: one CDP port serves every profile and a target carries
  no profile field. `tab close --window H --title T --exact --expect
  gone` is the destructive tab verb behind the `close` gate (exact title,
  strip snapshot in the receipt, `gone` read back from the strip), pressed
  through the tab row's own close button; a row without one (macOS
  Chromium exposes it on the active tab only) is typed `unsupported`
  (`tab_close_button_missing`), never a keyboard shortcut. Evidence: pure
  `browser_profiles` tests on `tests/fixtures/local_state.json`
  (synthetic names), gate / button-matcher tests in `executor/browser.rs`,
  and `scripts/cu-brave-live-smoke.sh` on the real five-profile Brave
  Origin instance (dated PASS line in the script header). Still open: CDP
  verbs (`page-js`, `page targets`, the `page find / click / fill / nav`
  background-tab verbs) on that instance need it started with
  `--remote-debugging-port`, which this leaf does not do.
- [x] `query --window HANDLE [--depth N] [--max-nodes N] [--role R,R]
  [--text T | --text-exact T] [--identifier ID] [--actionable] [--within
  X,Y,W,H] [--offset N] [--max N]` returns a flat, bounded, filtered node list
  with the same node identity `tree` uses (path id plus flatten `index`),
  plus `visited / matched / returned / truncated` counts (with
  `scan_truncated` / `page_truncated` split out). Depth (root = 0, at most
  64) and node budget (1..20000) apply *during* traversal through ABI 1.12
  `agt_a11y_tree_snapshot_bounded`; an unbounded tree is never built first.
  `--role` accepts the platform spelling (`AXTextArea`) or the contract's
  (`text-area`); `--text` is a case-insensitive substring of name or text,
  `--text-exact` / `--identifier` are exact, `--within` is a screen-rect
  intersection. The CLI shape is closed: an unknown flag, a missing value or
  a stray positional fails typed `usage` before any tree is read. Evidence:
  `cu-macos-smoke` STEP "query by role, identifier and exact text"
  (`cu.macos-ax-query`); pure tests own the filter, paging and bounds
  (`crates/agenterm-cu/src/observe.rs`). Linux / Windows answer through the
  same verb with their own backends; live evidence there is not claimed.
- [x] `tree --window HANDLE [--depth N] [--max-nodes N] [--flat]` applies
  the same traversal-time budget and reports `truncated` / `visited` /
  `returned` plus the requested `budget`; reaching a budget is a bounded
  reply, not an error. `--flat` lists the same nodes in walk order with
  `index` and `depth` per node — the numbering `query` reports and a later
  `invoke --index` addresses. Evidence: `cu-macos-smoke` STEP
  "tree --max-nodes 5 and --depth 0 report truncated" and STEP "full tree
  reports truncated false" (`cu.macos-ax-tree-bounded`).
- [x] `windows [--pid N] [--app SUB] [--title SUB] [--focused [BOOL]]
  [--minimized [BOOL]] [--offset N] [--max N]` filters the inventory
  (substrings case-insensitive) and pages it; with any filter or page flag
  the reply is `{windows, visited, matched, returned, offset, truncated}`,
  while the bare verb keeps its window-array reply. Evidence:
  `cu-macos-smoke` STEP "public wait and windows --pid prove PID, title and
  CGWindow identity" (`cu.macos-ax-window-identity`).
- [x] `tree` and `query` carry each node's available actions from the
  platform a11y backend (macOS: `AXUIElementCopyActionNames`, normalized —
  `AXPress` is `click`, `AXRaise` is `focus`, `AXShowMenu` is `show-menu`,
  other names kebab-cased); an empty action list means the backend reported
  none, never that it was not asked. Evidence: `cu-macos-smoke` STEP "full
  tree reports truncated false and AX action names on its controls"
  (`cu.macos-ax-tree-actions`: the `Fixture Press` button carries `click`).
  The `invoke` action vocabulary (`press`, `set-value`, `increment`, ...)
  is the leaf below. macOS also reports two-way control states so a caller
  can tell "off" from "not observable": `checked` / `unchecked` / `mixed`
  (AXCheckBox / AXRadioButton `AXValue` 0 / 1 / 2) and `expanded` /
  `collapsed` (`AXExpanded`, disclosure triangles), and a numeric `AXValue`
  (stepper, slider) is the node's `text`.
- [x] `invoke --window HANDLE (--node PATH | --index N | --name PAT [--role
  ROLE] | --identifier ID) <action> [VALUE]` performs one semantic action
  (`press`, `set-value TEXT`, `select-option NAME`, `set-checked
  true|false`, `set-expanded true|false`, `increment`, `decrement`) through
  the platform a11y backend (ABI 1.13 `agt_a11y_node_invoke`) without
  activating or raising the window. Two showing matches are typed
  `ambiguous` (with `count`), none is `a11y_node_not_found`, an action the
  node does not list is `unsupported` (`detail.reason =
  node_action_missing`, the offered actions in `detail.offered`), and a
  desired-state verb on a node with no readable state is `unsupported`
  (`state_unobservable`) — never a blind press. `set-checked` /
  `set-expanded` are desired states: an already-matching node is
  `performed: false` and still `verified: true`. Every reply carries
  `verified` with `verification.method` / `reason` (value / checked /
  expanded read-back; `increment` / `decrement` must change the numeric
  value; `press` is verified by a whole-tree diff, `no_observable_change`
  otherwise) and a receipt (`target`, `node`, `action`, `value`,
  `performed`, `before` / `after` state). Evidence: `cu-macos-smoke` STEPs
  "invoke set-value writes the text field and verify --expect reads it
  back" (`cu.macos-ax-invoke-set-value`), "invoke set-checked true twice:
  the first presses, the second is a verified no-op"
  (`cu.macos-ax-invoke-set-checked`), "invoke press advances the count
  label" (`cu.macos-ax-invoke-press`), "invoke increment / decrement on the
  stepper and select-option on the pop-up read back"
  (`cu.macos-ax-invoke-value-readback`), "ambiguous --name, missing
  action, unobservable state, missing target and observe-only grant are
  typed refusals" (`cu.macos-ax-invoke-refusals`). The CLI shape is closed
  (`usage` for an unknown action or stray flag; `invalid_input` for a bad
  value). Linux maps `press` / `set-value`, Windows `press` / `set-value` /
  `set-checked` (Toggle); the rest answer typed `unsupported` there
  (compile-checked, not live-proven).
- [x] `verify --window HANDLE --expect '[{"node"|"index"|"name"[+"role"]|
  "identifier"|"role", "value"?, "checked"?, "expanded"?, "focused"?}, ...]'`
  reads one tree and checks every item against the same fields `query`
  reports: all met is `ok` with `verified: true` and per-item `checks`
  (`expected` / `observed` / `met`); a known mismatch is typed
  `unverified` with the observation; a state the node does not expose
  (`checked` on a button, `focused` on a non-focusable node) is typed
  `unsupported` (`state_unobservable`) — fail closed, never "probably
  fine"; an unknown key is `usage` before any tree is read. `wait
  --timeout-ms MS --window HANDLE --expect JSON` polls the same matcher: a
  missing target keeps polling, ambiguity and an unobservable state fail at
  once, and the deadline is typed `timeout` carrying the last observation
  (what was seen, not just that time passed). Evidence: `cu-macos-smoke`
  STEP "invoke set-value writes the text field and verify --expect reads
  it back" (`verify` ok / `unverified` / `unsupported` / `usage`) and STEP
  "invoke press advances the count label; wait --expect and verify read it
  on another node" (`wait --expect` met and `timeout` with the last
  observed value).
- [x] second batch (slice 3 of `plan/design-mcu-absorption.md`, 2026-08-30,
  macOS `current` live; Linux / Windows answer typed `unsupported` from the
  platform, compile-checked only):
  - `menu inspect --window HANDLE [--depth N] [--max-nodes N] [--title T
    [--exact]] [--enabled true|false] [--offset N] [--max N]` reads the
    application's menu bar in the background through the a11y contract
    (ABI 1.14 `agt_a11y_menu_snapshot`: macOS `AXMenuBar` → `AXMenuBarItem`
    → `AXMenu` → `AXMenuItem`) without opening a menu on screen or
    activating the app. `--depth` counts menu levels (0 = bar items only,
    default 1, at most 8) and `--max-nodes` (1..5000, default 1000) bounds
    the walk *during* traversal; items carry the exact title `path`, `depth`,
    `enabled` / `checked` (mark) / `has_submenu` (as far as the walk
    reached), and the reply carries `nodes_visited / visited / matched /
    returned / truncated` with `scan_truncated` / `page_truncated` split out.
    The CLI shape is closed. Evidence: `cu-macos-smoke` STEP "menu inspect
    reads the background menu bar and finds File/Do Thing without opening
    it" (`cu.macos-ax-menu-inspect`: `File/Do Thing` enabled at depth 1,
    `File/Disabled Thing` disabled, two `File/Twin Thing`, `File/More`
    with a submenu, `File/More/Deeper Thing` at depth 2, `--title
    "Do Thing" --exact`, `--enabled false`, `--depth 0 --max-nodes 3`
    truncated, `--depth 9` `invalid_input`, unknown flag `usage`).
  - `menu invoke --window HANDLE --path 'Menu/Item' | '["Menu","Item"]'`
    (Actuate grant) presses the exact item by title path (ABI 1.14
    `agt_a11y_menu_invoke`): the platform resolves every segment to exactly
    one *enabled* item before pressing anything — `a11y_menu_item_not_found`
    / `a11y_menu_item_ambiguous` / `a11y_menu_item_disabled` — the last must
    be a leaf (`a11y_menu_item_not_leaf`), and a bare menu is `usage` /
    `invalid_input` because pressing it would open it on screen. The reply
    carries `verified` (mark read-back when the mark changed, otherwise a
    whole-window tree diff; `no_observable_change` when neither moved) and
    `mark_before` / `mark_after`. Evidence: `cu-macos-smoke` STEP "menu
    invoke presses File/Do Thing in the background; the label changes and
    the focused window does not" (`cu.macos-ax-menu-invoke`: `File/Do
    Thing` sets `fixture-menu-label` to `did thing 1` read by `verify`,
    `["File","More","Deeper Thing"]` to `deeper thing`, the five refusals
    above plus `--grant observe` → `refused`, the label unchanged after
    them, and the focused window handle the same before and after and
    never the fixture).
  - `focused --window HANDLE [--role ROLE] [--max-value-bytes N]` returns
    the application's own focused control (ABI 1.14
    `agt_a11y_focused_snapshot`: macOS `AXFocusedUIElement`, read without
    requiring the foreground) as a node whose `id` is its path in the same
    window tree `query` numbers, with role / name / identifier / states /
    `focused`, a value preview bounded by `--max-value-bytes` (default
    4096; `0` keeps only `value_bytes`) and `value_truncated`. `--role`
    binds the expected role: a mismatch is typed `unverified` carrying the
    observed control, never a guess. No focused element is
    `a11y_focus_unavailable`; one outside the window is
    `a11y_focus_outside_window`. `invoke --window HANDLE --focused [--role
    ROLE] <action>` writes to that control only after binding PID + window
    + focused identity (id, role, identifier) in the same tree read
    (`a11y_node_recycled` when the identity moved between reads; `--focused`
    with another selector is `usage`). Evidence: `cu-macos-smoke` STEP
    "focus moves the first responder to the text field and focused / invoke
    --focused bind it" (`cu.macos-ax-focused`: the fixture's initial focus is
    the `AXTextArea`, `focus --node` moves it to the `AXTextField`, `focused
    --role AXTextField --max-value-bytes 2` answers `wr` / `value_bytes 13`
    / truncated, `verify` reads `focused: true` / `false` on the two nodes,
    `--role AXButton` is `unverified`, `invoke --focused --role AXTextField
    set-value` is verified on the field, `--focused --role AXButton` is
    `unverified`, `--focused --identifier` is `usage`). This is also the
    first journey proof of `focus` on macOS.
  - `observe --window HANDLE (--duration S | --duration-ms N) [--depth N]
    [--max-nodes N] [--max-events N] [--notification A,B] [--interval-ms
    N]` emits a bounded, filtered event stream over the same bounded tree.
    The platform crate wires no AX notification observer, so the stream is
    a **poll-diff** (`mode: "poll-diff"`, default interval 50 ms, at least
    20 ms): every semantic difference between consecutive walks becomes an
    event — `ValueChanged` / `TitleChanged` / `StateChanged` /
    `FocusChanged` / `Created` / `Destroyed` (bounds ignored) — with a
    monotonic `seq` and `t_ms`, the node identity and `before` / `after`.
    `--notification` filters (AX spellings accepted; an unknown name is
    `usage`), `--max-events` (default 200, at most 5000) ends the stream
    early with `truncated: true` and `stopped: "max-events"`, and the reply
    counts `polls / poll_errors / emitted / filtered`. Duration is at most
    120 s. Evidence: `cu-macos-smoke` STEP "observe --duration 1.5 captures
    the ValueChanged of a set-value issued while it runs"
    (`cu.macos-ax-observe`: the observer is a second `agenterm-cu` spawned
    through the door while the script issues `invoke set-value`, the reply
    holds exactly that `ValueChanged` on `fixture-field` with the old and
    new value, `seq` / `t_ms` monotonic, `stopped: "deadline"`; a second
    observer with `--max-events 1` stops on the first of two writes with
    `truncated: true`; `--notification Moved` is `usage`).
- [x] `close --window HANDLE [--pid N] [--title T] --snapshot --expect gone`
  is the destructive verb (absorbed from `moltbaby/skills/mcu`, slice 4):
  it closes one top-level window in the background through the platform's
  own close control (macOS `AXCloseButton` + `AXPress`; never activating
  or raising the app), gated by [31](PRD_02_31_cu_authorization_safety.md)'s
  three-part destructive rule — an exact target, a prior snapshot written
  to the receipt, and a checkable postcondition read back — any missing
  part typed `refused` (`detail.reason = destructive_gate`). The CLI shape
  is closed. Evidence: `cu-macos-smoke` STEPs
  `cu.macos-ax-destructive-refusals` (missing snapshot / postcondition /
  target, wrong `--pid` / `--title`, unknown handle, observe grant, bad
  postcondition are each typed and perform nothing) and
  `cu.macos-ax-destructive-close` (the fixture's second window is closed
  and read back gone, the main window and process untouched).
- [x] `receipts [--window HANDLE] [--max N]` reads back the crash-persistent
  effect-receipt file every actuation appends to
  (`<audit dir>/cu-receipts/<target>.jsonl`): a `reserved` line before the
  mechanism and a `completed` / `failed` line after the read-back, newest
  last, filtered by `window`, at most `max` (default 50, ceiling 1000). It
  is Observe-grant observation; the file is not created here. Evidence:
  `cu-macos-smoke` STEP `cu.macos-ax-receipts` (the run's invoke / menu /
  focus / click / close lines listed in order, reserved before and
  completed after each, the close snapshot inside its reserved line;
  `--max 2` truncates with `truncated: true`, `--max 0` is `invalid_input`).
- [~] browser pages are reached through the platform's own web accessibility
  area (`role` WebArea and its descendants) on the same loop: `page text`,
  `query`, `invoke`, `tab list` / `tab select` need no browser extension or
  native-messaging bridge, and `browser *` stays typed `unsupported`. The
  devtools protocol is adopted only as the opt-in second knife (`page-js`,
  `page targets`, and -- because the AX tree never carries a background
  tab -- `page text / find / click / fill / nav / screenshot --target-*`, a
  port the caller opens on purpose), never as the default path. MCU
  ledger (2026-09-03): `inspect` / `find` / `read` are live aliases of
  `query`; `page read --js` / `page read` / `page targets` / `page text` /
  `page find` / `page click` / `page fill` / `page nav` / `page
  screenshot` are live under their cu spellings; `drag`, `minimize` /
  `restore`, `ps` and the non-desktop groups remain typed `unsupported`
  by name (`capabilities.verbs[*].reason` says why).
- [ ] every command carries an explicit target reference and returns a typed
  result. There is no ambient "current target" that a caller can forget to set.
- [ ] verb spellings converge with the existing AgenTerm surfaces where the
  action is the same (`screenshot`, `send-text`, `send-keys`, `send-wheel`,
  pointer verbs). A shared spelling must mean the same product action; where cu
  cannot honor an existing verb it omits it rather than shipping a weaker
  impostor. The workbench CLI contract is
  [15](PRD_02_15_command_line.md); the con contract is
  [26](https://github.com/partnernetsoftware/minicon/blob/main/prd/PRD_02_26_con_control_cli.md).
- [ ] machine-readable output is the primary interface and the human rendering
  is derived from it, never the reverse.
- [~] standalone `clipboard-read` is target-neutral and requires the Observe
  grant. Bare, `current` reads Unicode text through bounded
  `agt_clipboard_get_text` plus the host type list. `--type T` (ABI 1.23
  `agt_clipboard_get`) reads that native type as bounded bytes (default 1 MiB,
  max 16 MiB) with `sha256` and utf8/base64; `--out` writes the bytes instead
  of putting them in JSON. Empty text is success.
  It is independent of accessible-node `copy` / `paste`; clipboard content is
  absent from audit and evidence receipts. The Windows public smoke is
  non-mutating: it keeps one native Unicode-text snapshot only in memory,
  compares the command result, and never prints or persists the content. It
  does not seed then restore text because that would destroy unrelated native
  clipboard formats. ABI 1.24 `clipboard-write` / `clipboard-write-file` /
  `clipboard-clear --apply` require Actuate; clear without `--apply` is planned
  and performs nothing. Remote live evidence remains open.
- [~] standalone `pointer-move --x X --y Y` is target-neutral and requires the
  Actuate grant. `current` reuses the existing bounded
  `agt_input_pointer_move` mechanism; SSH and VNC preserve the exact signed
  32-bit coordinates while rewriting only the worker target to `current`.
  Missing, duplicate, extra, or overflowing CLI values fail typed before
  native dispatch. Success is a fixed-shape JSON result naming absolute-screen addressing and
  `button_effect:"none"`; the command performs no press, release, click, drag,
  or wheel operation. Unit evidence owns authorization, serde/CLI bounds and
  transport rewrite. Windows live smoke remains
  open because the current ABI has no independent cursor-position observation;
  moving a real user pointer without a reliable read/restore circuit is not
  acceptable evidence. Pointer press/release, drag, and wheel remain planned.
- [~] `pointer-position` supplies the independent observation half required by
  a safe pointer-move receipt. ABI 1.11 adds a null-checked, non-injecting
  query; CU requires that minor version and maps an old library or missing
  symbol to typed unsupported. Windows `GetCursorPos` and X11 `QueryPointer`
  are wired; macOS is now wired read-only too (cut 3.52, slice 4: a
  `CGEventCreate(NULL)` + `CGEventGetLocation` sample that posts no event).
  Pure/ABI/transport evidence is closed. macOS live evidence is now held:
  `cu-macos-smoke` reads `pointer-position` before and after every `click`
  and `close` and requires the same coordinates (the pointer invariant,
  `cu.macos-ax-click` / `cu.macos-ax-destructive-close`). The
  move → independent readback → restore black box stays open on Windows
  (that session cannot read its input desktop) and no macOS move is
  attempted (injection stays unsupported; only the read is wired).

## Structured observation

- [ ] a control-tree observation returns stable per-node identity, role, label,
  state and **exact bounds** — not a bitmap the caller must interpret.
- [ ] `tree` is sourced from the host's **platform accessibility backend**
  ([30 § Platform accessibility backends](PRD_02_30_cu_targets_transports.md#platform-accessibility-backends)):
  Windows native API + UIA, macOS AX (`NSAccessibility`), Linux AT-SPI2.
  `agenterm-cu` does not implement these stacks; it consumes `agenterm-platform`.
- [ ] node identity is stable enough to be re-addressed across observations, or
  the instability is reported. An agent must never silently act on a node whose
  identity has been recycled.
- [ ] where a target cannot expose a control tree, the response says so
  explicitly with typed `Unsupported` / `Failed`. Coordinate-only or
  screenshot-only operation is always visible in the result, never inferred by
  the caller.
- [x] structured `click` / `focus` by node id use the same platform a11y
  backend as `tree`. Coordinate `click` is a separate degraded path requiring
  an explicit marker; it never substitutes silently when structured actuation
  was requested. **Journey-proven on macOS `current`** (cut 3.52, slice 4,
  `cu-macos-smoke` STEP "click --node and click --name press the button ..."
  `cu.macos-ax-click`, and the slice-3 `focus` STEP `cu.macos-ax-focused`):
  `click --window H --node ID` presses the fixture button (verified by
  tree-diff, the count label advances), `focus --node ID` moves the first
  responder (verified by `focused`-readback), and both carry a
  crash-persistent receipt; `pointer-position` read before and after each
  click is unchanged (the real pointer never moves) and the focused window
  is never the fixture.
- [x] structured `click` / `focus` also accept an accessible name
  (`--window` + `--name` + optional `--role`). Resolution reuses the
  `wait --node-name-contains` matcher (showing/visible, case-insensitive
  substring; the `--role` narrowing uses the normalized role spelling,
  e.g. `button`) and then acts on the existing node-path a11y path. A miss
  is typed `a11y_node_not_found`. Two or more showing matches are typed
  `a11y_node_ambiguous` (with the match count); the command must not pick
  the first. Name addressing must not parse tree dumps, take screenshots,
  or fall through to `--coords`. A showing named node with no Action
  still uses the AT-SPI Component path and reports
  `addressing=accessibility-tree`. **Journey-proven on macOS**: cut 3.52
  `click --window H --name "Fixture Press"` presses the unique button and
  `--name "Fixture Twin"` (two showing matches) is `a11y_node_ambiguous`
  (`cu.macos-ax-click`).
- [x] `send-text` accepts the same name addressing (`--window` + `--name` +
  optional `--role`, with `--` ending flag parsing). Named write goes
  through native AT-SPI `EditableText` (`SetTextContents` / `InsertText`)
  when that interface exists, otherwise through AT-SPI `Text` plus the
  toolkit accessibility set-value (Chrome 151 and WebKitGTK 2.52 implement
  `Text` but not `EditableText`). Success is confirmed by `Text.GetText` and reports
  `addressing=accessibility-tree`. A named showing node with no writeable
  text interface typed-fails (`a11y_text_unavailable`) and never falls
  through to XTest / `input_inject::type_text`. A miss or an ambiguous
  name writes nothing and fails typed, so a loop-until caller never
  sprays text at the wrong control.
- [x] `send-text --window HANDLE` without `--name` writes that same
  AT-SPI path on the showing focused node (innermost `Text.GetText`
  candidate — the same node `get-text --window HANDLE` reads). Never
  XTest / `input_inject::type_text` when `--window` is set. Proof is
  independent `get-text --window HANDLE` (no `--name`) equal to the
  typed string, not `send-text` `matched.text`. Live hosts: agenterm-con
  named `Command` (native `EditableText`, `via=editable-text` on a
  second con that never steals the resident control socket), Chrome
  `GetTextField` (`fixtures/cu/311b-chrome-gettext.html`) after
  `focus --name`, and Reasonix composer `Message Reasonix…` under
  `scripts/reasonix-desktop-a11y.sh` (WebKit 2.52 has `Text` but not
  `EditableText`; write is AT-SPI `Text` plus the eval-helper
  set-value). Without `--window` it stays the plain "type into
  whatever is focused" inject. Do not mark this leaf shipped on
  worker JSON.
- [x] `copy --window HANDLE --name PAT [--role ROLE]` copies AT-SPI
  `Text.GetText` (`agt_a11y_node_get_text`) from the unique showing named
  node onto the native clipboard (`agt_clipboard_set_text`) and reports
  `addressing=accessibility-tree` / `via=gettext`. On Linux X11 the seed
  is a native CLIPBOARD selection owner (`SetSelectionOwner`), not
  `xclip` / `xsel`. A named showing node with no Text interface
  typed-fails (`a11y_text_unavailable`) and never falls through to XTest
  / `--coords` / screenshot. A miss or an ambiguous name copies nothing.
  Close the circuit with `paste --name` (no `--text`) then
  `wait --text-equals`; `copy` `matched.text` does not count. Live
  evidence: Chrome fixture fields and the Reasonix composer
  (`Message Reasonix…`) under `scripts/reasonix-desktop-a11y.sh`.
- [x] `copy --window HANDLE` without `--name` copies that same GetText
  path on the showing focused node (innermost `Text.GetText` candidate —
  the same node `get-text --window HANDLE` reads) onto native CLIPBOARD
  (`via=gettext`). Never XTest / `--coords` / screenshot when `--window`
  is set. Proof is independent host circuit: seed a unique string,
  `copy --window HANDLE` (no `--name`), clear the field,
  `paste --window HANDLE` (no `--name` / no `--text`), then
  `get-text --window HANDLE` (no `--name`) equals the seeded string —
  not `copy` `matched.text`. Live hosts: agenterm-con named `Command`
  after `focus --name` (`via=gettext` on copy; paste restore
  `via=editable-text` on a second con that never steals the resident
  control socket); Chrome `GetTextField`
  (`fixtures/cu/311b-chrome-gettext.html`) after `focus --name`; Reasonix
  composer `Message Reasonix…` after `focus --name` under
  `scripts/reasonix-desktop-a11y.sh` (`via=gettext`; paste restore uses
  eval-helper set-value, `via=text`). Without `--window` copy is invalid
  (no plain inject copy). Do not mark this leaf shipped on worker JSON.
- [x] `paste --window HANDLE --name PAT [--role ROLE] [--text TEXT]` writes
  the clipboard into the unique showing named field through that same
  native AT-SPI `EditableText` / `Text` path (`agt_a11y_node_set_text`)
  and reports `addressing=accessibility-tree`. `--text` only seeds the
  clipboard (`agt_clipboard_set_text`); the field write always reads
  `agt_clipboard_get_text`. On Linux X11, `--text` seeds CLIPBOARD
  through the native selection owner (not `xclip`). A named showing node
  with no writeable text interface typed-fails (`a11y_text_unavailable`)
  and never falls through to XTest / `--coords` / screenshot. A miss or
  an ambiguous name writes nothing. Close the circuit with
  `wait --text-equals`; `paste` `matched.text` does not count. Live
  Reasonix composer (`Message Reasonix…`) uses the same WebKit
  eval-helper set-value path as named `send-text`. A prior `copy --name`
  may seed the clipboard instead of `--text`.
- [x] `paste --window HANDLE` without `--name` writes that same clipboard
  path on the showing focused node (innermost `Text.GetText` candidate —
  the same node `get-text --window HANDLE` reads). Never XTest /
  `--coords` / screenshot when `--window` is set. Proof is independent
  `get-text --window HANDLE` (no `--name`) equal to the clipboard string,
  not `paste` `matched.text`. Live hosts: agenterm-con named `Command`
  (native `EditableText`, `via=editable-text` on a second con that never
  steals the resident control socket; optional `--text` seeds CLIPBOARD);
  Chrome `GetTextField` (`fixtures/cu/311b-chrome-gettext.html`) after
  `focus --name` (optional `--text` seeds CLIPBOARD); Reasonix composer
  `Message Reasonix…` after `focus --name` under
  `scripts/reasonix-desktop-a11y.sh` (eval-helper set-value, `via=text`,
  same as focused `send-text`). Without `--window` paste is invalid (no
  plain inject paste). Do not mark this leaf shipped on worker JSON.
- [x] `send-keys` accepts that same name addressing (`--window` + `--name` +
  optional `--role`, with `--` ending flag parsing). Named chords go through
  native AT-SPI Device/key events (`DeviceEventListener.NotifyEvent`) and
  report `addressing=accessibility-tree`. A named showing node with no key
  interface typed-fails (`a11y_key_unavailable`) and never falls through to
  XTest / `input_inject::send_keys`. A miss or an ambiguous name sends no
  chord at all.
- [x] `send-keys --window HANDLE` without `--name` targets the showing
  focused node (innermost `Text.GetText` candidate — the same node
  `get-text --window HANDLE` reads). Prefers `DeviceEventListener.NotifyEvent`
  (`via=device-event`). When that interface is absent (con `Command`;
  Chrome renderer entry; WebKitGTK textarea) and the payload is plain
  typeable text, writes through the same AT-SPI `EditableText` / `Text`
  path as focused `send-text`. Never XTest / `input_inject::send_keys`
  when `--window` is set. Proof is independent `get-text --window HANDLE`
  (no `--name`) equal to the typed string. Live hosts: agenterm-con
  named `Command` (native `EditableText`, `via=editable-text` on a
  second con that never steals the resident control socket), Chrome
  `GetTextField` (`fixtures/cu/311b-chrome-gettext.html`) after
  `focus --name` (`via=text`); Reasonix composer `Message Reasonix…`
  after `focus --name` under `scripts/reasonix-desktop-a11y.sh`
  (eval-helper set-value, `via=text`, same as focused `send-text`).
  Special chords without a key interface still typed-fail. Without
  `--window` it stays the plain "send to whatever is focused" inject.
  Do not mark this leaf shipped on worker JSON.
- [x] `wait --window HANDLE --name PAT [--role ROLE] --text-equals TEXT`
  (alias `--node-text-equals`) polls AT-SPI `Text.GetText` on the unique
  showing named node until that independent text equals `TEXT`. Timeout is
  typed `timeout` and reports the last GetText. `send-text` / `paste` /
  `copy` `matched.text`, a sidecar `tree` walk of snapshot `text` fields,
  `last_text_write_via`, and the WebKit eval helper's queued-job `OK` are
  not this condition.
  Never screenshot, XTest, or `--coords`. Live evidence includes Chrome
  `FixtureField` and the Reasonix composer (`Message Reasonix…`).
- [x] `wait --window HANDLE --name PAT [--role ROLE] --text-contains SUB`
  (alias `--node-text-contains`) polls that same independent
  `Text.GetText` until it contains `SUB`. Success publishes `via=gettext`
  and the full GetText (not only the substring). `send-text` / `paste` /
  `copy` `matched.text` do not count. Timeout is typed `timeout` and
  reports the last GetText. Never screenshot, XTest, or `--coords`.
- [x] `scroll --window HANDLE --name PAT [--role ROLE]` is one-shot AT-SPI
  `Component.ScrollTo(TopEdge)` (`agt_a11y_node_scroll`) on the unique
  showing named node and reports `addressing=accessibility-tree` /
  `via=scroll-to`. `--name` is required. Missing / false /
  `UnknownMethod` typed-fails (`a11y_scroll_unavailable`). ScrollTo true
  with no later independent geometry change is `a11y_scroll_no_effect`,
  not `timeout`. Never Action `scroll*`, XTest wheel, `--coords`, or
  screenshot. `matched.extents` / snapshot `node.bounds` do not count.
  WebKitGTK `ScrollTo` returns true without moving geometry; when the
  Reasonix eval helper is present the same verb applies
  `scrollIntoView({block:'start'})` (no ABI change).
- [x] `get-extents --window HANDLE --name PAT [--role ROLE]` reads
  independent AT-SPI `Component.GetExtents(Screen)`
  (`agt_a11y_node_get_extents`) for that unique showing named node.
  Snapshot `node.bounds` (hardcoded `0,0,0,0` during tree walk) do not
  count. Empty extents (w/h <= 0 or call fail) typed-fail
  (`a11y_extents_unavailable`). Never screenshot / XTest / `--coords`.
- [x] `select --window HANDLE --name PAT --start N --end M [--role ROLE]`
  is one-shot AT-SPI `Text.SetSelection(0, start, end)`
  (`agt_a11y_node_set_selection`) on the unique showing named node and
  reports `addressing=accessibility-tree` / `via=set-selection`.
  `--name` is required. Missing Text / `UnknownMethod` typed-fails
  (`a11y_selection_unavailable`). SetSelection false is
  `a11y_selection_no_effect`, not `timeout`. Miss / ambiguous keep the
  existing name codes. Never XTest, mouse-drag, `--coords`, or
  screenshot. The `select` reply is not proof.
- [x] `get-selection --window HANDLE --name PAT [--role ROLE]` reads
  independent AT-SPI `Text.GetNSelections` + `GetSelection(0)`
  (`agt_a11y_node_get_selection`) for that unique showing named node.
  The `select` reply payload does not count. Missing Text typed-fails
  (`a11y_selection_unavailable`). `n == 0` is empty success. Never
  screenshot / XTest / `--coords`. Live Chrome fixture field
  `SelectField` (`fixtures/cu/36-chrome-select.html`). Reasonix
  composer (`Message Reasonix…` under
  `scripts/reasonix-desktop-a11y.sh`) uses the same native
  `Text.SetSelection` / `GetNSelections` / `GetSelection` path — no
  eval-helper select glue (unlike `ScrollTo` / EditableText). Linux
  `agenterm-con` composer `Command` publishes those same Text methods
  (persistent publisher range; same ABI 1.8 verbs, no protocol change).
- [x] `set-caret --window HANDLE --name PAT --offset N [--role ROLE]`
  is one-shot AT-SPI `Text.SetCaretOffset`
  (`agt_a11y_node_set_caret_offset`) on the unique showing named node
  and reports `addressing=accessibility-tree` / `via=set-caret-offset`.
  `--name` is required. Missing Text / `UnknownMethod` typed-fails
  (`a11y_caret_unavailable`). SetCaretOffset false is
  `a11y_caret_no_effect`, not `timeout`. Miss / ambiguous keep the
  existing name codes. Never XTest, `--coords`, or screenshot. The
  `set-caret` reply is not proof. Live Chrome fixture field
  `CaretField` (`fixtures/cu/310-chrome-caret.html`) uses the same
  native `Text.SetCaretOffset` path — no protocol change (unlike
  `ScrollTo` / EditableText helpers). Reasonix composer
  (`Message Reasonix…` under `scripts/reasonix-desktop-a11y.sh`)
  uses that same native path — no eval-helper caret glue.
- [x] `get-caret --window HANDLE --name PAT [--role ROLE]` reads
  independent AT-SPI `Text.CaretOffset` / `GetCaretOffset`
  (`agt_a11y_node_get_caret_offset`) for that unique showing named
  node. The `set-caret` reply payload does not count. Missing Text
  typed-fails (`a11y_caret_unavailable`). Proof is independent
  `get-caret` after `set-caret --offset N` on Chrome `CaretField`
  (unfocused `CaretOffset` may be `-1`; after set it must equal `N`).
  Reasonix composer after `send-text HELLO` reports `CaretOffset=5`;
  after `set-caret --offset 2` independent `get-caret` is `2`.
  Linux `agenterm-con` composer `Command` publishes those same Text
  methods (persistent publisher caret; ABI 1.9 verbs).
- [x] `get-text --window HANDLE --name PAT [--role ROLE]` reads
  independent AT-SPI `Text.GetText` (`agt_a11y_node_get_text`) once
  for that unique showing named node — the same text authority
  `wait --text-equals` polls, without a timeout. `send-text` /
  `paste` / `copy` `matched.text`, `last_text_write_via`, the WebKit
  eval helper's queued-job `OK`, and tree snapshot `text` do not
  count. Missing Text typed-fails (`a11y_text_unavailable`). Never
  XTest / `--coords` / screenshot. Proof is independent `get-text`
  equal to what the field holds: con `Command` after `send-text
  HELLO`, Chrome fixture `GetTextField` (`fixtures/cu/311b-chrome-gettext.html`,
  prefilled), and Reasonix composer `Message Reasonix…` under
  `scripts/reasonix-desktop-a11y.sh` (WebKit 2.52 exposes `Text` on
  the composer `<textarea>` — no eval-helper get-text glue, unlike
  `ScrollTo` / EditableText helpers).
- [~] `get-text --window HANDLE` without `--name` reads the showing
  focused node (innermost `Text.GetText` candidate) and reports
  `via=gettext`. con and Reasonix already close that loop. Chrome
  `GetTextField` still needs the renderer on the same host AT-SPI bus
  the observer uses (`AT_SPI_BUS` / `AT_SPI_BUS_ADDRESS`, not a
  `GetAddress` `,guid=` owner). Do not mark this leaf shipped on
  worker JSON.
- [ ] screenshot, control tree and action results are causally identifiable
  against the same observation instant, so a caller can detect that the target
  changed underneath a plan.
- [ ] AgenTerm's own surfaces are first-class observation targets. Making
  AgenTerm a computer-use target with a real control tree is owned here jointly
  with the surfaces that publish `ui-snapshot`; this module owns the cu-side
  contract, not the publishing surfaces themselves.

## Determinism

- [~] every state transition a caller must observe is waitable with a bounded
  typed timeout. No documented workflow depends on a fixed sleep. Named
  `send-text` / `paste` / `copy` is waitable with `--text-equals` /
  `--node-text-equals` / `--text-contains` / `--node-text-contains`.
- [ ] a wait failure reports what was observed at the deadline, not only that
  the deadline passed.
- [ ] requests, responses, enumerations and transfers are size and time bounded.
  A pathological target cannot make the host allocate without limit or block
  indefinitely.

## Evidence

- [ ] the command set is proven by public black-box journeys against the real
  executable and a real target, waiting on state rather than sleeping, cleaning
  every process and file it owns.
- [ ] pure tests own command parsing, wire limits, identity/bounds
  normalization, degraded-mode selection and typed failure states.
- [ ] a layering test proves no outer layer links an inner-layer primitive
  directly, in the same spirit as the existing platform boundary gate.
