# qjswasm host-reply wire cost — result

Status: **VERDICT — owner = host-side field selection. No capability-state change,
no product change.** The experiment ended as §4 says it must; the implementation
it names is a new leaf that has not been started.

Experiment: [`plan/archive/design-qjswasm-host-reply-wire-cost-experiment.md`](../../plan/archive/design-qjswasm-host-reply-wire-cost-experiment.md)
§0–§8. This file is the third-party rerunnable receipt; `plan`'s §8 is the
decision record.

## 1. What was run, on what

| field | value |
|---|---|
| execution date | 2026-09-13 |
| AgenTerm HEAD | `df049328c494a5cf6516fa73657138807c0d1167` (clean at lane build) |
| `tinyvm` / `tinyvm-qjs` pin | `9ac2598` (both, unmoved by this experiment) |
| lane | repo-local `CARGO_TARGET_DIR=target/frontier-host-reply-wire` |
| lane binary | `target/frontier-host-reply-wire/debug/agenterm`, sha256 `67c3ba449d29d57981a618c0763811c7eeb4fa260dfb1c0867c71e18e2af70c8`, 33,773,464 B |
| toolchain | rustc 1.97.0 (2d8144b78 2026-07-07), cargo 1.97.0 (c980f4866 2026-06-30) |
| target / profile | native macOS aarch64, default dev (`cargo build --bin agenterm`) |
| lanes touched | one lane, one reporter, one court; no product Rust, `.qjs`, task manifest, budget or pin changed |

Budgets are the frozen task budgets of `agenterm.tasks.json`, unchanged:
`server-smoke` 300,000 ms / 10^9 operations; `native-ipc-smoke` 120,000 ms /
10^8; `workbench-smoke` 120,000 ms / 10^9 per phase, `--phase editing`,
`AGENTERM_NO_ACTIVATE=1`, `AGENTERM_QJS_MAX_MEMORY_PAGES=4096`.

## 2. Exact rerun commands

From the repository root:

```sh
export CARGO_TARGET_DIR="$PWD/target/frontier-host-reply-wire"   # repo-local lane
git rev-parse HEAD; git status --porcelain                        # run identity
cargo build --bin agenterm
shasum -a 256 "$CARGO_TARGET_DIR/debug/agenterm"

# V2 control runs, three per journey, unmodified commands and budgets
sh research/qjswasm-host-reply-wire-cost/tools/run-control.sh 3

# V1 reply capture, once per journey, through the instrumented reporter copies
sh research/qjswasm-host-reply-wire-cost/tools/run-capture.sh

# shapes A/B/C from the frozen replies, with V3/V4 and encoder parity enforced
python3 research/qjswasm-host-reply-wire-cost/tools/make_shapes.py

# pricing runs: one court, three shapes, three runs each (and the probe set)
sh research/qjswasm-host-reply-wire-cost/tools/run-court.sh server-smoke 3
sh research/qjswasm-host-reply-wire-cost/tools/run-court.sh native-ipc-smoke 3
sh research/qjswasm-host-reply-wire-cost/tools/run-court.sh workbench-smoke 3
sh research/qjswasm-host-reply-wire-cost/tools/run-court.sh <journey> 3 probe

# judged table, gate walk and the reference digests
python3 research/qjswasm-host-reply-wire-cost/tools/aggregate.py
python3 research/qjswasm-host-reply-wire-cost/tools/receipt.py
```

The court invocation actually run (the one `--project-root` is required because
a `research/` entry resolves `lib/rh_compat` against the project root; it
changes no journey byte):

```sh
"$CARGO_TARGET_DIR/debug/agenterm" cli script run --profile tool --json \
  --project-root scripts/qjs --timeout-ms 300000 --max-operations 1000000000 \
  research/qjswasm-host-reply-wire-cost/court.qjs \
  -- "$PWD/research/qjswasm-host-reply-wire-cost/shapes" <journey> A|B|C
```

## 3. V1 — the frozen reply sets

Captured once per journey through `capture/<journey>.capture.qjs`, which is the
pristine journey plus one appended reporter (the diff is stored beside each copy
as `<journey>.diff`). The reporter records, for every host CLI reply, the door
envelope text and the payload text the journey parses, in order.

| journey | replies | envelope bytes | payload bytes | telemetry capture |
|---|---:|---:|---:|---|
| server-smoke | 34 | 4,282 | 225,512 | `replies/server-smoke/` |
| native-ipc-smoke | 20 | 59,049 | 51,004 | `replies/native-ipc-smoke/` |
| workbench-smoke (`editing`) | 11 | 128,973 | 112,450 | `replies/workbench-smoke/` |

**Redaction is a pre-freeze step.** The captured replies are real answers from a
real host, so they carry host identity. Before freezing, the whole capture is
rewritten by a fixed, ordered substitution table and nothing else: an absolute
path into this clone becomes repo-relative, anything under the user home becomes
`~/...` (a stored harness `home/` segment included, so no stored path keeps a
home root), and the account name, the personal hostname and the labels derived
from them become the generic placeholders `<user>` and `<HOST>` (loopback
addresses are kept — they are not host identity). The temporary redaction script was
deleted after the pass, so the stored tree contains only the placeholders and
the generic rule, never the replaced values; `RESULTS.md` therefore describes the
categories, not the strings. Every shape, digest and measurement in this
directory derives from the redacted bytes. The unredacted capture no longer
exists on disk; its only residue is the pre-redaction figures quoted in §8.4.

The reply set is frozen; `shapes/manifest.json` carries a sha256 for every
frozen file of every shape, and `receipt.json` carries a per-shape digest over
the whole set (`reply_set_sha256`) plus a canonical value digest
(`value_set_sha256`). A rerun that does not reproduce those digests is invalid,
not "close enough". The capture run is a reporter: it adds `fs_write` calls and
its own steps are not judged.

**V1 reproducibility recheck** (`V1-recheck.json`): one further capture per
journey, into a repo-local scratch directory, reproduced each reply count
exactly (34 / 20 / 11) and the whole command sequence once volatile live values
(pid, address, port, timestamp, server-issued lease-id hex) are masked. Byte
identity across captures is explicitly **not** claimed: the answers carry live
identity (ports, pids, epochs) and the lease ids are issued per run. That is
exactly why the specification freezes *one* capture and digests it.

On this host the answer text crosses the wire differently per journey, and the
court reproduces each journey's own path rather than one favourite path:

- **server-smoke**: the journey's `run()` redirects stdout to a file with
  `sh -c 'exec "$@" > "$0"'` and reads it back with `rh.read_text` → the payload
  is a **second** door fetch (`fs_read_to_string`), and the door envelope stays
  122 B. The court does the same two fetches.
- **native-ipc-smoke / workbench-smoke**: the payload rides **inside** the
  `process_command` envelope, escaped, so the court fetches one envelope and
  takes `env.stdout` in memory. The court also reproduces the two route-arounds
  the journey already performs on this host: `grep`-to-seven-keys for
  `protocol-info --running`, and the brace-wrap + `trim` of that filtered
  fragment before `JSON.parse`.

Census derivation: `census.json`, one entry per captured reply with `parse`
(`json` / `text` / `none`), the read paths in code order, an anchor line in the
journey source, and a note. Paths that a projection keeps on *every* array
element while the journey reads them only from the matched element are labelled
as deliberate over-retention (conservative). Reads that exist in code but not in
the frozen reply (an optional `wait`, a null `cleanup_receipt`) are listed in
`shapes/<journey>/index.json` under `unresolved_reads`; a reply whose *every*
path resolves to nothing is an error, which is how the one real census bug of
this run was found (§8.3).

## 4. V2 — control reproduction (the denominators)

| journey | control steps (3 runs) | median | spread | exit class |
|---|---|---:|---:|---|
| server-smoke | 19,634,203 / 19,634,183 / 19,634,207 | **19,634,203** | 24 (0.0001%) | success |
| native-ipc-smoke | 20,019,101 / 20,016,458 / 20,016,620 | **20,016,620** | 2,643 (0.013%) | success |
| workbench-smoke | 20,810,109 / 20,810,109 / 20,810,109 | 20,810,109 | 0 | **script (failed)** |

Both usable journeys reproduce inside their own spread. The workbench row does
not: its control **fails** at the frozen macOS adapter boundary
(`process.window_pointer: macOS background pointer delivery is unavailable`),
so 20,810,109 steps are a *truncated stop point*, not a journey total, and the
journey is marked `usable: false` in `measurements.json` (with the reason). Its
A/B/C rows are kept as truncated data and are not counted by the gate.

## 5. V3 / V4 — shape validity

Enforced by `tools/make_shapes.py`; it exits non-zero on any failure.

- **V4**: for every parsed reply, `json.loads(B) == json.loads(A)` — value
  identity with whitespace removed. Independently confirmed across the whole
  set: `value_set_sha256(B) == value_set_sha256(A)` for all three journeys.
- **V3**: for every census path of every parsed reply, the value under C equals
  the value under A, **and the number of reads after array expansion is equal**
  (the read sequence is replayed over both and compared element by element).
- **Encoder parity**: the envelope rewriter re-emits each captured envelope
  byte-for-byte identical when the payload is unchanged, and its escape function
  reproduces the capture's own `stdout` token byte-for-byte. A shape B/C
  envelope is therefore the same envelope with the same escaping, carrying the
  shaped payload and nothing else.

Shapes carry the same reply count, the same call order and the same read
sequence; only bytes change.

## 6. Judged numbers (M0)

`cost.steps` from the lane binary, median of 3 runs per shape per journey. The
court is fully deterministic here: all three runs of every shape are identical,
so the medians are exact and the court spread is 0.

| journey | replies | envelope B | payload B | steps A | steps B | steps C | ΔB | ΔC | ΔC/total | ΔB/ΔC | `json_parse_bytes` |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| server-smoke | 34 | 4,282 | 225,512 | 11,724,596 | 10,498,811 | 1,989,614 | 1,225,785 | 9,734,982 | **49.58%** | **12.59%** | A 441,616 / B 441,616 / C 53,204 |
| native-ipc-smoke | 20 | 59,049 | 51,004 | 11,330,083 | 9,520,120 | 5,325,037 | 1,809,963 | 6,005,046 | **30.00%** | **30.14%** | A 331,428 / B 264,248 / C 137,168 |
| workbench-smoke *(unusable, retained)* | 11 | 128,973 | 112,450 | 19,215,824 | 13,678,714 | 754,289 | 5,537,110 | 18,461,535 | 88.71% | 29.99% | A 646,792 / B 467,552 / C 18,724 |

`ΔB = steps_A − steps_B`, `ΔC = steps_A − steps_C` (steps a shape **removes**).
Bytes are the **A** shape's frozen bytes (B and C are the same reply values with
fewer bytes: server-smoke A/B/C payload 225,512 / 126,022 / 2,208 B; native-ipc
envelope 59,049 / 47,875 / 20,693 B; workbench envelope 128,973 / 75,699 /
1,953 B). Percentages divide by the same journey's own control median and by
nothing else — no cross-pin and no cross-journey ratio. Every number is
真机执行 (measured in the lane binary) unless labelled 未测定. All of these figures
are **post-redaction** (§3); the pre-redaction pass is quoted in §8.4.

The `json_parse_bytes` column is the allocation probe's own field
(`AGENTERM_QJS_ALLOCATION_PROBE=1`, read at guest-compile time in
`src/script_engine.rs`); the probe compiles a different guest, so **probe steps
are reporters, not judged numbers**: probe-A steps exceed the non-probe A median
by +464 (server), +280 (native-ipc) and +152 (workbench) against a recorded
spread of 24 / 2,643 / 0 steps, so they do not reproduce the median within the
spread and are quoted as context only. Wall time is recorded in `runs/*.json`
and is not a gate.

## 7. Verdict path, walked §4 node by node

```text
V1 reply sets frozen and digest-stable for all three journeys        yes
V0  V2 controls reproduce (server, native-ipc); V3 equal at every
    census path with equal expanded read counts; V4 JSON-equal        yes
W0-C  deltaC / journey total >= 10% in at least two journeys           YES
      server-smoke 49.58%   native-ipc-smoke 30.00%   (2 of 2 usable)
W1    deltaB / deltaC >= 75% on those journeys?                        no
      server-smoke 12.59%   native-ipc-smoke 30.14%
=>    owner = HOST-SIDE FIELD SELECTION (product wire, new door/API
      shape, separate decision) — NOT compact reply text alone
=>    experiment ends; implementation is a new leaf
```

`W1` is a decomposition rule, not a second gate. Its consequence was frozen in
the specification before any number existed: with `ΔC/total ≥ 10%`, a 75% share
would have admitted a compact-only owner. Here the measured indentation-only
share is 12.6% and 30.1%, so indentation is **not** the owner — most of the
removable wire is bytes the journey never reads at all, which is a
field-selection question (what the host chooses to send), not a formatting
question.

`workbench-smoke` does not change the verdict under either reading. Excluded as
unusable (the primary reading, §4 of this file and `measurements.json`), W0-C
passes on 2 of 2 usable journeys. Counted anyway, W0-C passes on 3 of 3 and the
three `ΔB/ΔC` values (12.59%, 30.14%, 29.99%) are still all below 75%, so the
owner is the same. The verdict is stable across the only reading that is in
question here.

## 8. Deviations, corrections and spec bugs

1. **Platform projections.** `agenterm.tasks.json` labels `server-smoke` and
   `workbench-smoke` as Windows-only. As §5 allows, the same `.qjs` entries were
   run directly with the lane binary, with the frozen per-task budgets, and
   `native-ipc-smoke` — which the manifest lists for macOS — was run the same
   way for a uniform lane. `workbench-smoke` additionally needs
   `--phase editing`, which is the phase the frozen court
   (`scripts/qjs/workbench-court.qjs`) gives a 120,000 ms / 10^9 budget.
2. **Court interface.** The spec's §5 command sketch is
   `court.qjs -- <replies-dir> A|B|C`; the court here takes
   `SHAPES_DIR JOURNEY SHAPE` because the frozen reply set needed the three
   shapes side by side, plus `--project-root scripts/qjs`. Same ruler, same
   binary, same three shapes, three runs each.
3. **Corrections made after the first numbers existed, all recorded with their
   cause.** (a) The census path grammar for `native-ipc-smoke`'s
   `server-list --json` replies addressed a root **array** as if it were an
   object member (`rows[].x` where the parsed value *is* the array), so shape C
   silently retained the whole array; the projection now descends root arrays
   and a reply whose every census path resolves to nothing is a hard error. The
   first (wrong) native-ipc figures were `ΔC/total = 9.44% / ΔB/ΔC = 95.94%`;
   the corrected figures are 30.00% / 30.14%. (b) The court briefly applied a
   `.trim()` to every payload, which `server-smoke`'s `command_json` and
   `workbench-smoke`'s `json_cli` deliberately do **not** do (only native-ipc's
   two parse helpers trim); the trim is now driven per reply by the census. The
   intermediate server-smoke figure was `A 16,419,630 / C 1,987,142` (73.51% /
   22.97%); the corrected figure is 49.58% / 12.59%. (c) **Redaction** (§3): the capture carried host identity, so it was rewritten
   by one fixed substitution table before freezing, every shape, digest and
   measurement was regenerated from the redacted bytes, and the tool that did it
   was deleted. The pass had two published stages, because the first table missed
   a harness `home/` segment inside a stored run path. Both earlier number sets
   are recorded here as **superseded, and void as formal results** — §3, §6 and
   §7 carry the final set only, and `measurements.json` is the machine record:
   - before any redaction: server `A 11,730,259 / C 1,990,362`, ΔC/total
     49.6068%, ΔB/ΔC 12.5852%, payload 225,692 B; native-ipc
     `A 11,395,826 / C 5,338,866`, ΔC/total 30.2597%, ΔB/ΔC 29.9229%, envelope
     59,802 B;
   - after the first table only (clone root, account and host rewritten; the
     stored harness `home/` segment still spelled): native-ipc
     `A 11,330,281 / C 5,325,661`, ΔC/total 29.9982%, ΔB/ΔC 30.1305%, envelope
     59,073 B;
   - **final** (the table applied to the whole tree, that `home/` segment folded
     to `~/`): server `A 11,724,596 / C 1,989,614`, ΔC/total 49.5818%, ΔB/ΔC
     12.5915%, payload 225,512 B; native-ipc `A 11,330,083 / C 5,325,037`,
     ΔC/total 30.0003%, ΔB/ΔC 30.1407%, envelope 59,049 B.
   The redaction stages moved ΔC/total by at most 0.26 percentage points, always
   downward, because redaction changes reply spelling, not reply structure. All three changes **lowered or left flat** the
   measured saving; none of them touched a threshold, gate, journey, shape
   definition or read-field definition, and no shape was re-run to improve a
   number — the whole set was regenerated and re-measured from the corrected
   pipeline, with the old `runs/` deleted first, each time.
4. **The court does not replay everything the journey does.** It replays the
   reply path and the census read sequence (identical under A/B/C). It does not
   replay the journey's command-journal record (a `stdout + stderr` concat and a
   512-character bound per reply), its spawn/wait/state host calls, or its
   assertions. Those are constant across shapes, and the omitted concat would in
   fact make **C cheaper still**, so the omission is conservative for W0-C.
   Absolute court steps are therefore *not* journey steps; only the difference
   between two shapes and its ratio to the journey's own control median are
   judged, exactly as §1.3 defines.
5. **`json_parse_bytes` is reported, not interpreted.** For `server-smoke` A and
   B report the identical 441,616 while their payloads differ by 99,490 bytes,
   and for the other two journeys they differ. The counter's semantics are a pin
   property and this experiment does not infer them; the column is recorded
   verbatim as §1.3 requires, and no claim above rests on it.
6. **Spec bugs found.** (a) The gate is written as "at least two of the three
   journeys" while the tree's `V0`/kill path can leave only two usable
   journeys; the specification should say "at least two of the *usable*
   journeys, and never fewer than two usable journeys", and should state what an
   unusable journey means for the denominator (this run uses: `usable` requires
   a succeeding control run, because a failed run's step count is not a journey
   total). (b) §1.1's shape C is defined over "the fields the journey actually
   reads", which is only defined for a reply the journey *parses*; the
   specification should say so, and should say what C means for a text reply
   (here: unchanged) and for a reply that is carried but never parsed (here: no
   field is read, so C carries none). Written up here rather than in prose
   elsewhere.

## 9. Honesty clause

No metric, threshold, journey, shape definition or read-field definition was
changed after the numbers existed. The three corrections in §8.3 fixed a tool
defect, a court deviation and a redaction requirement; each was re-measured from
scratch from the regenerated shapes, and none of them raised the measured saving
(two lowered it, the redaction moved it by a few hundredths of a percentage
point). Every number set is printed rather than only the final one. The verdict
admits two readings only on the `workbench-smoke` question, both are written out
in §7, and they agree.

## 10. Surprises

- **The journeys have already routed around the wire, asymmetrically.**
  `native-ipc-smoke` cuts `protocol-info --running` to seven keys with `grep`
  before the guest ever parses it, so the only removable bytes left on that
  reply are the indentation of those seven keys — which is exactly why its
  `ΔB/ΔC` is 30.14% while its `ΔC/total` is 30.00%: on this host that journey's
  wire route is nearly exhausted. `server-smoke` does the same trick for its
  61 KB `protocol-info` answer only in the *file* direction (it redirects the
  answer to a file and reads it back) and pays for every byte it parses.
- **Two replies are carried and never parsed at all.**
  `workbench-smoke`'s `wait_for_server` probe reads only `probe.success` out of
  a 15 KB `ui-snapshot` answer, and its four `wait-ui` calls parse 15–21 KB
  answers and read no field from them. That is why shape C is so cheap there.
- **Indentation is not the owner anywhere.** The indentation-only share of the
  removable wire is 12.6% (server), 29.9% (native-ipc) and 30.0% (workbench) —
  the specification's 75% line is nowhere near met, so "compact reply text" is
  not the action this experiment names.

## 11. Follow-up

Owner named by the branch that ran: **host-side field selection** — the host
choosing which fields to send a guest that has declared what it reads. That is a
new door / API-shape decision with its own schema, cost and consumer questions
(`plan/archive/design-qjswasm-host-reply-wire-cost-experiment.md` §7 leaves them to it),
and it is a **new leaf**, never work inside this one. No product wire
implementation, no door or API addition, no guest change and no upstream work
happened here. The upstream frontier (per-node parse cost) is untouched and
stays where `plan/design-host-op-budget.md` §7.6 left it.
