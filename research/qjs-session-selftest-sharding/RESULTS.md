# QJS session-selftest sharding results

## Outcome

The oversized `singleton-safety-runner-session-selftest` was split into three
independently runnable catalog tasks backed by one test-only support module.
All three entries load below the unchanged QJS artifact decode ceiling, execute
successfully, preserve every original assertion label exactly once, and clean
three distinct owned roots. Repository lint is green after the new files are in
the Git index.

No qjswasm limit, tinyvm source or pin, or product orchestration module changed.
The product-facing files `lib/singleton_safety_runner.qjs`,
`lib/singleton_safety_browser_session.qjs`, and
`lib/singleton_safety_verdict.qjs` are byte-unchanged.

## Changed surface

- `scripts/qjs/singleton-safety-session-selftest-identity.qjs`: shard A,
  identity parsing, CLI guards, template validation, and two digest
  known-answer checks.
- `scripts/qjs/singleton-safety-session-selftest.qjs`: shard B, happy path,
  acknowledgement/causality, and admission/refusal cases.
- `scripts/qjs/singleton-safety-session-selftest-terminal.qjs`: shard C,
  terminal, cleanup, removal proof, and second-status exactness cases.
- `scripts/qjs/lib/singleton_safety_session_selftest_support.qjs`: the only
  copy of the fake executor, adapter, fixture records, roots, and frozen digest
  values. It imports `session` but never imports `runner` directly.
- `agenterm.tasks.json`: two new tasks/contracts and a truthful description for
  the retained shard-B task. There is no orchestration task and no dependency
  edge.

## Fixed digest facts

The runner hashes
`domain || 0x00 || canonical([pid,start_sec,start_usec])`; the scratch path is
not part of the input. Before editing, two independent local calculations
(Python SHA-256 and `printf | shasum -a 256`) agreed:

| identity | canonical body | SHA-256 |
|---|---|---|
| browser | `[901,1700000001,250000]` | `1b51aed230ce7d94a8c7b067e4292eaac26dc13dc0466ac43825ee2bde94a7b5` |
| same-pid | `[900,1700000000,500000]` | `ae1f5c3dc6678bedf38cd6c71190c09a0b486db963d2334086103f7a8e207245` |

Shard A checks these constants against `runner.digest_process_identity`;
shards B and C use the same independently derived values when judging the
product acknowledgement. Neither side manufactures the value it judges.

## Measurements

All targeted checks used the existing debug binary, profile `tool`, project
root `.`, and the unchanged 524,288-item JS-V1 decode ceiling.

| shard | lines | bytes | targeted check | task run |
|---|---:|---:|---:|---:|
| A | 116 | 5,978 | exit 0 / 96 ms | exit 0 / 541 ms |
| B | 223 | 14,940 | exit 0 / 121 ms | exit 0 / 1,930 ms |
| C | 287 | 18,948 | exit 0 / 124 ms | exit 0 / 3,204 ms |
| former HEAD entry | 756 | 47,045 | exit 2 / decode budget | not applicable |

The label multiset changed from 69 distinct original labels to those same 69
plus exactly the two pre-authorized labels
`identity_digest_browser_known_answer:` and
`identity_digest_same_known_answer:`. No original label was lost or shared by
two shards. Normalizing support qualifiers in the moved B/C bodies made their
case blocks line-identical to the HEAD blocks.

The first lint run passed but did not include the three untracked new files,
because its manifest is built from `git ls-files`. After staging the exact five
implementation paths, lint was run again and passed in 16.6 seconds. The second
run is the delivery evidence; the first is not used as coverage evidence.

## Import-accounting correction

The precommitted specification's short C4 table stopped expansion at
`singleton_safety_verdict` and `build_manifest`, even though both import
`rh_compat` and `canonical_json`. Calling its `rh`/`cj` numbers full closure
counts was therefore incorrect. A complete recursive walk gives:

| closure | `rh` | `cj` | `runner` | `session` | `verdict` | `build_manifest` |
|---|---:|---:|---:|---:|---:|---:|
| HEAD | 8 | 5 | 2 | 1 | 2 | 2 |
| A | 4 | 2 | 1 | 0 | 1 | 1 |
| B | 5 | 3 | 1 | 1 | 1 | 1 |
| C | 5 | 3 | 1 | 1 | 1 | 1 |

This is a specification-accounting defect, not hidden favorable filtering.
The stronger complete walk still proves the governing invariant: every shard
has no more paths than HEAD, and the runner path that caused the duplicate
include falls from two to one. The original shallow numbers are retained in
the precommit history and are not presented as full-transitive evidence.

## Gates

- A/B/C targeted checks: exit 0.
- Three catalog task runs: exit 0.
- Task/contract sets: 325/325, no duplicate or orphan IDs.
- Original-label multiset: exact preservation; two named additions only.
- Three owned roots: distinct and absent after task completion.
- `./lint.sh` after exact staging: `PASS: repository lint` (exit 0).
- `git diff --check`: exit 0.
- Document redaction check over all changed files: clean.

The test-only support file is not a standalone entry. A direct single-file
check of that library was mistakenly attempted during development and failed
module resolution; it is not C1 evidence. Its real consumers, shards B and C,
both compile and execute it successfully, and the post-staging repository lint
also includes it through the repository check-many manifest.
