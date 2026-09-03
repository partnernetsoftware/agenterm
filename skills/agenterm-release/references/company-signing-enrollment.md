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
  (`partnernetsoftware/minicon` `.github/workflows/company-signing.yml`).
- SignPath Foundation declined the open-source application in September 2026.
  There is no SignPath path any more; remove any SignPath placeholder when
  touching signing configuration.
- No AgenTerm production byte has been company-signed. AgenTerm still needs
  its own Entra app registration + federated credential
  (`repo:partnernetsoftware/agenterm:environment:release-signing`), the
  `Artifact Signing Certificate Profile Signer` role on the shared profile,
  and the six `release-signing` Environment values. `release-policy.json`
  therefore keeps Windows signing `off`.

The reusable, redacted procedure (CLI commands, login pitfalls, local jsign
rehearsal, workflow gates) lives in the company hub:
`company-dev-hub/skills/sign-windows-artifacts/`. Read it before creating
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

Do not put real coordinates in PRD, workflow logs, screenshots, receipts,
commit messages or handoff prompts. Receipts may retain provider-neutral request
identity/URL only when the provider documents it as non-secret and the exact
workflow redaction review approves it.

## First-signature court

The first provider run is qualification, not automatic release eligibility.
For every signed Windows PE/DLL it must prove:

1. the unsigned SHA-256 matches the selected exact-SHA Candidate input;
2. Authenticode was initially empty and the provider changed the bytes;
3. `Get-AuthenticodeSignature` reports `Valid`, the expected publisher and a
   timestamp certificate;
4. a receipt binds before SHA-256, final SHA-256, final byte count, source SHA,
   provider and timestamp evidence without private material;
5. Defender scans the final extracted Candidate files;
6. both Windows ISA courts execute those final bytes; and
7. Candidate aggregation accepts only the signed allowlist and fails closed on
   an extra, missing or renamed provider output.

Keep signing mode explicit. Missing credentials in `required` mode are a hard
failure; they must never downgrade the run to unsigned. While mode is `off`, no
signing action or credential import may execute.
