# Expression-depth decode-cost attribution experiment

| Field | Value |
|---|---|
| Date | 2026-09-16 |
| Purpose | Decide whether the measured expression-depth decode growth is duplicated accounting or the real emitted instruction stream |
| Implementation | temporary probe in sibling `tinyvm`; no product code until the verdict |
| Owning product node | `prd/PRD_02_36_agenterm_qjswasm.md` H3 |

This experiment is not a shipped capability. It chooses whether one bounded
performance leaf exists; it must not be cited as a runtime optimization until
the result section says so.

## 0. Fixed facts

1. The production QJS closure measured 347,290 decode items under the current
   expression-depth instrumentation and therefore needs more than tinyvm's
   generic 262,144-item default.
2. AgenTerm admits only its own `JsV1` artifacts at 524,288 items; raw Wasm
   keeps the generic default.
3. `Ceiling::DecodeItems` now names the configurable field, but a refusal stops
   immediately and cannot report the module's full requirement.
4. This experiment does not reconsider those ownership or classification
   decisions.

## 1. Hard constraints

- Use one fixed synthetic guest and the same compiler revision and options for
  both variants. Do not change workload after seeing a result.
- The only variable is `RuntimeLimits.expression_depth` on versus off.
- Count in the decoder's own unit: charged opcodes and charged immediate
  groups. Artifact bytes are supporting evidence, not a substitute unit.
- Separate emitted enter/leave checks from all other newly charged items.
- Do not retain a probe, raise a limit, or change emitted code before verdict.
- Any urge to change the guest to expose a more favorable fold is a detected
  experiment failure, not an implementation requirement.

## 2. Minimal experiment

Use the existing right-nested-expression generator and one preselected scale
from the earlier decode-headroom curve. Compile it twice, with expression-depth
off and on. For each artifact record:

| Dimension | Measurement |
|---|---|
| Artifact size | bytes from the same compiler/options |
| Decoder charge | total items in the same counting implementation |
| Emitted checks | enter/leave check instructions attributable to each evaluated expression |
| Residual | `on charge - off charge - attributed checks` |

## 3. Precommitted criteria

| ID | Nature | Pass/fail rule |
|---|---|---|
| G1 | Boolean | Accounting is one charge per real opcode/immediate group; no item is charged twice |
| G2 | Attribution | Residual is zero or explained by a named, necessary emitted construct |
| G3 | Opportunity | At least one repeated emitted construct can be folded without changing expression, call, throw/finally, callback, packed-artifact, or opt-out semantics |

Numbers must include the exact revision, command, scale and on/off options.
Never divide Rust test-binary size by emitted Wasm size.

## 4. Decision tree and time box

1. Run the fixed on/off pair once and compute the attribution identity.
2. If G1 passes and G2 has no unexplained residual, then the observed growth is
   the real emitted program. If G3 has no concrete fold, **reject an optimization
   leaf immediately** and document the intrinsic cost.
3. If G1 fails, the only admitted leaf is removing duplicate decoder charging.
4. If G1 passes but G3 identifies one concrete fold, admit only that fold and
   require the full expression-depth semantic court before acceptance.

Kill criterion: no second workload, no repeated search for a favorable fold,
and no limit change may be used to rescue a rejected optimization.

Time box: stop as soon as the one attribution identity has numbers and every
charged residual item is named or marked unexplained.

## 5. Temporary layout

The probe may live temporarily beside tinyvm's expression-depth tests. It must
be removed after recording the result unless the verdict admits a reusable,
asserting test rather than a print-only measurement.

## 6. Excluded options

| Option | Reason excluded |
|---|---|
| Raise the generic decode ceiling | Widens untrusted raw-Wasm work and does not explain cost |
| Raise only AgenTerm's ceiling again | Capacity decision, not optimization |
| Compile-time nesting approximation | Charges dead branches and violates the shipped semantics |
| Call-depth or activation-slot proxy | Counts a different runtime fact |
| Change workload after measurement | Makes the verdict post hoc |

## 7. Not answered

- General tinyvm throughput, cold-start or resident-size competitiveness.
- Whether a different expression-depth mechanism could satisfy the semantic
  court; that would need its own precommitment.
- Candidate or six-cell runtime evidence.

## 8. Result

**Verdict: reject an optimization leaf.** The one fixed run used tinyvm
`9420045`, the existing 64-level right-nested-expression generator and
`Options::default()`:

| Variant | Artifact bytes | Decode items | Items/byte |
|---|---:|---:|---:|
| expression depth off | 11,095 | 5,666 | 0.510680 |
| expression depth on | 15,028 | 7,870 | 0.523689 |
| delta | +3,933 | +2,204 | +0.013009 |

The decoder charges each ordinary body opcode once. One expression check emits
13 enter opcodes (including the three-op fault store) and four leave opcodes,
so the exact marginal charge is 17. The measured program has `2N+1 = 129`
evaluated expressions:

```text
129 × 17 = 2,193 expression-local items
2,204 − 2,193 = 11 shared items
```

The remaining 11 shared items are consistent with the one runtime-limit import,
two private globals and entry setup already named by the G4 mechanism
description; this run did not split those 11 further. Source inspection shows
the body decoder has one `charge(1)` point per opcode, while the exact delta
equation independently leaves no expression-local residual. Decision trace:
G1 pass → G2 pass → G3 fail → reject optimization. The
3.7× capacity growth observed on the production closure is real emitted
instrumentation, not decoder double-accounting.

The temporary probe compiled the same source twice with
`compile_qjs_m1_with_runtime_limits`, found each artifact's minimal loading
`Limits::max_decode_items` by binary search, printed the table, and was then
removed. Its run command was:

```text
cargo test -p tinyvm-qjs --test zz_decode_attribution_probe -- --nocapture
```

Deviation from the initial report: it first described immediate groups as an
extra part of the per-expression charge and gave a 16–17.1 interval. Reading
the decoder and emitted fault-store sequence closed that ambiguity: ordinary
expression instructions are charged once per opcode, giving the exact value
17 and the exact shared residual 11. No workload, criterion or limit was
changed after seeing the result.
