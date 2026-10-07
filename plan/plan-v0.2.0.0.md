# AgenTerm v0.2.0.0 — launcher on MiniCon, market later

Status: **preparing; owner's "开工" pending** (direction 2026-10-04)
Supersedes: the 0.1.x line and the 0.2.0 "Cockpit slice" row of
[`roadmap-0.1x-0.2x.md`](roadmap-0.1x-0.2x.md). 0.1.x is archive/reference
only (tag v0.1.19 + its Release stay); v0.1.20 is paused, unpublished.
Product truth: [`prd/PRD_03_01_launcher.md`](../prd/PRD_03_01_launcher.md).
Launcher execution: [`plan-v0.2.0.0-launcher.md`](plan-v0.2.0.0-launcher.md).
Contract partner: MiniCon `plan/plan-hostif-v1.md`, implemented in MiniCon
`fb40827`.

> Version label: product **0.2.0.0**; `Cargo.toml` carries `0.2.0` (Cargo
> accepts three components only).

## Owner intent (do not drift)

1. agenterm.com is a mini GUI (wry+tao). It checks whether MiniCon is present
   and current, asks, and with consent downloads the right MiniCon for this
   machine, then opens AgenTerm's own assistant UI.
2. Later, AgenTerm becomes the plugin & app market on top of MiniCon.
3. Whether the MiniCon core embeds unisa is undecided (waits on unisacc). That
   decision shapes the market, so **nothing in the launcher may depend on it**.

## Scope of 0.2.0.0

Launcher only. The market, plugin formats, plugin runtime and capability
extraction (CU, script engines, VNC, SQL) are **parked** until the unisa
decision; see "Parked" below. They are not milestones of this version.

```text
v0.2.0.0 — agenterm.com finds/obtains a verified MiniCon and opens the assistant UI
├─ M0 archive: index 0.1.x plans/PRD as reference; no deletions
└─ L0–L6 launcher (plan-v0.2.0.0-launcher.md)
```

## Gates

1. The launcher depends only on MiniCon `--version --json` and
   `--hostif-handshake` (already landed). Plugin manifest/index work never
   blocks L0–L6.
2. Every claim is proven by a black box against a **downloaded MiniCon release
   asset** (native first, `minicon.com` fallback), not a locally built MiniCon.
3. Courts never touch a real host GUI/browser; every external wait is bounded.
4. Public Promotion needs the owner's explicit approval.

## Boundary update 2026-10-07 (owner, via cc-minicon)

MiniCon permanently retracted harness-manage (multi-agent orchestration/
management) and the harness GUI workbench; MiniCon is now "base services +
interface provider". **Agent management, orchestration and the market all
belong to AgenTerm**, built on HOSTIF. The existing `minicon harness`
(two-tool worker, no orchestration) stays frozen in MiniCon; migrating it is
deferred until AgenTerm ships its own orchestration, and is not a 0.2.0.0
launcher milestone.

## Parked until the unisa decision (reference, not commitments)

- Agent management / orchestration surface (now AgenTerm-owned, see above).

- Plugin formats: wasm+gl sandbox → webui surface → signed native escape hatch.
- Ownership: MiniCon owns HOSTIF; AgenTerm owns plugin manifest + market index.
- Plugin manifest must declare a minimum HOSTIF version and required
  capabilities; never pin an exact MiniCon build.
- Capability extraction candidates: agenterm-cu(-provider) first;
  qjswasm/lua/script-common/wasmcore; vnc/sql later; agenterm-platform stays
  shared with MiniCon (coordinate, never fork).
