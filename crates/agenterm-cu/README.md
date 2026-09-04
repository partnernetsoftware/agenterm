# agenterm-cu

`agenterm-cu` is AgenTerm's computer-use foundation: one binary that lets an
orchestrator agent observe and actuate a desktop through **structured data**
(window inventory, accessibility tree, typed replies), not screenshot / OCR
coordinate guessing. Every reply is one JSON line on stdout:
`{"ok":bool,"target":..,"command":..,"data":..,"error":{"code","message","detail"}}`.

This file is the task-oriented quick guide. The exhaustive per-verb reference
is generated from the binary: [`docs/agenterm-cu-verbs.md`](../../docs/agenterm-cu-verbs.md)
(`scripts/gen-cu-verbs-doc.sh`; `agenterm-cu --help` is always the truth).
Product contract: [PRD 29](../../prd/PRD_02_29_cu_command_surface.md)
(verbs), [30](../../prd/PRD_02_30_cu_targets_transports.md) (targets),
[31](../../prd/PRD_02_31_cu_authorization_safety.md) (grants / audit),
[32](../../prd/PRD_02_32_cu_window_placement.md) (`window-place`).

Every call names a target and a grant. Below, `cu` stands for
`agenterm-cu --target current --grant observe` (add `actuate` for writes).

## Living skill source (`moltbaby/skills/mcu`)

The living desktop-bridge lab is sibling-repo `moltbaby/skills/mcu`
(`bin/mcu`). This crate is the **product destination**: absorb that skill's
command set and layering lessons onto AgenTerm's command / grant /
`libagenterm` ABI; never transplant the TypeScript (clean-room:
[PRD 14](../../prd/PRD_02_14_research_provenance.md)). Verb-level status is
[`plan/capability-mcu-cu.md`](../../plan/capability-mcu-cu.md). MCU
spellings that cu honours are aliases (`inspect` / `find` / `read` ->
`query`, `elements` -> `tree --flat`, `shot`, `type`, `frame`, ...); the
rest are typed `unsupported` by name, never silently missing.

## The loop

```text
loop until goal:
  find the window            windows / apps          (section 1)
  read structured state      page text / query / tree (section 2)
  act by identity            invoke / click / send-text --node|--name (section 3)
  close the loop             verify / wait --expect  -- bounded timeouts, never sleep
```

Nothing here activates or raises a window: semantic verbs go through the
a11y tree in the background; the focused window is the same before and after.

## 1. Find the window

```bash
cu windows                                    # bare: the window array
cu windows --app Brave --title Inbox --max 5  # any filter/page flag: {windows, visited, matched, returned, offset, truncated}
cu apps --running                             # pids + window counts; --all adds installed-but-not-running
cu windows-watch --app Brave --duration-ms 3000   # appeared / disappeared / changed (poll-diff)
```

Each row carries `handle`, `ref` (`"App#handle"`, accepted by every
`--window`), `process_id`, `title`, `focused`, `minimized`, `z_index`,
`occluded_percent`, `spaces`, and for Chromium windows `browser_profile`
(parsed from the window title's ` - <App> - <profile>` suffix) so the right
profile's window can be picked without a screenshot.

## 2. Read without screenshots

```bash
cu page text --window "$H" [--max-bytes N] [--within X,Y,W,H]   # visible words in reading order
cu query --window "$H" --role AXButton,AXLink --actionable --within 0,0,900,700 --max 50
cu query --window "$H" --text "Sign in"          # substring of name or text; --text-exact / --identifier are exact
cu query --window "$H" --selector 'AXButton@Save' # MCU Role[idx] / Role@title / *@title / #desc
cu tree --window "$H" --depth 3 --flat            # numbered walk order; invoke --index addresses it
cu focused --window "$H" --role AXTextField       # the app's own focused control, without foreground
cu observe --window "$H" --duration 3 --ready-path run/observe-ready.json
```

`page text` rows are `{id, role, text, bounds}` (+ `name` when it differs,
`focused`, `actionable`); a link / button is one row. Chromium keeps a web
node's words in `AXValue` (`text`), so a `name`-only reading looks empty.
Default 16 KiB (`--max-bytes`, max 1 MiB) with a `truncated` flag.

**Truncation is a budget, not an empty page.** The platform walk is
breadth-first under a default 1000-node / depth-32 budget; on a browser
window that is spent on the tab strip and toolbar before web content (which
nests past depth 40). `query` / `tree` say so in `next_actions`; pass
`--max-nodes 6000 --depth 64` (the defaults `page text` and `unlock` use).
An idle Chromium window is not blank either: replies carry `ax` plus
`next_actions` naming a deeper `query --role WebArea`, never "screenshot" or
"install an extension".

Concurrent `poll-diff` automation must wait for `--ready-path` before it
changes the UI. The marker is atomically published only after the complete
baseline tree walk; this prevents a slow backend from reading one control
before a mutation and another control after it, then silently treating that
torn state as the baseline. The caller chooses a unique path, verifies the
marker JSON, and removes it after reaping `observe`. Observation duration
starts after readiness. Native `--mode notifications` rejects this option
until its subscription layer can provide the same ordering guarantee.

**`unlock`** (actuate) asks the application to build its full a11y tree,
then bounded re-reads. The poke is per host and the reply's `poke` field
names the one that ran: macOS sets `AXManualAccessibility` +
`AXEnhancedUserInterface` on the app and `AXManualAccessibility` on the
window plus a renderer wake; Linux flips the desktop-wide `org.a11y.Status`
switch (`IsEnabled` + `ScreenReaderEnabled` on the session-bus name
`org.a11y.Bus`) that a Chromium renderer watches before it publishes a web
tree; Windows has **no** separate poke to make, because a Chromium process
turns accessibility on when it answers `WM_GETOBJECT` for its window and
the UIA walk sends that itself (`poked: false` **with** a reason -- a
silent success there would be the lie). It reports `poked`, `grew`,
`returned_before`, `web_nodes_before` / `web_nodes_after`, `rereads` and
`poke` separately because the poke's own status is not the outcome (AppKit
calls the attribute unsupported even when it lands, and the Linux flags may
already be set; only the re-read can claim anything).

## 3. Locate a node and act

```bash
cu --grant actuate invoke --window "$H" --node "$ID" press           # id from page text / query
cu --grant actuate invoke --window "$H" --identifier field-id set-value "text"
cu --grant actuate invoke --window "$H" --name "Remember me" --role AXCheckBox set-checked true
cu --grant actuate invoke --window "$H" --focused --role AXTextField set-value "typed into focus"
cu --grant actuate click --window "$H" --node "$ID"                  # a11y press; --name PAT [--role R] also works
cu --grant actuate send-text --window "$H" --name "Search" -- hello  # writes the named field; `--` ends flags
cu --grant actuate send-text --window "$H" -- hello                  # no --name: the window's focused node
cu menu inspect --window "$H" --depth 2 --title "Save" --exact       # the menu bar, read without opening it
cu --grant actuate menu invoke --window "$H" --path 'File/Save'      # or '["File","More","Deeper"]'
cu verify --window "$H" --expect '[{"identifier":"field-id","value":"text"},{"name":"Remember me","checked":true}]'
cu wait --timeout-ms 3000 --window "$H" --expect '[{"name":"Saved","role":"AXStaticText"}]'
```

Actions: `press`, `set-value`, `select-option`, `set-checked` /
`set-expanded` (desired states: already there is `performed:false,
verified:true`), `increment`, `decrement`, `scroll-to`, `set-selection`,
plus MCU `set-selected` / `cancel` / `show-default-ui` on macOS. Every write
is read back: the reply carries `verified` with `verification.method` /
`reason` and a receipt (`before` / `after`); `press` is verified by a
whole-tree diff (`no_observable_change` otherwise).

Refusals are typed and perform nothing: two showing matches -> `ambiguous`,
none -> `a11y_node_not_found` (the mechanism write ledger is untouched, so a
miss never types into the omnibox), an action the node does not offer ->
`unsupported` (`node_action_missing`, offered actions in `detail`), an
unreadable state -> `unsupported` (`state_unobservable`), observe-only
grant -> `refused`, unknown flag -> `usage` before any tree is read. `verify`
fails closed the same way: mismatch -> `unverified`, unexposed state ->
`unsupported`; `wait --expect` polls the same matcher and a timeout carries
the last observation. Menu paths must resolve to exactly one enabled leaf
per segment (`a11y_menu_item_not_found` / `_ambiguous` / `_disabled` /
`_not_leaf`). Plain `send-text TEXT` / `send-keys` without `--window` is the
keyboard inject into whatever has focus; with `--window` it never is.

Text verbs by name (`copy`, `paste`, `select` / `get-selection`,
`set-caret` / `get-caret`, `get-text`, `scroll`, `get-extents`) share the
unique-showing-name matcher; the independent read (`get-text`, `wait
--text-equals`) is the proof, never the setter's echo.

## 4. Browser tabs and pages

**Profiles first.** One running Chromium-family instance (Brave Origin,
Brave Browser, Google Chrome) serves every profile of its user data
directory, and each profile's window carries the profile name as
`browser_profile` in the inventory. The profile workflow on the *real*
running browser (never a restart, no CDP port needed):

```bash
cu browser profiles                                         # {name, directory, last_used, windows:[handles]} from Local State
cu browser profiles --app "Brave Origin"                    # or Brave Browser / Google Chrome; others -> unsupported
cu --grant actuate browser open --profile X --url "$U"      # open -na <app> --args --profile-directory=<dir> URL on the
                                                            # running instance; reply {handle, browser_profile, title, created,
                                                            # tab_index, tab_title} (the selected tab the strip gained)
cu windows --browser-profile X                              # only that profile's windows (case-insensitive substring)
cu page text --window "$H"                                  # read the active tab; pick a row, click --node / invoke --node
cu --grant actuate tab close --window "$H" --title "$T" --exact --expect gone   # the row's own close button; verified
cu --grant actuate tab close --window "$H" --index 3 --expect gone             # same-title duplicates: the tab-list index
cu --grant actuate tab close --window "$H" --title "$T" --exact --expect gone --port 9222   # Target.closeTarget when the
                                                            # title names one page target of the instance; else the a11y path
# unknown name -> browser_profile_not_found, two hits -> browser_profile_ambiguous (candidates in error.detail)
# no window within --timeout-ms (default 8000) -> browser_window_not_found
```

`browser open` with a URL into a profile that already has a window opens
a tab there (`created: false`, the window's title changes, and
`tab_index` / `tab_title` say which tab it became); without a URL, or for
a profile with no window, a new window appears (`created: true`). `tab
close` is destructive and gated like `close`: an exact tab (`--title T
--exact`, or `--index N` from `tab list` when two tabs share a title),
the strip snapshot in the receipt, `--expect gone` read back from the
strip. macOS Chromium exposes the row's close button on the selected (or
hovered) tab only, so a background tab is selected first inside its
window (never raised), closed, and the previously selected tab is
pressed again (`selection_restored: true|false`, `select_first` in the
reply); with `--port N` a title that names exactly one page target of
the whole instance is closed by `Target.closeTarget` instead (`via:
cdp-close-target`, nothing selected), and no listener / no unique target
falls back to the a11y path (`cdp_fallback`). A keyboard shortcut is
never substituted. The real-instance gates are
`scripts/cu-brave-live-smoke.sh` (a11y only) and
`scripts/cu-brave-live-cdp-smoke.sh` (every CDP verb on a background tab
of the real instance, the selected tab and the front window checked
after each); both take `AGENTERM_CU_SMOKE_PROFILE=<name>` and refuse to
guess a profile.

**Platform evidence.** Everything in this section is proven on macOS
against a real Brave instance. Linux and Windows are **code-complete but
have no live evidence**: the pure matchers accept the AT-SPI2 / UIA role
spellings and the profile suffix that those hosts' `app_name` produces,
and that is all unit tests can show. The two journeys are written --
`scripts/cu-linux-smoke.sh` (browser section) and
`scripts/qjs/cu-windows-browser-smoke.qjs` -- and each exits with a typed
`SKIP[no_chromium_browser]` and **no** evidence id until a guest with a
Chromium-family browser runs it. Read a skip as "not run", never as a
pass.

**After a browser restart.** Quitting Brave / Chrome and starting it
again restores only the *last-used* profile's session by itself; the
other profiles' windows stay closed until something opens them. `browser
open --profile X` with **no URL** hands `--profile-directory=<dir>` to
the running instance, which makes Brave restore that profile's own last
session (its windows and tabs), so that -- once per profile -- is the
way to bring the other profiles back; a URL would open one tab instead.
`focused-window` (= `windows --focused true`) is the "front window
unchanged" read for these checks: exactly one row, or the explicit
`{focused_app, window: null}` when the frontmost app has no window in
the inventory (never an empty list).

Chromium (Chrome, Brave, Edge) publishes only the **active** tab's
`web-area`; every other tab is a tab-strip row and nothing more. The strip
is read on every backend, whatever that backend calls the roles -- macOS AX
`tab-group` / `radio-button`, AT-SPI2 and UIA both `page tab list` / `page
tab` (roles compare on their alphanumeric core, so separator and `AX`
prefix spellings all match). Two paths, neither of which raises the window:

```bash
cu tab list --window "$H"                                   # index, title, selected
cu --grant actuate tab select --window "$H" --title "Inbox" # or --index 2; verified by selected read-back
# no such tab -> a11y_tab_not_found, two title hits -> a11y_tab_ambiguous

cu page targets --port 9222                                 # CDP /json: id, url, title, type, attached, websocket
cu page targets --port 9222 --browser-profile X             # only targets whose title equals a tab title of X's window
                                                            # (profile_match: "title" -- a heuristic: CDP has no profile field)
cu page-js --port 9222 --target-title "Inbox" --expression document.title   # runs in the background tab
cu page-js --port 9222 --target-url "mail.example" --expression 'location.href'
cu page-js --port 9222 --target-id "A1B2..." --expression document.title
cu page-js --port 9222 --match "Inbox" --expression document.title          # title + URL + description
# one selector; no match -> cdp_target_not_found, more than one -> cdp_target_ambiguous
# (candidates in error.detail); no selector keeps the first page
```

**Acting on a background tab (CDP, nothing becomes active).** The AX verbs
above need the tab to be the window's active tab (`tab select` first) and
only ever see the active tab's web-area. The CDP verbs do not: they address
one page target -- a background tab in a background window included -- run
on that target's own websocket, and never call `Target.activateTarget` /
`Page.bringToFront`, so the human's front window and active tab stay where
they are (every reply says `focus_changed: false` and echoes the target).
Read, locate, then act:

```bash
cu page targets --port 9222 --browser-profile X                       # pick the tab: id, url, title
cu page text --port 9222 --match "Inbox"                              # MCU-compatible broad selector; exactly one hit
cu page text --port 9222 --target-title "Inbox"                       # same {id, role, text} rows as the AX page text,
                                                                      # backend "cdp"; id = backend DOM node id
cu page find --port 9222 --target-title "Inbox" --text "Archive"      # {node, path, tag, role, name, text, box};
cu page find --port 9222 --target-title "Inbox" --selector "input[name=q]"   #   --text lifts to the button / link
cu page find --port 9222 --target-title "Inbox" --role button --name "Send"
cu --grant actuate page fill --port 9222 --target-title "Inbox" --selector "input[name=q]" --text "hello" --clear --submit
cu --grant actuate page click --port 9222 --target-title "Inbox" --text "Archive"     # or --node N from page find
cu --grant actuate page click --port 9222 --match "Inbox" --x 120 --y 40              # viewport CSS point; trusted down/up proof
cu --grant actuate page type --port 9222 --match "Inbox" "hello"                      # existing editable focus; plaintext-redacted proof
cu --grant actuate page nav --port 9222 --target-title "Inbox" --url "https://mail.example/sent" --wait-ms 8000
cu page screenshot --port 9222 --target-title "Inbox" --out shot.png  # may be typed cdp_screenshot_unavailable in the
                                                                      # background; --activate is the explicit opt-in
# zero nodes -> cdp_node_not_found; several for click / fill -> cdp_node_ambiguous (candidates in error.detail)
# click / fill / type / nav reply performed + verified and write a receipt
```

Every CDP verb needs the browser started with `--remote-debugging-port=9222`
(or `--port N`); without a listener the reply is typed `unsupported` with
`backend: debugger-runtime-evaluate`. **That port answers any local process
and grants full page control** -- open it only while needed and restart the
browser without the flag afterwards. `page-js` refuses MAIN-world
`Function` constructors. The throwaway-browser gates are
`scripts/cu-cdp-smoke.sh` (targets / page-js selectors) and
`scripts/cu-cdp-actuate-smoke.sh` (every verb above on a background tab,
with the active target and the front window checked after each), both on a
headless profile under `mktemp` that is removed on exit.

## 5. Receipts and grants

- Observation needs `observe`, actuation `actuate`: `--grant` beats
  `AGENTERM_CU_GRANT`, sources never union, `current` is not exempt. Missing
  grant -> `refused` (distinct from `unsupported` and mechanism failures).
- Bounded persisted grants: `agenterm-cu grant create --target current
  --scopes S --ttl-ms N (--one-shot|--max-uses N)`, `grant list`,
  `grant revoke --grant-id ID`; run with `--grant-id ID` (exclusive with every
  other auth source). Local / `current` only; SSH / VNC do not forward it.
- Every authorized actuation appends to the JSONL audit log
  (`AGENTERM_CU_AUDIT_PATH`, default `~/.local/share/agenterm/cu-audit.jsonl`);
  if it cannot be written, nothing executes. Clipboard text never enters it.
- `cu receipts --window "$H" --max 50` reads the crash-persistent effect
  receipts (`<audit dir>/cu-receipts/<target>.jsonl`): `invoke` / `menu
  invoke` / `click` / `focus` / `close` / `tab select` / `tab close` /
  `browser open` write `reserved`
  before the mechanism and `completed` / `failed` after the read-back. A
  `reserved` line with no partner is the crash signature -- uncertain, never
  "did not happen".
- Destructive verbs (`close`, `app quit`) need all three of an exact target
  (`--window` bound to `--pid` / exact `--title`), `--snapshot`, and
  `--expect gone`; anything missing is `refused` (`destructive_gate`) with
  nothing performed. `tab close` carries the same gate (`--title T --exact`,
  the strip snapshot in the receipt, `--expect gone`).

## 6. When a screenshot is genuinely needed

Pixels are the last resort, needed only for canvas / WebGL content, images
without alt text, and a page whose own accessibility is switched off by the
site (QQ Mail offers `Ctrl+~` to toggle its accessible mode; until then its
rows are empty groups). Everything else is a budget or a mode (section 2).

```bash
cu query --window "$H" --within 120,300,640,480 --max 20   # first: is the region really node-less?
cu screenshot --out shot.png --window "$H"                 # then: one window, not the whole screen
cu pointer-position                                         # read-only; prove the pointer did not move
```

Keep it cheap: `--window` crops capture to one window; `--within X,Y,W,H`
on `query` / `page text` narrows the a11y read to the region, so only the
part that truly lacks nodes costs pixels. macOS gates window capture on
Screen Recording (`capabilities.permissions.screen_recording` carries the
repair path). Coordinate clicks exist only as `click --coords X,Y
--degraded` and are audited apart from a11y actuation; global pointer verbs
(`pointer-move`) are separate from the semantic knife. Read the whole
permission picture directly with `agenterm-cu --target current --grant observe
permissions`; it is the same declaration embedded in `capabilities`, lists
every gated verb and exact repair guidance, and never changes system consent.
`verbs` lists status / alias / reason per verb.

## Native accessibility mapping (按图索骥)

| Concern | macOS (`current`, live) | Linux (`current`) | Windows |
|---------|-------------------------|-------------------|---------|
| Window list | `AXUIElement` app windows + SkyLight z-order / Spaces | X11 `_NET_CLIENT_LIST` | Win32 `EnumWindows` |
| Control tree | **AX** (`NSAccessibility`), bounded walk, `AXActionNames`, `AXIdentifier` | **AT-SPI2** on D-Bus | **UIA** |
| Node identity | child-index path + role + title + bounds + identifier (`backend:"ax"`) | path id (`/0/2/5`) + role + name | automation id + runtime id |
| Semantic write | `AXPress`, `AXValue` write + read-back, `AXExpanded`, `AXIncrement` / `AXDecrement` | `Action` press / `EditableText` (`Text` + toolkit set-value for Chrome / WebKitGTK) | `Invoke` / `Value` / `Toggle` |
| Background menus / focused control | `AXMenuBar` walk + `AXPress`; `AXFocusedUIElement` | tree search (`mode: tree-search` / `state-search`, mapped) | same, mapped |
| Event stream | poll-diff (default) or `--mode notifications` (AXObserver) | poll-diff | poll-diff |
| Concurrent baseline | poll-diff `--ready-path` | poll-diff `--ready-path` | poll-diff `--ready-path` |
| Screenshot | window capture (Screen Recording TCC) | X11 `GetImage` (TrueColor only) | GDI |

Missing Accessibility permission is typed `denied` with the repair path in
`error.detail.repair`; never an empty tree, never a silent coordinate
fallback. Linux Chrome needs `scripts/box-chrome-a11y.sh`
(`--force-renderer-accessibility`) and Reasonix `scripts/reasonix-desktop-a11y.sh`.
Actions are normalized (`AXPress` -> `click`, `AXRaise` -> `focus`,
`AXShowMenu` -> `show-menu`); macOS reports two-way states (`checked` /
`unchecked` / `mixed`, `expanded` / `collapsed`) so `verify` can tell "off"
from "not observable".

## Targets, host and evidence

- `--target current` runs in-process. `--ssh user@host` and `--vnc host:port`
  run the same verbs on a remote / session `agenterm-cu --target current`
  worker (OpenSSH stdio; RFB then a local worker). `--rdp` is a placeholder:
  parses, authorizes, fails closed `rdp_unavailable`. `capabilities` declares
  each tier and keeps the public target on the reply.
- `agenterm-cu host` is the macOS / Windows desktop host (menu-bar extra,
  Spectacle-default shortcuts over `window-place`). Install with
  `./scripts/install-cu-hotkeys.sh`; Accessibility trust is per signature and
  process (`~/.local/share/agenterm/ax-status`, see the rust cheatsheet).
- Gates: `scripts/qjs/cu-macos-smoke.qjs` (AX loop against
  `examples/objc/agenterm_ax_fixture.m`), `cu-macos-web-smoke` (WKWebView),
  `cu-macos-pointer-smoke`, `scripts/cu-linux-smoke.sh`,
  `scripts/cu-linux-cross-tier-tree.sh` (current / ssh / vnc same tree),
  `cu-windows-smoke`, `scripts/cu-cdp-smoke.sh` (CDP tab addressing),
  `scripts/cu-brave-live-smoke.sh` / `scripts/cu-brave-live-cdp-smoke.sh`
  (macOS, real Brave instance). The browser/tab journeys for the other two
  hosts -- `scripts/cu-linux-smoke.sh` (browser section) and
  `scripts/qjs/cu-windows-browser-smoke.qjs` -- exist and have **not** run:
  both exit with a typed `SKIP[no_chromium_browser]` until a guest with a
  Chromium-family browser exists.
- Layering: `libagenterm` (`agt_*` exports) <- `Command` / typed `CuReply`
  <- `Executor` <- the single `agenterm-cu` binary. Product code never opens
  raw OS APIs; mechanisms report typed `Available` / `Unsupported` / `Failed`.
