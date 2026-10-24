# PRD 03.01 — agenterm.com: MiniCon series manager (v0.2.0.0)

Status: **specified (owner direction 2026-10-24); L0 core placeholder
implemented; product build pending owner "开工"**
Execution: [`plan/plan-v0.2.0.0-launcher.md`](../plan/plan-v0.2.0.0-launcher.md)

## Problem

Through 0.1.x AgenTerm shipped its own terminal, computer-use host, script
engine and Control Center: six archives of 9–33 MB each (v0.1.20 Windows
x86_64 unpacks to 26.4 MB, 79% of it `agenterm-cu` and its provider). Those
capabilities now belong to the MiniCon client. A user needs one small entry
that gets the right MiniCon-series products onto this machine, keeps them
current and starts them.

## Product shape

- **One release asset: `agenterm.com`.** A Cosmopolitan fat APE built the way
  MiniCon builds `minicon.com` (MiniCon `loader/`, PRD 02.27 "APE join"): a
  cosmocc trampoline plus a ZipOS overlay of six native manager cells
  (`{osx,lnx,win}-{aarch64,x86_64}`). The trampoline selects the host cell,
  extracts it to a private directory, spawns it and cleans up. Nothing is
  recompiled against cosmopolitan libc.
- **A mini entry, not a client.** Each cell is a small manager. Terminal, GUI,
  mux, harness and every other end-user capability are MiniCon-series
  clients; the manager never re-implements them.
- **Manager of the MiniCon series.** It knows a catalog of products
  (`minicon`, `minicon-surf`, `minicon-winmgr`, `minicon-srcmgr`, …) and for
  each one can report, install, update, roll back, remove and start it. The
  catalog is AgenTerm-owned and is the seed of the later app market.

## User result

| Command | Result |
|---|---|
| `agenterm.com` (no arguments, double-click) | MiniCon present and compatible ⇒ start it; otherwise show the state and offer install after **Confirm** |
| `agenterm.com list` | every catalog product: installed version, latest version, availability for this OS/ISA |
| `agenterm.com install <product>` / `update [<product>]` | verified side-by-side install after **Confirm** (or `--yes`) |
| `agenterm.com run <product> [args…]` | exec the installed product with the arguments passed through |
| `agenterm.com rollback <product>` / `remove <product>` | pointer swap to the previous version / removal of owned files only |
| `agenterm.com doctor` | host cell, install root, per-product state and the reason for each failure |

Per-product states: Missing · Legacy/Incompatible · UpdateAvailable ·
Current · Offline · OfflineMissing · Unavailable (no formal asset for this
OS/ISA).

## Requirements

1. **Single asset.** A Release publishes `agenterm.com` plus its checksum,
   provenance, SBOM and receipts, and nothing else executable.
2. **Small.** The size ceiling is set from the first measured pack and
   recorded as an owner decision; an automatic raise is forbidden. The target
   is single-digit MB, i.e. smaller than `minicon.com`.
3. **Consent.** No network request that downloads a product without an
   explicit confirm (`--yes` is an explicit confirm). Declining makes zero
   download requests.
4. **Right package.** For each product, the formal native asset for this
   OS/ISA from its own latest formal Release. A product without one is
   `Unavailable` with the reason — never a silent substitute.
5. **Trust.** Size and SHA-256 must match the product's published sidecar or
   manifest. Windows and macOS executables must carry the company signer
   (any other signer is rejected); Linux relies on TLS + SHA-256 (documented
   limit). `agenterm.com` itself is Authenticode-signed with signed inner
   Windows cells, as `minicon.com` is.
6. **Compatibility.** For `minicon`, HOSTIF major 1 with capabilities
   `exec`, `mux`, `pty`; an older MiniCon is detected without opening a
   window. Other products declare their own probe in the catalog.
7. **No downgrade.** A remote release older than the installed one is never
   offered.
8. **Safe install.** Immutable `<root>/<product>/<version>/` directories and
   an atomic `current.json` pointer; a running product is never overwritten;
   one-step rollback; a failure leaves the previous installation usable and
   no owned temporary data behind.
9. **Bounded waits.** Every network and process wait has a limit and expires
   into Offline or a failure state.
10. **Reuse MiniCon.** The trampoline, pack, receipt and size-report tooling
    come from MiniCon's `loader/` (shared, not forked); cells use
    `minicon-platform` contracts rather than raw OS APIs.

## Non-goals (0.2.0.0)

Shipping any 0.1.x capability (terminal GUI, `agenterm-cu`, qjswasm, Control
Center, Chassis); a GUI for the manager; plugin formats or runtime; market
browsing beyond the static catalog; unisa integration; manifest signing
(v2+ of the MiniCon contract). The 0.1.x sources stay in the repository as
reference until a separate archival decision.

## Evidence

Each requirement maps to a milestone (L0–L6) whose completion criteria are
falsifiable black boxes against downloaded formal product assets; see the
execution plan.
