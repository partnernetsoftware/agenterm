# AgenTerm v0.1.20 — signed desktop release

Status: **planned after public v0.1.19**
Product owner: [`prd/PRD_02_17_delivery_quality.md`](../prd/PRD_02_17_delivery_quality.md)
Version assignment: [`prd/PRD_02_18_roadmap.md`](../prd/PRD_02_18_roadmap.md)
Operational authority: [`skills/agenterm-release/SKILL.md`](../skills/agenterm-release/SKILL.md)

## Release decision

v0.1.19 shipped without platform code signatures. Its sealed Candidate manifest
marks both Windows archives `signed: false`; both macOS archives are explicitly
`unsigned-preview`, `signed: false`, and `notarized: false`. Linux artifacts
also have no platform code signature and rely on the existing checksums and
provenance. This followed the checked-in v0.1.19 policy (`windows: off`,
`macos: unsigned-preview`); it was not a failed signing transaction. Defender
scanning and artifact hashes did not supply a code signature.

**v0.1.20 must close the Windows and macOS signing gap before public
Promotion.** Do not treat a successful unsigned Candidate, a non-promotable
signing qualification, a Defender pass, or a release rehearsal as completion.
Do not silently retain `off` or `unsigned-preview` to meet a release date. If
one platform cannot meet its signed gate, the owner must make an explicit new
version/scope decision before a public release; no agent changes the gate to
make a failing Candidate green.

## Outcome tree

```text
v0.1.20 — final Candidate bytes carry verified platform trust
├─ Windows: Azure Authenticode + timestamp for all declared PEs on both ISAs
│  ├─ reconcile the current six-per-ISA manifest with stale ten-file docs
│  ├─ pass the first live, non-promotable signing qualification
│  └─ set signing.windows=required; verify receipt, native execution and Defender
├─ macOS: Developer ID + hardened runtime + notarized, stapled app on both ISAs
│  ├─ pass the first live, non-promotable Apple qualification
│  ├─ move Candidate signing into protected jobs; notarize, staple, then package
│  └─ set signing.macos=required; verify Gatekeeper and final archive hashes
└─ exact-SHA six-cell Candidate → reputation → rehearsal → human Promotion
```

## Missing evidence and implementation

| Gate | Current state | Required v0.1.20 proof |
|---|---|---|
| W0 — inventory | `scripts/artifacts.json` and the signing transformer derive six PE files per Windows ISA: four executables plus `agenterm.dll` and `agenterm-cu-provider.dll`. Some signing prose still says ten total. | Align the owning PRD, signing instructions and tests with the derived **twelve-file** set. Inspect each unsigned PE for ProductName, ProductVersion and empty Security Directory. The old v0.1.16 `agenterm.com` metadata failure is historical and does not prove that v0.1.19 bytes fail. |
| W1 — first provider court | Azure organization validation, Public Trust profile, AgenTerm OIDC identity and protected Environment are prepared. Earlier v0.1.16 qualification stopped before Azure login; no AgenTerm live signing receipt exists. | Run `windows-signing-qualification.yml` against a retained, conforming unsigned Candidate. Require all twelve Authenticode statuses `Valid`, expected publisher, RFC 3161 timestamps, before/after hashes, both Windows runtime cells and Defender scans. Keep `release_eligible=false`. |
| W2 — release Candidate | v0.1.19 policy selected `windows: off`. The `required` path exists but has not sealed a signed AgenTerm Candidate. | After W1, change the future-version policy to `windows: required`; run the exact-SHA Candidate. Its two final Windows archives and signing receipt must bind the same bytes observed by native execution and Defender. Missing credentials or any bad signature blocks aggregate. |
| M1 — first provider court | The company Developer ID certificate and notarization key are configured in the protected Environment. AgenTerm has never used them for a live signed court. | Run `macos-signing-qualification.yml` on both macOS ISAs. Require hardened-runtime signatures, secure timestamps, `Accepted` notarization, stapled `AgenTerm.app`, Gatekeeper acceptance and redacted `release_eligible=false` receipts. |
| M2 — release Candidate | The current Candidate's Apple branch is not protected end to end and packages before stapling. It cannot produce an offline-verifiable stable app as written. | Put signing credentials only in protected macOS signing jobs. Sign inner-out, notarize, staple, assess, **then** package/hash. Bind the post-staple archives and receipts to both runtime cells. After M1 and this integration pass, set `macos: required`; missing credentials, ticket or acceptance must fail closed. |
| R — publication | Promotion already copies sealed bytes without rebuilding or signing. | One successful six-cell Candidate with W2 and M2 evidence, final-byte Defender qualification, release rehearsal, then explicit human approval for that exact Candidate. Public asset integrity must verify the promoted bytes. |

The signing transformation consumes the locally cross-compiled six-cell inputs;
GitHub jobs execute, sign and verify those bytes without invoking Cargo or a
compiler. Linux remains on its checksum/provenance lane; this plan does not
invent a Linux executable-signing format. Windows and Apple have separate
providers, credentials, receipts and failure gates.

## Validation and safe failure

Use the non-promotable courts before switching either policy. Confirm each
court's negative controls: missing signature, wrong publisher or timestamp,
missing Apple staple, foreign archive hash and missing runtime receipt must
fail. The first release-eligible Candidate must show that all six runtime
cells consumed its sealed final bytes. Preserve the signed receipts and
submission identities without protected values. If either signing lane is
blocked, record the exact missing evidence in the owning PRD and stop before
public Promotion.
