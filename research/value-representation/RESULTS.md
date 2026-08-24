# Results — universal value representation for `.qjs → .wasm`

**Verdict: V1, the two-word `(tag: i32, payload: i64)` pair.**

Reached at step 3 of the specification's decision tree (criterion ③ Δsteps),
after criterion ④ tied; corroborated at step 5 by criterion ⑥ and unaffected by
criterion ⑦, whose leak list contains no losing condition for either side.
The verdict survives the one lowering choice it turned on (sensitivity S-ADD).

Specification (authoritative): [`plan/design-value-representation-experiment.md`](../../plan/design-value-representation-experiment.md).
This file is the third-party-reproducible form of its §8.

---

## 1. Measurement conditions

| field | value |
|---|---|
| date | 2026-08-24 (UTC) |
| host | Darwin arm64 (aarch64-apple-darwin) |
| rustc / cargo | 1.97.0 (`2d8144b78`) / 1.97.0 (`c980f4866`) |
| execution core | `tinyvm` path dependency `../../../tinyvm/crates/tinyvm` @ `f694733` |
| this repo | branch `qjswasm-m0` |
| host budget | tinyvm `Limits::default()` — `max_steps` 16 000 000, `max_call_depth` 512, `max_activation_slots` 1 048 576 |
| products | 4 builds (V1P1 / V1P2 / V2P1 / V2P2) × 9 corpus programs + 2 probes = **36 `.wasm` modules** |
| execution status | **real execution** for every number in every table. No row is byte-measurement only. |

### Rerun

Requires a sibling checkout of `tinyvm` (path dependency `../../../tinyvm`).

```sh
cd research/value-representation
./measure.sh          # every criterion below, with conditions re-stated
cargo test            # independent-validator cross-check
```

`measure.sh` exits non-zero if any product fails to load, traps, or returns
something other than its expected value. Every number in this file comes from
that one command.

### Measurement definitions (口径), stated once

Byte counts carry four parts: **what** is counted, **where** it is counted,
**what is excluded**, and **how** it was obtained. No number below is divided
by a number taken under a different definition.

| label | what | where | excludes | how |
|---|---|---|---|---|
| **L1** | encoded bytes of instructions the value-representation layer emitted | inside the code section, all functions | function framing, size prefixes, locals declarations, section headers, corpus code, runtime code | per-instruction provenance tag (`ir::Origin::Repr`) summed at encode time; instruction encoding is context free, so this is exact, not apportioned |
| **L2** | L1 + everything else this variant must emit to run the corpus | code section (runtime functions, whole entries) + memory section + global section | corpus functions, data section, type/function/export sections | `L1 + (runtime_func_total − L1_inside_runtime) + memory/global section bytes` |
| **L3** | the whole `.wasm` file | file | nothing | `Vec<u8>::len()` of the encoder output |
| **corpus code** | whole code-section entries of the compiled `.qjs` functions | code section | runtime functions, all other sections | sum of size prefix + locals declaration + instructions + terminating `end` |
| **shared runtime** | whole code-section entries of the emitted runtime functions, plus the memory and global declarations | code section + sections 5 and 6 | corpus functions, data section | same construction |
| **d** (Δ) | `P2 − P1` under one of the two readings in §4 | — | — | subtraction within one label |

- **`steps`** — `Instance::last_steps()`, one top-level `main` call, deterministic, no wall clock.
- **`peak_activation_slots`** — `Instance::last_peak_activation_slots()`: peak aggregate locals + operand values + control frames across the active function and every suspended caller.
- **LOC** — non-blank, non-comment lines. Command: `grep -cvE "^[[:space:]]*(//|/\*|\*|$)" <file>`.
- Ratios are written inline with their baseline named, and never cross two labels.

---

## 2. Criterion ① — correctness and the load gate (boolean)

**Both variants pass.** 28 corpus runs (14 per variant: 5 group-A programs at
P1, 5 group-A + 4 group-B at P2) all cleared `Module::from_bytes_with` +
`instantiate` + `invoke_by_name("main")` and returned the value in the shared
`corpus/expected.tsv` table. Numbers are compared bit-exactly, so `-0` does not
satisfy `+0`.

Second opinion, because "my encoder thinks it is correct" is not evidence and
neither is "my VM accepted it": all 36 products also validate under
`wasmparser` 0.239 with only MVP + `MULTI_VALUE` + `FLOATS` enabled
(`tests/second_validator.rs`). No other post-MVP proposal is required by either
side.

## 3. Criterion ② — f64 fidelity (boolean, safety)

The specification's single sentence asked two different questions. Split — see
spec fix **S1** in §7.

### ②a — observable ECMA-262 semantics (**the gate**)

`-0` distinct from `+0`; `±Infinity` preserved and distinct; a NaN still a NaN;
nothing silently re-typed. Probe: `corpus/probe_neg2.qjs`, `-(-x)`, which is
IEEE-754 negate applied twice and therefore the identity on every double — for
any representation that can hold the double it was handed.

| input | V1 returns | V2 returns | ②a |
|---|---|---|---|
| `+0` | `0x0000000000000000` | `0x0000000000000000` | both pass |
| `-0` | `0x8000000000000000` | `0x8000000000000000` | both pass |
| `+Infinity` | `0x7ff0000000000000` | `0x7ff0000000000000` | both pass |
| `-Infinity` | `0xfff0000000000000` | `0xfff0000000000000` | both pass |
| canonical NaN `0x7ff8000000000000` | `0x7ff8000000000000` | `0x7ff8000000000000` | both pass |
| NaN payload `0x7ff8000000000007` | `0x7ff8000000000007` | `0x7ff8000000000000` | both pass (still a NaN) |
| negative NaN `0xfff800000000000a` | `0xfff800000000000a` | `0x7ff8000000000000` | both pass (still a NaN) |

**V1 ②a PASS. V2 ②a PASS.**

### ②b — bit-exact NaN payload round trip (informational, **not** a gate)

- V1: round trips every NaN bit pattern exactly.
- V2: **loses NaN payloads.** Every NaN is canonicalised to
  `0x7ff8000000000000` at the door.

ECMA-262 6.1.6.1 gives the Number type exactly one NaN value and notes that an
implementation may distinguish bit patterns internally while ECMAScript code
cannot observe the difference. ECMA-262 is this project's declared semantic
authority, so bit preservation is a **property**, not a conformance
requirement. Reported, not judged.

### ②c — type confusion under a hostile bit pattern

Probe `corpus/probe_selfeq.qjs`, `x == x`. All 14 rows pass: the hostile
`0xfff800000000000a` is answered `false` (it is a NaN) by both variants, not
mistaken for anything else.

That is not free. Measured counterfactual, computed host-side rather than built
as a third variant: **without** the canonicalisation in `Nanbox::box_number`,
`0xfff800000000000a` reads back under V2 as tag `0x0`, payload `0xa` — a
*string at address 10* rather than a number. The canonicalisation is therefore
load-bearing in V2 and is not an optimisation that could be dropped to make V2
cheaper. Its cost is charged in ③ and ⑥ below.

## 4. Criteria ③ ④ ⑤ ⑥ — the numbers

Two readings of `P2 − P1`, both reported because they answer different
questions:

- **shared-corpus** — the *same five group-A programs* compiled at both points.
  The program is held fixed, so the delta is purely "the representation gained
  a type". This is the sharper reading and the one the decision tree is walked
  on.
- **whole-corpus** — P1 is the five group-A programs, P2 is all nine. Carries
  the cost of the four new string programs as well.

### ③ Δsteps · ④ Δpeak_activation_slots

| variant | reading | Δ steps | Δ peak slots (sum over programs) |
|---|---|---|---|
| V1 | shared-corpus | **2 619** | **0** |
| V2 | shared-corpus | **4 365** | **0** |
| V1 | whole-corpus | 4 502 | 95 |
| V2 | whole-corpus | 6 482 | 84 |

Intercepts, for context (these are *not* what ③ and ④ judge):

| variant | point | scope | steps | peak slots (sum) |
|---|---|---|---|---|
| V1 | P1 | group A (5) | 43 429 | 126 |
| V2 | P1 | group A (5) | 49 231 | 92 |
| V1 | P2 | all (9) | 47 931 | 221 |
| V2 | P2 | all (9) | 55 713 | 176 |

### ⑤ Δemitted bytes, two columns

Column meanings are opposite: shared-runtime growth is paid once per product;
per-function growth multiplies by the number of functions a script has.

| variant | column | P1 | P2 | Δ |
|---|---|---|---|---|
| V1 | shared runtime, one module (B) | 488 | 907 | **+419** |
| V2 | shared runtime, one module (B) | 916 | 1 563 | **+647** |
| V1 | corpus code, 5 group-A programs (B) | 684 | 684 | **0** |
| V2 | corpus code, 5 group-A programs (B) | 587 | 587 | **0** |

The per-function column is exactly zero for both: a corpus function's own code
does not change when the language gains a type, because every operator is
already a runtime call. Group-B string programs add 301 B (V1) and 346 B (V2)
of corpus code at P2, which is the whole-corpus reading.

### ⑥ Size intercept, three tiers

| variant | point | scope | L1 (B) | L2 (B) | L3 (B) |
|---|---|---|---|---|---|
| V1 | P1 | group A (5 modules) | 2 230 | 2 760 | 3 467 |
| V2 | P1 | group A (5 modules) | 4 368 | 4 913 | 5 475 |
| V1 | P2 | all (9 modules) | 4 876 | 8 593 | 9 996 |
| V2 | P2 | all (9 modules) | 10 842 | 14 613 | 15 785 |

No tier is 未测定 — all three were measured for all four builds.

**Honesty check on L1.** LEB128 charges an immediate by magnitude. V1's tags are
2-byte `i32.const`s; V2's box base and tag masks are 64-bit and cost 9–10 bytes
each. Counterfactual with every representation constant hoisted into a module
global and read with a 2-byte `global.get` (that build was **not** produced —
hoisting is an optimisation and constraint 4 forbids it):

| variant | point | L1 (B) | of which constants (B) | constants | L1 with constants hoisted (B) |
|---|---|---|---|---|---|
| V1 | P1 | 2 230 | 760 | 294 | 2 058 |
| V2 | P1 | 4 368 | 2 548 | 287 | 2 394 |
| V1 | P2 | 4 876 | 1 466 | 630 | 4 670 |
| V2 | P2 | 10 842 | 6 435 | 733 | 5 873 |

Most of V2's raw L1 excess is the constant encoding: at P1 the gap shrinks from
+95.9% of V1's L1 to +16.3% of V1's L1. **The ordering does not invert.** Step
counts are unaffected by this confound either way — a hoisted constant is still
one executed instruction.

### LOC

Both representations implement every method of the same `Repr` trait, so the
capability sets are equal, which is the precondition for comparing them.

| part | LOC |
|---|---|
| shared (front end, lowering, runtime, IR, encoder, harness, measure) | 2 511 |
| `src/repr_pair.rs` (V1) | 106 |
| `src/repr_nanbox.rs` (V2) | 126 |

### Why the slope differs — exact arithmetic, not a story

The whole shared-corpus Δsteps is one mechanism: at P2, `__add` gains a
two-operand "is this a string" test in front of the number path, and that test
runs on **every** numeric addition.

- V1 `is_string`: `local.get`, `i32.const`, `i32.eq` — the tag is its own
  machine word, so one compare.
- V2 `is_string`: `local.get`, `i64.reinterpret_f64`, `i64.const`, `i64.and`,
  `i64.const`, `i64.eq` — the tag is packed into the double, so it must be
  extracted before it can be compared.

Per `__add` call the P2 prologue therefore costs V1 **9** executed instructions
(3 + 3 + `i32.or` + `if` + `end`) and V2 **15** (6 + 6 + `i32.or` + `if` +
`end`). Every group-A Δ is exactly that number times the count of `__add` calls:

| program | `__add` calls | V1 Δsteps | 9 × calls | V2 Δsteps | 15 × calls |
|---|---|---|---|---|---|
| arith | 1 | 9 | 9 | 15 | 15 |
| compare | 2 | 18 | 18 | 30 | 30 |
| loop | 40 | 360 | 360 | 600 | 600 |
| call | 232 | 2 088 | 2 088 | 3 480 | 3 480 |
| mixed | 16 | 144 | 144 | 240 | 240 |

This generalises past strings: **each type a language gains adds one type test
per dispatch site per call, and a NaN-boxed tag test costs twice a two-word tag
test.** That is the finding with the longest reach — it predicts M4 objects and
M5 closures, not just M3 strings.

### Sensitivity S-ADD — the one lowering choice ③ turned on

`__add` testing for strings *before* numbers is what makes the shared-corpus
Δsteps non-zero at all. Flipped for **both** variants together (it is a property
of the lowering, not of a representation):

| ordering | variant | P1 steps (group A) | P2 steps (group A) | Δ shared-corpus |
|---|---|---|---|---|
| string first (the measured build) | V1 | 43 429 | 46 048 | 2 619 |
| string first (the measured build) | V2 | 49 231 | 53 596 | 4 365 |
| number first | V1 | 45 757 | 45 757 | **0** |
| number first | V2 | 52 141 | 52 141 | **0** |

Under number-first, ③ ties at zero, the tree falls through to step 4 (⑤
per-function column, which also ties at zero) and then to step 5, where ⑥
favours V1 on every tier at both points. **Same verdict by a different path** —
see the second trace in §5.

## 5. Decision trace

Walked exactly as the specification's §4 tree is written, with the two fixes
recorded in §7.

### Primary trace (the measured build)

**Step 0 — kill criteria.**
- ① : both variants pass (§2). Neither is killed.
- Proposition not falsified: a universal value representation does lower to MVP
  wasm at acceptable cost, on both designs.
- ⑦ : neither leak list contains "this variant only works if I break the
  representation" (§6). No variant is killed here. This is the check most likely
  to have decided the experiment, and it did not fire for either side.
→ continue.

**Step 1 — ② f64 fidelity.** ②a passes for both (§3). Neither is killed.
→ continue.

> The specification predicted V2 was at risk here, and named a V2 pass as one of
> the experiment's most valuable possible findings. **It passed.** Under the
> specification's original wording — bit-exact NaN payload preservation — V2
> would have lost the whole experiment at this step, on a property ECMA-262
> explicitly does not require. See spec fix S1.

**Step 2 — ④ Δpeak_activation_slots (the primary criterion).**
- shared-corpus: 0 vs 0. Tied.
- whole-corpus: V1 95, V2 84 — V2 is 11.6% below V1 (baseline V1 = 95), inside
  the specification's ≤ 20% tie band.
→ **tie**, go to step 3.

**Step 3 — ③ Δsteps.**
- shared-corpus: V1 2 619, V2 4 365 — V2 is 66.7% above V1 (baseline V1).
- whole-corpus: V1 4 502, V2 6 482 — V2 is 44.0% above V1 (baseline V1).
- Both readings exceed the 20% threshold in the same direction.
→ **V1 wins.** Go to step 5 as a cross-check (spec fix S2).

**Step 5 — intercept cross-check, cannot overturn.**
- ⑥ bytes: V1 is smaller on L1, L2 and L3 at both points, and the ordering
  survives the constant-encoding counterfactual. **Corroborates V1.**
- ④ intercept (added by spec fix S3): V1 uses 37.0% more peak activation slots
  at P1 (126 vs 92, baseline V2) and 25.6% more at P2 (221 vs 176). **Favours
  V2.**
- The two intercepts disagree, they are the tree's lowest-priority evidence by
  its own stated reasoning, and neither may overturn a step-3 win.
→ **Verdict stands: V1.**

**⑦ — parallel main output, may overturn a winner.** V1's leak list (§6)
contains no "must special-case outside the representation" entry. It does
contain one conditional dependency — multi-value returns — which is satisfied
by tinyvm today and measured, not assumed. Not a losing condition.
→ **Verdict final: V1.**

### Robustness trace (sensitivity S-ADD, number-first `__add`)

Step 0 and step 1 unchanged. Step 2: ④ ties as before. Step 3: ③ ties at 0 for
both. Step 4: ⑤ per-function column ties at 0 for both. Step 5: ⑥ intercept —
V1 smaller on every tier at both points, decisively, so the tree resolves here
without needing its explicit "prefer V1" tie default.
→ **V1 again.** The verdict does not depend on the `__add` dispatch order.

## 6. Criterion ⑦ — the leak list

The specification's disease detector: any urge to special-case *outside* the
value representation is a symptom to record, not a requirement to satisfy.
Recorded whether or not it was acted on. Nothing on this list was quietly
implemented.

### V1 — two-word `(tag: i32, payload: i64)`

| # | entry | acted on? | shape of the boundary |
|---|---|---|---|
| L1.1 | **Multi-value returns are load-bearing.** One JS value is two wasm values, so every function returns `(i32, i64)`. Measured, not assumed: `tests/second_validator.rs::multi_value_is_load_bearing_for_the_two_word_abi_only` shows V1's products are *rejected* by `wasmparser` with multi-value off, and V2's are accepted. | n/a — tinyvm has multi-value | Conditional. On a target without multi-value, V1 would have to return through linear memory, and *that* would be an escape hatch outside the representation — a losing condition. It is not one here. |
| L1.2 | Every variable read is two `local.get`s and every write two `local.set`s. | accepted as the design | No escape hatch wanted. Shows up honestly in ⑤ and ④'s intercept. |
| L1.3 | `select` cannot pick between two JS values in one instruction; it would need one `select` per word or an `if`/`else`. | not exercised — the corpus never needs it | Unexercised risk. Named so a later milestone does not discover it as a surprise. |
| L1.4 | Boxing needs no scratch local: the tag is pushed before the payload. | — | An advantage, recorded for symmetry with L2.4. |

**No entry of the form "this only works if I break the representation".**

### V2 — NaN-boxed `f64`

| # | entry | acted on? | shape of the boundary |
|---|---|---|---|
| L2.1 | **Every boxed double must be canonicalised.** Not optional: §3 ②c shows the alternative is silent type confusion. | implemented, inside the representation | Not a leak — it is *in* the representation. But it is a permanent per-arithmetic-result cost, and it accounts for most of V2's step intercept (see L2.6). |
| L2.2 | **NaN payloads cannot be represented at all.** | accepted | ECMA-262 permits it, so not a leak. But V2 cannot double as a general 64-bit value carrier — a future raw-`f64` host pass-through, or an `int64`, would need a heap box where V1 would not. |
| L2.3 | **Payload is 48 bits.** Fine for wasm32's 32-bit pointers. | accepted | Caps any future value that wants more than 48 bits of payload inline. |
| L2.4 | **A scratch `f64` local is required in every function that boxes a number** (the canonicalisation `select` needs the value three times). | implemented | Inside the representation, but it means the representation dictates a per-function local. V1 does not. This is why V2's slot advantage is smaller than "one word instead of two" predicts. |
| L2.5 | **Urge: skip the canonicalisation where the compiler statically knows the value cannot be NaN** — e.g. `__len`, whose number comes from `f64.convert_i32_s`. | **detected, not acted on** | The classic shape of the disease: a per-call-site exemption from the representation's own invariant. It would work today and become a correctness hazard the moment a new producer of numbers is added. |
| L2.6 | **Urge: give strings a non-boxed fast path** so `__str_concat` and `__str_eq` need not pay the tag extraction. | **detected, not acted on** | The specification's named example of the disease, arrived at independently while implementing P2. |
| L2.7 | **Urge: hoist the 64-bit tag constants into module globals** to recover the L1 size gap. | **detected, not acted on** (constraint 4). Quantified instead as a counterfactual column in §4. | Not a special case outside the representation — an ordinary optimisation. Recorded because it is the single largest confound in the size numbers, and because "V2 would be smaller if only…" is exactly the argument a reader will make. |

**No entry of the form "this only works if I break the representation"** — the
two that came closest, L2.5 and L2.6, were detected and refused, and the
experiment still ran. So ⑦ does not overturn either variant, and in particular
does not overturn the winner.

### Shared, applied identically to both (so they cannot tilt anything)

- Number is IEEE-754 double, per ECMA-262 6.1.6.1 — no small-integer tag on
  either side. §7 item 6 of the specification deliberately does not ask whether
  an integer fast path is worth it, because asking induces the disease; this
  experiment answers by not having one. See deviation D2 for the bias that
  creates.
- `==` compares within a type and is `false` across types; no coercion ladder.
- Relational `<` `<=` `>` `>=` are numbers only at both points; §2 of the
  specification puts only literal/concat/length/equality in P2.
- One flat function scope: no block scoping, no shadowing.
- No `%`: wasm has no `f64.rem`, and hand-writing `fmod` is runtime work
  orthogonal to the value representation.
- String representation is `[len: i32][bytes]`, UTF-8, no interning, no 8/16-bit
  forms, no collector, bump allocation with no free — the research-grade
  minimum the specification declares.

## 7. Deviations from the specification, and fixes to it

Listed in full, including the ones that make the conclusion look worse.

### Deviations

**D1 — the front end is not reused from `tinyvm-qjs`; it is written here.**
Specification §5 requires "shared lex/parse/AST/encode, *reused from the
compiler, not copied*". That is not satisfiable against today's compiler, and
this was measured rather than assumed. Probe: `tinyvm-qjs`'s `lex.rs` and
`diag.rs` were `#[path]`-included into a foreign crate — they *compile* — and
then run against corpus-shaped input:

| input | tokens the upstream lexer reports as unsupported |
|---|---|
| `function f(a) { return a + 1; }` | `` `function` keyword ``, `block statements`, `` `return` keyword ``, `block statements` |
| `"hello"` | `string literals` |
| `a == b` | `comparison operators` |
| `while (i < 10) { i = i + 1; }` | `` `while` keyword ``, `comparison operators`, `block statements`, `assignment`, `block statements` |

`parse`/`ast`/`emit` are strictly narrower still (one expression, `i32` only, no
functions, no control flow), and `encode` has no memory, global or data section
and no `i64`/`f64`/control-flow opcodes. So none of the four stages can be
reused as-is. **This is a finding to report, not something this experiment
fixes** — the compiler was not modified.

*What it costs the experiment:* nothing on the axis being measured. §5's
"reuse, do not copy" exists to stop the two variants drifting apart, and here
both variants share one front end inside one crate, so the drift it guards
against is structurally impossible. What is lost is drift between this crate
and `tinyvm-qjs`, which is acceptable for a disposable spike.

**D2 — Number is an IEEE-754 double, not an `i32`, even at P1.**
§2 labels P1 "integers only" and §0 calls M2 the "integer world". Taken
literally that would mean a 32-bit integer number type. It is read here as
"the corpus only uses integer-valued numbers", with the *type* being the
ECMA-262 Number, because criterion ② requires the representation to hold real
doubles and ECMA-262 is the declared semantic authority. Every corpus program
returns an exact integer.

*This deviation makes the winner look better than an `i32` design would, and it
must not be waved away.* With `Number = f64`, V2 pays the canonicalisation
`select` on every boxed arithmetic result; with `Number = i32` it would pay a
cheap `i64.or` and no NaN risk at all. Quantified: in `loop` at P1, V2 spends
2 282 steps to V1's 2 079 — a gap of 203 — of which 160 (40 boxes × 4 extra
instructions) is canonicalisation, about 79%. So on the **absolute** step
intercept, an integer-typed design could plausibly reverse the sign.

It cannot reverse ③, which is what decided the experiment: canonicalisation is
present at both P1 and P2 and therefore *cancels in the delta*. The
shared-corpus Δsteps is accounted for, instruction by instruction, by the type
test alone (§4), and a two-word tag test is one compare where a NaN-boxed one is
an extract-then-compare regardless of what the number type is.

**D3 — criterion ②'s "dedicated corpus" is two `.qjs` probe programs driven by
host-supplied arguments**, rather than source-level literals. There is no way to
write `0xfff800000000000a` as a JavaScript numeric literal, so the hostile bit
pattern has to enter through the call ABI. Both probes are ordinary corpus
programs compiled by the same pipeline; only the argument comes from the host.

**D4 — every operator is a runtime call, not inlined.** This is the
straightforward lowering for a compiler with no optimiser (constraint 4), and it
is identical on both sides, but it is a choice: inlining would shrink the
per-call ABI cost, which is the axis V1 loses on. It therefore *understates*
V1's per-function code advantage and *overstates* the call-ABI cost V1 pays.
Both effects are symmetric in structure but not in magnitude, and this is not
quantified.

**D5 — the runtime set is emitted whole at each point, including functions a
given program never calls.** Simpler and identical on both sides, and it makes
⑤'s first column exactly one number per build, which is what §3 asks for. It
inflates L2 and L3 for small programs on both sides equally.

**D6 — one fairness bug found and fixed mid-experiment, disclosed rather than
buried.** The first implementation reserved V2's scratch `f64` local in *every*
function including corpus functions, which never box a number. That charged V2
an unused local per function and inflated exactly the criterion-4 number the
experiment turns on. Fixed before the reported numbers were taken. Effect of the
fix, at P1 group A: V2's summed peak activation slots fell from 111 to 92, which
*widened* V2's ④-intercept advantage over V1 from 13.5% to 37.0%. The fix helps
the loser and was applied anyway.

**D7 — a third-party validator was added as a dev-dependency.** Constraint 5
forbids a third-party wasm *encoder*; `wasmparser` here is a *validator*, used
the way tinyvm's own suite uses `wat`, and is never a runtime dependency. Read
as within the constraint's intent; disclosed in case it is read otherwise.

**D8 — not measured, and why.** Wall-clock time (deliberately: `steps` is
deterministic and §3 asks for it). Memory high-water mark beyond the bump
pointer (no criterion asks). Any point past P2. Objects, arrays, closures,
collection, the `fleet.js` gate — all excluded by §2 and §7.

### Fixes made to the specification itself

**S1 — criterion ② was ambiguous, and the ambiguity would have decided the
experiment.** "真 NaN（含 payload）… 存进值、取出来仍相等" does not say what
"equal" means for a NaN. Read as bit-equality, V2 loses at step 1 and the
experiment ends there. But ECMA-262 6.1.6.1 — this project's declared semantic
authority — gives the Number type exactly one NaN value and notes that
ECMAScript code cannot observe a payload. Requiring bit preservation therefore
over-specifies past the authority and would have killed a variant on a
non-requirement. **Split into ②a (observable semantics — the gate) and ②b
(bit-exact payload round trip — informational).** ②c (no type confusion under a
hostile pattern) is stated explicitly because it is the actual safety question
hiding inside the original sentence.

**S2 — a gap in the decision tree.** Step 2 routes its winner to "step 5 for
cross-check", but steps 3 and 4 name no successor, so a win there had no defined
next move. **Fixed: a win at step 2, 3 or 4 proceeds to step 5 as a
cross-check that records agreement or disagreement and cannot overturn**,
because §4 already establishes that intercepts are the lowest-priority evidence.

**S3 — §0 and the tree disagree about which activation-slot number matters.**
§0 argues `max_activation_slots` is a real tinyvm budget and that this is the
core reason the experiment exists; the tree judges only the *slope* of ④. But
the budget is enforced against an *absolute*, so the slope alone cannot answer
"how deep can a script go". **Fixed: step 5 reports the ④ intercept alongside
⑥, both as non-overturning cross-checks.** In this experiment the two intercepts
point in opposite directions, which is exactly the situation the fix exists to
make visible instead of invisible.

**S4 — §5's "reuse from the compiler, do not copy" is not satisfiable today.**
Amended with a pointer to D1 and its measured evidence, so the next reader does
not re-derive it.

**S5 — two criteria named an API that does not exist.** ③ and ④ said
`Outcome::steps` / `Outcome::peak_activation_slots`; tinyvm has no `Outcome`
type. The real accessors are `Instance::last_steps()` and
`Instance::last_peak_activation_slots()`. Corrected in place so the next reader
does not go looking for `Outcome`.

## 8. Honesty clause

Every number in this file was produced by `./measure.sh` on the date and host in
§1, from products that cleared tinyvm's load gate, were validated a second time
by an independent validator, and were actually executed. Nothing is estimated,
extrapolated, or carried over from a previous run.

Three things in this file weaken the conclusion, and none of them is buried:
deviation **D2** (the number type choice, which could reverse the absolute step
intercept though not the slope that decided the verdict), the **L1 constant
confound** in §4 (most of V2's raw size excess is LEB128 encoding, not
mechanism), and the **④ intercept** in §5, which is the one criterion where V2
wins clearly and where the specification's §0 expected the experiment to be
decided.

The conclusion is a **bound, not a value**. The string implementation is the
research-grade minimum the specification mandates — no interning, no 8/16-bit
dual form, no collector — so the measured marginal costs are a *lower bound* on
what M3's real strings will cost. The correct reading of every Δ here is "within
this subset, V1's marginal cost of gaining a type is below V2's", never "V1's
marginal cost is N".

## 9. What overturned expectation

1. **NaN-boxing is bigger, not smaller.** The received framing — and the
   compiler's own design note — has NaN-boxing as the space-saving option. At
   P1 it emits 95.9% more L1 bytes and 57.9% more whole-file bytes than the
   two-word pair (baselines V1). Even with the constant confound removed by
   counterfactual, it is still larger. On a 32-bit target with LEB128-encoded
   immediates, packing a tag into a double costs *code* to buy *data*, and the
   corpus never has enough live values for the data saving to show.
2. **NaN-boxing is slower, not faster**, both in intercept (+13.4% steps at P1,
   baseline V1) and in slope (+66.7% Δsteps, baseline V1). The tag test is the
   whole reason, and it gets worse with each type added.
3. **④ — the criterion §0 built the experiment around — tied at exactly zero**
   on the sharpest reading, and 11.6% on the other. The predicted "two-word
   costs twice the slots" did not appear anywhere: at P1 V1 uses 37% more peak
   slots, not 100% more. Two mechanisms cancel most of the difference —
   `peak_activation_slots` counts control frames and fixed-type runtime locals
   as well as JS values, and V2's mandatory canonicalisation scratch (L2.4) adds
   back one local per boxing function. In `call` (recursive `fib`), V1's frame is
   2 parameter words + 0 scratch and V2's is 1 parameter word + 1 scratch —
   *identical*.
4. **The activation-slot budget is not the binding constraint anyway.** At the
   measured ~3.4 slots per frame (V1, from `call`: 48 slots at depth 14),
   `max_call_depth` = 512 is reached at roughly 1 750 slots, 0.2% of the default
   `max_activation_slots` = 1 048 576. §0's premise that the slot budget would
   decide "how deep a script can go" does not hold for frames of this shape.
5. **V2 passed ②.** The specification predicted risk here and called a V2 pass
   one of the most valuable possible findings. It passes ②a and ②c cleanly — but
   only because canonicalisation is mandatory, and only once ②'s wording is
   corrected to what ECMA-262 actually requires (S1). Under the original wording
   V2 would have lost the entire experiment at step 1 on a non-requirement.
6. **The disease detector fired twice, on V2, and both times were refusable.**
   L2.5 and L2.6 were real urges arrived at while implementing, not
   hypotheticals. Neither had to be acted on, so ⑦ produced no losing condition
   for either side — which means the criterion the brief flagged as most likely
   to decide the experiment did not decide it. The numbers did.

## 10. What this does not answer

Unchanged from specification §7: property lookup and shapes (M4), collection
strategy (M4–M5), the real string form — interning, 8/16-bit dual form,
equality fast paths (M3), closure environment layout (M5), and how the runtime
is delivered. One addition from this experiment:

- **A third representation was not built.** JavaScriptCore-style value shifting
  (store `bits + 2^49` so every double, NaN payloads included, lands outside the
  pointer range) would round-trip ②b exactly while keeping one word. It is a
  real point in the design space, it was not measured, and §4's timebox forbids
  a third variant. If the single-word slot advantage ever becomes the binding
  constraint, that is the experiment to run next — not V2 as built here.
