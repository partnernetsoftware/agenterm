# Host-side field selection — first producer wired (`protocol-info --json --select`)

Receipt for the leaf that turns the frozen wire-cost verdict
(`owner = host-side field selection`, see
`plan/archive/design-qjswasm-host-reply-wire-cost-experiment.md` §8 and
`research/qjswasm-host-reply-wire-cost/RESULTS.md`) into one small, typed,
permission-free capability: a pure JSON projection core plus its first
product producer, and one real journey migrated off its `sh` + `grep`
workaround.

No new door, no `IpcResponse` wire change, no dyn/qjswasm/tinyvm/pin change,
no per-command field allowlist, no permission or policy semantics.

## 1. Identity

| Fact | Value |
|------|-------|
| Baseline HEAD (this leaf's parent) | `1ccdc8f3` |
| Lane | `target/json-select-leaf/{before-tree-head,before-build,after-build}` |
| before binary sha256 (parent tree, debug) | `a8c85acf1bf8799eae280fce89030f6b2aac7711ab836d333075e84ef9cc3919` |
| after binary sha256 (working tree, debug) | `5adb6857646e2edf021427eb35368abaada0a6a2c03aa745f724b64bec05df53` |
| tinyvm pin (both sides, identical) | `tinyvm?rev=9ac2598#9ac25980ddf73899201f8b8d56298b8af16a006b` |
| rustc | 1.97.0 (2d8144b78 2026-07-07) |
| budget (both sides, identical) | task `native-ipc-smoke`: `--profile tool --timeout-ms 120000 --max-operations 100000000` |

The two document-only commits the primary session landed while this leaf was
open (`170c7f4a`, `1ccdc8f3`, both `prd/PRD_02_34_agenterm_dyn.md` +
`prd/PRD_02_36_agenterm_qjswasm.md`) are untouched by this leaf: every file
below is code, script, or research evidence. The parent tree that produced the
*before* binary was re-archived at the current HEAD, so the before/after pair
differs only by this leaf's own edits.

## 2. What was built

- `src/json_select.rs` (new, pure): `Selector::parse` + `Selector::project` +
  `apply_selection_request`. Grammar: dotted keys, `key[]` element walk, `[]`
  root element walk. Limits: 64 paths, depth 8, and a 4 KiB text budget.
  Duplicates collapse; a shallower path keeps its whole subtree; an absent
  path stays absent (never an error); no wildcard, recursion, filter, or regex.
  Projection keeps the producer's own `serde_json::Value` — it never re-parses
  text — and can only ever drop members, so it can never be larger than the
  source. `apply_selection_request` owns the whole caller-facing contract
  (`--select` / `--json` precedence, the two refusal codes, parse, project)
  so the client-side and host-side producers of the same document cannot drift.
- `src/client/mod.rs`: `protocol_info_json_with_ui_bridge` split into
  `protocol_info_value_with_ui_bridge` (the producer's `Value`) plus a thin
  serializer. The no-selector path calls the same pair it always did. The
  **client-side** `protocol-info` path (no `--running`) now routes through
  `apply_selection_request` too, closing the silent-ignore hole where `--select`
  was accepted by the option validator and then discarded.
- `src/control_dispatch.rs`: `protocol_info_response(host, args)` applies the
  selection to that `Value` before serialization. No per-command constant and
  no support table: the command table entry plus the shared contract carry it.
- `src/commands.rs`: `protocol-info` declares the two options. `--json` is the
  machine-output contract flag that *enables* `--select`; it changes nothing
  about a call that does not select, and existing callers need no new flag.
- `scripts/qjs/native-ipc-smoke.qjs`: the `sh` + `grep` workaround
  (`PROTOCOL_KEYS`, `protocol_filter_script`, `run_filtered`,
  `parse_filtered_protocol` and their call sites) is deleted; the journey
  names the seven fields it reads and the host sends nothing else. No `sh`,
  no braces repair, no trailing-comma repair remains for `protocol-info`.

Corrections made during handoff review (the predecessor's draft was incomplete):

- the silent-ignore hole above (client-side path);
- the single-item `SELECTOR_PRODUCER` constant was removed;
- the test named `..._leaves_the_document_byte_identical` asserted `{}`, which
  is the opposite; it is split into a correctly-named empty-projection test and
  a real byte-preservation/never-grows test;
- `--json` is documented as a **selection precondition marker**, not a
  behaviour switch: `protocol-info` has always answered JSON.

## 3. Owner gates (all run on this leaf's tree)

Formatting / static: `cargo fmt -p agenterm -- --check` (clean) and
`CARGO_TARGET_DIR=target/json-select-leaf cargo clippy -p agenterm --all-targets -- -D warnings`
(clean).

Unit (lane `target/json-select-leaf`):

- `json_select` 12/12: named paths kept; nothing-matches yields `{}` and is not
  an error; fully-retained subtrees are byte-identical and never grow; shallower
  path wins; type mismatch keeps the value whole; duplicates collapse; element
  walks cover root and nested arrays; malformed selectors are named; `--select`
  without `--json` is refused while `--select` with it is applied; error code
  stable.
- `protocol_info` split 1/1: the no-selector rendering equals
  `to_string_pretty` of the producer's `Value`, a selection on the same value
  keeps the named paths, and the projection is smaller.
- `commands` 19/19 (option-spec ownership).

Black-box, against a real `agenterm server` on macOS (after host; the full
answer is 61,670 B on that run):

| Case | Result |
|------|--------|
| `protocol-info --running` (no selector) | exit 0, unchanged document |
| `--json --select pid,ui_bridge.ownership_mode,transport,identity_scope` | exit 0, 213 B, every selected value equals the full document's value |
| `--select` without `--json` | exit 1, named (`selection_requires_json`) |
| `--select 'a.*'` | exit 1, named (names the character and the key alphabet) |
| `--select` 9 segments deep | exit 1, named (depth limit 8) |
| `--select` 65 paths | exit 1, named (limit 64) |
| `--select ''` | exit 1, named (at least one path) |
| `--select 'pid,pid,pid'` | exit 0, one member |
| `--select 'ui_bridge,ui_bridge.ownership_mode'` | exit 0, whole subtree (shallower wins) |
| `--select 'pid,no.such.path'` | exit 0, `no.such.path` absent, no error |
| client path, `protocol-info --json --select pid` (no `--running`) | exit 0, `{ "pid": N }` — the silent-ignore hole is closed |
| client path, `protocol-info --select pid` (no `--json`) | exit 1, named |

No-selector byte preservation, three ways:

1. same host, before client vs after client (`--running`): **byte-identical**
   (61,670 B, `cmp` clean) — the client path did not move;
2. client-side path (`protocol-info`, no `--running`), before binary vs after
   binary: the only differing line in 61,824 B is `"pid"`; a before-vs-before
   rerun differs on exactly the same line, so the difference is the live
   process id and nothing else (23 keys, identical set, identical values);
3. cross-host: before server vs after server, both 61,670 B and 23 identical
   keys, differing only on `address`, `endpoint`, `pid`, and identical once
   those live-identity fields are masked.

## 4. Real journey, before and after

`scripts/qjs/native-ipc-smoke.qjs`, three runs per side, same pin, same budget,
same read sequence (the same seven fields), medians reported.

| Side | ok | exit_class | steps (median) | spread | host_bytes | host_ops |
|------|----|-----------|----------------|--------|-----------|----------|
| before (parent binary + old script) | true | success | 21,073,417 | 2,847 | 187,538 | 325 |
| after (this leaf + new script) | true | success | 20,120,279 | 7,411 | 173,974 | 323 |

- Δ steps = **953,138 fewer** (−4.52 % of the before median); Δ host_bytes =
  **13,564 fewer** (−7.23 %).
- 10 `protocol-info` calls per run (counted from the frozen envelope index of a
  real baseline run, `research/qjswasm-host-reply-wire-cost/replies/native-ipc-smoke/index.jsonl`),
  so the deltas divide to roughly 95,314 steps and 1,356 envelope bytes per
  call. That is derived by division, not separately measured, and it agrees
  with the frozen price model.
- The per-answer payload barely moves (the old `grep` fragment was 290 B, the
  selection is 287 B). The saving is the `sh` + `grep` pair per call plus the
  envelope shrink: the guest no longer ships a ~250-character `grep` alternation
  and a wrapped argv, and the 61.7 KB document is never built for the guest.
- Removed per run: 10 `sh -c` processes and 10 `grep` children. The journey
  makes no `sh` call for `protocol-info` any more (the one remaining `sh` is the
  journal `cat >>` append, untouched by this leaf).
- Both sides printed the same 14 `STEP` lines and `EVIDENCE
  control.native-local-ipc`, and exited 0 on all six runs.

## 5. Four-column engineering economy

**Column 1 — deleted / replaced.** Journey: −44/+34 lines (net −10), four
symbols and five call sites deleted. Per run: 10 `sh -c` + 10 `grep` child
processes eliminated, 10 shell-side filter passes replaced by one in-process
projection at the producer. Key lists move out of a shell alternation string
into the caller's request.

**Column 2 — preserved public semantics and failures.** The seven values are
identical; the journey's assertions, step order, and exit classes are unchanged
(all three runs `ok=true`, `exit_class=success`); the no-selector document is
unchanged (byte-identical same-host, identical after masking only run identity
cross-host); misuse is refused by name with a stable code at each of the five
misuse cases above; unknown options still come from the pre-existing validator;
no new door, no `IpcResponse` change, no authority semantics — a selector only
subtracts from a `Value` the caller was already going to receive.

**Column 3 — released.** Yes, measured: 213–287 B instead of 61,670 B per full
answer (≈215–290×), −13,564 envelope bytes and −953,138 steps per journey run
(−4.52 %), and one workaround removed from a real production journey. Source
touchpoints: −10 lines net in the journey; one new module (695 lines); 49 + 38 +
3 + 1 lines of glue in `client/mod.rs`, `control_dispatch.rs`, `commands.rs`,
`lib.rs` (35 of the 49 are a test).

**Column 4 — freed budget, next capability.** The same pure module can take a
second producer (the other host-reply families and the CU/qjswasm reply path)
with no new syntax, no new door, and no allowlist: each producer adds a `Value`
boundary plus one option-table entry, both routed through the one shared
`apply_selection_request` so the client and host producers of a document cannot
diverge. Freed steps/bytes are what a CU reply or a `shell-exec` answer can
spend instead of transiting the full document.

**New abstraction's own size, and net subtraction.** `src/json_select.rs` is
695 lines: **253 lines of production code** (112 of them the module's own
contract documentation), 275 lines of test, 16 test comments, 40 blank.
Wiring for the first producer: `client/mod.rs` 49 added lines (35 of its 84 are
the equivalence test), `control_dispatch.rs` 38, `commands.rs` 3 (rewritten),
`lib.rs` 1 = **91 wiring lines**. The journey *shrinks*: +34/−44, net −10.

Leaf total: +854 / −59 = **net +795 lines** (net +450 excluding the two test
blocks). The source is therefore **net additive today**: this leaf lays down a
general core and its evidence, and wires exactly one producer. It is *not* a
global fold.

Payback condition, stated honestly: marginal cost per additional producer is
the measured glue (26–89 lines, no per-command table, no new failure modes), and
the module's own size does not grow with producers. Each wired producer that
previously filtered a document with a shell pipeline or a second parse removes
that work. The source becomes a net subtraction when a **second producer is
wired and one non-court consumer is measured** in the same chain — the natural
candidates are the remaining host-reply producers and the CU reply/`shell-exec`
path that already carries a `max_output_bytes` shape. Until then the honest
statement is: general core placed, one producer wired, saving measured on one
real journey, global fold not yet earned.

## 6. Redaction

All artifacts here are free of home paths, account names, hosts, and tokens by
construction: the run envelopes contain no absolute path at all, and this file
and the checked-in run envelopes name paths repo-relatively. `./scripts/doc-redact-check.sh`
was run over every file in this directory.

## 7. Open items

- `./check.cmd --quick` / the full public-interface regression was **not** run
  in this leaf (Windows-only gates; this host is macOS). The directly owning
  gates listed in §3 were run.
- The CLI prints only the message for a refused call on the client path, not the
  error code; that matches the existing rendering of every other typed failure
  in that path, so no change was made. On the host path the refusal is a full
  `IpcResponse` with `error_code` / `error_category` set.
- Second-producer wiring and the payback measurement are the next leaf, and the
  CU/CLI doc deltas are handed back to the primary session rather than written
  here.
- The `--json` option is deliberately not a behaviour switch: it is a selection
  precondition marker. An earlier draft of this receipt described three refusal
  codes including `selection_unsupported`; only two exist
  (`selection_requires_json`, `selection_malformed`), because the grammar has no
  unsupported construct that is not a malformed path.
