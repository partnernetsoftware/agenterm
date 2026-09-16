# QJS session-selftest sharding — experiment design

**Status:** spec only. Nothing here is implemented, measured or claimed as shipped.
Written **before** any probe, edit or check run of this experiment.

## 0. Established facts

1. `scripts/qjs/singleton-safety-session-selftest.qjs` at HEAD is **756 lines /
   47,045 B** and is the entry of the task `singleton-safety-runner-session-selftest`
   (`agenterm.tasks.json`, `profile: tool`, `platforms: ["macos"]`).
   ⚠️ **47,013 B belongs to the #199 de-duplicated working tree only**; that change
   was **rolled back to HEAD** (`research/qjs-module-include-budget/RESULTS.md`),
   so it is not this experiment's baseline.
2. `./lint.sh` fails on it: `qjs_check -> loading wasm: module decode budget`,
   reproducible standalone as
   `target/debug/agenterm cli script check --profile tool <file>` ⇒ **exit 2**.
3. The ceiling is `QJS_ARTIFACT_MAX_DECODE_ITEMS = 524_288`
   (`crates/agenterm-qjswasm/src/slot.rs:56`, only for `Convention::JsV1`, `:64-70`)
   and **the check path carries the same ceiling the engine loads under**
   (`crates/agenterm-qjswasm/src/lib.rs:437-450` → `:1695-1697`;
   `crates/agenterm-qjswasm/src/check_many.rs:94-98`). ⇒ no limit change.
4. **In the sibling `tinyvm` repository** (not this repository): an `import` is
   compile-time inclusion, not a link — `crates/tinyvm-qjs/src/lib.rs:541-544`,
   every import re-running `resolve → lex::tokenize → parse`
   (`crates/tinyvm-qjs/src/parse.rs:1019-1090`; `Parser.loading` `:391-418` is a
   cycle stack, not a memo). ⇒ a module reached twice is inlined twice.
5. **Import multiplicity of the current closure (C4 baseline), measured from HEAD.**
   - entry imports `rh`(`:12`), `cj`(`:13`), `runner`(`:14`), `session`(`:15`);
   - `lib/singleton_safety_browser_session.qjs` imports `rh`(`:23`), `runner`(`:24`);
   - `lib/singleton_safety_runner.qjs` imports `rh`(`:11`), `cj`(`:12`),
     `singleton_safety_verdict`(`:13`), `build_manifest`(`:14`).
   ⇒ reach counts in one entry's closure: **`rh` 3, `cj` 2, `runner` 2**
   (entry + session), **`session` 1, `singleton_safety_verdict` 1,
   `build_manifest` 1**. ⚠️ **"each module once" is already false at HEAD** and is
   claimed nowhere in this spec.
6. Closed include-budget experiment: with the duplicate `runner` reach removed the
   entry **still** exceeded; a 92-line piece checked OK, a 693-line piece exceeded.
7. `context`, `replies`, `session_data`, `reply` are **locals of
   `test_session_to_ownership`** (`context` at `:269`); a helper outside it fails
   with `no host function named \`context\``.
8. **The digest is a pure function of fixed inputs** (this is what makes §9.3's
   golden possible): `lib/singleton_safety_runner.qjs:325-334`
   `digest_process_identity(domain, scratch, identity)` = three integer shape
   checks → `cj.canonical([pid, start_sec, start_usec])` → `sha256_scratch(domain,
   scratch, body)`, and `sha256_scratch`(`:306-315+`) writes exactly
   `domain + "\u0000" + body` to the scratch path and returns `rh.sha256_file` of
   it. The scratch path is only a container; it does not enter the digest. The
   two identities in play are fixed: `{pid: 901, start_sec: 1700000001,
   start_usec: 250000}` (`browser_digest`, entry `:282-283`) and `{pid: 900,
   start_sec: 1700000000, start_usec: 500000}` (`same_digest`, `:326-327`).
9. **The template read is separable from the runner.** `runner.load_runner_template`
   (`:24-29`) = `verdict.load_template(path)` + exactly two assertions
   (`runtime_cli_contract`, `window_identity_preimage` present); and
   `verdict.load_template` (`lib/singleton_safety_verdict.qjs:30-39`) is
   `rh.parse_file(path)` + its own section/binding checks — **it depends only on
   `rh`**.

## 1. Hard constraints

- **One support module, no duplicated fixture.** Exactly **one** new test-only
  library carries the fake executor/adapter/options/latest-state builders, the
  fixture constants, `context`/`replies`/`session_data`/`reply`, and the
  template read. No entry may re-declare them.
- **The support module must not import `runner` directly.** It gets everything
  runner-side **through `session`** (§9.1), because `session` already imports
  `runner` (`:24`) — importing both would recreate the #197 double reach.
- **Import multiplicity may not exceed §0.5's baseline, per shard** (C4).
- **Every original label and assertion belongs to exactly one piece**, and the
  **union** of the three pieces' label/assertion multisets must equal the current
  set exactly (C3). Nothing may be dropped, merged, weakened or reordered within a
  case.
- **Per-case order and per-shard order are fixed**; the cross-shard total order is
  explicitly **not** preserved (accepted by review).
- **Each piece owns its own run root and cleans it up** on success and failure
  (`…-selftest-a` / `-b` / `-c`).
- **No task dependencies and no orchestration task.** `task run` never executes
  `dependencies` (`src/client/mod.rs:3006-3019` → `:3145-3167+` reads
  `env`/`entry`/`profile`/`env_allow`, never `dependencies`).
- **The manifest is a necessary part of the design**: exactly **two new tasks +
  their contracts** (one for A, one for C) with the **existing task/entry
  redefined as B** and its `description` updated **truthfully** (it currently
  claims the whole suite). This is a **catalog surface expansion**.
- **Fixed shards**: **A** identity + CLI guards; **B** happy / admission /
  identity / refusal through foreign-session (`:285-472`); **C** terminal /
  cleanup / status2 (`:473-740`).
- **No limit change, no sibling-`tinyvm` change, no product-module change, no
  semantic change.** `lib/singleton_safety_browser_session.qjs` and
  `lib/singleton_safety_runner.qjs` are **read-only** here.

## 2. The lever and the single variable

**Lever:** entry size. **Single variable:** which assertions/setup live in each
entry. Everything else held constant (ceiling, libraries, product entry, resolver
roots, command).

| piece | source | lines (est.) |
|---|---|---:|
| support lib (test-only) | current `:20-40` helpers + `:91-215` fixture builders + the `:233-284` fixture/constructors lifted out of the test + the template read | ~180 |
| entry A | current `:42-90` (identity + CLI guards) + digest known-answer + `load_runner_template` validation + `run()` | ~90 |
| entry B | current `:285-472` | ~190 |
| entry C | current `:473-740` | ~270 |

## 3. Boolean criteria

- **C1**: each of A, B, C passes `cli script check --profile tool` with **exit 0**
  on the same 524,288 ceiling. Any failure ⇒ kill (§4).
- **C2**: the three tasks each run **exit 0**, and `./lint.sh` is green.
- **C3**: **the union of the three pieces' original label/assertion multisets is
  exactly equal to the current multiset** (counts included, no omissions or
  duplicates), and no original label occurs in two pieces. Exactly two additional,
  named strengthening assertions are allowed: A carries the two runner
  **known-answer checks** from §9.3 (the two frozen goldens checked against
  `runner.digest_process_identity`) **and B/C check the product ack against the
  same independent goldens** — the two sides are independently sourced, so neither
  proves itself. No other assertion addition is allowed. Because the manifest gains two tasks, this is explicitly a
  **catalog surface expansion**: the original task now covers B only and must say
  so.
- **C4** (import multiplicity, **counted per shard** — no claim that a module is
  inlined once):

  | module | A | B | C |
  |---|---:|---:|---:|
  | `session` | **0** | 1 (support) | 1 (support) |
  | `runner` | 1 (entry) | 1 (**only via session**) | 1 (**only via session**) |
  | `verdict` | 1 (via runner) | 1 (via runner) | 1 (via runner) |
  | `build_manifest` | 1 (via runner) | 1 (via runner) | 1 (via runner) |
  | `rh` | 1–2 (entry, runner) | ≤ 3 (support, session, runner) | ≤ 3 (same) |
  | `cj` | 1 (via runner) | 1–2 (support if needed, runner) | 1–2 (same) |

  Compared with §0.5's baselines (`rh ≤ 3`, `cj ≤ 2`, `runner ≤ 2`, `session ≤ 1`,
  `verdict ≤ 1`, `build_manifest ≤ 1`), **no shard adds a path** — and A/B/C never
  reach `runner` twice.
- **C5**: the three pieces' roots are distinct and each removes its own root on
  success and on failure.

## 4. Decision tree, kill criteria, time box

```
does the HEAD entry exceed the ceiling?                     (yes, §0.2)
  └─ same ceiling as the engine loads under?                (yes, §0.3) -> do NOT raise it
      └─ would any shard reach runner/session twice?        (C4 must be no) -> fix the graph first
          └─ do A, B and C all check green?                 (C1) -> proceed to C2/C3/C5
              └─ does any piece still exceed?               -> KILL: sharding cannot meet the ceiling
```

Kill criteria (fixed before any probe):

1. **Any of A/B/C still exits non-zero on `check`** ⇒ kill.
2. **Any original label or assertion lost, duplicated across pieces or weakened,
   or any new assertion beyond the two named digest known-answer checks** (C3) ⇒ kill.
3. **Any shard exceeds the §0.5 baseline path count, or reaches `runner` twice**
   (C4) ⇒ fix the graph; if it cannot be fixed without duplicating the fixture, kill.
4. **Roots shared or leaked** (C5) ⇒ kill.
5. **Extra orchestration required** (a fourth driver task, or any `dependencies`
   edge) ⇒ kill. Adding **only** the two A/C tasks is allowed.
6. **Any limit, sibling-`tinyvm`, product-module or product-code change needed** ⇒
   kill. Two `agenterm.tasks.json` task entries + contracts are the only manifest
   change allowed.
7. **The §9.3 golden cannot be closed without weakening an assertion or touching a
   product module** ⇒ **pre-judged kill, experiment stops** (see §9.3's fallback).

Time box: this spec round; then — after approval — one implementation round, one
targeted check per piece, then the three `task run`s and `./lint.sh`. Stop at the
first kill.

## 6. Excluded

- Raising the ceiling or changing the sibling `tinyvm` / its pin.
- Editing `lib/singleton_safety_browser_session.qjs`, `lib/singleton_safety_runner.qjs`
  or `lib/singleton_safety_verdict.qjs`.
- Deleting/merging/re-scoping cases; changing label text; relaxing an expected error.
- A fourth driver task, or any `dependencies` edge.
- Any product code path.

## 7. Not answered

- Whether it works at all — §2's numbers are estimates; C1 is the test.
- Whether a two-way split would suffice (it has a larger piece ⇒ fails first).
- Whether `./lint.sh`'s runtime budget holds with three tasks; and since
  `scripts/qjs/lint.qjs:209-212` takes `git ls-files "scripts/qjs/*.qjs"`, the gate
  will compile **two more** entries — cost unmeasured.
- Whether the pieces may run concurrently (C5 asks only for distinct roots).

## 8. Result trace

To be filled after an approved implementation round: per-piece lines/bytes/exit/wall,
the C3 union comparison, the C4 per-shard path counts, the §9.3 golden reproduction,
the three `task run` results and `./lint.sh`, plus the final verdict.

## 9. Specification self-review

### 9.1 Import graph (this supersedes the earlier draft, which was wrong)

The previous draft had support import **both** `runner` and `session` while
claiming `runner` was reached once — but `session` itself imports `runner`
(`lib/singleton_safety_browser_session.qjs:24`), so that graph gives `runner`
**two** paths inside support (`support→runner` and `support→session→runner`) and
reproduces #197 exactly. Corrected graph:

```text
support (new, test-only)   imports: session, rh            [+ cj only if it needs canonical JSON]
                           NO direct import of runner.
                           Everything runner-side arrives through session.
                           Re-exports (pure delegations, from session's own surface):
                             session_to_ownership
                           Reads the template itself:  rh.parse_file(<same tracked template path>)
                           Carries the lifted fixture (constants, fake executor/adapter/options/
                             latest-state, context, replies, session_data, reply)

entry A                    imports: runner, rh, cj         (NO support)
                           identity + CLI guards; digest known-answer (§9.3);
                           runner.load_runner_template validation (§9.4)

entry B / entry C          imports: support                (NO runner, NO session)
```

Reach counts are in C4. A reaches `runner` once and `session` **zero** times;
B/C reach `session` once and `runner` once, and that one reach is **through
session**, which is the same file the baseline already had.

### 9.2 Three conflicts resolved

1. **"Each entry calls the product entry" vs C4** ⇒ resolved by §9.1: A never
   touches `session`; B/C reach it only through support, which reaches it once.
2. **Each piece owning its root vs C3** ⇒ roots are
   `target/singleton-safety-session-selftest-{a,b,c}`, a per-piece constant passed
   through support. Paths change, assertions do not.
3. **The current `run()`** ⇒ each piece gets its own `run()`; the **original**
   label/assertion union is unchanged (C3), with only the two named digest
   known-answer checks added. The cross-shard order is dropped by agreement;
   per-case and per-shard order is preserved.

### 9.3 The digest: dynamic computation becomes a frozen golden (review item 3 and 6)

Today `browser_digest`/`same_digest` are computed at run time by
`runner.digest_process_identity` (entry `:282-283`, `:326-327`) and used as
`expected_root_digest` in ~15 places. If B/C keep calling the runner for that, they
would need the runner — which §9.1 forbids. §0.8 shows the digest is a **pure
function** of `(domain, pid, start_sec, start_usec)`: the scratch path is only a
container and does not enter the hash. Therefore:

- **Two goldens are frozen offline** (one 64-hex constant per identity, derived
  once from `cj.canonical([pid, sec, usec])` and the domain string — computed
  outside the court, not at run time), and they are **the same constants** in A,
  B and C.
- **A proves the goldens are the product's own answers**: it calls
  `runner.digest_process_identity` with the two fixed identities and asserts the
  results equal the frozen constants (a **known-answer test**). A already imports
  the runner, so this costs nothing and needs no new reach.
- **B/C use the frozen constants** as `expected_root_digest` and check the
  product's browser-root acknowledgement against them. They never call the runner.
- **Neither side proves itself**: A's constants come from the offline derivation
  and are cross-checked against the runner; B/C's constants come from the same
  offline derivation and are checked against the **product's** ack. If the digest
  algorithm ever drifts, A reddens; if the product's ack drifts, B/C redden.

**Fallback (pre-judged kill).** If it turns out the golden cannot be reproduced
offline (for example `cj.canonical`'s byte shape cannot be reproduced outside the
court) **and** the alternative would be to weaken an assertion or to touch a
product module, then this experiment is **killed here and not implemented** — that
is kill #7 in §4, decided in advance rather than discovered mid-round.

### 9.4 The template (review item 5)

`runner.load_runner_template` is `verdict.load_template` **plus two assertions**
(`runtime_cli_contract`, `window_identity_preimage` present; `:24-29`), and
`verdict.load_template` itself is `rh.parse_file` plus its own checks
(`lib/singleton_safety_verdict.qjs:30-39`). So the read is separable from the
runner:

- **support** reads the **same tracked template file** with `rh.parse_file` and
  uses the resulting object for the fixture — it does **not** go through the
  runner and does **not** claim to validate it;
- **A** keeps the runner's validation by calling
  `runner.load_runner_template(<same path>)`, so the two assertions that the
  current entry relies on are still executed **in the same task suite, on the same
  file**.

⇒ **Joint coverage is not weakened**: the file is the same tracked file, the
object B/C use is the parse of that file, and the "this template has the two
required sections" claim is still asserted — just in A rather than in every piece.

### 9.5 What this document deliberately does not have

- **No probe, no measurement, no edit**: the only artifact of this round is this
  document. The two goldens in §9.3 are **not** computed here.
- **No manifest change yet**: `agenterm.tasks.json` is untouched until approval.
- **No re-litigation of closed axes** (module include budget, receipt/projection,
  digest/catalog, pin, release/six-cell/device/GUI/native court).
