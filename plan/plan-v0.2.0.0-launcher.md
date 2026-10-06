# AgenTerm v0.2.0.0 — launcher execution plan (agenterm.com)

Status: **ready to start on owner's "开工"; not started**
Parent: [`plan-v0.2.0.0.md`](plan-v0.2.0.0.md)
Contract: MiniCon `plan/plan-hostif-v1.md` (v2 text, approved) implemented in
MiniCon `fb40827` (`--version --json`, `--hostif-handshake`).

## Hold condition

The owner is waiting for unisacc to mature before deciding whether the MiniCon
core embeds unisa. That decision changes the **plugin/market** direction, not
the launcher: detect → confirm → download → verify → launch is independent of
what runs inside MiniCon. The launcher therefore keeps plugin concerns behind
one seam (`plugins` panel = placeholder) so either outcome lands without
rework. Re-read this section when the unisa decision arrives.

## Scope (one user result)

A user runs agenterm.com, sees whether MiniCon is present / current, confirms a
download of the right package for this machine, and lands in a small AgenTerm
assistant window attached to a running MiniCon.

Out of scope: plugins, market index, wasm/webui plugin runtime, unisa.

## Fixed contract inputs

| Input | Value |
|---|---|
| Probe | `minicon --version --json` → `{version,hostif,os,arch,asset}` |
| Handshake | `minicon --hostif-handshake` → `{hostif,version,capabilities[]}` |
| Legacy rule | non-JSON stdout **or** non-zero exit ⇒ `hostif = 0` (upgrade needed) |
| Latest | `https://github.com/<org>/minicon/releases/latest/download/candidate-manifest.json` |
| Manifest | `schema` (refuse unknown), `version`, `assets[]{name,bytes,sha256,sidecar}` |
| Asset pick | `minicon-<version>-<asset>.<ext>`; fallback `minicon.com` if no native asset |
| Trust (v1) | TLS + manifest sha256 + platform signature; manifest unsigned = known limit |
| Required hostif | major == 1; capabilities ⊇ {exec, mux, pty} |

## Architecture

```text
crates/agenterm-launcher-core   (no GUI, no network policy hidden in UI)
├─ locate      user-pinned → <data_dir>/agenterm/minicon/<ver>/ → PATH
├─ probe       spawn with timeout 5s, no window; parse JSON; legacy rule
├─ latest      bounded GET (10s connect / 60s total); timeout ⇒ Offline
├─ select      host os/arch → asset name; .com fallback; .dmg never chosen
├─ fetch       temp file + sha256 stream; size must equal manifest bytes
├─ verify      sha256; Windows WinVerifyTrust / macOS codesign+spctl / Linux sha only
├─ install     extract → <data_dir>/.../<ver>/; atomic rename; keep previous = rollback
└─ state       enum { Missing, Legacy, Incompatible, UpdateAvailable, Current, Offline, Failed(reason) }

src/bin/agenterm (agenterm.com)  wry+tao mini web UI over launcher-core
├─ status card   state + local/latest version + asset + size
├─ confirm modal Missing/Legacy/Incompatible: what, size, from where, to where
├─ banner        UpdateAvailable (non-blocking)
├─ failure panel retry · use local · choose path
└─ assistant     after handshake: MiniCon status, update/rollback, plugins placeholder
```

Rules: nothing downloads without consent; every external wait bounded;
fail closed on hash/signature mismatch; no telemetry; the UI never runs the
network itself (core owns it, UI renders state).

## Milestones

| # | Deliverable | Done when |
|---|---|---|
| L0 | crate skeleton + state enum + fixture manifests (v0.2.3 real, schema=2, missing asset) | `cargo test -p agenterm-launcher-core` green on Mac |
| L1 | locate + probe + legacy rule | tests with stub binaries: JSON ok, plain-text `--version`, exit 2, hang (timeout) |
| L2 | latest + select + fetch + verify (sha) | local HTTP fixture server; truncated/oversize/bad-sha all fail closed |
| L3 | install + rollback + platform signature verify | Windows/macOS courts (UTM), never a real host GUI |
| L4 | wry+tao UI over core | headless snapshot of each state; confirm required before fetch |
| L5 | end-to-end black box | clean court: agenterm.com → confirm → real MiniCon release → handshake → assistant window |
| L6 | release wiring | agenterm.com signed (Authenticode/notarize); owner approves Promotion |

Cross-compile and run in UTM before any CI Candidate (never debug in the
release pipeline).

## Risks

- `releases/latest` can lag or point to a release missing a native asset →
  `.com` fallback + explicit message.
- macOS: downloaded tar needs quarantine handling; verify with `spctl` after
  extract, not before.
- Windows SmartScreen on the launcher itself → launcher must be signed from L6.
- GitHub rate limits on unauthenticated GET → cache manifest with ETag.
- unisa decision may add a HOSTIF capability; launcher only checks membership,
  so it absorbs additive change with a constant edit.

## Start checklist (on "开工")

1. Commit `plan-v0.2.0.0.md` + this file with `git commit --only`.
2. Bump `Cargo.toml` to `0.2.0` (label 0.2.0.0) — confirm with cdx first that
   0.1.20 stays paused.
3. L0 → L2 locally in one session; report each milestone with test output.
