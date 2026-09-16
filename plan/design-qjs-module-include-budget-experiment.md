# QJS module include budget — experiment design

**Status:** research record. Not a shipped capability, not gate evidence.

**Provenance honesty.** This specification was **written after** the first failure
(`scripts/qjs/singleton-safety-session-selftest.qjs` failing `./lint.sh` with
`loading wasm: module decode budget`) and after the bypass measurements. It does
**not** claim to have been pre-registered before them. What *was* fixed before the
implementations were attempted is the **two-way kill** in §4: it was decided, on
the upstream evidence in `plan/design-*` terms (see §4 and the RESULTS trace),
that if the compiler is *by contract* a text-include rather than an ECMA module
linker, then the fix belongs to the script side and not to the compiler. That
kill was fixed by the upstream reading recorded as #198 in the RESULTS trace,
i.e. before any implementation edit was made here.

## 0. Established facts

1. `scripts/qjs/singleton-safety-session-selftest.qjs` (756 lines, 47,045 B) is
   the entry of the task `singleton-safety-runner-session-selftest`
   (`agenterm.tasks.json`, `profile: tool`, `platforms: ["macos"]`).
2. `./lint.sh` fails with exactly one error for that file:
   `... -> qjs_check -> loading wasm: module decode budget`.
3. The same failure is reproducible in isolation with
   `target/debug/agenterm cli script check --profile tool <file>`: **exit 2**,
   same message, ~115–156 ms.
4. The decode ceiling for product-compiled `.qjs` artifacts is
   `QJS_ARTIFACT_MAX_DECODE_ITEMS = 524_288`
   (`crates/agenterm-qjswasm/src/slot.rs:56`), applied **only** to
   `Convention::JsV1` (`slot.rs:64-70`). Every other guest keeps tinyvm's
   default 262,144.
5. The check path uses **the same** ceiling as the engine that will run the
   bytes: `lib.rs:437-450 qjs_artifact_budget` → `:1695-1697
   validate_qjs_artifact_tool_with`, and `check-many` reaches it through
   `check_qjs_tool_with_modules` (`crates/agenterm-qjswasm/src/check_many.rs:94-98`).
   **So "the budget differs between check and production" is false here.**
6. In the sibling `tinyvm` checkout, `tinyvm-qjs` says an `import` is
   **compile-time inclusion, not a link**
   (`crates/tinyvm-qjs/src/lib.rs:541-544` there); every import re-runs
   `resolve` → `lex::tokenize` → parse
   (`crates/tinyvm-qjs/src/parse.rs:1019-1090` there),
   and `Parser.loading` (`parse.rs:391-418`, used at `:1026-1035`) is a **cycle
   stack**, not a memo. A module reached twice is therefore inlined twice.
7. Byte size is not the driver: `scripts/qjs/cu-macos-smoke.qjs` (159,045 B)
   checks **OK** under the identical command, as do `remote-ui-smoke.qjs`
   (113,320 B), `cu-linux-smoke.qjs` (96,563 B) and `script-smoke.qjs` (89,803 B).

## 1. Hard constraints

- Do not raise any limit. §0.5 shows the ceiling is the **same** one production
  loads under, so the "budget is inconsistent" escape does not apply.
- Do not touch `tinyvm` or its pin. §0.6 means the include semantics are the
  compiler's stated contract.
- Do not change the task manifest, the gate lists, or `./lint.sh`.
- The only lever this experiment may move is the **import graph of two files**
  (`scripts/qjs/lib/singleton_safety_browser_session.qjs` and
  `scripts/qjs/singleton-safety-session-selftest.qjs`), and only in a way that is
  a pure delegation (no behaviour change).

## 2. Bypass: same closure, one variable

The lever is "how many times does one module get inlined into the single
`.wasm`". The unit of comparison is a **minimal importer** placed in a
repository-local untracked directory (`target/probe-197/`), run through the
identical command with an explicit resolver root:

```
target/debug/agenterm cli script check --profile tool --project-root scripts/qjs <importer>
```

(`--project-root .` cannot resolve `lib/...`; `--project-root scripts/qjs` can.)

| importer imports | modules inlined | exit | wall |
|---|---|---:|---:|
| `lib/singleton_safety_runner` | runner, rh_compat | 0 OK | 102 ms |
| **`lib/singleton_safety_browser_session`** | session, **runner**, rh_compat | **0 OK** | 108 / 27 ms |
| `lib/rh_compat` | rh_compat | 0 OK | 63 ms |
| `lib/canonical_json` | canonical_json | 0 OK | 39 ms |
| **`lib/singleton_safety_runner` + `lib/singleton_safety_browser_session`** | **the same set as row 2** | **2 / `loading wasm: module decode budget`** | 151 / 135 ms |

Two rows inlined the *same* modules; adding one `import` line flipped OK to over
budget. That is the whole point of the experiment.

## 3. Boolean criteria

- **C1**: a closure inlining each module **once** must check OK.
- **C2**: the same closure inlining one module **twice** must exceed the budget.
- **C3**: the shipped entry, after removing the duplicate, must check OK.
- **C4**: no behaviour change: every re-exported helper is a pure delegation
  with the parameter list copied verbatim from its owner.

C1 and C2 hold (§2). **C3 does not** — see the RESULTS trace: after the duplicate
import was removed the entry **still** exceeds the budget. C4 holds by
construction.

## 4. Decision tree, kill criteria, time box

```
does the entry fail in isolation with a decode-budget error?      (yes, §0.3)
  └─ is the ceiling the same one production loads under?          (yes, §0.5)  -> do NOT raise a limit
      └─ is the compiler contract include, not link?              (yes, §0.6)  -> fix belongs to the SCRIPT side, never upstream
          └─ does de-duplicating the include fix the entry?       (NO, RESULTS) -> the remaining size is the entry's own closure
              └─ does any pure-delegation re-export change behaviour? (no, §3.C4) -> keep it, but it is NOT sufficient
```

Kill criteria, fixed **before** the implementation attempt:

1. **Kill-upstream**: if `tinyvm-qjs` is *by contract* a text-include compiler
   (§0.6), an upstream "de-duplication" would silently change visible behaviour
   (a diamond would stop running a module's top level twice) and needs a new
   precommitment. ⇒ **Upstream is NOT the fix.** This kill was decided from the
   upstream reading before any edit here.
2. **Kill-script-side**: if the duplicate include is the *whole* cause, removing
   it must make C3 hold. ⇒ **This kill fires** (C3 failed), so the script-side
   de-duplication alone is **not sufficient** either.
3. **Kill-raise-limit**: never. Superseded by §0.5.

Time box: one implementation attempt, one targeted check, then stop. **The
experiment stopped on the targeted check failing**; the task run and `./lint.sh`
were deliberately not executed afterwards.

## 6. Excluded

- Raising `QJS_ARTIFACT_MAX_DECODE_ITEMS` (or tinyvm's default) — §0.5.
- Changing `check-many`'s wiring — §0.5 shows it already carries the product
  ceiling.
- Adding a probe, a diagnostic field, or a count to the load path.
- Editing the task manifest or the gate lists.
- Rewriting the two scripts beyond pure delegation.
- Any change in `tinyvm` or its pin.

## 7. Not answered

- **How much** the duplicate costs: the load path reports only that the ceiling
  was crossed, so "how many items" is not observable without a bypass
  measurement, and no measurement was made.
- Whether the residual over-budget is the entry's own body, `lib/canonical_json`,
  or the single copy of the runner inlined through the session lib. §2 suggests
  the entry's own text is the difference (the same closure *without* the entry's
  756 lines checks OK), but that is an inference from one comparison, not a
  measurement.
- Whether a thin shared library (moving the six helpers and their private
  dependencies out of the runner) would be enough. Not attempted.

## 8. Result trace

See `research/qjs-module-include-budget/RESULTS.md` for the importer table, the
exact commands, exit codes and wall times, the #196 mis-attribution with the
#197 correction, and the #198 upstream verdict.

## 9. Subsequent rounds (recorded after this document was written)

- **#199** implemented the §4 script-side fix (six pure delegations in the
  session library + removal of the entry's direct runner import) and **failed
  its targeted check**: still `exit 2 / loading wasm: module decode budget`.
  ⇒ the duplicate include was real and removable, but **not sufficient**.
- **#200** tested the "split into two entries" route in a repository-local
  untracked directory: the small half checks OK (92 lines / 4,773 B) and the
  half that keeps the 516-line test does not (693 lines / 43,741 B, exit 2).
  ⇒ splitting by removing one test does not clear the ceiling either.
- **#201** tried to compress the refusal matrix itself. The table-driven
  `attempt` helper was implemented for 4 of the 19 call sites and measured
  **net-negative** (lines 756 → 756, bytes 47,013 → 47,078, still exit 2);
  the change was then **rolled back** to the #199 state. Two further findings
  from that round: (a) `context`/`replies`/`session_data`/`reply` are locals of
  `test_session_to_ownership`, so any helper must live inside that one test;
  (b) a table plus a single loop cannot express the cases without either
  losing the per-case `ctx` assertions or merely moving the same field text
  into a table — the optimistic net is ~5% of the file, which is not a step
  toward the ceiling.
  ⇒ **This experiment is closed with no production change shipped.** The
  duplicate include is real, but **every route §1 allows failed to restore the
  gate**: script-side de-duplication (#199) removed the duplicate and the entry
  still exceeded the ceiling; splitting the entry (#200) left the half that
  carries the 516-line test over budget; compressing the refusal matrix (#201)
  measured net-negative and was rolled back. The upstream route is excluded by
  §0.6 (include, not link, is the compiler's stated contract) and raising a
  limit by §0.5 (check and the engine share the ceiling). **Both script edits
  were rolled back to HEAD** — the tree carries this record, not a fix.
