# PRD 03.01 — AgenTerm launcher (v0.2.0.0)

Status: **specified; L0 placeholder implemented; product build pending owner "开工"**
Execution: [`plan/plan-v0.2.0.0-launcher.md`](../plan/plan-v0.2.0.0-launcher.md)

## Problem

AgenTerm 0.2.x runs on top of MiniCon instead of shipping its own terminal
core. A user who starts AgenTerm must end up on a trusted, compatible MiniCon
without knowing where to get it or which package fits the machine.

## User result

Starting AgenTerm shows one of these states and acts only with consent:

| State | User sees | Action |
|---|---|---|
| Missing | "MiniCon is not installed" + version, size, source, destination | download after **Confirm** |
| Legacy / Incompatible | "This MiniCon is too old for AgenTerm" + reason | upgrade after **Confirm** |
| UpdateAvailable | non-blocking banner | optional upgrade after **Confirm** |
| Current | assistant UI opens | none |
| Offline | assistant UI opens on the local MiniCon, "latest unknown" | none |
| OfflineMissing / failure | clear reason | Retry · Use local · Choose path |

## Requirements

1. **Consent.** No network download without an explicit confirm.
2. **Right package.** The native package for this OS/arch is preferred
   (0.96–4.1 MB, platform-signed). The 11 MB `minicon.com` APE is the fallback
   when no native package exists.
3. **Trust.** Size and sha256 must match the release manifest; Windows and
   macOS binaries must be signed by the company identity (any other signer is
   rejected); Linux/APE rely on TLS + sha256 (documented limit).
4. **Compatibility.** MiniCon must report HOSTIF major 1 and capabilities
   exec, mux, pty. Older MiniCon is detected safely (no window opens).
5. **No downgrade.** A remote release older than the local one is never
   offered.
6. **Safe install.** Versions are installed side by side and switched by an
   atomic pointer; a running MiniCon is never overwritten; one-step rollback.
7. **Bounded waits.** Every network or process wait has a limit; a limit
   expiring yields Offline or a failure state, never a frozen UI.
8. **Small UI.** wry+tao web UI; plugin list is a placeholder in 0.2.0.0.

## Non-goals (0.2.0.0)

Plugin market, plugin formats/runtime, capability plugins, unisa integration,
manifest signing (v2+ of the MiniCon contract).

## Evidence

Each requirement maps to a launcher milestone (L0–L6) whose completion
criteria are falsifiable black boxes; see the execution plan.
