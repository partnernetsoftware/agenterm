# QJS module include budget — results

Design: `plan/design-qjs-module-include-budget-experiment.md` (written after the
first failure and the bypass measurements; see the honesty note there).

## Trigger

`./lint.sh` exit 1 with a single failure:

```
scripts/qjs/singleton-safety-session-selftest.qjs -> qjs_check -> loading wasm: module decode budget
invocation check-many-48177-313
```

## Reproduced in isolation

```
target/debug/agenterm cli script check --profile tool scripts/qjs/singleton-safety-session-selftest.qjs
{"code":"qjswasm_backend","exit_class":"configuration","message":"loading wasm: module decode budget"}
exit=2   elapsed=156 ms   (a second run: exit=2, 115 ms)
```

(Without `--profile tool` it first reports `this engine has no host function
named 'arg'`; that is the wrong profile, not this problem.)

## Size is not the driver

Same command, same binary, larger files all pass:

| file | bytes | result |
|---|---:|---|
| `scripts/qjs/cu-macos-smoke.qjs` | 159,045 | OK, exit 0 |
| `scripts/qjs/remote-ui-smoke.qjs` | 113,320 | OK, exit 0 |
| `scripts/qjs/cu-linux-smoke.qjs` | 96,563 | OK, exit 0 |
| `scripts/qjs/script-smoke.qjs` | 89,803 | OK, exit 0 |
| **`scripts/qjs/singleton-safety-session-selftest.qjs`** | **47,045** | **exit 2, decode budget** |
| `scripts/qjs/singleton-safety-runner-selftest.qjs` | — | OK, exit 0 |

## Importer combinations (the decisive comparison)

Minimal importers were placed in a repository-local **untracked** directory
(`target/probe-197/`, written with a patch tool, deleted afterwards) and run with
an explicit resolver root:

```
target/debug/agenterm cli script check --profile tool --project-root scripts/qjs <importer>
```

| # | importer imports | modules inlined into the one `.wasm` | exit | wall |
|---|---|---|---:|---:|
| a | `lib/singleton_safety_runner` | runner, rh_compat | 0 OK | 102 ms |
| b | **`lib/singleton_safety_browser_session`** | session, **runner**, rh_compat | **0 OK** | 108 ms |
| c | `lib/rh_compat` | rh_compat | 0 OK | 63 ms |
| d | `lib/canonical_json` | canonical_json | 0 OK | 39 ms |
| e | **`lib/singleton_safety_runner` + `lib/singleton_safety_browser_session`** | **same set as `b`** | **2, `loading wasm: module decode budget`** | 151 ms |
| f | `lib/singleton_safety_browser_session` (repeat of `b` under the compliant path) | session, runner, rh_compat | 0 OK | 27 ms |
| e′ | `lib/singleton_safety_runner` + `lib/singleton_safety_browser_session` (repeat) | same set | **2** | 135 ms |

Rows `b`/`f` and `e`/`e′` inline the **same modules**; the only difference is one
extra `import` line naming a module the session library already imports.

## #196 mis-attribution, #197 correction

- **#196** concluded "real script over budget, **not** emitted duplication", and
  excluded duplication using the size table above (a 159 KB script passes, so a
  47 KB one "cannot" be duplicating). **That inference was wrong**: a size
  comparison cannot exclude duplication — a 47 KB closure that inlines a 59 KB
  module twice can cost more than a 159 KB closure that inlines everything once.
- **#197** corrected it with the same-closure comparison above: `b` OK vs `e`
  over budget, identical inlined module set.

## #198 upstream verdict (include, not link)

In the sibling `tinyvm` checkout, `crates/tinyvm-qjs/src/lib.rs:541-544`
states the contract verbatim: *"An
`import` is compile-time inclusion, not a link: the imported source is parsed
into a scope of its own …"*. Mechanism: `parse.rs:1019-1090` re-runs
`resolve` → `lex::tokenize` → parse for **every** import; `Parser.loading`
(`parse.rs:391-418`, checked at `:1026-1035`) is a **cycle stack**, not a memo,
so a module reached twice is inlined twice. There is **no diamond import test**
anywhere in that checkout's `crates/tinyvm-qjs/`. The crate is byte-identical
between the pin `9420045` and sibling HEAD `6889c09` (`git diff --stat
9420045..HEAD -- crates/tinyvm-qjs`, run from the sibling checkout root, is
empty), so HEAD does not fix it either.

⇒ De-duplicating upstream would turn include into link, change visible behaviour
(a diamond would stop running a module's top level twice) and needs a new
precommitment. **Upstream is not the fix**; the fix has to be on the script side.

## Script-side attempt (#199) and its result

Change made, keeping behaviour identical by construction — six **pure
delegations** added to `scripts/qjs/lib/singleton_safety_browser_session.qjs`
(`load_runner_template`, `digest_process_identity`,
`macos_start_identity_to_identity`, `require_timeout_range`,
`require_session_name`, `require_url`; parameter lists copied verbatim from
`lib/singleton_safety_runner`), and in
`scripts/qjs/singleton-safety-session-selftest.qjs` the direct
`import * as runner from "lib/singleton_safety_runner"` was **removed** with all
`runner.` call sites rewritten to `session.`.

Verification:

```
grep -n "runner\."              scripts/qjs/singleton-safety-session-selftest.qjs   -> no match
grep -n "singleton_safety_runner" scripts/qjs/singleton-safety-session-selftest.qjs -> no match
target/debug/agenterm cli script check --profile tool scripts/qjs/singleton-safety-session-selftest.qjs
  -> {"code":"qjswasm_backend","exit_class":"configuration","message":"loading wasm: module decode budget"}
  -> exit=2, 115 ms
```

**The de-duplication alone does not fix it.** With the runner now inlined once
the entry still crosses the ceiling: the same closure *minus the entry's 756
lines* checks OK (row `b`/`f`), while the entry itself does not. The honest
reading is therefore:

- the duplicate include was **real** and **removable** (row `b` vs row `e`), and
  the change removes it without altering behaviour;
- but it is **not sufficient**: the entry's own closure is already at the
  ceiling (or above) even with every module inlined exactly once.

No measurement exists for *how far* over either state is — the load path reports
only that the ceiling was crossed — so "how much narrower should the entry be"
is not answered here.

Per the design's time box, verification stopped at the targeted check: the task
run and `./lint.sh` were **not** executed after the failure. No limit was raised,
no manifest or gate list was touched, and nothing in `tinyvm` or its pin moved.

## Files changed by the attempt

```
 M scripts/qjs/lib/singleton_safety_browser_session.qjs      (+18)
 M scripts/qjs/singleton-safety-session-selftest.qjs         (+41 / -24)
```

`git diff --check` exit 0. No commit was made.

## Later rounds

### #200 — split into two entries (bypassed, untracked)

Two temporary entries were generated in a repository-local untracked directory
and checked with `--project-root scripts/qjs`:

| piece | content | lines | bytes | exit | wall |
|---|---|---:|---:|---:|---:|
| A | helpers + `test_macos_identity` + `test_cli_guards` | 92 | 4,773 | **0 OK** | 110 ms |
| B | helpers + session helpers + `test_session_to_ownership` + tail | 693 | 43,741 | **2 / decode budget** | 113 ms |

The slice that keeps the 516-line test still exceeds the ceiling; splitting by
removing one test is not enough. (A first attempt at B omitted the `:20-40`
helper block and failed with `no host function named \`count_item\`` — a
compile error, not a budget error; helpers must travel with the slice.)

### #201 — compressing the refusal matrix (measured, then rolled back)

The 25 `try/catch` blocks in `test_session_to_ownership` (19 calls + 6
single-line `finish`/`abort`) were the largest repeated shape. A table-driven
`attempt(name, over, replies_over, fail_verb)` helper was implemented for **4 of
the 19** call sites and measured:

| | before | after |
|---|---:|---:|
| lines | 756 | **756 (±0)** |
| bytes | 47,013 | **47,078 (+65)** |
| check | exit 2 | **exit 2, 115 ms** |
| entry diff stat | +41 / −24 | +102 / −51 |

⇒ **net-negative**: the helper is 15 lines and the four sites save 13. Extrapolated
to all 19 the net is ≈ −46 lines / ≈ −3 KB (≈ −5–6% of the file), and the six
`finish`/`abort` blocks contain no construction to factor, so the second round
would save 0–1 line each. **The change was rolled back to the #199 state**
(`attempt(` count 0, diff stat +41/−24, check back to the decode-budget error).

Two structural facts from that round:

1. `context` (`:269`), `replies`, `session_data` and `reply` are **locals of
   `test_session_to_ownership`** — a helper placed outside it fails immediately
   with `this engine has no host function named \`context\``. Any abstraction
   must live inside that single test, so it can only factor the directory
   derivation and the `try/catch` frame.
2. A table plus one loop cannot express these cases without either dropping the
   per-case `ctx` assertions (`plat_ctx.state.aborted`, `badarg_ctx.state.aborted`)
   or simply relocating the same field text into a table. The subset itself is
   not the obstacle — `for...of`, array/object literals and indexed loops are
   used 174+1759 times across `scripts/qjs/**`, and the sibling
   `scripts/qjs/singleton-safety-court-selftest.qjs` is already table-shaped —
   the obstacle is that each case's fields, label and extra assertion must all
   survive verbatim.

⇒ **Closed with no production change shipped.** The de-duplication this file
first reported as a real reduction (#199) was **rolled back to HEAD** together
with the #201 abstraction, because no allowed route restored the gate:

| route | measured outcome |
|---|---|
| #199 remove the duplicate include | duplicate gone, entry **still exit 2** |
| #200 split the entry in two | the half that keeps the 516-line test **still exit 2** |
| #201 compress the refusal matrix | **net-negative** (bytes +65), rolled back |
| #198 upstream de-duplication | excluded: the compiler contract is *include, not link* |
| raise the ceiling | excluded: check and the engine share it |

`git diff` for both scripts is empty at HEAD; the experiment's deliverable is
this record. The `./lint.sh` failure described at the top of this file is
**still present** — it is a known red, not a pending re-run.
