# AgenTerm v0.2.0.0 — plugin & app marketplace on MiniCon

Status: **draft (owner direction 2026-10-04), not started**
Supersedes: the 0.1.x line and the 0.2.0 "Cockpit slice" row of
[`roadmap-0.1x-0.2x.md`](roadmap-0.1x-0.2x.md). 0.1.x is archive/reference only;
v0.1.20 is paused and will not be published.
Co-design partner: MiniCon `plan/plan-v0.2.4.md` leaf `{HOSTIF}`.

> Version label: the product is called **0.2.0.0**. Cargo/semver only accept
> three components, so `Cargo.toml` carries `0.2.0`; the fourth component is a
> release-label suffix only.

## Product decision

AgenTerm is redesigned as a small launcher plus a plugin/app marketplace that
runs on top of MiniCon:

- **MiniCon** stays a light core: terminal + exec + mux. It owns the
  host-interface (HOSTIF) that plugins call.
- **agenterm.com** is a small launcher. It downloads `minicon.com` (the existing
  `minicon-com.yml` artifact — no second packaging) and verifies signature and
  sha256 against that existing trust chain before running it.
- **Capabilities become plugins.** CU, the qjs/lua script engines, VNC, SQL,
  etc. are official marketplace plugins, not code compiled into AgenTerm.
- **Plugin formats, in order:** wasm+gl sandbox (default) → webui (wry/tao) as
  the UI surface → native POSIX-C dynamic load last, signed allowlist only.

## Launcher design (0.2.x core requirement)

agenterm.com is a mini GUI (wry+tao web UI, proven fast to ship; a self-built
wasm VM + ABI UI is a later option). Flow:

1. **Detect** MiniCon: user-pinned path → AgenTerm-managed cache
   (`<data_dir>/agenterm/minicon/<version>/`) → PATH. Probe with
   `minicon --version --json` → `{version, hostif, os, arch}`.
2. **Compare** against the latest MiniCon Release `candidate-manifest.json`
   (bounded fetch; timeout = offline, never blocks the UI). States:
   missing / incompatible (hostif major) / update available / current / offline.
3. **Confirm**: missing or incompatible → modal showing version, asset name,
   size, destination; nothing downloads without consent. Optional update →
   non-blocking banner.
4. **Download the native per-arch asset by default**, APE only as fallback.
   Measured on MiniCon v0.2.3: native assets 0.96–4.1 MB and carry platform
   trust (Authenticode zip, notarized universal dmg/tar), while `minicon.com`
   is 11 MB and only one Authenticode identity covers it. APE is used only when
   the host OS/arch has no native asset.
5. **Verify** sha256 from the manifest + platform signature, download to a temp
   file, atomic rename; keep the previous version as rollback; fail closed.
6. **Launch** MiniCon and handshake HOSTIF; then open AgenTerm's assistant UI
   (status/version, update/rollback, plugin list placeholder).
7. **Failure UX**: offline / failed download / incompatible always offer
   retry · use local · choose path — never exit with a bare error.

Asks for MiniCon `{HOSTIF}`: `--version --json` with `hostif`, and a minimal
handshake call. Sent by cc-agenterm, not an owner question.

## Ownership split (agreed with cc-minicon)

| Side | Drafts | Contract rule |
|---|---|---|
| MiniCon | HOSTIF: host calls, capability probe, semver | major = breaking, minor = additive; never pin an exact build |
| AgenTerm | plugin manifest + marketplace/index format, launcher | plugin declares minimum HOSTIF version + required capabilities |

Both drafts are written independently, then aligned once before any code.

## Outcome tree

```text
v0.2.0.0 — one official plugin installs from the market and runs on a downloaded MiniCon
├─ M0 archive & reset
│  ├─ freeze 0.1.x: tag v0.1.19 + Release stay; index 0.1.x plans/PRD under archive
│  └─ inventory crates: launcher-keep / plugin-extract / archive-only
├─ M1 contracts (paper first, no code)
│  ├─ {MANIFEST} plugin manifest schema (id, version, format, min_hostif, caps, sha256, signature)
│  ├─ {INDEX} marketplace index format (signed, offline-cacheable)
│  └─ {ALIGN} one alignment pass with MiniCon {HOSTIF}; frozen as hostif 1.0
├─ M2 launcher (agenterm.com)
│  ├─ fetch minicon.com; verify signature + sha256 via minicon-com.yml chain
│  ├─ cache/last-good; fail closed on mismatch
│  └─ HOSTIF negotiation: refuse plugins whose min_hostif > host
├─ M3 plugin runtime path
│  ├─ wasm+gl loader in the sandbox (reuse tinyvm/wasmcore as reference)
│  └─ install / update / rollback / uninstall from the index
├─ M4 first official plugin: CU extracted from agenterm-cu
│  └─ black box: install from index → run one CU journey → uninstall leaves no residue
└─ M5 release: launcher + plugin artifacts signed (Windows Authenticode, macOS notarized)
```

## Crate inventory (M0 input, to verify)

| Current crate | Proposed fate |
|---|---|
| agenterm-abi, agenterm-chassis | reference for HOSTIF / ABI versioning lessons |
| agenterm-cu, agenterm-cu-provider | → first official plugin (M4) |
| agenterm-qjswasm, agenterm-lua, agenterm-script-common, agenterm-wasmcore | → script-engine plugins; wasmcore is input to M3 loader |
| agenterm-vnc, agenterm-vnc-app, agenterm-sql | → later plugins |
| agenterm-platform | shared with MiniCon; coordinate, do not fork |
| agenterm-ui-core, agenterm-dyn, agenterm-control-client | decide in M0 |

## Gates

1. No code before M1 `{ALIGN}` is done and hostif 1.0 is frozen.
2. Every claim is proven by a black box on the downloaded `minicon.com`, not on a
   locally built MiniCon.
3. Courts never touch a real host GUI/browser; every external wait is bounded.
4. Public Promotion needs the owner's explicit approval.

## Open decisions for the owner

- Archive form for 0.1.x: tag + Release only, or an `archive/0.1.x` index doc.
- Marketplace hosting: GitHub Releases as the index, or agenterm.com static host.
- Whether M4 (CU plugin) or a smaller "hello" plugin is the first plugin.
