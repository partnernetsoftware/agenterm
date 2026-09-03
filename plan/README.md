# `plan/` execution index

Product truth lives in `PRD.md` and `prd/`; source-layout truth lives only in
[`ARCHITECTURE.md`](ARCHITECTURE.md). This directory owns sequencing, risks and
evidence plans. Completed or superseded material belongs in
[`plan/archive/`](archive/).

## Current execution path

| File | Role |
|---|---|
| [`roadmap-0.1x-0.2x.md`](roadmap-0.1x-0.2x.md) | series dependency tree and memory palace |
| [`plan-v0.1.18.md`](plan-v0.1.18.md) | only active version: qualified three-host `agenterm-cu` current tier |
| [`plan-v0.1.19.md`](plan-v0.1.19.md) | next-version draft: prove the fast-change Chassis boundary |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | sole living source map and boundary catalog |
| [`goal-chassis-l1-l2-l3.md`](goal-chassis-l1-l2-l3.md) | Chassis implementation/evidence owner |
| [`goal-local-six-cell.md`](goal-local-six-cell.md) | local cross-build and runtime-court contract |
| [`goal-crate-platform.md`](goal-crate-platform.md) | platform encapsulation boundary |
| [`design-cu-multi-os-parity.md`](design-cu-multi-os-parity.md) | current CU multi-host parity design |
| [`platform-ux-parity-evidence-matrix.md`](platform-ux-parity-evidence-matrix.md) | cross-host UX evidence matrix |
| [`goal-company-windows-signing.md`](goal-company-windows-signing.md) | reusable Windows signing qualification and remaining product gate |

Capability-specific designs remain living only while their owning PRD leaf is
open. Their presence in this directory does not assign them to a version.

## Portfolio horizon, not current dispatch

- `plan-control-center-ux.md` / `design-control-center-ux.md` — 0.2.0 Cockpit
  input; no current-version authority.
- `plan-mobile.md` — product scope belongs to PRD 33; no version commitment.
- `research-decentralized-network.md` — research only.
- `design-dynacore-*`, `goal-agenterm-dyn-macos.md` — isolated internal
  mechanism research; not a release theme.

## Archive rule

1. Upsert surviving product truth into its owning PRD.
2. Move a shipped, rejected or superseded execution document into
   `plan/archive/` and add an archive banner.
3. Repair active links; archives may link to newer truth but never become it.
4. Never delete the decision record merely to make the root look clean.
5. Keep this index small enough that an agent can identify the active version
   without reading historical campaigns.
