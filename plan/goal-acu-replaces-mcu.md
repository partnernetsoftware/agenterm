# Goal: ACU replaces MCU

Status: **active**

Product owners: [`prd/PRD_02_28_agenterm_cu.md`](../prd/PRD_02_28_agenterm_cu.md) and
[`prd/PRD_02_36_agenterm_qjswasm.md`](../prd/PRD_02_36_agenterm_qjswasm.md)

Capability ledger: [`plan/capability-mcu-cu.md`](capability-mcu-cu.md)

Machine-readable state ledger:
[`plan/acu-mcu-capability-ledger.json`](acu-mcu-capability-ledger.json). It is
now exhaustive across 11 public capability families and R0 has zero
unclassified public shapes. This closes accounting, not implementation: every
`gap` and `platform-limited` row still needs its owning slice and court.

## Product outcome

`agenterm-cu` becomes the one installed machine/computer-use entry for agents.
It replaces the user-visible MCU/Bun runtime while reusing the correct owning
mechanism behind typed facades. MCU remains only a temporary compatibility
adapter and an experiment incubator until its retirement gates pass.

Replacement means **all useful workflows remain reachable without MCU**, not
that every MCU TypeScript module is translated into Rust. A capability may be
implemented by ACU itself or delegated to AgenTerm, qjswasm, libagenterm, or an
`agenterm-platform` adapter, but the public command, result schema, target
identity, deadline, cleanup and evidence contract belong to ACU.

## Markdown-tree DAG

```text
ACU replaces MCU
├─ capability accounting
│  ├─ every MCU public verb and sub-verb has one stable capability id
│  ├─ state = native | delegated | gap | platform-limited | retired
│  ├─ available requires named black-box evidence; catalog presence is not proof
│  └─ no permanent catch-all equivalent to acu.ts STAY
├─ one product entry
│  ├─ desktop/a11y/browser/window/input → CU + platform/libagenterm
│  ├─ PTY/process/task composition → AgenTerm + qjswasm
│  ├─ file/network/device/service/privilege → typed platform facades
│  ├─ setup/doctor/permissions → truthful probes + exact repair actions
│  ├─ daemon/session/lock/audit → one durable native ACU coordinator
│  ├─ Simulator → explicit macOS-limited typed facade
│  └─ current/ssh/vnc/VM targets preserve one command and result schema
├─ qjswasm execution core
│  ├─ release-critical workflows are .qjs, not Bun/TS or archived Rh
│  ├─ typed compile/host/budget/deadline/cancel failures
│  ├─ bounded output, memory, operations and concurrency
│  ├─ synchronous child calls default to a 60-second wall deadline
│  ├─ all child entry paths enter native containment before user code
│  └─ invocation-owned cleanup with no cross-run global state
├─ evidence
│  ├─ Win UIA, macOS AX and Linux AT-SPI2 native journeys
│  ├─ six OS/ISA execute-only delivery cells
│  ├─ MCU workflow parity corpus run against ACU
│  └─ comparative court: structure, background control, success and recovery
└─ retirement
   ├─ default docs and PATH resolve to agenterm-cu
   ├─ no production workflow imports skills/mcu or requires Bun
   ├─ acu.ts compatibility telemetry has zero unexplained stays
   └─ MCU removal rehearsal leaves all declared gates green
```

## Mermaid flowchart memory palace

```mermaid
flowchart LR
  M["MCU capability inventory"]
  M --> DESK["desktop ✓"] & PROC["process ✓"] & BROW["browser ✓"] & PTY["PTY/job/terminal ✓"] & FILE["file/storage ✓"] & NET["network ✓"] & DEV["device/audio ✓"]
  M --> RUN["service/runtime/session/audit ✓"] & SET["setup/doctor/permissions ✓"] & PRIV["privilege ✓"] & SIM["Simulator ✓"]
  DESK & PROC & BROW & PTY & FILE & NET & DEV & RUN & SET & PRIV & SIM --> L["11-family machine-readable ledger<br/>R0 accounting complete"]
  L --> C{"capability state"}
  C -->|native| CU["agenterm-cu mechanism"]
  C -->|delegated| F["typed owning facade"]
  C -->|gap| I["implementation slice"]
  C -->|platform-limited| P["reproduction + typed refusal"]
  CU --> Q["qjswasm workflow"]
  F --> Q
  I --> Q
  P --> Q
  SET --> PS["permissions status facade live<br/>same declaration as capabilities"]
  PS --> Q
  SET --> HO["host open + notification live<br/>shell-free · acceptance ≠ verification"]
  HO --> Q
  PRIV --> PW["provider wire live<br/>closed · bounded · digest ≠ consent"]
  PW --> PC["native consent + provider-owned reserve<br/>next blocking court"]
  PC --> Q
  PTY --> TL["owned tab lifecycle live<br/>new → read/send/wait → close"]
  PTY --> SE["shell-exec live<br/>contained · bounded dual-stream · exact exit"]
  PTY --> MJ["managed job live on macOS + Linux x86_64<br/>resident owner · native IPC · exact replay"]
  NET --> NI["network interfaces live<br/>native ids · stable order · bounded snapshot"]
  DESK --> DS["desktop-state live<br/>bounded · exact target · drift refusal"]
  TL --> Q
  SE --> Q
  MJ --> Q
  NI --> Q
  DS --> Q
  PROC --> SIG["exact process signal live<br/>pidfd / audit token / HANDLE<br/>delivery ≠ application acknowledgement"]
  SIG --> Q
  Q --> H["three-host native court"]
  H --> S["six-cell sealed execution"]
  S --> D{"all replacement gates?"}
  D -->|no| L
  D -->|yes| R["switch default entry"]
  R --> X["MCU compatibility-only"]
  X --> Z["removal rehearsal"]
  L --> RT["retirement critical path<br/>runtime spine → jobs → transactions"]
  RT --> D
```

## Capability-state contract

| State | Meaning | Evidence required |
|---|---|---|
| `native` | ACU owns and executes the mechanism | public command journey plus post-state |
| `delegated` | ACU calls another AgenTerm-owned mechanism through a typed facade | facade identity, timeout, error and cleanup tests |
| `gap` | required replacement capability is not implemented | owning slice and safe typed failure |
| `platform-limited` | the OS/backend cannot supply the promised behavior | reproducible court evidence and an alternative path when one exists |
| `retired` | obsolete MCU behavior intentionally has no successor | user-impact review and migration note |

`unsupported` is a runtime result, not a roadmap state. It must map to either a
temporary `gap`, a proved `platform-limited` cell, or a reviewed `retired`
behavior. A group or verb appearing in `capabilities` does not make it shipped.

## Current retirement frontier (2026-09-05)

The active cut is deliberately narrow: clear every unexplained compatibility
`STAY`, then pass the MCU-absence court. General qjswasm optimization and new
product branches do not pre-empt these blockers.

```text
MCU retirement blockers
├─ [x] desktop state: bounded native desktop-state + state alias
├─ [~] shared runtime spine
│  ├─ [x] session create/list/status/renew/end + target lock acquire/list/release/expiry sweep
│  ├─ [x] audit query with bounded result/scan/byte budgets
│  ├─ [x] audit retention: plan/apply + atomic bounded compact + qjswasm court
│  ├─ [x] MCU-shaped session/lock/audit-query/compact compatibility routes to ACU
│  └─ [ ] daemon ownership
├─ [~] managed job facade
│  ├─ [x] private crash-safe identity/state registry; no command/env/lease persistence
│  ├─ [x] contained owner core + dual bounded stdout/stderr cursor rings
│  ├─ [x] cross-platform owned stdin pipe; drop is explicit EOF, not process stop
│  ├─ [x] closed current-user native IPC + opaque bounded endpoint + resident expiry loop
│  ├─ [x] detached launcher + public spawn/list/status/events/write/wait/renew/stop
│  ├─ [x] exact request replay returns the same public job identity without a second spawn
│  ├─ [x] session-end closes admission, stops every bound job, releases locks and retries idempotently
│  ├─ [x] macOS public-process lifecycle: dual output, write/EOF, wait, renew and stop
│  ├─ [x] registered qjswasm journey green on macOS
│  ├─ [x] same journey green on Linux x86_64 execute-only court
│  └─ [ ] Linux aarch64 and Windows journeys green
├─ [~] machine transactions
│  ├─ [x] recoverable file copy: marker-owned no-replace publication + crash recovery
│  ├─ [~] recoverable file move: macOS qjswasm green; Linux/Windows courts pending
│  ├─ [~] process set-state: exact-object stop/resume on macOS/Linux; Windows typed gap
│  │  ├─ [x] start-identity gate + retained pidfd/audit-token effect authority
│  │  ├─ [x] scheduler read-back + closed success/failure receipt + public qjswasm court
│  │  └─ [ ] Linux native and Windows typed-refusal courts
│  ├─ [~] process signal: exact-object closed signal set; macOS qjswasm green
│  │  ├─ [x] stale identity refusal + pidfd/audit-token/HANDLE effect authority
│  │  ├─ [x] TERM/KILL exit, STOP/CONT state and generic-delivery truth separated
│  │  ├─ [x] MCU single-process unprivileged shape routes to ACU
│  │  └─ [ ] Linux/Windows native courts; privileged shapes remain gaps
│  ├─ [~] tree-signal crash recovery
│  │  ├─ [x] receipt reserved before freeze; handled failure restores retained objects
│  │  └─ [ ] restart recovery resolves or reports every member after owner death
│  ├─ [~] host dispatch: open + notification typed; macOS qjswasm green, Linux/Windows courts pending
│  ├─ MV3 browser bridge and managed profile/window lifecycle
│  ├─ [~] privilege plan/broker/OS consent
│  │  ├─ [x] read-only `process.set-priority` plan on macOS/Linux: exact start identity + before/after + expiry + dual digest
│  │  ├─ [x] public qjswasm `cu.privilege-plan`; Windows names the semantic gap instead of fabricating Unix nice
│  │  ├─ [x] closed protocol-v1 provider wire: deny unknown/tampered/expired/non-current requests; digest means intent, never consent
│  │  ├─ [x] provider-owned replay ledger: namespace + OS-principal digest + request id; reserved crash never reopens effect
│  │  └─ [ ] native consented provider, one-shot postcondition, TTL broker and Windows priority-class contract
│  └─ CoreSimulator plus required device/service operations
├─ [ ] classify and remove every remaining argument-shape fallback
└─ [ ] MCU-absent three-host parity + six-cell delivery rehearsal
```

`acu.ts` now has **18 top-level `STAY` spellings**. This is not the remaining
capability count: group verbs contain multiple independently gated shapes. The
machine-readable ledger, not the top-level number, decides retirement.

Recoverable `file move` is now the same proven copy-transaction ownership rule,
not a second loose file mechanism: the random marker and both backup names use
atomic no-replace publication; the temporary stays on one opened read/write
object through identity persistence and copying; source and destination path
locks are acquired in stable order. The macOS qjswasm journey is green.
Linux/Windows native evidence remains required.

The unprivileged `process set-state PID running|stopped` shape has left the
sub-verb fallback ledger. ACU freezes the public start identity, opens one
retained native object, performs stop/resume through Linux pidfd or the macOS
audit token, and reads scheduler state back. Every post-effect failure closes
its durable receipt as performed-but-unverified. Windows refuses the operation
because no documented exact-process suspension primitive is currently owned;
MCU `--sudo/--broker` shapes remain behind the privilege-provider branch.

The unprivileged single-process and bounded-tree `signal PID SIGNAL` shapes
have also left `STAY`. ACU opens native process objects before binding start
identities, reserves before effect, and separates observable postconditions
from mere delivery: exit and scheduler transitions can be verified, but
HUP/INT/USR application meaning cannot. Unix tree mode freezes until two exact
snapshots agree, delivers deepest-first, and restores only members that were
running before the freeze. The macOS public qjswasm court is green. Sudo and
broker shapes remain on MCU until the native consent contract lands.

The latest removed fallbacks are `session`, `lock` and `audit`; all their MCU
public shapes now rewrite onto the native ACU runtime spine. `audit compact`
defaults to a read-only plan and applies only under explicit actuation, sharing
the append lock and publishing bounded retained bytes atomically. The adapter
suite is green at 69 tests, and the platform-neutral public qjswasm journey
proved create/status/acquire/list/query/retention/release/end on macOS.

The preceding removed fallback is `state`. `desktop-state` (also spelled `state`)
composes one complete bounded window inventory, one exact window accessibility
tree and pointer position without a screenshot. It refuses ambiguous focus,
unknown handles, inventories over 512 windows, and target drift during capture;
tree truncation remains explicit. The macOS public CLI is live; Linux and
Windows journey evidence is still required before the row is three-host green.

The legacy `acu caps` spelling now projects directly onto ACU `capabilities`.
It no longer keeps MCU's private capability manifest alive; live mechanism
depth that ACU has not proved remains explicit in the replacement truth table.

The legacy `open PATH_OR_URL [--app APPLICATION] [--bg]` spelling now routes
to ACU `host-open`. Its platform facade never invokes a shell, rejects
option-like/NUL/oversized values before dispatch, uses the native registered
application mechanism, and writes a pre-effect receipt containing only byte
lengths and digests. Native dispatcher acceptance is deliberately reported as
`performed=true, accepted=true, verified=false`; it is not proof that the
handler rendered or consumed the target. A no-window macOS `.app` fixture is
green through the public qjswasm court; Linux and Windows handler-owned courts
remain before three-host promotion.

`notify TITLE [BODY] [--subtitle TEXT] [--sound]` now routes to
`host-notify`. Text is bounded and passed as argv data rather than shell or
AppleScript source; durable evidence contains lengths and SHA-256 only. A
normal macOS Notification Center dispatch is green through
`cu.host-notify.macos`, while the reply remains `verified=false` because user
presentation and attention are not observable. Linux and Windows mechanisms
compile in both ISAs and await their native courts; unsupported subtitle/sound
shapes fail typed instead of being ignored.

## Historical observed frontier (2026-09-04)

- MCU currently exposes **79 top-level verbs**. This is only the first
  accounting dimension: `page`, `browser`, `process`, `resource`, `file` and
  other families contain independently meaningful sub-verbs and argument
  shapes that R0 must also enumerate.
- The transitional `acu.ts` adapter now keeps **31 top-level spellings** on
  MCU. Its 97-name pass-through set is not a parity count: it mixes MCU
  spellings, ACU-native spellings and group aliases. The adapter's 45 green
  tests prove lossless argv routing and honest refusal only; they do not prove
  native post-state or platform parity.
- The 31 current stays split into four implementation queues:
  - desktop closure: window activation (`focus`) and the diagnostic `ghost`
    overlay;
  - process/runtime: `exec`, `process`, `job`, `pty`, `term`, `signal`,
    `kill`, `service`, `daemon`, `session`, `lock` and `audit`;
  - machine/system: `setup`, `permissions`, `caps`, `state`, `open`,
    `notify`, `resource`, `power`, `login-session`, `storage`, `file`,
    `network`, `device`, `audio`, `privilege` and `desktop-helper`;
  - platform product: `simulator`.
  This grouping is sequencing, not permanent ownership. Every retained item
  must leave `STAY` before R4/R6 can pass.
- The ACU catalog already names the broad desktop and machine groups. PTY,
  process, resource, power, login-session, storage, file, network, device,
  privilege, runtime, desktop-helper and simulator are currently mostly typed
  refusals. Under this goal those declarations are **gaps to close through
  facades**, not permanent ownership exclusions.
- The first desktop closure implementation is integrated. MCU-shaped
  `raise`/`minimize`/`restore`/`hit`/`snapshot`/`diff` now rewrite to ACU, and
  ACU-native `drag`/`zoom` are reachable through the compatibility entry.
  MCU-shaped `drag` remains a gap because it promises background-local input
  while the current ACU path requires explicit degraded global-pointer
  admission; MCU-shaped `zoom` remains a gap because its window-local corner
  coordinates and percentage padding are not the same contract as ACU's
  screen rectangle and pixel padding. Both fail typed instead of silently
  changing behavior.
- The browser endpoint gap is closed at the compatibility boundary. All ACU
  CDP page verbs accept either `--port N` or `--pid PID`; the PID route binds
  the process start identity around a bounded native command-line read,
  extracts only an explicit debugging-port flag, never scans, and never
  publishes the command line. `scripts/cu-cdp-smoke.sh` proves the real PID
  route against a throwaway headless browser; the MCU-shaped adapter now
  forwards it instead of retaining the call.
  A native Windows ARM64 UTM court also returned the owned Edge page through
  this PID route. `scripts/qjs/cu-windows-browser-smoke.qjs` now owns that
  assertion for the next integrated Windows journey. Linux ARM64 and x86_64
  court probes found no Chromium-family browser, so their runtime evidence
  remains open with the prerequisite named instead of being skipped as green.
  The Windows x86_64 court currently fails earlier at its interactive-job-agent
  registration; QGA can execute `cmd.exe` and transfer files, but its
  PowerShell child produced no result receipt. That cell is infrastructure
  unavailable, not a failed ACU assertion, and remains open until the court
  channel is repaired or the GitHub native runner executes the journey.
- macOS now has native journey evidence for `snapshot`/`diff`, `hit`/`zoom`,
  `raise` and gated `minimize`/`restore`. The restore step exposed a real
  platform bug: `kCGWindowListOptionIncludingWindow` alone returned no owner
  row after minimize. The adapter now filters `kCGWindowListOptionAll` by the
  stable `CGWindowID`; the journey proves minimize, off-screen lookup, restore,
  foreground preservation and owned cleanup together. Linux and Windows are
  still required before this tranche is three-host complete.
- The first non-desktop facade is integrated: `ps --pid/--parent/--name` with
  bounded pagination now calls `agenterm-platform::process::list`, the same
  native mechanism used by qjswasm `process.list`. Richer MCU process filters
  remain explicit gaps; the compatibility router forwards the proven subset
  and temporarily falls back for unsupported shapes.
- The second process slice is integrated: `process-state --pid N` returns
  `live|dead|unknown`, a platform start identity where available, and
  `verified=false` for unknown evidence. The compatibility router maps MCU
  `process state N` to this facade. This identity is a prerequisite for later
  signal/kill operations; a reusable PID alone is never sufficient.
- `process-usage --pid N` adds a one-shot cumulative CPU, resident-memory and
  page-fault sample. It reads the start identity before and after the sample,
  fails if they differ, and encodes wide counters as decimal strings so a
  JavaScript/qjswasm consumer cannot silently round them. `--watch-ms` adds a
  monotonic bounded series with independent interval/sample ceilings and binds
  every sample to that first identity. MCU `process usage N [--watch ...]`
  can now route here when no privilege-only shape was requested.
- `process-wait --pid N --start-identity ID --timeout-ms N` retains a native
  process object (pidfd/kqueue/Windows HANDLE), verifies the caller's prior
  identity, and waits monotonically for that exact object. A live timeout is a
  verified result. This is intentionally stronger than MCU's repeated PID
  inventory polling; the native ACU spelling is reachable through the shim.
- `process-kill` now owns an exact native mutation on all three hosts: Linux
  signals its pidfd, Windows terminates its retained HANDLE, and macOS signals
  a retained task audit token whose pidversion is checked by XNU. The macOS
  public qjswasm journey proves graceful exit and reaping; arbitrary signals
  and bounded tree termination remain separate gaps.
- The three public native journeys now bind those commands to each owned GUI
  fixture. macOS exact source `986863c0` is live green at 40 STEP / 41 evidence
  ids in 28.504 s. Linux x86_64 is
  also exact-SHA green at 24 / 24 STEP and 25 / 25 evidence ids: the atomic
  ready marker, real owned-child exit, accessibility observation and cleanup
  all passed in 54.106 s. This proves the integrated source state, not a final
  explanation for the previous mixed-time AT-SPI snapshot; complete poll/error
  accounting remains in the failure bundle so a recurrence is diagnosable.
  Windows x86 now crosses the public no-console `.com` entry and the recovered
  interactive UTM worker. The current-source journey proves capabilities,
  fixture identity, process state/usage/watch/wait, UIA tree/query/invoke and
  background menu inspection before stopping at the focus court. Two false
  infrastructure/test failures were removed: the worker now synchronizes
  asynchronous Scheduled Task creation with a nonce receipt, and the lifecycle
  probe uses an inbox native short process rather than cold PowerShell startup.
  UIA root resolution retries only named transient HRESULTs within the existing
  action budget. The remaining result is a truthful product/court boundary:
  the background desktop reports `focus performed=true` but focused read-back
  remains false, so the 16-evidence journey stays red rather than claiming an
  unverified action.
- `process-watch` closes MCU's lifecycle-observation shape with a stronger
  identity contract: composable PID/parent/name filters or explicit all, an immediate
  bounded baseline, and started/exited events keyed by PID plus start identity.
  Duration, interval, event count and matched inventory are separately capped;
  broad selectors report incomplete identity coverage rather than emitting
  PID-only rows;
  macOS public-CLI evidence has observed a real owned child exit. The three
  qjswasm native journeys now declare the same event and await integrated runs.
- `process-argv` closes one more MCU sub-verb without leaking its default
  payload. Linux reads NUL-delimited `/proc` entries, macOS consumes exactly
  the native `argc`, and Windows reconstructs the documented native command-line
  lexical projection. ACU brackets the read with the same process start
  identity, pages at a hard 4,096-row ceiling, and returns only index, byte
  length and SHA-256 unless the caller explicitly supplies `--values`. The MCU
  compatibility adapter routes this exact shape to ACU; three qjswasm journeys
  declare the evidence, with integrated Linux/Windows reruns still open.
- The PTY/job/terminal family is now fully classified by public shape. The
  existing AgenTerm session/tab kernel is the owner for product-terminal
  create/close, inventory, capture, input and deterministic waits. The typed
  ACU facade now reaches all six through one scope+epoch+`@tab` identity and
  verifies lifecycle effects by inventory read-back; the registered qjswasm
  journey owns the public assertion and awaits the next three-host run.
  The first headless layer is also live: `pty-start/status/read/send/wait/wait-exit/stop`
  map one validated job name to one isolated zero-UI server instance and exact
  epoch+`@tab` identity. A macOS race court proved one owner under concurrent
  start, loss-aware output continuation, literal input, cross-page exact wait,
  exact exit mismatch, and verified shutdown. Reuse/list/prune, events/screen projection, stale registry
  reclamation and process-group control remain open; one process is not claimed
  as job-group coverage.
- `shell-exec` now closes MCU's synchronous `exec <command...>` shape without
  overloading ACU's transport-only `exec --json`. The root is contained before
  its first instruction; stdout and stderr drain concurrently under one shared
  ceiling; timeout/output-limit paths kill and reap or return
  `cleanup_uncertain`. A nonzero nested exit is a completed typed result, and
  the compatibility router propagates that code. Audit rows retain only counts,
  timing and exit facts, never command output. macOS and Windows arm64 public
  runs are green; Linux and Windows x86_64 promotion evidence remains open.
- File/storage accounting is complete. Existing platform primitives for stable
  entry identity, no-overwrite publication and per-volume capacity are useful
  building blocks, but they do not yet constitute MCU's recoverable copy/move
  transaction or physical-device inventory. Unix modes/xattrs and Windows
  ACLs/attributes remain distinct platform vocabularies; parity must not be
  manufactured by renaming one as the other.
- Network accounting is complete across interfaces, routes, DNS, sockets and
  DNS+TCP probes. `network-interfaces` now exposes a bounded native snapshot:
  Unix uses `getifaddrs` plus ifindex, Windows uses `GetAdaptersAddresses` plus
  adapter LUID, and ACU preserves stable ordering, explicit unavailable fields,
  a 10,000-record scan ceiling and a 1 MiB response ceiling. MCU-shaped
  `network interfaces [--max N]` routes to it; Linux and Windows public runtime
  courts remain before promotion from `platform-limited` to `native`.
  The active qjswasm/tinyvm host surface still has no generic TCP or DNS API;
  historical catalog names are not implementation. Routes/DNS remain native
  platform work. Process-owned socket rows now join one native fd to a snapshot
  bracketed by the same start identity; global/name-selected socket inventory
  and watch/diff remain open rather than reusing a naked PID.
- Device/audio accounting is complete across peripheral inventory and events,
  exclusive device leases, byte I/O, serial configuration and default-output
  state. A path alone is never durable device identity, and audio backends stay
  explicitly platform-limited until each native court proves them.
- R0 accounting is complete across all 11 families. Runtime/service/session/
  audit contains user/system services, native coordinator, login service,
  leases, target locks, request idempotency, desktop delivery, audit
  query/retention/replay and console-session locking. Setup/doctor/permissions,
  the privilege broker chain and all CoreSimulator device/app/deployment/
  foreground/capture shapes are separately classified. An empty
  `remaining_families` means there is no hidden command family; it does not
  turn any gap into an implementation.
- The compatibility adapter now states the complete replacement goal and
  labels every `stay` as a migration gap. The flat set is still transitional;
  R0 replaces it with the machine-readable state ledger.
- `doctor` is the first setup-family spelling to leave `STAY`: MCU-shaped
  `acu doctor` now routes to the ACU-native bounded diagnostic. Adapter tests
  and a live macOS invocation are green; this does not promote `setup` or
  permission-opening mutations.
- Existing adapter tests prove only honest argv rewriting. They do not prove
  the target ACU mechanism, post-state, cleanup, platform parity or MCU
  independence; those belong to R1-R6.

## Work tranches

1. **Ledger court** — replace the flat `STAY` concept with the state contract
   above; account for sub-verbs and parameter shapes, not only top-level names.
2. **Desktop closure** — snapshots/diffs, hit testing, drag/wheel, window
   lifecycle, browser/CDP operations and deterministic waits on all three OSes.
3. **Machine facade** — make PTY, process, file, network, device, service,
   privilege and VM operations reachable through ACU without duplicating their
   owning kernels.
4. **qjswasm closure** — run the parity corpus under bounded qjswasm, then fix
   measured language/host-operation gaps without raising limits to hide cost.
5. **Delivery court** — native three-host journeys, six-cell sealed execution,
   package identity and failure cleanup.
6. **Retirement court** — switch examples/default PATH, remove Bun from the
   production dependency graph, rehearse MCU absence, then archive the shim.

## Ordered execution queue

```text
Q0 truthful boundary
├─ [x] runtime/help text: no useful capability is described as permanently MCU-owned
└─ [x] replace top-level STAY counting with sub-verb + argument-shape ledger
   ├─ [x] process family accounted shape by shape in the JSON ledger
   ├─ [x] desktop family accounted shape by shape in the JSON ledger
   ├─ [x] browser family accounted shape by shape in the JSON ledger
   ├─ [x] PTY/job/terminal family accounted shape by shape in the JSON ledger
   ├─ [x] file/storage family accounted shape by shape in the JSON ledger
   ├─ [x] network family accounted shape by shape in the JSON ledger
   ├─ [x] device/audio family accounted shape by shape in the JSON ledger
   ├─ [x] service/runtime/session/lock/audit family accounted shape by shape
   ├─ [x] setup/doctor/permissions family accounted shape by shape
   ├─ [x] privilege family accounted shape by shape
   └─ [x] Simulator family accounted shape by shape
Q1 desktop closure
├─ [x] macOS snapshot/diff/hit/zoom/raise/minimize/restore native journey
├─ [~] Linux and Windows journeys for the same verbs
│  ├─ [x] Linux exact-3390 rerun: 24/24 STEP · 25/25 evidence · cleanup green
│  └─ [~] Windows x86 UTM: public `.com` + process/UIA path live; background focus court remains typed-red
└─ [ ] explicit pointer court for drag/wheel/global input; never hide degradation
Q2 fast delegated facades
├─ [~] caps/doctor/permissions/setup and app inventory
│  ├─ [x] permissions: read-only platform state + gated verbs + repair guidance
│  ├─ [ ] permissions: required/optional three-host evidence + open-next action
│  ├─ [~] doctor: bounded read-only health receipt; local CLI green, three-host pending
│  └─ [ ] setup: idempotent launcher/runtime repair
├─ [~] open/notify/state and terminal adoption
│  └─ [x] state → bounded native desktop-state; open/notify/adoption remain
└─ [~] process inventory/exec/signal through bounded qjswasm/AgenTerm contracts
   ├─ [x] basic ps: pid/parent/name + bounded page through shared platform process facade
   ├─ [x] process-state: live/dead/unknown + stable start identity, observe-only
   ├─ [x] process-usage: one-shot or bounded identity-bound series, lossless counters
   ├─ [x] process-wait: prior identity + native exact-object reference + monotonic timeout
   ├─ [x] process-watch: bounded baseline + identity-safe started/exited diff
   ├─ [x] exact-object process signal/tree: closed names + typed postcondition semantics
   ├─ [~] process fds/maps/threads: native one-shot + public qjswasm macOS court ✓
   │  └─ [ ] bounded watch/diff + Linux native and Windows typed-refusal courts
   ├─ [~] process sockets: native one-shot + public qjswasm macOS court ✓
   │  └─ [ ] bounded watch/diff + Linux native and Windows typed-refusal courts
   └─ [ ] remaining rich filters and privileged mutation
Q3 owned runtime facades
├─ [~] PTY/job/daemon/session/lock/audit/service
│  ├─ [x] typed ACU terminal-new/close/list/read/send/wait facade over stable scope+epoch+tab identity
│  ├─ [~] terminal lifecycle: macOS registered qjswasm journey green; Linux/Windows courts pending
│  ├─ [x] terminal-snapshot/events: structured screen + loss-aware epoch/sequence cursor
│  ├─ [~] terminal cursor qualification: macOS registered qjswasm journey green; Linux/Windows pending
│  ├─ [x] terminal-output: retained raw bytes + absolute cursor + typed gap/future failures
│  ├─ [~] raw-output cursor qualification: macOS registered qjswasm journey green; Linux/Windows pending
│  ├─ [~] portable owned headless PTY: existing agenterm server selected as sole owner
│  │  ├─ [x] decision court: cross-process, zero-tab, cursor continuation, shutdown green
│  │  ├─ [x] public pty-start/status/read/send/wait/wait-exit/stop + exact identity
│  │  ├─ [x] literal input + loss-aware cross-page exact-output wait
│  │  ├─ [x] exact-source `a6a1c7b9` local six-cell qjswasm public court; macOS x86_64 via Rosetta
│  │  ├─ [x] concurrent-start single-owner + typed exit mismatch + verified shutdown court
│  │  └─ [~] list/prune ✓; reuse + orphan process-tree cleanup pending
│  ├─ [~] durable-job screen/event projection
│  │  ├─ [x] pty-snapshot: sole-tab structured screen + exact job/scope/epoch/tab cursor
│  │  ├─ [x] pty-diff: persisted identity-bound rows/metadata + bounded atomic advance
│  │  ├─ [x] pty-events: same-epoch continuation + all-scanned cursor advance
│  │  ├─ [x] pty-resize: temporary lease + exact grid/epoch/tab read-back + detach proof
│  │  ├─ [x] local macOS public qjswasm snapshot → resize/diff → output/diff → event continuation → restart refusal
│  │  └─ [ ] enlarged journey six-cell rerun
│  ├─ [x] lease-owned registry, streams and explicit session-end cleanup
│  └─ [ ] adopt/prune, aggregate resources, policy/priority/state/signal and expiry-detach shape
└─ [~] file/network/storage/device/audio/resource/power/privilege
   ├─ [x] bounded identity-aware network-probe
   ├─ [~] network-interfaces: native ifindex/LUID + bounded stable snapshot
   │  ├─ [x] macOS public CLI schema/count/prefix court
   │  ├─ [x] Windows x86_64 + arm64 compile/Clippy
   │  └─ [ ] Linux + Windows native public runtime courts
   ├─ [~] file-inspect: no-follow final entry + bounded metadata + stable identity
   │  ├─ [x] macOS public qjswasm journey: 41 STEP / 42 evidence + cleanup
   │  ├─ [x] Linux x86_64 focused native UTM court: exact-byte pair + file/link/missing
   │  ├─ [x] Windows x86_64 cargo-xwin compile
   │  ├─ [x] Windows x86_64 focused native UTM court: exact-byte pair + file/missing
   │  └─ [~] Linux + Windows focused leaves await full qjswasm journey promotion
   ├─ [~] recoverable file-copy transaction: plan/apply/status/rollback/recover/finalize
   │  ├─ [x] receipt-before-effect + exact object/content snapshots + destination lock
   │  ├─ [x] retained replacement backup; changed post-state refuses rollback/finalize
   │  ├─ [x] qjswasm public macOS journey `cu.file-copy-transaction`
   │  ├─ [x] MCU adapter routes copy and explicit applied transaction actions
   │  └─ [ ] Linux + Windows native public journeys through independent `utm-court`
   ├─ [~] recoverable file-move transaction: copy then retire with two retained backups
   │  ├─ [x] both path locks + atomic no-replace marker/backup publication
   │  ├─ [x] rollback/finalize + installed/retirement crash recovery
   │  ├─ [x] qjswasm public macOS journey `cu.file-move-transaction`
   │  └─ [ ] Linux + Windows native public journeys through independent `utm-court`
   ├─ [~] privilege plan: read-only closed operation before any administrator boundary
   │  ├─ [x] `process.set-priority` binds exact process start identity and two stable priority reads
   │  ├─ [x] contract digest excludes time; approval digest binds issue/expiry; mutation is always false
   │  ├─ [x] macOS public qjswasm journey `cu.privilege-plan`
   │  ├─ [x] provider wire rejects unknown fields, digest tamper, expiry and non-current scope before consent
   │  ├─ [x] provider-owned replay court: exact completion replays; reserved/unknown never dispatches twice; changed fingerprint conflicts
   │  └─ [ ] native consent/apply provider + Linux/Windows public courts; Windows nice mapping remains typed unavailable
   ├─ [~] process scheduler state mutation
   │  ├─ [x] macOS/Linux exact retained-object stop/resume + native state read-back
   │  ├─ [x] qjswasm public journey `cu.process-set-state`; stale identity cannot mutate
   │  ├─ [x] MCU unprivileged shape routes through observe-identity then ACU effect
   │  └─ [ ] Linux court + Windows typed-refusal court; privileged shapes stay with broker work
   ├─ [~] exact-object process signal and bounded tree
   │  ├─ [x] HUP/INT/TERM/KILL/STOP/CONT/USR1/USR2 closed portable set
   │  ├─ [x] public macOS qjswasm journey `cu.process-signal` includes root + two descendants
   │  ├─ [x] MCU unprivileged single/tree shapes route without naked-PID mutation
   │  ├─ [ ] owner-death recovery court for an interrupted freeze/delivery
   │  └─ [ ] Linux native court + Windows typed-refusal court; privileged shape remains
   └─ [ ] remaining transaction/device/service facades follow the machine ledger
Q4 browser and platform depth
├─ [x] CDP core live
│  ├─ [x] page hover: trusted mousemove target read-back; MCU positional shape routed
│  ├─ [x] page scroll: owned-container scroll event + offset read-back; MCU positional shape routed
│  ├─ [x] page drag: trusted down/held-move/up read-back; release cleanup; MCU positional shape routed
│  ├─ [x] page dialog: opening/closed event proof; prompt contents redacted; MCU shape routed
│  ├─ [x] page files: exact FileList read-back; bounded regular non-symlink inputs; paths redacted
│  ├─ [x] page pixel click: frozen viewport hit + trusted down/up read-back + release cleanup
│  ├─ [x] page current-focus type: editable preflight + same-focus/value-growth proof; plaintext redacted
│  └─ [x] MCU --match: title+URL+description; unique or typed ambiguity; routed for lossless page shapes
├─ [~] browser control without a pre-opened CDP port
│  ├─ [~] owned browser-session: public lifecycle + macOS live cleanup ✓; Windows exact-Job first instruction win-x86/ARM64 ✓; Win ARM64 managed-Job Edge ready→status→stopped→removed ✓; Linux + descendant-kill courts pending
│  ├─ [~] MV3/Native Messaging: protocol v1 framing/validation core; host/extension/installer pending
│  └─ [x] no fake attach: an existing process without a startup debug endpoint stays AX-only
├─ [ ] Simulator facade
└─ [ ] current/ssh/vnc/VM schema parity
Q5 retirement
└─ [ ] parity corpus + three-host native + six-cell + MCU-absent rehearsal
```

`moltbaby/skills/mcu/acu.ts` is only the transition router. Its `stay` result
means “ACU cannot yet express this exact public shape; use MCU temporarily,”
not “this capability belongs to MCU forever.” A same-named command also stays
when forwarding would silently change meaning, such as window activation vs.
node focus or shell execution vs. one JSON CU command. The ledger and queues
above turn every such honest refusal into owned removal work.

## Hard gates

- **R0 Accounting:** zero unclassified MCU public command shapes.
- **R1 Reachability:** every retained capability is `native` or `delegated`, or
  has cell-specific `platform-limited` evidence; no required item remains `gap`.
- **R2 Behavior:** the parity corpus proves success, refusal, timeout and cleanup
  through the ACU public interface.
- **R3 Portability:** Win/macOS/Linux native courts pass and all six sealed
  artifacts execute; a compile-only result cannot satisfy this gate.
- **R4 Independence:** production workflows do not import MCU TypeScript and do
  not require Bun.
- **R5 Superiority:** comparative evidence demonstrates at least structured
  identity, background-safe actuation, deterministic waits, typed recovery,
  auditable receipts and remote-target reuse. Marketing claims cannot substitute.
- **R6 Removal:** running the full declared gate set with MCU unavailable remains
  green before the compatibility layer is archived.

## Non-goals

- Do not translate MCU TypeScript line by line.
- Do not fork PTY, process, filesystem or platform kernels inside the CU crate.
- Do not call a typed refusal feature parity.
- Do not weaken qjswasm budgets or action verification to make a court green.
- Do not claim superiority from a command-count comparison.
