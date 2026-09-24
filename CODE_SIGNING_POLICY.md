# Code signing policy

AgenTerm's accepted Windows publisher is **PARTNERNET SOFTWARE PTY LTD** through
Azure Artifact Signing Public Trust. The signing key is managed and
non-exportable; this repository never stores a PFX, private key, client secret,
tenant coordinate, account name, certificate-profile name, or OIDC identifier.

## Current release status

The public v0.1.19 Windows assets are unsigned, and its macOS archives are
explicitly labelled unsigned previews. The Windows signing implementation and
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

Before dispatch, operators use the company signing skill's read-only
`check-product-signing-readiness.sh` court. It rejects dirty/stale source,
published version reuse, policy/version drift, missing workflows, and inspector
drift before provider time or signing quota is spent. `READY` is only source
eligibility; it is not signature or release evidence.

## Signed boundary and byte identity

Each Windows architecture archive contains the six entries declared in
`scripts/artifacts.json`: `agenterm.exe`, `agenterm.com`, `agenterm-cc.exe`,
`agenterm-cu.exe`, `agenterm-cu-provider.dll`, and `agenterm.dll`. All twelve
files across x86_64 and aarch64 must be signed together; handwritten globs and
partial signing are rejected.

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

## macOS: Apple Developer ID

AgenTerm's accepted macOS publisher is the company Developer ID Application
certificate. The Apple lane and the Azure lane are independent switches with
independent credentials, independent receipts and independent failure modes;
nothing may be shared between them, and turning one on says nothing about the
other.

`release-policy.json` `signing.macos` is either:

- `unsigned-preview`: Candidate publishes the visibly labelled preview channel
  (`agenterm-<version>-macos-<arch>-unsigned-preview.zip`, provenance `channel:
  "macos-unsigned-preview"`, `signed: false`, `notarized: false`);
- `required`: missing credentials, an invalid signature, a failed notarization
  or a rejected Gatekeeper verdict blocks Candidate creation. The workflow never
  falls back from `required` to the preview channel.

The current value is `unsigned-preview` and no AgenTerm byte has ever been
signed with the Developer ID certificate. Changing the value to `required` is an
explicit owner decision for a future version, and it is the **last** step of the
sequence in `skills/agenterm-release/references/apple-signing-enrollment.md`,
not the first.

Before that flip, `.github/workflows/macos-signing-qualification.yml` exercises
the real certificate and the real notary credential against the exact Mach-O
bytes of one already successful unsigned Candidate. It performs no Cargo build,
it is dispatchable only while the policy says `unsigned-preview`, and every
receipt it writes is `release_eligible: false`, so nothing downstream can
promote it. For each macOS architecture it proves:

1. every input is an unsigned Mach-O whose name comes from
   `scripts/artifacts.json`, never a glob;
2. the provider changed the bytes, recorded as per-file before/after SHA-256;
3. every file carries the hardened runtime and a secure timestamp, and
   `AgenTerm.app` is sealed inner-out including the privileged helper;
4. Apple notarization returned `Accepted`, and the submission id is recorded;
5. `xcrun stapler staple` succeeded **on the bundle** and
   `spctl -a -t exec` reports `accepted` / `source=Notarized Developer ID`.

Step 5 is the one a bare binary can never pass. A loose Mach-O cannot be
stapled, and Gatekeeper's launch path rejects it as "not an app" even when Apple
holds the ticket — which is why the double-clickable deliverable is the bundle.

Public macOS receipts may carry the Developer ID publisher name, the Apple Team
identifier, hashes, sizes, the notarization submission id, the stapling result
and the Gatekeeper verdict. The `.p12` and `.p8` material, the `.p12` password,
the App Store Connect Key ID and Issuer ID, and any keychain password are
protected and never enter source, logs, receipts or handoff text.
`scripts/audit-macos-signing-receipt.py` rejects a receipt carrying any of them
before it can be published.

## Final-byte reputation

`release-policy.json` `reputation.windows_final_candidate_bytes` is `required`.
That means a Promotion must carry a Microsoft Defender scan of the **sealed
Candidate's** Windows archives, performed on a real Windows machine, bound to
that Candidate's manifest by hash:

1. `scripts/utm-win-defender-court.sh` leases an AgenTerm Windows UTM court,
   pushes exactly the two archives the sealed manifest names, scans them with
   `MpCmdRun.exe -DisableRemediation`, and writes an `agenterm-defender-court`
   receipt recording each archive's hash before and after the scan;
2. `scripts/agenterm-reputation-court.py qualify` converts that receipt into an
   `agenterm-reputation-qualification`, refusing any receipt whose bytes, source
   SHA or Candidate run identity do not match the manifest;
3. `.github/workflows/reputation.yml` re-verifies the qualification against the
   Candidate it downloads itself, and publishes it;
4. `.github/workflows/release.yml` requires that reputation run and re-derives
   the binding a third time against the manifest it is about to publish.

Equal before and after hashes are load-bearing: they prove the scan observed the
shipped bytes and did not quarantine or remediate them. The court is scoped to
the Windows archives because that is what the policy field names; extending it
is a policy change, not a script change.

The `DEFENDER PASS` lines inside `candidate.yml` and
`windows-signing-qualification.yml` remain useful in-run signals, but they scan
a freshly built artifact on the machine that produced it and are bound to no
manifest. They are not this gate and never satisfy it.

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
An independently verified PEM bundle containing Microsoft's Artifact Signing
root plus timestamp CA may be supplied as `--ca-file ./trust.pem` when the
portable host's CA bundle lacks them; the option does not modify system trust
and never supersedes the Windows verdict.

## Evidence, privacy and reputation

Public signing receipts may contain source/run identity, hashes, sizes,
publisher certificate facts, timestamp-certificate facts, policy mode and
release eligibility. Protected Azure and OIDC coordinates never enter source,
logs, receipts, screenshots or handoff text. A valid signature establishes
publisher identity and byte integrity; it does not guarantee immediate
Microsoft SmartScreen reputation. Reputation is measured against the exact
final Candidate bytes rather than by removing product functionality.

If signing authority is suspected of misuse, maintainers stop new signing
authority and preserve exact hashes, receipts, and private provider transaction
records first. Deleting a certificate profile does not revoke signatures
already issued. Certificate revocation is a separate, irreversible owner action
that can invalidate affected files from the selected revocation time; it is not
an automated workflow fallback.

The owning product requirements and evidence DAG are
`prd/PRD_02_17_delivery_quality.md` and
`plan/goal-company-windows-signing.md`. Operational release authority remains
`skills/agenterm-release/SKILL.md`; the reusable cross-product implementation
guide is the company
[Windows signing skill](https://github.com/partnernetsoftware/company-dev-hub/tree/main/skills/sign-windows-artifacts).
