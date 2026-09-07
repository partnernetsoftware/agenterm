# ACU compatibility corpus provenance

`compat-corpus.json` is a one-time, engine-neutral freeze of the legacy ACU rewrite contract. Its source blobs are the sibling moltbaby repository's `skills/acu/acu.ts` and `skills/acu/acu.test.ts`; their exact SHA-256 digests are recorded in the JSON.

The corpus was generated programmatically by importing `DYNAMIC_STAY_CORPUS`, `ACU_COMPATIBILITY_VERBS`, the three gap-classification sets, and `rewrite` from `acu.ts`, then parsing the `PREFERRED_PROBE` object literal from `acu.test.ts`. The 42 positive rows are the ordered intersection selected by `ACU_COMPATIBILITY_VERBS`; the five additional probe-only verbs are intentionally excluded. Absolute probe arguments are deterministically normalized to `fixture-<basename>` before evaluating and freezing their expected rewrite.

Generation fails unless there are exactly 95 dynamic rows and 42 positive rows, dynamic gap IDs and argv are unique, positive verbs and argv are unique, and every positive result is `exec` or `identity-kill`. A final serialized-text check rejects host-home and absolute application paths.

To update this fixture during migration, rerun the same extraction against the two source blobs, review any hash, count, classification, argv, or expected-result change, and regenerate the whole JSON rather than editing rows by hand. Bun is permitted only as the migration-time oracle used to evaluate the TypeScript source. It is not a runtime dependency and must not be invoked by `acu.qjs`, its launcher, or production compatibility tests.
