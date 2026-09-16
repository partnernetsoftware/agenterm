> **已归档 2026-09-16 — 已判决。** Canonical read/charge 已在仓内交付；single evaluation 因编译器接口没有 identity channel 而停止，仓内 parser/rewriter 路线被明确否决。
> 现行权威：`prd/PRD_02_36_agenterm_qjswasm.md`；测量与判决证据保留在 `research/qjs-module-identity/RESULTS.md`。

# QJS module resolution identity experiment

This experiment does not enter must-ship by itself. It decides which part of
recursive module accounting can be delivered inside this repository without
inventing a second module system.

| Field | Value |
|---|---|
| Date | 2026-09-15 |
| Purpose | Separate canonical file accounting from compiler module identity |
| Implementation | `research/qjs-module-identity/` |
| Required reading | `AGENTS.md`, `prd/PRD_02_36_agenterm_qjswasm.md`, `docs/agenterm-rust-cheatsheet.md` |
| Source discipline | Repository-visible interfaces and public CLI only; do not enter the upstream tinyvm checkout |

## 0. Fixed background

1. Single-file `check`, `run`, and source `hash` call the same product resolver.
2. `check-many` already accounts canonical imported files once for reading and
   budgeting.
3. The pinned compiler-facing resolver type visible in this repository is
   `Fn(&str) -> Option<String>`: specifier in, source text out.
4. Source-byte, module-count, cancellation, and deadline limits are robustness
   controls, not authorization policy.

## 1. Hard constraints

1. One canonical file may be read and charged only once per compilation.
2. Imported source must be read through the per-source ceiling plus one byte;
   a metadata precheck alone must not permit growth between stat and read to
   allocate without the declared bound.
3. The implementation must preserve a named failure after the upstream resolver
   reports its generic unresolved-module error.
4. No import/export parser or source rewriter may be added here.
5. No result may claim canonical single evaluation unless canonical identity
   crosses the compiler interface or a public court proves equivalent behavior.
6. The disease detector is any attempt to call a source cache a module-identity
   cache, or to silently discard a second specifier to make a court green.

## 2. Minimal experiment

Use one file with a top-level observable effect and import it from one entry by
both `lib/side` and `lib/side.qjs`. Compare:

| Variant | Resolver answer | Purpose |
|---|---|---|
| A | Read source independently for each specifier | Current single-file baseline |
| B | Cache source by canonical file path, return the cached text for the second specifier | Isolate whether canonical accounting changes compiler identity |

No compiler, pin, source grammar, or runtime setting changes between variants.

## 3. Frozen criteria

| ID | Criterion | Nature | Pass condition |
|---|---|---|---|
| C1 | Canonical read/charge count | Boolean | Variant B reads and charges the file once |
| C2 | Top-level evaluation count | Boolean | Variant B emits the observable effect once |
| C3 | Identity channel inventory | List | A repository-visible argument or return field can carry canonical identity |
| C4 | Second module truth | Safety | No product-side import parser or rewriter is introduced |

The same public CLI, compiler pin, entry, and fixture are used for C1 and C2.
Counts are exact integers, not timing proxies.

## 4. Decision tree, kill criterion, and time box

1. If C2 passes and C4 passes, canonical caching is sufficient; deliver one
   shared resolver/accounting owner.
2. If C2 fails and C3 is empty, canonical single evaluation is blocked on an
   upstream identity-bearing interface. Deliver C1 separately and name the
   boundary.
3. If C3 is non-empty, test that interface before proposing an upstream change.
4. A product-side parser or rewriter fails C4 and is rejected regardless of C1
   or C2.

Kill criterion: if cached canonical source is still evaluated twice and the
visible compiler interface carries no identity field, stop. Do not approximate
single evaluation.

Time box: stop as soon as C1 through C4 have one reproducible answer. Do not
prototype a module parser or change the upstream pin.

## 5. Result files

- `research/qjs-module-identity/RESULTS.md`

## 6. Excluded alternatives

| Alternative | Reason |
|---|---|
| Product-side import/export expansion | Creates a second module grammar and evaluation owner |
| Silent discard of an alias import | Changes program meaning while pretending to deduplicate |
| Lowercasing Windows paths | String case folding is not a reliable filesystem identity algorithm |
| Profile-specific enforcement | Module accounting is a robustness invariant, not a profile permission tier |

## 7. Not answered here

- The shape or landing schedule of a future upstream identity-bearing API.
- Whether alias imports should later be accepted once or rejected by name.
- Platform-native file identity beyond the existing canonical `PathBuf` rule.

## 8. Verdict

C1 passes: the existing `check-many` resolver demonstrates one canonical read
and charge. C2 fails: the public fixture prints its top-level marker twice when
the same file is imported as `lib/side` and `lib/side.qjs`. C3 is empty in every
repository-visible module compile entry; each accepts only the source-returning
callback. C4 therefore rejects the only repository-local route that could force
single evaluation, namely reimplementing module expansion.

Decision trace: C2 failed, then C3 was empty, so branch 2 applies. Canonical
read and budget accounting may ship here; canonical single evaluation is
blocked on an identity-bearing compiler interface. No criterion was changed
after observing the result.
