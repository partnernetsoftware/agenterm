# ACU compatibility corpus provenance

`compat-corpus.json` is a one-time, engine-neutral freeze of the legacy ACU rewrite contract. Its source blobs are the sibling moltbaby repository's `fbfda6294:skills/acu/acu.ts` and `fbfda6294:skills/acu/acu.test.ts`; their exact SHA-256 digests are recorded in the JSON. The live MoltBaby tree no longer contains these TypeScript files: its final adapter snapshot is archived for historical reading, while Git retains the exact frozen blobs named here.

The corpus was generated programmatically by importing `DYNAMIC_STAY_CORPUS`, `ACU_COMPATIBILITY_VERBS`, the three gap-classification sets, and `rewrite` from `acu.ts`, then parsing the `PREFERRED_PROBE` object literal from `acu.test.ts`. The 42 positive rows are the ordered intersection selected by `ACU_COMPATIBILITY_VERBS`; the five additional probe-only verbs are intentionally excluded. Absolute probe arguments are deterministically normalized to `fixture-<basename>` before evaluating and freezing their expected rewrite.

Generation fails unless there are exactly 95 dynamic rows and 42 positive rows, dynamic gap IDs and argv are unique, positive verbs and argv are unique, and every positive result is `exec` or `identity-kill`. A final serialized-text check rejects host-home and absolute application paths.

The migration oracle is sealed. Future capability repairs update the QJS compatibility implementation and its expected disposition without executing TypeScript or Bun. If historical regeneration is ever required for audit, recover the two exact Git blobs named above, review every hash/count/classification/argv/result change, and write a new corpus schema rather than silently rewriting this one. Bun is never a runtime or qualification dependency.
