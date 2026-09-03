# Code signing policy

AgenTerm's accepted Windows publisher is **PARTNERNET SOFTWARE PTY LTD** through
Azure Artifact Signing Public Trust. The signing key is managed and
non-exportable; this repository never stores a PFX, private key, client secret,
tenant coordinate, account name, certificate-profile name, or OIDC identifier.

## Current release status

The public v0.1.16 assets are unsigned. The Windows signing implementation and
repository-specific OIDC identity are prepared, but AgenTerm has not yet
completed its first live signed qualification. A qualification artifact is
never a signed Release: it is deliberately marked `release_eligible=false` and
cannot be promoted.

`release-policy.json` is the machine-readable authority. Its
`signing.windows` value is either:

- `off`: Candidate uses the exact unsigned Windows archives and never enters
  the protected signing Environment;
- `required`: missing configuration, an invalid signature, a receipt mismatch,
  a native-runtime failure, or a Defender finding blocks Candidate creation.

Changing this value to `required` is an explicit owner decision for a future
version. The workflow never silently falls back from required signing to an
unsigned release.

## Signed boundary and byte identity

Each Windows architecture archive contains the five entries declared in
`scripts/artifacts.json`: `agenterm.exe`, `agenterm.com`, `agenterm-cc.exe`,
`agenterm-cu.exe`, and `agenterm.dll`. All ten files across x86_64 and aarch64
must be signed together; handwritten globs and partial signing are rejected.

Signing is a Candidate transformation, not a Promotion operation:

1. consume one exact-SHA unsigned Candidate without rebuilding;
2. verify the two archive hashes, exact file set, VERSIONINFO and empty
   Authenticode Security Directories;
3. sign with SHA-256 and a Microsoft RFC 3161 SHA-256 timestamp;
4. record every before/after SHA-256, size, public publisher/timestamp facts,
   source SHA, and run identity in a redacted receipt;
5. execute and Defender-scan the exact signed archives on native Windows
   x86_64 and aarch64 courts;
6. seal the complete six-platform Candidate; Promotion publishes those bytes
   without rebuilding, repacking, signing, or timestamping.

Linux signing and Apple Developer ID/notarization are separate policy lanes;
Windows Authenticode evidence does not make those artifacts signed.

## Inspect a downloaded file

On Windows, right-click a file and open **Properties → Digital Signatures** for
a quick human view. The machine-readable, authoritative check is:

```powershell
.\scripts\inspect-authenticode.ps1 .\agenterm.exe `
  -ExpectedProductName AgenTerm -ExpectedProductVersion '<VERSION>'
```

Exit `0` requires Windows trust status `Valid`, the expected company publisher,
a timestamp certificate, and matching requested VERSIONINFO. Exit `2` means
unsigned; `3` invalid/incomplete; `4` another publisher; `5` no timestamp; `6`
product/version mismatch; and `69` that the Windows trust API is unavailable.
The JSON report contains only the basename, SHA-256, byte count, VERSIONINFO,
signer facts and timestamp-certificate facts; it does not expose the expanded
local path.

On macOS/Linux, `scripts/inspect-authenticode.sh ./agenterm.exe` prints the
SHA-256, byte count and portable `osslsigncode` report. Exit `2` means no
extractable embedded signature; `3` means a signature exists but portable
verification failed, which may be a local CA-chain gap or a real integrity
failure. Windows remains the final trust authority.

## Evidence, privacy and reputation

Public signing receipts may contain source/run identity, hashes, sizes,
publisher certificate facts, timestamp-certificate facts, policy mode and
release eligibility. Protected Azure and OIDC coordinates never enter source,
logs, receipts, screenshots or handoff text. A valid signature establishes
publisher identity and byte integrity; it does not guarantee immediate
Microsoft SmartScreen reputation. Reputation is measured against the exact
final Candidate bytes rather than by removing product functionality.

The owning product requirements and evidence DAG are
`prd/PRD_02_17_delivery_quality.md` and
`plan/goal-company-windows-signing.md`. Operational release authority remains
`skills/agenterm-release/SKILL.md`; the reusable cross-product implementation
guide is the company
[Windows signing skill](https://github.com/partnernetsoftware/company-dev-hub/tree/main/skills/sign-windows-artifacts).
