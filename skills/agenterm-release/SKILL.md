---
name: agenterm-release
description: Operate and diagnose AgenTerm's exact-SHA Release Candidate and approved byte-promotion workflows. Use for Candidate creation, release rehearsal, GitHub Actions monitoring, release authentication or dispatch failures, Promotion approval, tag/Release publication, or delivery-latency investigation in the AgenTerm repository.
---

# AgenTerm Release

## Establish authority

Read these repository-owned contracts before acting:

1. `AGENTS.md`, especially Development loop, formal delivery, and GitHub
   Actions observation.
2. `prd/PRD_02_17_delivery_quality.md`.
3. `.github/workflows/candidate.yml`, `.github/workflows/release.yml`, and
   `.github/workflows/release-integrity.yml`.
4. `scripts/qjs/release.qjs`, `scripts/qjs/candidate-verify.qjs`,
   `scripts/qjs/candidate-aggregate.qjs`, and `agenterm.tasks.json`.
5. `release-policy.json`; signing mode is checked-in source identity. Missing
   credentials must fail a `required` mode and must never select unsigned mode
   implicitly.

Read `references/github-auth-and-dispatch.md` when authentication, workflow
dispatch, monitoring, rate limits, or human approval is involved.
Read `references/company-signing-enrollment.md` before changing signing policy,
provider configuration, signature receipts, or final-byte reputation courts.
Read `references/apple-signing-enrollment.md` before touching the macOS lane:
Apple Developer ID is a separate provider with an exportable key, its own
qualification court (`.github/workflows/macos-signing-qualification.yml`), and
its own owner checklist. Never share a switch, a secret or a receipt between it
and the Windows lane.
Read `references/protected-console-transfer.md` when an account holder has
opened an authenticated Apple, Azure or GitHub console and one existing value
must be transferred into the protected GitHub Environment. It makes
`agenterm-cu` the preferred observation path and forbids logs, clipboard use or
temporary secret files.
That product reference records AgenTerm-specific state; the canonical reusable
implementation and operations procedure is
`~/repos/company-dev-hub/skills/sign-windows-artifacts/SKILL.md`. Before a
Candidate or signing dispatch, run its read-only
`scripts/check-product-signing-readiness.sh` against this repository and require
`READY`; this does not replace exact-SHA authorization or live provider courts.

Treat the current files and remote run state as authoritative. Do not infer the
delivery topology from an older release or from Git push behavior.

## Candidate workflow

1. Synchronize and inspect `origin/main`; preserve other platform agents'
   commits.
2. Require the exact lowercase 40-character current `origin/main` HEAD and a
   clean worktree. Ordinary feedback CI remains separate from release
   qualification. Candidate rejects a historical main ancestor because
   `workflow_dispatch` controller identity, provenance, and Promotion must
   remain one unambiguous commit.
3. Run `scripts/build-local-six-cell.sh` on that exact SHA. It runs the
   pre-push gate, cross-compiles all six client cells and Chassis loaders in
   one local lane, packages host-supported archives, and emits a checksummed
   input bundle. GitHub Candidate jobs must consume that bundle; hosted jobs
   must not invoke Cargo, rustup, build/check aliases, or a compiler setup.
4. When Windows signing or qualification is in scope, also require the company
   signing readiness court; do not spend a provider operation on a dirty tree,
   stale main, or published version identity.
5. Verify and upload the bundle to an unpublished staging draft with
   `scripts/stage-local-candidate-draft.sh`. Record the returned numeric release
   ID. Dispatch `candidate.yml` with both `source_sha` and
   `staging_release_id` through an available authenticated Actions capability.
   The workflow verifies that the draft tag resolves to the exact source SHA,
   verifies the bundle and checksum, and only then uploads the input artifact
   consumed by its execute-only jobs.
6. If dispatch is unavailable, stop and give the human the exact workflow
   link, SHA, fields, and non-publishing effect. Never extract a GCM secret to
   manufacture REST authentication.
7. After dispatch, resolve the newest `Release Candidate` run matching the
   exact SHA and `workflow_dispatch` once; record its `run_id` and
   `run_attempt`. If a human dispatched it, their `已启动` confirmation is
   enough to begin that one-time resolution; they do not need to copy an ID.
8. Observe the retained run ID through one bounded observer. Verify the six
   imported parts, six runtime cells, installed Chassis journey, Defender and
   ACU receipts, aggregate, and sealed Candidate artifact. The current schema-v2
   receipt is `prebuilt-six-cell-execute-only` and explicitly sets
   `stress_included: false`; do not describe it as the former Windows
   stress-inclusive qualification. The workflow's success and sealed artifact
   are hard Candidate validity requirements. The separate read-only Workflow
   Observer is delivery-quality evidence, but an observer outage does not turn
   a valid Candidate into a failed build.
9. On failure, fetch only the failed job log/artifact, fix the owning cause,
   validate locally, push a coherent increment, and create a new Candidate.
   Never rebuild silently during Promotion.

Candidate dispatch is mechanical and creates no tag or public Release. An
explicit release-Candidate goal authorizes the whole continuous qualification
loop: when a failed Candidate yields a scoped fix and a new current-main SHA,
validate, push, and dispatch the replacement exact-SHA Candidate without asking
the human to repeat authorization for each repair commit. A request limited to
one named SHA does not authorize later SHAs. Public Promotion remains separate.

## Reputation gate

Between Candidate and Promotion, `release-policy.json`
`reputation.windows_final_candidate_bytes: "required"` demands a Defender scan
of the sealed Windows bytes. Run it locally against the downloaded Candidate,
then publish the qualification:

```sh
gh run download <candidate_run_id> --name release-candidate-<candidate_run_id> --dir target/candidate-<v>
scripts/utm-win-defender-court.sh target/candidate-<v> target/defender-court-<v>.json
python3 scripts/agenterm-reputation-court.py qualify \
  --manifest target/candidate-<v>/agenterm-<v>-candidate-manifest.json \
  --defender target/defender-court-<v>.json \
  --output target/reputation-qualification-<v>.json
Q="$(base64 -i target/reputation-qualification-<v>.json | tr -d '\n')"
gh workflow run reputation.yml --ref <branch-or-tag-at-candidate-sha> \
  -f candidate_run_id=<candidate_run_id> -f source_sha=<sha> -f qualification_base64="$Q"
```

`reputation.yml` and `release.yml` both assert `GITHUB_SHA == source_sha`, so if
`main` has moved past the Candidate SHA — any commit, even docs — pin a
throwaway branch at that SHA and dispatch `--ref` it, then delete it after
publish. Default the court to `win-aarch64-desktop`: Defender scans statically,
so a native ARM guest is a valid scanner for x86_64 archives and is far more
reliable than the emulated x86 guest on Apple Silicon. Never run exploratory VMs
in the UTM instance the release court uses.

Do not relax the scan, edit an assertion, or hand-write a verdict to get a green
gate. `AGENTERM_DEFENDER_VERDICT` is refused by the court for exactly that
reason.

## Promotion workflow

Promotion is a separate human authority boundary.

1. Do not dispatch `release.yml` until the user explicitly approves public
   publication for the exact Candidate. Pass `reputation_run_id` from the
   reputation gate above; while the policy says `required`, omitting it is a
   hard failure rather than an unscanned publish.
2. Bind `candidate_run_id`, source SHA, version, expected tag, artifact
   identity, expiry, and confirmation `publish-vX.Y.Z`.
3. Require the configured `release` environment approval when available.
4. Verify that Promotion performs no Cargo build, test, package, signing,
   notarization, or overwrite.
5. A retry may resume only an exact-SHA tag and unpublished matching draft.
   Verify its Candidate marker, exact title/body/body hash, and every retained
   asset by allowlisted name, size, and SHA-256; upload only missing assets
   without overwrite.
6. Verify the tag points to the Candidate SHA, the published Release contains
   the exact allowlisted bytes, and `Release asset integrity` succeeds.
7. Report remaining risk and links; never claim success from a draft,
   incomplete matrix, or merely green tag-independent CI.

Without explicit public-release approval, stop after Candidate verification.

## Local coordinator

Use `release.cmd --rehearse` for read-only validation/rehearsal.
`release.cmd` intentionally refuses local publication. Do not restore the old
local tag-push path merely to avoid workflow dispatch.

Keep Candidate/Promotion policy tests line-ending independent. Pin every source
file whose bytes enter cross-platform provenance to LF in `.gitattributes`;
otherwise Windows and Unix can hash different working-tree bytes for one Git
commit.

## Delivery discipline

- Push small, coherent, reviewed progress to `main` early so Linux/macOS agents
  can rebase and test.
- Keep Candidate and Promotion permissions least-privileged and actions pinned
  to immutable commits.
- Keep observation read-only and bounded; observation loss is not workflow
  failure.
- Preserve exact-SHA receipts, hashes, SBOM, provenance, artifact allowlists,
  expiry, and no-overwrite semantics.
- Never create a tag or GitHub Release without explicit user approval.
