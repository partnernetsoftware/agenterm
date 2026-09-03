# Company signing enrollment and activation

This is the redacted operational memory for AgenTerm. Stable company identity,
account, subscription, tenant, validation, mailbox and provider-request values
must remain outside the repository.

## Current state

- The company Azure Artifact Signing account exists on the Basic tier in East
  US. Public Organization identity validation reached **Completed** on
  2026-09-03 and one Public Trust certificate profile is Active with the
  company as subject. MiniCon already has a federated GitHub identity with the
  profile-scoped signer role and a wired signing workflow
  (`partnernetsoftware/minicon` `.github/workflows/company-signing.yml`). Its
  first non-promotable live qualification signed all three intended inputs and
  executed the signed APE in all six native cells; this proves the shared
  provider/profile route without making any MiniCon Release signed.
- SignPath Foundation declined the open-source application in September 2026.
  There is no SignPath path any more; remove any SignPath placeholder when
  touching signing configuration.
- No AgenTerm production byte has been company-signed. The Candidate workflow
  now has fail-closed `off|required` branches, exact ten-file signing input,
  signed repack, receipt and final-byte courts. AgenTerm has its own Entra app
  registration + federated credential using GitHub's immutable
  `repo:<ORG>@<OWNER_ID>/<REPO>@<REPO_ID>:environment:release-signing`
  subject, the
  `Artifact Signing Certificate Profile Signer` role on the shared profile,
  three OIDC Environment secrets, and three shared provider-coordinate
  variables. Real values remain outside git and receipts.
- The remaining activation gate is the first live signed Candidate court, not
  identity provisioning. A dedicated `Windows Signing Qualification` workflow
  can first consume an exact successful unsigned Candidate and exercise the
  same ten-file transformer with `release_eligible=false`; its output is
  structurally rejected by Candidate/Promotion. Until AgenTerm proves the ten signed files,
  timestamp, receipts, Defender scans and two Windows runtime cells,
  `release-policy.json` keeps Windows signing `off`. The implementation and
  evidence DAG is `plan/goal-company-windows-signing.md`.

The reusable, redacted procedure (CLI commands, login pitfalls, local jsign
rehearsal, workflow gates) lives in the company hub:
`~/repos/company-dev-hub/skills/sign-windows-artifacts/`. Read it before creating
Azure or GitHub signing configuration for AgenTerm.

## Activation sequence

```text
organization validation approved
└── Public Trust certificate profile
    └── workload identity + profile-scoped signer role
        └── policy selects provider + required signing
            └── exact unsigned Candidate input hash
                └── provider signs and timestamps
                    └── verify publisher + timestamp + final hash
                        └── Defender + six execute-only courts
                            └── seal Candidate
```

Signing is a Candidate transformation, never a Promotion transformation.
Promotion downloads and publishes the already sealed bytes without signing or
rebuilding them.

## Protected configuration

Use the protected `release-signing` GitHub Environment. Store provider
coordinates as protected variables or secrets using placeholders in docs:

- Azure workload identity: `<AZURE_CLIENT_ID>`, `<AZURE_TENANT_ID>`,
  `<AZURE_SUBSCRIPTION_ID>`, `<SIGNING_ACCOUNT_URI>`, `<CERTIFICATE_PROFILE>`.
  Prefer GitHub OIDC; managed signing keys remain in Azure and are never
  exported.

The reusable identity shape is deliberately asymmetric: reuse the company
Artifact Signing account and Public Trust profile, but create one Entra
application and one federated credential per repository. Scope each service
principal only to the shared certificate profile. Never copy another product's
client id, create a client secret, or grant subscription-wide signing access.
For repositories on GitHub's immutable-subject format, derive both numeric IDs
from GitHub's organization/repository APIs and compare the resulting subject
as an opaque exact string. Do not substitute a GraphQL node id, an Entra object
id, the legacy name-only subject, or an issuer with a trailing slash.

Do not put real coordinates in PRD, workflow logs, screenshots, receipts,
commit messages or handoff prompts. Receipts may retain provider-neutral request
identity/URL only when the provider documents it as non-secret and the exact
workflow redaction review approves it.

## First-signature court

The first provider run is qualification, not automatic release eligibility.
Its implementation owner is
`.github/workflows/windows-signing-qualification.yml`; it accepts only a
successful unsigned Candidate at the exact current `main` SHA and performs no
Cargo build. The same PowerShell transformer serves formal Candidate and
qualification modes, while the checked-in policy and receipt eligibility keep
their authority distinct.
For every signed Windows PE/DLL it must prove:

1. the unsigned SHA-256 matches the selected exact-SHA Candidate input;
2. Authenticode was initially empty and the provider changed the bytes;
3. `Get-AuthenticodeSignature` reports `Valid`, the expected publisher and a
   timestamp certificate;
4. a receipt binds before SHA-256, final SHA-256, final byte count, source SHA,
   provider and timestamp evidence without private material or Azure/OIDC
   resource coordinates, and explicitly records release eligibility plus each
   Authenticode status;
5. Defender scans the final extracted Candidate files;
6. both Windows ISA courts execute those final bytes and record the archive SHA
   they actually consumed;
7. qualification aggregation matches both observed archive hashes to the
   signing receipt's platform after-hashes; and
8. Candidate aggregation accepts only the signed allowlist and fails closed on
   an extra, missing or renamed provider output.

Keep signing mode explicit. Missing credentials in `required` mode are a hard
failure; they must never downgrade the run to unsigned. While mode is `off`, no
signing action or credential import may execute.

## AgenTerm Windows allowlist

The allowlist is derived from `scripts/artifacts.json`, not handwritten globs.
Each of `win-x86_64` and `win-aarch64` contains exactly these five PE files:

- `agenterm.exe`
- `agenterm.com` (a Console-subsystem PE despite its extension)
- `agenterm-cc.exe`
- `agenterm-cu.exe`
- `agenterm.dll`

All ten files are signed or the required-mode Candidate fails. The public
v0.1.16 baseline is unsigned and every Security Directory is empty. The root
package and ABI now compile target-aware VERSIONINFO on Windows, Linux, or
macOS build hosts. Two-ISA inspection proves all five files per architecture
carry nonempty product and version fields before signing and have an empty
Security Directory. The root package owns `agenterm.exe`, `agenterm.com`, and
`agenterm-cc.exe`; the ABI and CU crates own their DLL/EXE resources. The tiny
forwarder keeps a metadata-only resource: do not add an icon, CRT startup, or
product logic merely to satisfy signing metadata.

Inspect a local or extracted Candidate file with the reusable company tools:

```powershell
& ~/repos/company-dev-hub/skills/sign-windows-artifacts/scripts/inspect-authenticode.ps1 -Path dist/agenterm.exe -ExpectedOrganization "<COMPANY_PUBLISHER>" -ExpectedProductName "AgenTerm" -ExpectedProductVersion "<VERSION>"
```

```bash
~/repos/company-dev-hub/skills/sign-windows-artifacts/scripts/inspect-authenticode.sh \
  dist/agenterm.exe
```

The Windows tool is authoritative: exit `0` means signature, expected
organization, and timestamp passed; `2` means unsigned, `3` invalid or
incomplete, `4` publisher mismatch, `5` timestamp absent, and `69` means the
Windows trust API is unavailable. The portable shell tool checks structural
verification but does not replace the Windows publisher court. File properties
in Explorer are a useful human view, but the Candidate gate and receipt must
use machine-readable checks over every allowlisted file.
