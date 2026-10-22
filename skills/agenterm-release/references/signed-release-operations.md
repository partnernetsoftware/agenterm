# Signed release operations (learned shipping v0.1.20)

Operational facts from the first signed AgenTerm release (2026-10-23). The
shipped identities and full defect list live in
`plan/archive/plan-v0.1.20.md`; this page keeps only what changes how the next
release is run.

## Chain without a developer Mac

1. Dispatch `six-cell-cloud-build.yml` with `source_sha` = current
   `origin/main`. It runs the unchanged `build-local-six-cell.sh` and
   `stage-local-candidate-encrypted.sh` on hosted `macos-15` and prints
   `STAGING_RELEASE_ID=<n>` (~85–100 min cold).
2. Dispatch `candidate.yml` with that `source_sha` and `staging_release_id`.
   Find the run by `headSha`, not by "latest run": a dropped connection after
   dispatch otherwise leaves the operator holding no run id.
3. With signing `required`, `windows_sign` and `macos_sign` pause on
   `release-signing`. The owner has delegated approval of these pauses for an
   owner-directed release (2026-10-22); approve with
   `gh api -X POST repos/<repo>/actions/runs/<id>/pending_deployments
   -F 'environment_ids[]=<env id>' -f state=approved -f comment=...`.
   Public Promotion still needs the owner's explicit instruction for the
   release.

## Rerun rules for a signed Candidate

- Never use "re-run failed jobs" once `signing.windows` is `required`. The
  rerun keeps the earlier attempt's `windows-signing-receipt.json`, and
  aggregate rejects it with `candidate_windows_signing_receipt_run`. Re-run the
  whole run; signing repeats and the staged input is reused, so no rebuild.
- `stapler staple`/`validate` fetch the ticket from Apple's CloudKit endpoint,
  which intermittently drops hosted-runner connections right after an
  `Accepted` notarization (error 68, `NSURLErrorDomain -1005`). The lanes retry
  with backoff; if a run still fails there, a whole-run rerun is the remedy.
- The staging script's anonymous readability probe can see HTTP 403 when the
  shared runner egress has spent its anonymous API quota; it retries
  anonymously and never falls back to credentials.

## What forces a rebuild

The Candidate binds `GITHUB_SHA == source_sha`, and the staged bundle embeds
its source, so any fix needed *by the running Candidate* (including a
workflow fix) needs a new commit and a new cloud build. A workflow-, plan- or
doc-only commit that the running chain does not need is safe to land: it does
not change `scripts/product-source-hash.sh`, and reputation/Promotion then
dispatch from a throwaway branch pinned at the Candidate SHA
(`release-pin-v<version>`), deleted after publication.

## Reputation on a Windows host

On a real Windows machine the UTM court is unnecessary:

```sh
gh run download <candidate> --name release-candidate-<candidate> --dir target/candidate-<v>
powershell -File scripts/host-win-defender-court.ps1 -CandidateDir <abs path> \
  -ReceiptPath <abs path>/defender-court-<v>.json -Court <host label>
python scripts/agenterm-reputation-court.py qualify --manifest target/candidate-<v>/agenterm-<v>-candidate-manifest.json \
  --defender <receipt> --output target/reputation-qualification-<v>.json
```

The court refuses to report clean unless the EICAR control is detected
through the same `MpCmdRun` invocation, refuses any digest drift, and rejects
`AGENTERM_DEFENDER_VERDICT`.

## After Promotion

- `Release asset integrity` must pass; v0.1.20 needed an audit-only repair
  (`0dcd52f42`) to expect the newly published signing receipt. Re-audit with
  `gh workflow run release-integrity.yml -f promotion_run_id=<id>`.
- Delete the encrypted staging prereleases with
  `scripts/cleanup-local-candidate-encrypted.sh <id>` (it refuses anything but
  `local-candidate-encrypted-<sha>-<stamp>` prereleases) and the pin branch.
