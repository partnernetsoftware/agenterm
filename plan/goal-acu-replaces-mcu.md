# Goal: ACU replaces MCU

Status: **active**

Product owners: [`prd/PRD_02_28_agenterm_cu.md`](../prd/PRD_02_28_agenterm_cu.md) and
[`prd/PRD_02_36_agenterm_qjswasm.md`](../prd/PRD_02_36_agenterm_qjswasm.md)

Capability ledger: [`plan/capability-mcu-cu.md`](capability-mcu-cu.md)

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
│  └─ current/ssh/vnc/VM targets preserve one command and result schema
├─ qjswasm execution core
│  ├─ release-critical workflows are .qjs, not Bun/TS or archived Rh
│  ├─ typed compile/host/budget/deadline/cancel failures
│  ├─ bounded output, memory, operations and concurrency
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
  M["MCU capability inventory"] --> L["machine-readable replacement ledger"]
  L --> C{"capability state"}
  C -->|native| CU["agenterm-cu mechanism"]
  C -->|delegated| F["typed owning facade"]
  C -->|gap| I["implementation slice"]
  C -->|platform-limited| P["reproduction + typed refusal"]
  CU --> Q["qjswasm workflow"]
  F --> Q
  I --> Q
  P --> Q
  Q --> H["three-host native court"]
  H --> S["six-cell sealed execution"]
  S --> D{"all replacement gates?"}
  D -->|no| L
  D -->|yes| R["switch default entry"]
  R --> X["MCU compatibility-only"]
  X --> Z["removal rehearsal"]
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

## Current observed frontier (2026-09-04)

- MCU currently exposes **79 top-level verbs**. This is only the first
  accounting dimension: `page`, `browser`, `process`, `resource`, `file` and
  other families contain independently meaningful sub-verbs and argument
  shapes that R0 must also enumerate.
- The transitional `acu.ts` adapter now keeps **32 top-level spellings** on
  MCU. Its 92-name pass-through set is not a parity count: it mixes MCU
  spellings, ACU-native spellings and group aliases. The adapter's 42 green
  tests prove lossless argv routing and honest refusal only; they do not prove
  native post-state or platform parity.
- The 33 current stays split into four implementation queues:
  - desktop closure: window activation (`focus`) and the diagnostic `ghost`
    overlay;
  - process/runtime: `exec`, `ps`, `process`, `job`, `pty`, `term`, `signal`,
    `kill`, `service`, `daemon`, `session`, `lock` and `audit`;
  - machine/system: `setup`, `doctor`, `permissions`, `caps`, `state`, `open`,
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
- The compatibility adapter now states the complete replacement goal and
  labels every `stay` as a migration gap. The flat set is still transitional;
  R0 replaces it with the machine-readable state ledger.
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
└─ [~] replace top-level STAY counting with sub-verb + argument-shape ledger
Q1 desktop closure
├─ [x] macOS snapshot/diff/hit/zoom/raise/minimize/restore native journey
├─ [ ] Linux and Windows journeys for the same verbs
└─ [ ] explicit pointer court for drag/wheel/global input; never hide degradation
Q2 fast delegated facades
├─ [ ] caps/doctor/permissions/setup and app inventory
├─ [ ] open/notify/state and terminal adoption
└─ [~] process inventory/exec/signal through bounded qjswasm/AgenTerm contracts
   ├─ [x] basic ps: pid/parent/name + bounded page through shared platform process facade
   └─ [ ] rich filters, process detail, exec, signal and lifecycle
Q3 owned runtime facades
├─ [ ] PTY/job/daemon/session/lock/audit/service
└─ [ ] file/network/storage/device/audio/resource/power/privilege
Q4 browser and platform depth
├─ [ ] remaining CDP argument shapes + MV3/Native Messaging
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
