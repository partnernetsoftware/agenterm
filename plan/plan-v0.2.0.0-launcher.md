# AgenTerm v0.2.0.0 — agenterm.com execution plan (MiniCon series manager)

Status: **L0 core placeholder landed (`crates/agenterm-launcher-core`,
standalone, 16 tests); L1+ wait for the owner's "开工"**
Parent: [`plan-v0.2.0.0.md`](plan-v0.2.0.0.md) ·
PRD: [`prd/PRD_03_01_launcher.md`](../prd/PRD_03_01_launcher.md)
Contract: MiniCon `plan/plan-hostif-v1.md`, implemented in MiniCon `fb40827`.
Revised 2026-10-24 (owner): `agenterm.com` is the only release asset, a mini
entry and manager of the MiniCon series built on the `minicon.com` APE
scheme; most features are carried by MiniCon clients. This replaces the
2026-10-06 per-platform wry+tao launcher design.

## Packaging (from MiniCon `loader/`)

```text
agenterm.com             ← cosmocc fat trampoline (ISA x2), Authenticode-signable
 └── /zip/cells/         ← ZipOS overlay
      osx-aarch64/agenterm   osx-x86_64/agenterm
      lnx-aarch64/agenterm   lnx-x86_64/agenterm
      win-aarch64/agenterm.exe   win-x86_64/agenterm.exe
```

- Trampoline: MiniCon `loader/loader.c` behaviour unchanged — runtime OS,
  compile-time ISA, private `mkdtemp` extract (0700 + owner marker),
  `posix_spawn` + `waitpid`, loader-owned cleanup, guarded stale reaper.
  Preferred: MiniCon parameterizes the loader by product name so both `.com`
  files share one implementation; copying is the fallback only if MiniCon
  declines, recorded with the reason.
- Pack: `pack.sh` steps — stage six cells, VERSIONINFO on the x86_64 PE face,
  `cosmocc -Os -static`, Authenticode pad/prepare, `zip -r cells` +
  `zip -A`, build receipt, size report.
- Signing order: inner `cells/win-*/agenterm.exe` signed first and repacked,
  then `agenterm.com` itself (MiniCon `company-signing.yml` shape). macOS
  cell signing inside the overlay follows whatever MiniCon's `minicon.com`
  does; verify it before L6 rather than assume it.
- CI: one hosted macOS pack job, six native execute-only runners
  (`--version`, unknown flag ⇒ non-zero, `doctor`), aggregate requiring six
  unique cells with one SHA — MiniCon `minicon-com.yml` shape. Replaces the
  six-archive Candidate in this repository.

## Cell architecture (Rust, small)

```text
crates/agenterm-launcher-core      pure decisions: no I/O, no network, no spawn
├─ catalog   product entries: repo, asset rules per OS/ISA, probe, signer
├─ probe     parse --version --json / --hostif-handshake; legacy rule
├─ manifest  parse + schema/kind/tag/size checks
├─ select    host → asset; Unavailable when no formal asset exists
├─ state     Missing | Legacy | Incompatible | UpdateAvailable | Current | Offline | OfflineMissing | Unavailable
├─ verify    streamed sha256 with hard byte cap; SignerCheck trait, fail closed
└─ install   immutable <root>/<product>/<ver>/ + atomic current.json; rollback = pointer swap

manager cell (L1+)                 effects behind traits, via minicon-platform contracts
├─ cli       list | install | update | run | rollback | remove | doctor; bare = entry
├─ locate    user-pinned → <data_dir>/agenterm/<product>/current.json → PATH (report only)
├─ spawn     probe with 5 s timeout, no window; refuse an untrusted signer before exec
├─ fetch     manifest 60 s; asset idle-timeout 30 s + byte cap
├─ extract   zip / tar.gz / dmg
└─ signer    Authenticode subject; codesign TeamIdentifier + stapler validate
```

No GUI, no webview. HTTPS cost is the dominant size lever (MiniCon measured
`ring`/`rustls`/`ureq` at 1.6 MB per cell); L2 measures in-cell TLS against
delegating transport to the OS-provided client, and the owner picks with the
numbers in hand.

## Catalog seed (state 2026-10-24)

| Product | Formal assets today |
|---|---|
| `minicon` | v0.2.21: native archives for all six cells + `candidate-manifest.json` |
| `minicon-winmgr` | v0.1.20: Windows x86_64 `.exe` + `SHA256SUMS.txt` only |
| `minicon-srcmgr` | v0.0.4: no release assets |
| `minicon-surf` | no Release |

Everything without a formal asset for the host is `Unavailable`; the catalog
never builds or substitutes one.

## Milestones

| # | Deliverable | Done when (falsifiable) |
|---|---|---|
| L0 ✅ | core crate, state enum, real MiniCon v0.2.3 manifest fixture | `cargo test` 16/16 |
| L1 | catalog + locate + spawn probe + CLI skeleton | stub binaries: JSON, plain text exit 0, exit 2, hang ⇒ timeout; wrong-signer PATH hit never executed; unknown product ⇒ explicit error |
| L2 | fetch + verify + transport size decision | local HTTP fixture: truncated, oversize, bad sha, stalled, schema 2 — all fail closed, zero residue; size table for both transport options |
| L3 | extract + signer + install + rollback | UTM courts: company-signed passes; a validly signed binary from another signer fails; update while the old product runs ⇒ old intact, new on next start |
| L4 | `agenterm.com` pack | six-runner smoke on one packed SHA; measured size recorded; owner sets the ceiling |
| L5 | black box | clean court: `agenterm.com` → confirm → real `minicon` release → handshake → `run`; offline uses local; declining makes zero download requests; bad sha leaves no files; `Unavailable` products reported per host |
| L6 | signed single-asset release | inner cells + APE Authenticode, Defender reputation on `agenterm.com`; Promotion remains a separate owner gate |

Cross-compile and run in UTM before any CI Candidate.

## Start checklist (on "开工")

1. Ask MiniCon for the product-parameterized loader (or record the copy
   decision).
2. Join `agenterm-launcher-core` to the workspace and set root `Cargo.toml`
   to `0.2.0`; add the manager cell binary without touching 0.1.x crates.
3. Rewrite `scripts/artifacts.json`, `release-policy.json` and the Candidate
   chain for the single asset at L4, not before.
4. L1 → L2 locally; report each milestone with test output.
