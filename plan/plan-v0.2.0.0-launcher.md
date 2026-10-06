# AgenTerm v0.2.0.0 — launcher execution plan (agenterm.com)

Status: **L0 placeholder landed (`crates/agenterm-launcher-core`, standalone,
16 tests); L1+ wait for the owner's "开工"**
Parent: [`plan-v0.2.0.0.md`](plan-v0.2.0.0.md) ·
PRD: [`prd/PRD_03_01_launcher.md`](../prd/PRD_03_01_launcher.md)
Contract: MiniCon `plan/plan-hostif-v1.md`, implemented in MiniCon `fb40827`.
Revised 2026-10-06 after an adversarial review (signer identity, macOS asset,
Windows atomic install, schema fixture, downgrade, timeouts, binary naming).

## Hold condition

The unisa decision changes the market, not the launcher. Plugin concerns stay
behind one seam (the assistant UI's plugin panel is a placeholder).

## Fixed contract inputs

| Input | Value |
|---|---|
| Probe | `minicon --version --json` → `{version,hostif,os,arch,asset}` |
| Handshake | `minicon --hostif-handshake` → `{hostif,version,capabilities[]}` |
| Legacy rule | non-JSON stdout **or** non-zero exit ⇒ Legacy (pre-HOSTIF) |
| Required | hostif major == 1; capabilities ⊇ {exec, mux, pty} |
| Latest | `https://github.com/<org>/minicon/releases/latest/download/candidate-manifest.json` |
| Manifest | `schema ∈ {1}`, `kind == "minicon-release-candidate"`, `expected_tag == "v"+version`, ≤ 256 KiB |
| Asset pick | `minicon-<version>-<asset>.<ext>`; windows `zip`; linux `tar.gz`; macOS `dmg` then `tar.gz`; else `minicon.com` |
| Version order | numeric dotted; remote older ⇒ Current (no downgrade offered) |
| Signer | Windows: Authenticode signer of `minicon.exe` inside the zip == company subject; macOS: `MiniCon.app` TeamIdentifier == company team + `stapler validate`; Linux/APE: TLS + sha256 only (v1 limit) |

Signer values are taken from the `sign-windows-artifacts` /
`sign-macos-artifacts` skills at L3, never guessed.

## Architecture

```text
crates/agenterm-launcher-core      pure decisions, no GUI, no network, no spawn
├─ probe     parse version/handshake JSON; legacy rule
├─ manifest  parse + schema/kind/tag/size checks
├─ select    host → asset key → native asset; macOS dmg first; APE fallback
├─ state     Missing | Legacy | Incompatible | UpdateAvailable | Current | Offline | OfflineMissing
├─ verify    streamed sha256 with hard byte cap; SignerCheck trait (fail closed on any other signer)
└─ install   immutable <root>/<ver>/ + atomic current.json pointer; rollback = pointer swap

launcher shell (L1+)               effects behind traits, fed into core
├─ locate    user-pinned → <data_dir>/agenterm/minicon/current.json → PATH (PATH: report only, never auto-adopt)
├─ spawn     probe with 5 s timeout, no window; refuse untrusted signer before executing a PATH hit
├─ fetch     manifest 60 s; asset idle-timeout 30 s + manifest byte cap (no fixed total)
├─ extract   zip / tar.gz / dmg (hdiutil attach → copy MiniCon.app → detach)
└─ signer    WinVerifyTrust + signer subject; codesign -dv TeamIdentifier + stapler validate

agenterm UI (L4)                   wry+tao; renders core state; fetch only on a confirm event
```

Binary naming: wry+tao cannot be an APE, so the launcher ships one native
binary per platform. On Windows it is a GUI-subsystem `agenterm.exe`;
`agenterm.com` remains the console forwarder per AGENTS.md until the owner
renames it. The product name "agenterm.com" in owner intent means the launcher.

Updates never overwrite a running MiniCon: a new version is committed as a new
immutable directory and takes effect on the next launch.

## Milestones

| # | Deliverable | Done when (falsifiable) |
|---|---|---|
| L0 ✅ | core crate, state enum, real v0.2.3 manifest fixture | `cargo test` 16/16: schema≠1 refused, plain-text `--version` exit 0 ⇒ Legacy, remote older ⇒ no download, oversize stream aborted, rollback swaps pointer only |
| L1 | locate + spawn probe | stub binaries: JSON, plain text exit 0, exit 2, hang ⇒ timeout; PATH hit with wrong signer ⇒ untrusted, never executed |
| L2 | fetch + verify | local HTTP fixture: truncated, oversize, bad sha, stalled (idle timeout), schema 2 — all fail closed, zero residue |
| L3 | extract + signer + install | UTM courts: company-signed passes; **a validly signed binary from another signer fails**; update while old MiniCon runs ⇒ old intact, new on next start |
| L4 | wry+tao UI | Rust unit test renders each state to HTML and compares; fetch reachable only via confirm (negative control: bypass confirm ⇒ test fails) |
| L5 | black box | clean court: launcher → confirm → real MiniCon release → handshake → assistant; offline start uses local; declining makes zero network requests; bad sha leaves no files |
| L6 | signed launcher | Authenticode / notarized; court shows no SmartScreen/Gatekeeper prompt. Promotion is a separate owner gate |

Cross-compile and run in UTM before any CI Candidate.

## Start checklist (on "开工")

1. Join `agenterm-launcher-core` to the workspace (or keep it standalone until
   0.1.x code is archived) and set root `Cargo.toml` to `0.2.0`.
2. L1 → L2 locally; report each milestone with test output.
