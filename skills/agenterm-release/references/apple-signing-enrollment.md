# Apple signing enrollment and activation (AgenTerm)

This is AgenTerm's redacted operational memory for the macOS lane. The canonical
reusable procedure is the company hub skill
`~/repos/company-dev-hub/skills/sign-macos-artifacts/` — read its
`references/apple-signing-setup.md` before creating or renewing any Apple
credential. This page records only what is AgenTerm-specific.

Key material, the `.p12` password, the App Store Connect Key ID and Issuer ID
never appear in this repository. The Developer ID publisher name and the Apple
Team identifier are public provenance, exactly like the Windows publisher name,
and may appear in receipts.

## Why the Apple lane is not the Azure lane

The Windows key is non-exportable inside Azure: there is nothing to leak from a
developer machine. The Apple key is the opposite — an exportable `.p12` that can
sign anything attributed to the company until it is revoked. So:

- the `.p12` and its password are AgenTerm's most sensitive signing secret;
- rotation and revocation are owner actions with a documented trigger;
- a developer machine may hold the identity for rehearsal only, in a **dedicated
  temporary keychain**, removed afterwards.

Never sign non-interactively from the login keychain. Its `SecurityAgent`
authorization prompt cannot be automated and hangs the job until it is killed.
Every AgenTerm signing path creates a throwaway keychain, imports with
`security set-key-partition-list`, and deletes it in an `if: always()` teardown.

## Current state

- `release-policy.json` has `signing.macos: "unsigned-preview"`. Public macOS
  assets are the visibly labelled preview channel
  (`agenterm-<version>-macos-<arch>-unsigned-preview.zip`, `channel:
  "macos-unsigned-preview"`, `signed: false`, `notarized: false`).
- The company Developer ID Application certificate and App Store Connect API
  notary key already exist, are retained in the company vault, and have been
  proven by MiniCon's signed, notarized and stapled release. AgenTerm must reuse
  them; it must not create a second publisher certificate or notary identity.
- AgenTerm's `release-signing` Environment now contains the certificate, its
  password, the notary key, Key ID and public Team ID. The Issuer ID remains the
  one missing protected value. Secret values were not read back or written to
  the repository.
- The Environment requires the repository owner as its human reviewer. The
  single-owner setup permits self-review; signing jobs therefore pause for an
  explicit account-holder action without becoming permanently unapprovable.
- The signing **mechanics exist but are not yet release-complete**:
  `scripts/sign-macos-release.sh` signs every Mach-O named by
  `scripts/artifacts.json`, stages `AgenTerm.app` through
  `packaging/privilege/macos/stage-app-bundle.sh`, and seals the bundle
  inner-out — nested dylibs, then the privileged helper with its own identifier
  and entitlements, then each executable, then the outer bundle. `candidate.yml`
  has an inline credential/sign/notarize branch gated behind `signed_macos`, but
  its build matrix is not bound to the protected Environment and it packages
  before notarization without stapling. It must not be enabled as written.
- **No AgenTerm byte has ever been signed with the Developer ID certificate.**
  The mechanics have never been executed once.
- `.github/workflows/macos-signing-qualification.yml` is the non-promotable
  court that executes them for the first time, against the exact Mach-O bytes of
  an already successful unsigned Candidate, while the policy still says
  `unsigned-preview`. Every receipt it writes is `release_eligible: false` and
  nothing downstream can consume it. This is the macOS twin of
  `windows-signing-qualification.yml`.

## What the qualification court proves that the Candidate lane does not

1. **The credentials work at all.** `AGENTERM_APPLE_TEAM_ID` was required by
   `sign-macos-release.sh` but never supplied by `candidate.yml`; the first
   signed Candidate would have died at its first `codesign` call, after the
   owner had already flipped the policy. Fixed, and pinned by
   `tests/macos_signing_qualification_policy.rs`.
2. **The bundle is stapled and Gatekeeper accepts it offline.** A bare Mach-O
   cannot be stapled (`stapler` error 73) and `spctl -a -t exec` rejects it as
   "does not seem to be an app" *even when Apple holds the ticket*, producing the
   "Apple could not verify this app is free of malware" dialog on double-click.
   Only a bundle can carry the ticket. The court staples `AgenTerm.app` and
   asserts `accepted` / `source=Notarized Developer ID`.
3. **The provider actually changed the bytes**, per file, with before/after
   SHA-256 in a receipt audited by `scripts/audit-macos-signing-receipt.py`.

## Known gap the court will expose

`candidate.yml` today signs, then packages the `.zip`, then submits that `.zip`
to `notarytool`. It never runs `xcrun stapler staple`. A ticket obtained that way
lives only on Apple's servers, so a signed AgenTerm release would still warn any
user whose machine is offline or rate-limited at first launch.

Stapling has to happen **between** notarization and packaging, which changes the
packaging order and therefore the archive hashes and provenance. That is a
deliberate engineering change to the Candidate packaging lane, not a fix to slip
in blind — the qualification court proves the correct order works before anyone
edits the Candidate.

## Owner checklist

Nothing below can be done by an agent. Do it in this order, as the Apple
account holder, in a browser. Cost is zero: the Developer Program membership is
already paid and notarization is free. The only irreversible mistake is losing
the `.p8`, which is downloadable exactly once.

1. **Confirm eligibility** — developer.apple.com → Membership.
   Needs: membership active and unexpired; newest Developer Program license
   agreement accepted; role **Account Holder** or **Admin**; 2FA on the Apple
   ID. A Developer-or-lower role fails at the portal, not in CI.
   Produces: nothing — this is a precondition. *No wait.*
2. **Create the Developer ID Application certificate.**
   Xcode → Settings → Accounts → the company team → Manage Certificates → `+` →
   **Developer ID Application**. (The portal + local CSR route also works and is
   what the company used; pick the G2 Sub-CA intermediary there.)
   Gate: `security find-identity -v -p codesigning` shows exactly one
   `Developer ID Application: <COMPANY> (<TEAM_ID>)`.
   Produces: the certificate and its private key in the login keychain.
   *No wait.* **Reuse the existing company certificate if one is already in the
   vault — do not issue a second publisher identity for AgenTerm.**
3. **Export it for automation.** Keychain Access → export the certificate **with
   its private key**, full chain including the Developer ID G2 intermediate, as
   `.p12` under a strong password.
   Produces: the `.p12` and its password → straight to the company vault. These
   are the two most sensitive values in this document. *No wait.*
4. **Create the App Store Connect API key** — App Store Connect → Users and
   Access → Integrations → App Store Connect API → `+`, role **Developer**.
   Record the Issuer ID and the Key ID, and download the `.p8`.
   **The `.p8` downloads exactly once.** If it is lost, revoke that key and
   create another. Watch for the native Save panel and the "allow multiple
   downloads" prompt — they are OS windows, not web content, and a missed click
   silently loses the file.
   Gate: `xcrun notarytool store-credentials --validate` reports Success.
   Produces: Issuer ID, Key ID, `.p8` → vault. *No wait.*
5. **Configure the GitHub `release-signing` Environment** (Settings →
   Environments → `release-signing`). AgenTerm must have its **own** Environment
   entries; do not point at another repository's.
   Secrets — `APPLE_DEVELOPER_ID_P12_BASE64` (base64 of the `.p12`),
   `APPLE_DEVELOPER_ID_P12_PASSWORD`, `APPLE_NOTARY_KEY_P8_BASE64` (base64 of
   the `.p8`), `APPLE_NOTARY_KEY_ID`, `APPLE_NOTARY_ISSUER_ID`.
   Variable — `AGENTERM_APPLE_TEAM_ID` (the 10-character Team ID; public
   provenance, so a variable and not a secret).
   AgenTerm currently requires the repository owner as reviewer and permits
   self-review for this single-owner repository. Preserve an explicit human
   approval before a signing job receives these values.
   Current AgenTerm state: all names except `APPLE_NOTARY_ISSUER_ID` are
   configured. Complete that one value from the existing company App Store
   Connect key; do not create a replacement key merely to recover its Issuer
   ID. Gate: the workflow's "Require protected Apple signing configuration" step
   fails closed and names any missing value. *No wait.*
   **Note:** `candidate.yml`'s `build` job currently reads these as ordinary
   repository secrets and is **not** bound to `environment: release-signing`.
   Decide whether to move them; adding the environment to that job applies to
   all six matrix cells, including the Windows ones, so it is an owner call.
6. **Run the macOS qualification court** — dispatch
   `macos-signing-qualification.yml` with the source SHA and a successful
   unsigned Candidate run id. This is the first real use of the credentials.
   It signs, notarizes, staples and assesses both macOS cells and produces a
   `release_eligible: false` aggregate. *Wait: Apple notarization is typically
   minutes but has no SLA; the job budget is 60 minutes per cell.*
   Produces: proof the mechanism works, with no release claim.
7. **Move Candidate signing behind the protected boundary and fix packaging
   order** (see "Known gap"). MiniCon has already settled the order: sign the
   bundle, notarize it, staple and validate it, then package and hash the final
   bytes. AgenTerm still needs a dedicated protected signing stage (or an
   equivalent split job) so unrelated build-matrix cells never receive Apple
   credentials. This engineering change must land and be re-qualified before a
   signed release is honest. *No provider cost.*
8. **Only then, flip the policy.** Set `release-policy.json`
   `signing.macos: "required"` in a commit, for a future unreleased version.
   This is the explicit owner decision `CODE_SIGNING_POLICY.md` reserves. It
   makes missing credentials a hard Candidate failure — the workflow never falls
   back to the preview channel — and it retires the qualification court, whose
   preflight requires `unsigned-preview`.

## Rotation and revocation

Renew the certificate before expiry and repeat steps 3, 5 and 6; a lapsed
membership invalidates signing and notarization. Revocation is different from
deletion: revoking the certificate invalidates affected signed files from the
selected revocation time and is irreversible. On suspected misuse, stop new
signing authority first — remove the Environment secrets — and preserve exact
hashes, receipts and submission ids before considering revocation.
