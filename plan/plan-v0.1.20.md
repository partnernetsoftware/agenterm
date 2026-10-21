# AgenTerm v0.1.20 — signed desktop release

Status: **active (owner decision 2026-10-21; supersedes the earlier
"paused, unpublished" note in [`plan-v0.2.0.0.md`](plan-v0.2.0.0.md) — 0.2.x
starts only after this version ships publicly)**

Scope note 2026-10-21: besides the signing gates below, v0.1.20 ships the
MiniCon externalization already on `main`: `minicon-platform` /
`minicon-ui-core` / `minicon-core` consumed by pinned git rev (see
[`plan-v0.2.0.0-platform-move.md`](plan-v0.2.0.0-platform-move.md), executed
ahead of the 0.2.x line). Version identity is bumped to `0.1.20` in the
workspace Cargo manifests and `release-policy.json`; signing modes flip to
`required` only after their non-promotable qualification courts pass (W1/M1).
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

## Execution state (2026-10-21, Windows x86_64 host)

- Done: version identity `0.1.20` across the workspace manifests and
  `release-policy.json`; W0 doc reconciliation — the signing allowlist prose in
  `skills/agenterm-release/references/company-signing-enrollment.md` and
  `plan/goal-company-windows-signing.md` now states the manifest-derived
  twelve-file set (six PEs per Windows ISA, including
  `agenterm-cu-provider.dll`); the owning PRD already said twelve. The
  transformer and policy tests already derive the set from
  `scripts/artifacts.json`, so no code change was needed.
- Gate repair (2026-10-21, Windows x86_64): the quick gate's PRD capability
  alignment failed on this host because Git-for-Windows GNU grep 3.0 aborts on
  the `-o -i -F` combination, silently emptying the mention set
  (`prd_alignment_public_command_missing:acu`). `prd-alignment.qjs` now
  lowercases the corpus with `tr` and matches without `-i`; verified
  fail→pass with no other change. The same run measured ~168 s on this host
  against a budget tuned to a 21 s measurement, so the prd-alignment wall
  clock guard was raised to 360 s (twice the slowest measured host) in
  `check.qjs` and `agenterm.tasks.json` — a budget retune recorded here, not
  an assertion change.
- Readiness: `check-product-signing-readiness.sh` (company hub) reports the
  consistent `0.1.20` version identity; it returns `READY` once the worktree
  is clean on exact `origin/main`.
- BLOCKED on this host: `scripts/build-local-six-cell.sh` — the two macOS
  cells build with plain `cargo` and need the macOS cross-compilation host.
  The Candidate input bundle therefore cannot be produced from the Windows
  host; run the six-cell build + encrypted staging on the Mac host, then any
  host may dispatch `candidate.yml`.
- Order after the Candidate: W1 (`windows-signing-qualification.yml`) and M1
  (`macos-signing-qualification.yml`) non-promotable courts → M2 protected
  sign→notarize→staple→package restructure of `candidate.yml` → flip both
  policies to `required` → new exact-SHA Candidate → reputation → rehearsal →
  human Promotion.

### Mac-host runbook (the one step this Windows agent cannot run)

On the macOS cross-compilation host, with a clean checkout at the exact
current `origin/main` HEAD (readiness court `READY` at
`d42c29e511eb96f0bb9ee9a080594d63c9dbba81`, 2026-10-21 — re-verify if main
has moved):

1. `scripts/build-local-six-cell.sh` (pre-push gate + six cells + Chassis
   loaders + checksummed candidate input).
2. `scripts/stage-local-candidate-encrypted.sh` — record the numeric
   temporary prerelease id it prints.
3. Dispatch `candidate.yml` with `source_sha=<that exact SHA>` and
   `staging_release_id=<the id>` (any authenticated host may dispatch).

After one successful unsigned Candidate, either host dispatches the two
non-promotable courts with `source_sha` + `candidate_run_id`:
`windows-signing-qualification.yml`, then `macos-signing-qualification.yml`.
Both pause on the protected `release-signing` Environment for the owner's
explicit approval. Evidence retention, the M2 restructure, the policy flips
and the next Candidate then continue per the gate table above.

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
