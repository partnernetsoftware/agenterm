//! Produce all four builds and print every criterion in section 3 of the
//! specification.
//!
//! Run: `cargo run --bin measure`. Every number below is printed with the
//! measurement definition it was taken under; nothing here divides across two
//! definitions.

use std::collections::BTreeMap;

use tinyvm::Val;
use valrepr::harness::{self, Run};
use valrepr::repr::Expect;
use valrepr::repr_nanbox::{self, CANONICAL_NAN};
use valrepr::{Point, Variant, compile};

struct Case {
    name: &'static str,
    group: char,
    source: &'static str,
    expect: Expect,
}

fn corpus() -> Vec<Case> {
    let sources: BTreeMap<&str, &str> = BTreeMap::from([
        ("arith", include_str!("../../corpus/arith.qjs")),
        ("compare", include_str!("../../corpus/compare.qjs")),
        ("loop", include_str!("../../corpus/loop.qjs")),
        ("call", include_str!("../../corpus/call.qjs")),
        ("mixed", include_str!("../../corpus/mixed.qjs")),
        ("str_len", include_str!("../../corpus/str_len.qjs")),
        ("str_concat", include_str!("../../corpus/str_concat.qjs")),
        ("str_eq", include_str!("../../corpus/str_eq.qjs")),
        ("str_poly", include_str!("../../corpus/str_poly.qjs")),
    ]);
    let table = include_str!("../../corpus/expected.tsv");
    let mut out = Vec::new();
    for line in table.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert!(cols.len() == 4, "malformed expected.tsv row: {line:?}");
        let name = cols[0];
        let expect = match cols[2] {
            "num" => Expect::Number(cols[3].parse().expect("decimal double")),
            "bits" => Expect::NumberBits(
                u64::from_str_radix(cols[3].trim_start_matches("0x"), 16).expect("hex bits"),
            ),
            "bool" => Expect::Bool(cols[3] == "true"),
            "str" => Expect::Str(Box::leak(cols[3].to_string().into_boxed_str())),
            other => panic!("unknown expected kind {other}"),
        };
        out.push(Case {
            name: Box::leak(name.to_string().into_boxed_str()),
            group: cols[1].chars().next().expect("group letter"),
            source: sources[name],
            expect,
        });
    }
    out
}

/// One measured program: its name, its run, and the byte tiers of its product.
struct Row {
    name: String,
    run: Run,
    l1: usize,
    l2: usize,
    l3: usize,
    corpus_code: usize,
    runtime_code: usize,
}

#[derive(Clone, Copy, Default)]
struct Totals {
    steps: u64,
    slots: usize,
    depth: usize,
    l1: usize,
    l2: usize,
    l3: usize,
    corpus_code: usize,
    runtime_code: usize,
}

fn main() {
    println!("# Raw measurement output");
    println!();
    println!("Toolchain: rustc {}", env!("CARGO_PKG_RUST_VERSION"));
    println!(
        "Limits: tinyvm `Limits::default()` (max_steps {}, max_call_depth {}, max_activation_slots {})",
        tinyvm::wasm::WASM_MAX_STEPS,
        tinyvm::wasm::WASM_MAX_DEPTH,
        tinyvm::wasm::WASM_MAX_ACTIVATION_SLOTS
    );
    println!();

    let cases = corpus();
    let mut per_build: BTreeMap<(Variant, Point), Vec<Row>> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();

    println!("## Criterion 1 -- correctness and the load gate (boolean)");
    println!();
    println!("| variant | point | program | load gate | result | expected | verdict |");
    println!("|---|---|---|---|---|---|---|");
    for variant in Variant::ALL {
        for point in [Point::P1, Point::P2] {
            for case in &cases {
                if case.group == 'B' && point == Point::P1 {
                    continue; // P1 has no strings; section 2 puts these in P2 only
                }
                let product = match compile(case.source, variant, point) {
                    Ok(p) => p,
                    Err(e) => {
                        failures.push(format!(
                            "{} {} {}: compile: {e}",
                            variant.label(),
                            point.label(),
                            case.name
                        ));
                        println!(
                            "| {} | {} | {} | n/a | COMPILE ERROR: {e} | | FAIL |",
                            variant.label(),
                            point.label(),
                            case.name
                        );
                        continue;
                    }
                };
                match harness::run(&product.wasm, variant.repr(), &[]) {
                    Ok(run) => {
                        let ok = harness::matches(&run, &case.expect);
                        if !ok {
                            failures.push(format!(
                                "{} {} {}: got {}, wanted {:?}",
                                variant.label(),
                                point.label(),
                                case.name,
                                harness::describe(&run),
                                case.expect
                            ));
                        }
                        println!(
                            "| {} | {} | {} | pass | {} | {:?} | {} |",
                            variant.label(),
                            point.label(),
                            case.name,
                            harness::describe(&run),
                            case.expect,
                            if ok { "PASS" } else { "**FAIL**" }
                        );
                        let s = product.size;
                        per_build.entry((variant, point)).or_default().push(Row {
                            name: case.name.to_string(),
                            run,
                            l1: s.l1_repr_ins,
                            l2: s.l2(),
                            l3: s.l3_total,
                            corpus_code: s.corpus_func_total,
                            runtime_code: s.runtime_func_total + s.heap_decl_bytes,
                        });
                    }
                    Err(e) => {
                        failures.push(format!(
                            "{} {} {}: {e}",
                            variant.label(),
                            point.label(),
                            case.name
                        ));
                        println!(
                            "| {} | {} | {} | see error | {e} | {:?} | **FAIL** |",
                            variant.label(),
                            point.label(),
                            case.name,
                            case.expect
                        );
                    }
                }
            }
        }
    }
    println!();
    println!(
        "Execution status for every row above: **real execution** (`Module::from_bytes_with` + \
         `instantiate` + `invoke_by_name`). No row is byte-measurement only."
    );
    println!();

    criterion_two();

    // ---- per-build totals ------------------------------------------------
    let group_of: BTreeMap<&str, char> = cases.iter().map(|c| (c.name, c.group)).collect();
    let mut totals: BTreeMap<(Variant, Point, char), Totals> = BTreeMap::new();
    for ((variant, point), rows) in &per_build {
        for row in rows {
            for key in [
                (*variant, *point, group_of[row.name.as_str()]),
                (*variant, *point, '*'),
            ] {
                let t = totals.entry(key).or_default();
                t.steps += row.run.steps;
                t.slots += row.run.peak_activation_slots;
                t.depth = t.depth.max(row.run.peak_call_depth);
                t.l1 += row.l1;
                t.l2 += row.l2;
                t.l3 += row.l3;
                t.corpus_code += row.corpus_code;
                t.runtime_code += row.runtime_code;
            }
        }
    }

    println!("## Per-program execution numbers (criteria 3 and 4)");
    println!();
    println!("| variant | point | program | steps | peak_activation_slots | peak_call_depth |");
    println!("|---|---|---|---|---|---|");
    for ((variant, point), rows) in &per_build {
        for row in rows {
            println!(
                "| {} | {} | {} | {} | {} | {} |",
                variant.label(),
                point.label(),
                row.name,
                row.run.steps,
                row.run.peak_activation_slots,
                row.run.peak_call_depth
            );
        }
    }
    println!();

    println!("## Per-program size numbers (criteria 5 and 6)");
    println!();
    println!(
        "| variant | point | program | L1 repr ins B | L2 B | L3 file B | corpus code B | shared runtime B |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for ((variant, point), rows) in &per_build {
        for row in rows {
            println!(
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                variant.label(),
                point.label(),
                row.name,
                row.l1,
                row.l2,
                row.l3,
                row.corpus_code,
                row.runtime_code
            );
        }
    }
    println!();

    println!("## Slopes");
    println!();
    println!(
        "Two readings of `P2 - P1`. **shared-corpus** compiles the *same five group-A programs* \
         at both points, so it is the marginal cost of the representation having gained a type \
         with the program held fixed. **whole-corpus** adds the four group-B string programs to \
         P2, so it also carries the cost of the new programs themselves."
    );
    println!();
    println!(
        "| variant | reading | d steps | d peak slots (sum) | d L1 B | d L2 B | d L3 B | d corpus code B | d shared runtime B |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    let mut slopes: BTreeMap<(Variant, &str), Totals> = BTreeMap::new();
    for variant in Variant::ALL {
        let p1a = totals[&(variant, Point::P1, 'A')];
        let p2a = totals[&(variant, Point::P2, 'A')];
        let p1all = totals[&(variant, Point::P1, '*')];
        let p2all = totals[&(variant, Point::P2, '*')];
        for (label, before, after) in [("shared-corpus", p1a, p2a), ("whole-corpus", p1all, p2all)]
        {
            let d = Totals {
                steps: after.steps - before.steps,
                slots: after.slots - before.slots,
                depth: 0,
                l1: after.l1 - before.l1,
                l2: after.l2 - before.l2,
                l3: after.l3 - before.l3,
                corpus_code: after.corpus_code.wrapping_sub(before.corpus_code),
                runtime_code: after.runtime_code - before.runtime_code,
            };
            println!(
                "| {} | {label} | {} | {} | {} | {} | {} | {} | {} |",
                variant.label(),
                d.steps,
                d.slots,
                d.l1,
                d.l2,
                d.l3,
                d.corpus_code as i64,
                d.runtime_code
            );
            slopes.insert((variant, label), d);
        }
    }
    println!();

    println!("## Absolute totals per build (criterion 6 intercept)");
    println!();
    println!(
        "| variant | point | scope | programs | steps | peak slots (sum) | L1 B | L2 B | L3 B | corpus code B | shared runtime B |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for variant in Variant::ALL {
        for point in [Point::P1, Point::P2] {
            for group in ['A', '*'] {
                if let Some(t) = totals.get(&(variant, point, group)) {
                    let n = per_build[&(variant, point)]
                        .iter()
                        .filter(|row| group == '*' || group_of[row.name.as_str()] == group)
                        .count();
                    println!(
                        "| {} | {} | {} | {n} | {} | {} | {} | {} | {} | {} | {} |",
                        variant.label(),
                        point.label(),
                        if group == '*' { "all" } else { "group A" },
                        t.steps,
                        t.slots,
                        t.l1,
                        t.l2,
                        t.l3,
                        t.corpus_code,
                        t.runtime_code
                    );
                }
            }
        }
    }
    println!();

    // one-module runtime footprint, which is identical across programs at a point
    println!("## L1 constant-encoding confound (criterion 6 honesty check)");
    println!();
    println!(
        "LEB128 charges an immediate by magnitude. V1's tags are small `i32.const`s (2 bytes); \
         V2's box base and tag masks are 64-bit (9-10 bytes each). The right column is the \
         counterfactual where every representation constant is hoisted into a module global and \
         read with a two-byte `global.get`. That build was not produced -- hoisting is an \
         optimisation and constraint 4 forbids it -- the column exists only to say whether the L1 \
         ordering survives the confound. Step counts are unaffected either way: a hoisted constant \
         is still one instruction."
    );
    println!();
    println!(
        "| variant | point | L1 B | of which constants B | constants | L1 with constants hoisted B |"
    );
    println!("|---|---|---|---|---|---|");
    for variant in Variant::ALL {
        for point in [Point::P1, Point::P2] {
            let mut l1 = 0usize;
            let mut cb = 0usize;
            let mut cn = 0usize;
            let mut hoisted = 0usize;
            for case in &cases {
                if case.group == 'B' && point == Point::P1 {
                    continue;
                }
                let p = compile(case.source, variant, point).expect("compiles");
                l1 += p.size.l1_repr_ins;
                cb += p.size.l1_const_bytes;
                cn += p.size.l1_const_count;
                hoisted += p.size.l1_constants_hoisted();
            }
            println!(
                "| {} | {} | {l1} | {cb} | {cn} | {hoisted} |",
                variant.label(),
                point.label()
            );
        }
    }
    println!();

    println!("## Shared runtime footprint, one module (criterion 5, first column)");
    println!();
    println!("| variant | point | runtime funcs | runtime code B | memory+global decl B |");
    println!("|---|---|---|---|---|");
    for variant in Variant::ALL {
        for point in [Point::P1, Point::P2] {
            let p =
                compile(cases[0].source, variant, point).expect("group A compiles at both points");
            println!(
                "| {} | {} | {} | {} | {} |",
                variant.label(),
                point.label(),
                p.size.runtime_func_count,
                p.size.runtime_func_total,
                p.size.heap_decl_bytes
            );
        }
    }
    println!();

    sensitivity(&cases, &group_of);

    println!("## Failures");
    println!();
    if failures.is_empty() {
        println!("None. Every product cleared tinyvm's load gate and produced its expected value.");
    } else {
        for f in &failures {
            println!("- {f}");
        }
    }
    if !failures.is_empty() {
        std::process::exit(1);
    }
}

/// Sensitivity S-ADD. The whole of criterion 3's shared-corpus slope comes from
/// one lowering choice -- `__add` testing for the new type before the old one.
/// Flip it for *both* variants and re-measure, so the verdict is known to be
/// robust rather than assumed to be.
fn sensitivity(cases: &[Case], group_of: &BTreeMap<&str, char>) {
    println!("## Sensitivity S-ADD -- `__add` dispatch order");
    println!();
    println!(
        "Default: `__add` tests for strings first, so every numeric addition pays the new type's \
         test. Flipped: the number path goes first, so on the shared corpus the added type costs \
         nothing at run time. Applied to both variants together -- it is a property of the \
         lowering, not of a representation."
    );
    println!();
    println!(
        "| ordering | variant | P1 steps (group A) | P2 steps (group A) | d steps shared-corpus |"
    );
    println!("|---|---|---|---|---|");
    for number_first in [false, true] {
        for variant in Variant::ALL {
            let mut at = [0u64; 2];
            for (i, point) in [Point::P1, Point::P2].into_iter().enumerate() {
                for case in cases {
                    if group_of[case.name] != 'A' {
                        continue;
                    }
                    let p = valrepr::compile_with(case.source, variant, point, number_first)
                        .expect("compiles");
                    let run = harness::run(&p.wasm, variant.repr(), &[]).expect("runs");
                    assert!(
                        harness::matches(&run, &case.expect),
                        "{} still correct",
                        case.name
                    );
                    at[i] += run.steps;
                }
            }
            println!(
                "| {} | {} | {} | {} | {} |",
                if number_first {
                    "number first"
                } else {
                    "string first (measured build)"
                },
                variant.label(),
                at[0],
                at[1],
                at[1] - at[0]
            );
        }
    }
    println!();
}

/// Criterion 2. Split into the two questions the specification's single
/// sentence conflated -- see RESULTS.md, spec fix S1.
fn criterion_two() {
    println!("## Criterion 2 -- f64 fidelity (boolean, safety)");
    println!();
    let neg2 = include_str!("../../corpus/probe_neg2.qjs");
    let selfeq = include_str!("../../corpus/probe_selfeq.qjs");

    let inputs: [(&str, f64); 7] = [
        ("+0", 0.0),
        ("-0", -0.0),
        ("+Infinity", f64::INFINITY),
        ("-Infinity", f64::NEG_INFINITY),
        (
            "canonical NaN 0x7ff8000000000000",
            f64::from_bits(0x7FF8_0000_0000_0000),
        ),
        (
            "NaN payload 0x7ff8000000000007",
            f64::from_bits(0x7FF8_0000_0000_0007),
        ),
        (
            "negative NaN 0xfff800000000000a",
            f64::from_bits(0xFFF8_0000_0000_000A),
        ),
    ];

    println!("### 2a -- observable ECMA-262 semantics (the gate)");
    println!();
    println!(
        "`-0` distinct from `+0`, `+/-Infinity` preserved and distinct, a NaN still a NaN, and no \
         value silently re-typed. ECMA-262 6.1.6.1 gives the Number type exactly one NaN value, so \
         these are the properties a conforming engine must keep."
    );
    println!();
    println!("| variant | input | `-(-x)` returns | kind | 2a |");
    println!("|---|---|---|---|---|");
    let mut pass_2a = BTreeMap::new();
    let mut pass_2b = BTreeMap::new();
    for variant in Variant::ALL {
        let repr = variant.repr();
        let product = compile(neg2, variant, Point::P1).expect("probe compiles");
        let mut all_a = true;
        let mut all_b = true;
        for (label, input) in inputs {
            let args = repr.host_encode_number(input);
            let run = harness::run(&product.wasm, repr, &args).expect("probe runs");
            let got = match run.value {
                valrepr::HostVal::Number(x) => x,
                other => {
                    println!(
                        "| {} | {label} | {other:?} | **NOT A NUMBER** | **FAIL** |",
                        variant.label()
                    );
                    all_a = false;
                    all_b = false;
                    continue;
                }
            };
            let semantic_ok = if input.is_nan() {
                got.is_nan()
            } else {
                got.to_bits() == input.to_bits()
            };
            let bit_ok = got.to_bits() == input.to_bits();
            all_a &= semantic_ok;
            all_b &= bit_ok;
            println!(
                "| {} | {label} | {:#018x} | {} | {} |",
                variant.label(),
                got.to_bits(),
                if got.is_nan() { "NaN" } else { "finite/inf" },
                if semantic_ok { "pass" } else { "**FAIL**" }
            );
        }
        pass_2a.insert(variant, all_a);
        pass_2b.insert(variant, all_b);
    }
    println!();

    println!("### 2b -- bit-exact NaN payload round trip (informational, not a gate)");
    println!();
    println!(
        "ECMA-262 6.1.6.1 note: an implementation may distinguish NaN bit patterns internally but \
         ECMAScript code cannot observe the difference. So this is a property of the \
         representation, not a conformance requirement -- reported, not judged."
    );
    println!();
    for variant in Variant::ALL {
        println!(
            "- {}: {}",
            variant.full_name(),
            if pass_2b[&variant] {
                "round trips every NaN bit pattern exactly"
            } else {
                "**loses NaN payloads** (canonicalised at the door)"
            }
        );
    }
    println!();

    println!("### 2c -- type confusion under a hostile bit pattern");
    println!();
    println!("| variant | input | `x == x` | expected | verdict |");
    println!("|---|---|---|---|---|");
    for variant in Variant::ALL {
        let repr = variant.repr();
        let product = compile(selfeq, variant, Point::P1).expect("probe compiles");
        for (label, input) in inputs {
            let args = repr.host_encode_number(input);
            let want = !input.is_nan();
            match harness::run(&product.wasm, repr, &args) {
                Ok(run) => {
                    let got = matches!(run.value, valrepr::HostVal::Number(x) if x == 1.0);
                    println!(
                        "| {} | {label} | {} | {} | {} |",
                        variant.label(),
                        got,
                        want,
                        if got == want { "pass" } else { "**FAIL**" }
                    );
                }
                Err(e) => println!(
                    "| {} | {label} | trap: {e} | {want} | **FAIL** |",
                    variant.label()
                ),
            }
        }
    }
    println!();
    println!(
        "Counterfactual, computed host-side rather than built as a third variant: without the \
         canonicalisation in `Nanbox::box_number`, the input `0xfff800000000000a` would read back \
         under V2 as tag {:#x}, payload {:#x} -- a *string at address 10* rather than a number. \
         That is why the canonicalisation is in the measured V2 and not an optimisation that could \
         be dropped.",
        (0xFFF8_0000_0000_000Au64 >> 48) & 0x7,
        0xFFF8_0000_0000_000Au64 & 0xFFFF_FFFF
    );
    println!();
    println!(
        "Canonical NaN this representation stores: {:#018x}",
        CANONICAL_NAN
    );
    println!("V2 box base: {:#018x}", repr_nanbox::BOX_BASE);
    println!();
    println!("**Criterion 2 verdict (2a is the gate):**");
    for variant in Variant::ALL {
        println!(
            "- {}: 2a {} / 2b {}",
            variant.full_name(),
            if pass_2a[&variant] { "PASS" } else { "FAIL" },
            if pass_2b[&variant] { "exact" } else { "lossy" }
        );
    }
    println!();
    let _ = Val::I32(0);
}
