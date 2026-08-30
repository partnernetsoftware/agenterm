//! The `.qjs` half of this crate's face, now that the compiler lives upstream.
//!
//! The compiler's own acceptance suite -- what the subset lowers, what it
//! rejects, and the exact wording of every capability diagnostic -- moved to
//! `tinyvm-qjs` with the compiler, where the code under test is. Duplicating it
//! here would be a copy that rots.
//!
//! What is still agenterm's to prove is the seam: that a `.qjs` guest goes all
//! the way through *this* engine -- compile, load gate, slot, call, cost
//! counters -- and that a compile failure is its own error class with the
//! diagnostic intact rather than a generic load rejection. That is the part a
//! version bump upstream could break, and the part no upstream test covers.
//!
//! Two of those seam facts are new as of the `df8decd` bump and are what most
//! of this file is now about:
//!
//! * **The calling convention changed.** A compiled entry point speaks the V1
//!   representation -- one JavaScript value is a `(tag: i32, payload: i64)`
//!   pair -- so a `.qjs` slot takes and returns [`JsValue`], not raw wasm
//!   numerics. Which convention a slot speaks is settled at load time.
//! * **A returned value is projected, not forwarded.** A String's payload is a
//!   pointer into the slot's linear memory, and `run_once` drops that memory
//!   before its caller sees the result, so the text is read out at the seam.
//!
//! The small corpus in [`the_capability_claims_in_this_crates_own_copy`] is the
//! exception to "language tests live upstream", and deliberately so: this
//! crate's README and PRD 36 make capability claims in agenterm's voice, and
//! the evidence gate says product copy is locked by a test rather than by a
//! reading of someone else's source.

use agenterm_qjswasm::{
    Budget, Engine, Guest, GuestKind, JsValue, QjswasmError, Value, compile_qjs,
    guest_kind_for_path, validate_wasm,
};

fn engine() -> Engine {
    Engine::new()
}

fn number(x: f64) -> Value {
    Value::Js(JsValue::Number(x))
}

/// Run a whole `.qjs` script with no arguments and hand back the one JS value
/// it evaluated to.
#[track_caller]
fn returns(source: &str) -> JsValue {
    let mut eng = engine();
    let out = eng
        .run_once(Guest::Qjs(source), None, "main", &[])
        .unwrap_or_else(|e| panic!("{source:?}: {e}"));
    match out.values.as_slice() {
        [Value::Js(value)] => value.clone(),
        other => panic!("{source:?}: a `.qjs` guest must return one JS value, got {other:?}"),
    }
}

#[test]
fn a_qjs_guest_runs_end_to_end_through_a_slot() {
    let mut eng = engine();
    let out = eng
        .run_once(Guest::Qjs("$0*2+2"), None, "main", &[number(20.0)])
        .expect("a `.qjs` guest must compile, load and run");
    assert_eq!(out.values, vec![number(42.0)]);
    // The cost counters are the engine's, not the compiler's: a compiled guest
    // must be measured on exactly the same terms as a hand-written one.
    assert!(out.steps > 0, "a compiled guest reported zero steps");
    assert!(out.stdout.is_empty());
    assert!(!out.truncated_stdout);
}

#[test]
fn a_qjs_slot_is_persistent_like_any_other() {
    let mut eng = engine();
    let slot = eng.spawn(Guest::Qjs("$0+$1"), None).expect("spawn");
    for (a, b, want) in [(1.0, 2.0, 3.0), (40.0, 2.0, 42.0), (-5.0, 5.0, 0.0)] {
        let out = eng
            .call(slot, "main", &[number(a), number(b)])
            .expect("call");
        assert_eq!(out.values, vec![number(want)]);
    }
    assert_eq!(eng.live_slots(), 1);
    eng.kill(slot);
    assert_eq!(eng.live_slots(), 0);
}

/// Every ECMA-262 language type the compiler has, crossing the face as itself.
///
/// This is the assertion the M0 seam could not make: with an `i32`-in/`i32`-out
/// entry point there was exactly one thing a `.qjs` guest could return, so the
/// face had nothing to get wrong. Now a guest returns one of five kinds and the
/// projection has to name which -- collapsing them back to a number would throw
/// away the milestone this bump is here to pick up.
#[test]
fn every_kind_of_javascript_value_crosses_the_face() {
    assert_eq!(returns("return 42;"), JsValue::Number(42.0));
    assert_eq!(returns("return \"hello\";"), JsValue::Str("hello".into()));
    assert_eq!(returns("return true;"), JsValue::Bool(true));
    assert_eq!(returns("return false;"), JsValue::Bool(false));
    assert_eq!(returns("return null;"), JsValue::Null);
    assert_eq!(returns("return undefined;"), JsValue::Undefined);
    // A script with no `return` evaluates to its completion value, which is a
    // value like any other and must arrive as one.
    assert_eq!(returns("1 + 1"), JsValue::Number(2.0));
    assert_eq!(returns("let x = 1;"), JsValue::Undefined);
}

/// A returned String is text, read out of the guest's memory before the slot
/// dies.
///
/// The V1 payload for a String is a pointer into *that instance's* linear
/// memory, and [`Engine::run_once`] kills the slot before returning. Forwarding
/// the pointer would therefore hand every `run_once` caller a dangling
/// reference -- not occasionally, but on the common path. So the seam resolves
/// it while the instance is alive, and this test is what says so: the value
/// below is read after the memory it came from has been dropped.
#[test]
fn a_returned_string_is_text_and_outlives_the_slot_it_came_from() {
    let mut eng = engine();
    let out = eng
        .run_once(
            Guest::Qjs("return \"tab\" + \"s.list\";"),
            None,
            "main",
            &[],
        )
        .expect("string concatenation runs");
    assert_eq!(eng.live_slots(), 0, "run_once must not leave a slot behind");
    assert_eq!(
        out.values,
        vec![Value::Js(JsValue::Str("tabs.list".into()))]
    );
    // Escapes and non-ASCII are decoded by the compiler and must survive the
    // trip through linear memory as UTF-8, not as bytes.
    assert_eq!(
        returns("return \"caf\\u00e9\";"),
        JsValue::Str("café".into())
    );
}

/// A source the compiler will not lower is [`QjswasmError::Compile`] -- not a
/// load rejection, and not a trap.
///
/// Five failure classes exist so a caller can tell them apart without matching
/// on strings, and this is the one boundary where the distinction is easiest to
/// lose: everything downstream of the compiler also reports failures about
/// bytes, so a compile error that arrived as `Load` would look plausible and be
/// wrong about who to talk to.
///
/// The sources are chosen to still be outside the subset, and each is measured
/// rather than read off upstream's source. This test is the lock on the
/// "honest boundary" copy in this crate's README and in PRD 36 -- if the subset
/// grows past one of these, the copy has to be rewritten in the same commit
/// that makes it stale.
///
/// It has now caught that five times, which is what it is for.
///
/// (1) The bump to `6920c60` -- taken for `Names::Declared`, the mechanism that
/// lets a `.qjs` script reach the door -- also brought `%` (dd35c44) and
/// `typeof` (c707558), this list's first two entries at the time. Measured:
/// `return 1 % 2;` is `Number(1.0)` and `return typeof 1;` is `Str("number")`.
///
/// (2) The bump to `f21f0f2` brought the conditional expression (5bdb557) and
/// object literals (f203858), the next two entries. Measured: `return 1 ? 2 :
/// 3;` is `Number(2.0)`, and `return {};` now compiles and runs -- it fails one
/// step later, at this crate's own face, because `Value` has no variant for a
/// guest heap reference. That is a different boundary with its own test, and
/// deliberately not this one: a source that compiles does not belong in a list
/// of sources the compiler refuses, whatever happens to it afterwards.
///
/// (3) The bump to `048bcf2` brought arrays, this list's third entry. Measured:
/// `return [1, 2, 3];` compiles and runs, and its `.length` is `3`. Its
/// replacement is the one array form that did *not* land -- the elision
/// `[1, , 2]`, which is a hole and not an `undefined`, and which the engine
/// refuses by name rather than pick one of the two. Choosing that as the
/// replacement is deliberate: it keeps a `[` in the list, so a future bump
/// that widens array syntax lands here rather than nowhere.
///
/// (4) The bump to `68afb35` brought **closures that capture**, this list's
/// last entry. Measured: `function outer() { let a = 1; function inner() {
/// return a; } return inner(); }` runs and answers `1`, and
/// `function mk(n) { return function () { return n; }; }` gives a closure that
/// outlives the frame. Its replacement is `new`, which is a keyword this
/// engine has no plan for rather than a capability queued behind one -- so the
/// list keeps an entry that will not be overtaken by the next language bump.
///
/// (5) The bump to `ab29522` brought the whole DecimalLiteral grammar, so
/// `return 1.5;` -- this list's fourth entry since the beginning -- compiles.
/// Measured: `Number(1.5)`, and `0.1 + 0.2` is `0.30000000000000004`, which is
/// what says these are doubles rather than decimals being humoured. Its
/// replacement is a numeric separator, `1_000`: still a named refusal, and a
/// *different grammar* rather than the next thing queued behind the one that
/// just landed.
///
/// (6) The bump to `653cebe` brought **template literals** -- this list's
/// first entry since the beginning. Measured: `` return `a${1}b`; `` answers
/// `"a1b"`, and `` `${1}${2}` `` answers `"12"` rather than `3`. Its
/// replacement is a *tagged* template, which stays refused for a structural
/// reason rather than a queued one: a tag is a call whose first argument is a
/// frozen array of cooked strings carrying `raw`, and this engine has neither
/// the array methods nor the property definition to build one. Same logic as
/// (3): it keeps a backtick in the list, so a bump that widens template
/// syntax lands here rather than nowhere.
///
/// (7) The bump to `9e02e37` brought **arrow functions** (which landed
/// upstream in `ee3842b`; the pin skipped straight past it) -- and brought them
/// because closures had: in that engine an arrow *is* a function expression,
/// so once a function expression could capture, so could an arrow. Measured:
/// `let f = (x) => x + 1; return f(1);` answers `2`, `x => x` needs no
/// parentheses, and `function mk(n) { return () => n; }` gives a capturing
/// arrow. Its replacement is a **default parameter**, `(a = 1) => a`: still a
/// named refusal, and parameter syntax rather than the next expression form
/// queued behind the one that just landed.
///
/// Every time, the README's refusal list was corrected in the same commit.
#[test]
fn a_source_outside_the_subset_is_a_compile_error_not_a_load_error() {
    for source in [
        "function t(s) { return s; } return t`x`;",
        "let f = (a = 1) => a; return f();",
        "return [1, , 2];",
        "return 1_000;",
        "class A {} return 1;",
        "return new Object();",
    ] {
        let mut eng = engine();
        let err = eng
            .spawn(Guest::Qjs(source), None)
            .expect_err("this source is not in the subset yet");
        match &err {
            QjswasmError::Compile(e) => {
                // The diagnostic must survive the trip: it speaks for the
                // engine, and it says where. Its exact wording is the
                // compiler's contract and is locked upstream.
                assert!(
                    e.message.starts_with("this engine "),
                    "diagnostic blames the script: {e}"
                );
                assert!(e.to_string().contains("at byte"), "{e}");
            }
            other => panic!("{source:?}: expected a compile error, got {other:?}"),
        }
        assert!(
            err.to_string().starts_with("compiling .qjs:"),
            "{err}, which does not say the compile step failed"
        );
        assert_eq!(
            eng.live_slots(),
            0,
            "a rejected source must not take a slot"
        );
    }
}

/// The claims this crate's own README and PRD 36 make about `.qjs`, executed.
///
/// Not a language suite -- that is upstream's, next to the code. This is the
/// documentation lock the evidence gate asks for: every capability agenterm
/// advertises in its own voice is a line here that runs, so the copy cannot
/// drift ahead of the engine the way "M0, integer expressions only" drifted
/// behind it.
#[test]
fn the_capability_claims_in_this_crates_own_copy() {
    // Declarations with real scoping, and a block that does not leak.
    assert_eq!(
        returns("const y = 2; { let y = 3; } return y;"),
        JsValue::Number(2.0)
    );
    assert_eq!(returns("var z = 9; return z;"), JsValue::Number(9.0));
    assert_eq!(returns("let u; u = 5; return u;"), JsValue::Number(5.0));
    // Control flow.
    assert_eq!(
        returns("if (1 < 2) { return 10; } else { return 20; }"),
        JsValue::Number(10.0)
    );
    assert_eq!(
        returns("let i = 0; while (i < 5) { i = i + 1; } return i;"),
        JsValue::Number(5.0)
    );
    assert_eq!(
        returns("let s = 0; for (let k = 0; k < 4; k = k + 1) { s = s + k; } return s;"),
        JsValue::Number(6.0)
    );
    // Functions with parameters, and recursion.
    assert_eq!(
        returns("function add(a, b) { return a + b; } return add(2, 40);"),
        JsValue::Number(42.0)
    );
    assert_eq!(
        returns("function f(n) { if (n < 2) { return n; } return f(n-1) + f(n-2); } return f(10);"),
        JsValue::Number(55.0)
    );
    // The operator ladder, including short-circuit and compound assignment.
    assert_eq!(
        returns("return 1 < 2 && 3 >= 3 || false;"),
        JsValue::Bool(true)
    );
    assert_eq!(
        returns("return true && \"yes\";"),
        JsValue::Str("yes".into())
    );
    assert_eq!(
        returns("let n = 1; n += 2; n *= 3; return n;"),
        JsValue::Number(9.0)
    );
    assert_eq!(
        returns("let m = 0; return m++ + ++m;"),
        JsValue::Number(2.0)
    );
    // ASI: no semicolon anywhere, and the same answer.
    assert_eq!(
        returns("let a = 1\nlet b = 2\nreturn a + b"),
        JsValue::Number(3.0)
    );
}

/// The array half of the same lock, added when the README grew an array
/// section it did not have a test for.
///
/// Split into its own function only because the one above was already long;
/// it is the same gate and the same reason. Every claim here is one this
/// crate's README makes **in agenterm's voice**, and the evidence rule is that
/// such a claim is locked by a test in this crate rather than by a reading of
/// upstream's suite -- upstream can be right and this README still be stale,
/// which is exactly the drift the rule exists to catch.
///
/// Each returns a scalar, because `Value` has no Array variant: an array is a
/// guest heap reference this face cannot carry, which is itself one of the
/// README's claims and is asserted at the end.
#[test]
fn the_array_claims_in_this_crates_own_copy() {
    // "字面量、`a[i]` 读写、`a.length`、任意嵌套、与对象互相嵌套"
    assert_eq!(returns("return [1, 2, 3].length;"), JsValue::Number(3.0));
    assert_eq!(returns("return [10, 20, 30][1];"), JsValue::Number(20.0));
    assert_eq!(
        returns("let a = [1, 2]; a[0] = 9; return a[0];"),
        JsValue::Number(9.0)
    );
    assert_eq!(
        returns("return [[1, 2], [3, 4]][1][0];"),
        JsValue::Number(3.0)
    );
    assert_eq!(returns("return [{ a: 7 }][0].a;"), JsValue::Number(7.0));
    assert_eq!(returns("return { a: [1, 2] }.a[1];"), JsValue::Number(2.0));

    // "越界读是 `undefined` 不是 fault"
    assert_eq!(returns("return [1, 2][5];"), JsValue::Undefined);
    assert_eq!(returns("return [][0];"), JsValue::Undefined);

    // "越界写把中间补成 `undefined` 而不是 hole"
    assert_eq!(
        returns("let a = [1]; a[3] = 9; return a.length;"),
        JsValue::Number(4.0)
    );
    assert_eq!(
        returns("let a = [1]; a[3] = 9; return a[2];"),
        JsValue::Undefined
    );

    // "`typeof []` 是 `\"object\"`，`[]` 是真值，`===` 是引用相等"
    assert_eq!(returns("return typeof [];"), JsValue::Str("object".into()));
    assert_eq!(
        returns("if ([]) { return true; } return false;"),
        JsValue::Bool(true)
    );
    assert_eq!(returns("let a = [1]; return a === a;"), JsValue::Bool(true));
    assert_eq!(
        returns("let a = [1]; let b = [1]; return a === b;"),
        JsValue::Bool(false)
    );

    // "字符串 key 不是索引" -- the named divergence from ECMA-262 10.4.2.1.
    assert_eq!(returns("return [10, 20][\"0\"];"), JsValue::Undefined);

    // "没有任何数组方法" -- absent at run time, not a compile diagnostic.
    assert_eq!(returns("return [1, 2].map;"), JsValue::Undefined);
    assert_eq!(returns("return [1, 2].push;"), JsValue::Undefined);

    // "含数组的 JSON 现在也行"
    assert_eq!(
        returns("return JSON.parse(\"[1,2,3]\")[1];"),
        JsValue::Number(2.0)
    );
    assert_eq!(
        returns("return JSON.stringify([1, [2, { c: 3 }]]);"),
        JsValue::Str("[1,[2,{\"c\":3}]]".into())
    );
    // The shape `tabs.list` actually answers with.
    assert_eq!(
        returns("return JSON.parse(\"[{\\\"id\\\":\\\"tab1\\\"}]\")[0].id;"),
        JsValue::Str("tab1".into())
    );
    // "`[undefined,1]` 是 `[null,1]` 而 `{a:undefined,b:1}` 是 `{\"b\":1}`"
    assert_eq!(
        returns("return JSON.stringify([undefined, 1]);"),
        JsValue::Str("[null,1]".into())
    );
    assert_eq!(
        returns("return JSON.stringify({ a: undefined, b: 1 });"),
        JsValue::Str("{\"b\":1}".into())
    );
    // "自引用数组和自引用对象一样是可 catch 的 TypeError"
    assert_eq!(
        returns(
            "let a = [1]; a[1] = a; let n = 0; \
             try { JSON.stringify(a); } catch (e) { n = 1; } return n;"
        ),
        JsValue::Number(1.0)
    );
}

/// The five methods this crate's copy claims, executed.
///
/// These arrived by a **measured** decision rather than a chosen one: upstream
/// `research/method-binding/` built three ways of getting the receiver to a
/// method body, ran all three against one corpus written before any of them
/// existed, and compared marginal costs. What is pinned here is only what the
/// product can see -- that the methods work through this crate's door, under
/// `Names::Declared`, and that the neighbours which did *not* land still fail
/// the way they did before.
#[test]
fn the_method_claims_in_this_crates_own_copy() {
    // "字符串：trim、indexOf"
    assert_eq!(
        returns("return \"  ab  \".trim();"),
        JsValue::Str("ab".into())
    );
    assert_eq!(
        returns("return \"abc\".indexOf(\"b\");"),
        JsValue::Number(1.0)
    );
    assert_eq!(
        returns("return \"abc\".indexOf(\"z\");"),
        JsValue::Number(-1.0)
    );
    // 空白集合是整个 ECMA-262 12.2 + 12.3，不是「看起来像空格」。
    assert_eq!(
        returns("return \"\u{3000}ab\u{2003}\".trim();"),
        JsValue::Str("ab".into())
    );
    // 位置是 UTF-16 码元，与 `.length` 对得上。
    assert_eq!(
        returns("return \"caf\u{e9}x\".indexOf(\"x\");"),
        JsValue::Number(4.0)
    );

    // "数组：push、pop、map"
    assert_eq!(
        returns("let a = [1, 2]; return a.push(3);"),
        JsValue::Number(3.0)
    );
    assert_eq!(
        returns("let a = [1, 2]; a.push(3); return a[2];"),
        JsValue::Number(3.0)
    );
    assert_eq!(
        returns("let a = [1, 2, 3]; return a.pop();"),
        JsValue::Number(3.0)
    );
    assert_eq!(
        returns("let a = [1]; a.pop(); return a.length;"),
        JsValue::Number(0.0)
    );
    assert_eq!(
        returns("let a = [1, 2]; return a.map(x => x + 1)[1];"),
        JsValue::Number(3.0)
    );
    // 回调能捕获外层绑定，且 map 可链。
    assert_eq!(
        returns("let k = 10; let a = [1]; return a.map(x => x + k)[0];"),
        JsValue::Number(11.0)
    );
    assert_eq!(
        returns("let a = [1]; return a.map(x => x + 1).map(x => x * 2)[0];"),
        JsValue::Number(4.0)
    );

    // "普通对象上同名的属性不受影响"
    assert_eq!(
        returns("const o = { trim: function () { return 7; } }; return o.trim();"),
        JsValue::Number(7.0)
    );

    // "没落地的方法仍然按各自接收者的规矩拒绝"
    for source in ["return \"ab\".toUpperCase();", "return (1).toFixed();"] {
        let mut eng = engine();
        assert!(
            eng.run_once(Guest::Qjs(source), None, "main", &[]).is_err(),
            "{source:?}: a String method this engine lacks must still trap"
        );
    }
    // 数组上没落地的方法读出来是 `undefined`，调用才 trap——两种接收者的规矩不同，
    // 这条差别是上游刻意保留的。
    assert_eq!(returns("let a = [1]; return a.filter;"), JsValue::Undefined);
}

/// The one built-in property this crate's copy claims, executed -- and the
/// neighbours it deliberately does not claim.
#[test]
fn the_string_length_claim_in_this_crates_own_copy() {
    // "`"ab".length` 现在给正确答案，数的是 UTF-16 码元不是 UTF-8 字节"
    assert_eq!(returns("return \"ab\".length;"), JsValue::Number(2.0));
    assert_eq!(returns("return \"\".length;"), JsValue::Number(0.0));
    assert_eq!(
        returns("return \"caf\u{e9}\".length;"),
        JsValue::Number(4.0)
    );
    assert_eq!(
        returns("return \"\u{1f600}\".length;"),
        JsValue::Number(2.0)
    );
    assert_eq!(
        returns("let s = \"abc\"; return s.length;"),
        JsValue::Number(3.0)
    );
    assert_eq!(returns("return `a${1}b`.length;"), JsValue::Number(3.0));

    // "那是一条臂，不是原型链：其它属性仍然 trap，而且是故意不给 undefined"
    // -- and since tinyvm d2e66b3 a String receiver's trap names the
    // property; since 1707721 so does a Number receiver's.
    for (source, want) in [
        ("return \"ab\".trim;", Some("trim")),
        ("return \"ab\".toUpperCase;", Some("toUpperCase")),
        ("return (1).toFixed;", None),
    ] {
        let mut eng = engine();
        let err = eng
            .run_once(Guest::Qjs(source), None, "main", &[])
            .expect_err("a String property this engine has no answer for must stop the script");
        match want {
            Some(name) => assert!(
                matches!(&err, QjswasmError::UnsupportedMethod(Some(n)) if n == name),
                "{source:?}: want `{name}` named, got {err:?}"
            ),
            // A Number receiver names its key too since tinyvm 1707721.
            None => assert!(
                matches!(&err, QjswasmError::PropertyOfNonObject(Some(k)) if k == "toFixed"),
                "{source:?}: want `toFixed` named off a Number, got {err:?}"
            ),
        }
    }
}

/// The arrow-function claims this crate's own copy makes, executed.
///
/// The reason they are worth their own test rather than folding into the
/// function tests: in the upstream engine an arrow is a function expression
/// *because* four things are absent -- `this`, `arguments`, `new`, function
/// properties. That equivalence is upstream's to keep and is pinned there
/// (`arrows_m3::the_absences_the_arrow_equivalence_rests_on`); what is pinned
/// here is only that this crate can reach the feature at all, under
/// `Names::Declared` and through the real door.
#[test]
fn the_arrow_claims_in_this_crates_own_copy() {
    // "括号参数表、单参数免括号、空参数表"
    assert_eq!(
        returns("let f = (x) => x + 1; return f(1);"),
        JsValue::Number(2.0)
    );
    assert_eq!(
        returns("let f = x => x * 3; return f(4);"),
        JsValue::Number(12.0)
    );
    assert_eq!(
        returns("let f = () => 7; return f();"),
        JsValue::Number(7.0)
    );
    assert_eq!(
        returns("let f = (a, b) => a * b; return f(3, 4);"),
        JsValue::Number(12.0)
    );

    // "简洁体就是它的 return，块体是普通函数体"
    assert_eq!(
        returns("let f = (x) => { let y = x * 2; return y; }; return f(5);"),
        JsValue::Number(10.0)
    );

    // "捕获也能用"——箭头是函数表达式，所以闭包那套原样适用。
    assert_eq!(
        returns("function mk(n) { return () => n; } return mk(6)();"),
        JsValue::Number(6.0)
    );
    assert_eq!(
        returns("let f = (x) => (y) => x + y; return f(1)(2);"),
        JsValue::Number(3.0)
    );

    // "分组括号还是分组括号"——覆盖文法没有把它吃掉。
    assert_eq!(
        returns("let g = (n) => n; return (1 + 2) * g(3);"),
        JsValue::Number(9.0)
    );

    // 与本仓已有的其它特性合用。
    assert_eq!(
        returns("let f = (a) => a[1]; return f([1, 2, 3]);"),
        JsValue::Number(2.0)
    );
    assert_eq!(
        returns("let f = (x) => `v${x}`; return f(3);"),
        JsValue::Str("v3".into())
    );
}

/// The template-literal claims this crate's own copy makes, executed.
///
/// The last group is the one that touches production, and it is smaller than
/// it was first claimed to be. `scripts/qjs/lib/fleet.qjs` builds its params
/// with `JSON.stringify` -- the right way, and one this engine already had --
/// so it hand-rolls concatenation in exactly **three** places, all of them
/// error messages. `grep -c '" *+ \|+ *"' scripts/qjs/lib/fleet.qjs` says 3.
/// Those three are what a template replaces there; the general case is
/// asserted anyway, because scripts other than that one binding library will
/// build strings that are not JSON.
#[test]
fn the_template_claims_in_this_crates_own_copy() {
    // "无替换的模板就是一个字符串"
    assert_eq!(returns("return `abc`;"), JsValue::Str("abc".into()));
    assert_eq!(returns("return ``;"), JsValue::Str(String::new()));
    assert_eq!(returns("return typeof `x`;"), JsValue::Str("string".into()));

    // "替换取的是 ToString"
    assert_eq!(returns("return `a${1}b`;"), JsValue::Str("a1b".into()));
    assert_eq!(returns("return `${true}`;"), JsValue::Str("true".into()));
    assert_eq!(returns("return `${null}`;"), JsValue::Str("null".into()));
    assert_eq!(returns("return `${1.5}`;"), JsValue::Str("1.5".into()));

    // "相邻的两个替换是拼接不是相加" -- `${1}${2}` 是 "12" 而不是 3.
    assert_eq!(returns("return `${1}${2}`;"), JsValue::Str("12".into()));

    // "替换里可以写任何表达式，包括带花括号的"
    assert_eq!(
        returns("return `${ { a: 7 }.a }`;"),
        JsValue::Str("7".into())
    );
    assert_eq!(
        returns("return `${[1, 2].length}`;"),
        JsValue::Str("2".into())
    );
    assert_eq!(
        returns("return `a${`b${1}`}c`;"),
        JsValue::Str("ab1c".into())
    );

    // The shape `fleet.qjs` actually hand-rolls: `throw "fleet " + opId + ...`.
    // A template must mean exactly what the concatenation meant.
    let concatenated = returns("let op = \"tabs.list\"; return \"fleet \" + op + \": \" + 2;");
    let templated = returns("let op = \"tabs.list\"; return `fleet ${op}: ${2}`;");
    assert_eq!(concatenated, templated);
    assert_eq!(templated, JsValue::Str("fleet tabs.list: 2".into()));
}

/// The two array claims the README states as **refusals**, which the corpus
/// above cannot hold because a refusal is not a returned value.
///
/// Both are deliberate answers and not accidents, so both are pinned: a
/// dropped write and a fabricated method result are each a wrong answer that
/// looks like a right one, which is the failure this engine refuses. They
/// were bare traps until tinyvm afc1e34; now a non-index write on an Array
/// says so (`InvalidWrite`), and the absent method stays a trap of its own
/// kind.
#[test]
fn the_array_claims_that_are_traps() {
    for (source, named) in [
        // "非索引属性写会拒绝" -- `foo` and `length` are both non-index keys here.
        ("let a = [1]; a.foo = 2; return 0;", true),
        ("let a = [1]; a.length = 0; return 0;", true),
        // "调用它 trap" -- an absent method read as `undefined`, then called.
        ("return [1, 2].map(1);", false),
    ] {
        let mut eng = engine();
        let err = eng
            .run_once(Guest::Qjs(source), None, "main", &[])
            .expect_err("this is one of the README's refusal claims");
        if named {
            assert!(
                matches!(&err, QjswasmError::InvalidWrite(Some(what)) if what.starts_with("an Array key")),
                "{source:?}: expected the named refusal, got {err:?}"
            );
        } else {
            assert!(
                matches!(err, QjswasmError::Trap(_) | QjswasmError::NotAFunction(_)),
                "{source:?}: expected a trap, got {err:?}"
            );
        }
    }
}

/// An Array cannot leave through this crate's face, and the refusal names what
/// it cannot carry.
///
/// The same answer an Object gets, for the same reason -- the payload is a
/// guest heap reference the host has no layout for and no way to keep alive --
/// and it is upstream's `Value` that has no variant, not this crate's. Pinned
/// here because the README says so in agenterm's voice.
#[test]
fn an_array_does_not_cross_this_crates_face() {
    let mut eng = engine();
    let err = eng
        .run_once(Guest::Qjs("return [1];"), None, "main", &[])
        .expect_err("an Array has no host-side variant");
    match &err {
        // The exact sentence, not a `contains`. Writing this test is what
        // found that upstream had added the tag without adding the arm that
        // names it, so a host was told "V1: unknown tag 7" -- which reads as
        // an engine defect and says nothing about what to do instead. Fixed
        // upstream at `577af37`; an equality here is what keeps it fixed.
        QjswasmError::Door(text) => assert_eq!(
            text,
            "the `.qjs` entry point returned V1: an Array is a guest heap \
             reference; `Value` has no variant for one yet"
        ),
        other => panic!("expected a door error, got {other:?}"),
    }
}

/// Numbers are ECMA-262 binary64, and the seam reports them as such.
///
/// This assertion is the *opposite* of the one M0 shipped here, which asserted
/// that `$0/0` trapped. That was true of an `i32` division and is now wrong:
/// 6.1.6.1 says a Number is an IEEE-754 double, so `1/0` is `Infinity`, `0/0`
/// is `NaN`, and `2147483647 + 1` does not wrap. This is a deliberate
/// correctness improvement, not a loosened assertion -- the old test locked a
/// limitation, and the limitation is gone.
#[test]
fn arithmetic_is_binary64_so_division_by_zero_is_infinity() {
    let mut eng = engine();
    let out = eng
        .run_once(Guest::Qjs("$0/0"), None, "main", &[number(1.0)])
        .expect("dividing by zero is a Number, not a fault");
    assert_eq!(out.values, vec![number(f64::INFINITY)]);

    // No NaN equals itself, so this one is asserted by predicate.
    assert!(
        matches!(returns("return 0/0;"), JsValue::Number(x) if x.is_nan()),
        "0/0 must be NaN"
    );
    assert_eq!(
        returns("return 2147483647 + 1;"),
        JsValue::Number(2147483648.0)
    );
    // `-x` keeps the sign of a zero, which an integer engine cannot express.
    assert_eq!(
        returns("let z = 0; return 1 / -z;"),
        JsValue::Number(f64::NEG_INFINITY)
    );
}

/// A `.qjs` guest that fails at *run* time is a trap, not a compile error.
///
/// The other side of the same boundary: reporting a run-time fault as a compile
/// failure would tell the author to fix their syntax.
///
/// The source calls a value that is not callable. ECMA-262 makes that a
/// TypeError; this engine has no exception objects for host-raised errors yet,
/// so it traps -- a recorded divergence, not an accident, and the trap is the
/// honest answer until `throw new TypeError(...)` exists to replace it.
///
/// It used to be `"2" * 2`, a String-to-Number coercion. The `f21f0f2` bump
/// implemented all three ECMA-262 string conversions (ba143c5), so that source
/// now evaluates to `Number(4.0)` and stopped testing anything. Then it was a
/// call on a non-function, named at a4e12fd; then `"" + {}`, named at the
/// bump after it. Every stop a `.qjs` program can reach now has a name, which
/// is the goal -- so the nameless trap this test classifies is a hand-written
/// guest whose `main` is an `unreachable`, the one shape that can never carry
/// a reason. What this test protects is the *classification* of a bare
/// run-time fault, not any particular gap in the language.
#[test]
fn a_runtime_fault_in_a_compiled_guest_is_a_trap() {
    let bytes =
        wat::parse_str(r#"(module (func (export "main") (unreachable)))"#).expect("fixture");
    let mut eng = engine();
    let err = eng
        .run_once(Guest::Wasm(&bytes), None, "main", &[])
        .expect_err("an unreachable traps rather than proceeding");
    assert!(
        matches!(err, QjswasmError::Trap(_)),
        "expected a trap from a bare unreachable, got {err:?}"
    );
}

/// A module refused while validating a function body says which function,
/// by the name the compiler wrote into the `name` section. Until tinyvm
/// 3e21027's successor this face said "loading wasm: validation: type
/// mismatch" and nothing else, and the author bisected (PRD A7).
#[test]
fn a_module_refused_in_a_function_names_the_function() {
    let bytes = wat::parse_str(
        r#"(module
             (func $fine (result i32) (i32.const 1))
             (func $broken (result i32) (i64.const 1))
             (export "main" (func $broken)))"#,
    )
    .expect("well-formed text");
    let err = engine()
        .run_once(Guest::Wasm(&bytes), None, "main", &[])
        .expect_err("the i64 result is a type mismatch");
    assert!(
        matches!(&err, QjswasmError::LoadInFunction { index: 1, name: Some(name), .. } if name == "broken"),
        "got {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "loading wasm: validation: type mismatch in function `broken` (#1)"
    );
    // With the `qjs.lines` section the compiler writes beside the names --
    // appended by hand here, since the compiler never emits a body that
    // fails validation -- the face also says which line the author wrote
    // the function on: index 1 on line 7.
    let mut lined = bytes.clone();
    lined.extend_from_slice(&[0, 15, 9]);
    lined.extend_from_slice(b"qjs.lines");
    lined.extend_from_slice(&[2, 0, 3, 1, 7]);
    let err = engine()
        .run_once(Guest::Wasm(&lined), None, "main", &[])
        .expect_err("still a type mismatch");
    assert!(
        matches!(
            &err,
            QjswasmError::LoadInFunction {
                index: 1,
                line: Some(7),
                ..
            }
        ),
        "got {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "loading wasm: validation: type mismatch in function `broken` (#1) (line 7)"
    );
    // A `.qjs` guest's functions carry the script's names: a failure inside
    // `helper` would be reported as such. (No such failure is known today --
    // this pins the channel, not a defect.)
    assert!(
        matches!(
            validate_wasm(&bytes),
            Err(QjswasmError::LoadInFunction { .. })
        ),
        "the static gate reports it the same way"
    );
}

/// The last two kinds of stop a `.qjs` program can reach are named at this
/// face too (tinyvm A11 b/c/d): a refused write is the script's own doing,
/// class `Script`; `split("")` and a mid-surrogate `slice` are boundaries of
/// the representation and stay `CapabilityBoundary`, now saying which.
#[test]
fn a_refused_write_and_a_named_boundary_are_named_here_too() {
    let mut eng = engine();
    let err = eng
        .run_once(
            Guest::Qjs("let a = [1]; a[\"x\"] = 2; return a.length;"),
            None,
            "main",
            &[],
        )
        .expect_err("a non-index key on an Array is refused");
    assert!(
        matches!(&err, QjswasmError::InvalidWrite(Some(what)) if what.starts_with("an Array key")),
        "got {err:?}"
    );
    assert!(err.to_string().contains("integer indices"), "{err}");
    let err = eng
        .run_once(
            Guest::Qjs("return \"ab\".split(\"\").length;"),
            None,
            "main",
            &[],
        )
        .expect_err("split by the empty string is a boundary");
    assert!(
        matches!(&err, QjswasmError::CapabilityBoundary(Some(which)) if which == "split with an empty separator"),
        "got {err:?}"
    );
    assert!(err.to_string().contains("real separator"), "{err}");
}

/// ToString or ToNumber of an Object, an Array or a function is a *named*
/// refusal at this face, with the kind. ECMA-262 would answer `[object
/// Object]`; this engine never converts one quietly (a value silently becoming
/// that text in a command line is the footgun), and until the bump after
/// a4e12fd the stop was a bare trap that this test's neighbour used as its
/// example. The class is `Script`: the script's own doing, and
/// `JSON.stringify` is the spelling that says what was meant.
#[test]
fn a_value_with_no_primitive_form_is_named_with_its_kind() {
    let mut eng = engine();
    for (source, kind) in [
        ("let o = {}; return \"\" + o;", "an Object"),
        ("let a = [1]; return a * 2;", "an Array"),
        (
            "let f = function () { return 1; }; return f + 1;",
            "a function",
        ),
    ] {
        let err = eng
            .run_once(Guest::Qjs(source), None, "main", &[])
            .expect_err("no quiet conversion");
        assert!(
            matches!(&err, QjswasmError::NoPrimitiveForm(Some(k)) if k == kind),
            "{source}: expected NoPrimitiveForm({kind:?}), got {err:?}"
        );
        assert!(
            err.to_string().contains(kind) && err.to_string().contains("JSON.stringify"),
            "{err}"
        );
    }
}

/// An uncaught `throw` is its own answer, and specifically **not** the trap the
/// core reported.
///
/// Both failures leave the guest through the same `unreachable`, so tinyvm
/// hands this face the identical `WasmError` in both cases and folding them
/// together is the path of least resistance. It is also wrong three ways: a
/// script that throws is not broken, is not over budget, and did not do
/// anything the author needs to fix -- ECMA-262 says a program whose exception
/// reaches the top terminates with it, which is the script running as written.
///
/// The compiler goes to real trouble to make the distinction available --
/// it writes a code into the first word of the guest's linear memory *before*
/// executing the `unreachable`, precisely because the trap itself cannot carry
/// a reason. This test is the reason that trouble was worth taking: it pins
/// that this crate spends the evidence instead of leaving it on the floor,
/// which is what it did until now.
/// A String property this engine lacks is named at the engine face. Until
/// tinyvm d2e66b3 the guest trapped bare (or, if the program said `.length`
/// somewhere, as a nameless capability boundary), and every migrated script
/// that reached `slice` reported "guest trapped: unreachable executed"
/// (`slice` has since landed; `substring` stands in as the missing one).
#[test]
fn a_missing_string_method_is_named_at_the_engine_face() {
    let mut eng = engine();
    let err = eng
        .run_once(
            Guest::Qjs("let s = \"abc\"; return s.substring(0, 2);"),
            None,
            "main",
            &[],
        )
        .expect_err("a property this engine lacks stops the script");
    assert!(
        matches!(&err, QjswasmError::UnsupportedMethod(Some(name)) if name == "substring"),
        "expected the property to be named, got {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "this engine does not support `substring` on a String yet; the script reached it at run time"
    );
}

/// `print(s.length)` names the call and the argument since tinyvm 1012da1;
/// it was "guest trapped: unreachable executed", the first thing every
/// script author saw.
#[test]
fn a_host_argument_of_the_wrong_type_is_named_at_the_engine_face() {
    let mut eng = engine();
    let err = eng
        .run_once(
            Guest::Qjs("let s = \"abc\"; print(s.length);"),
            None,
            "main",
            &[],
        )
        .expect_err("a Number where print wants a String stops the script");
    assert!(
        matches!(&err, QjswasmError::HostArgument(Some((host, 1))) if host == "print"),
        "expected print#1 named, got {err:?}"
    );
    assert!(
        err.to_string()
            .starts_with("host function `print` needs a String for argument 1"),
        "{err}"
    );
}

/// `undefined.x` names the key since tinyvm 1707721; it was "guest trapped:
/// unreachable executed", and every migrated script guards JSON fields with
/// `=== undefined` because of it.
#[test]
fn a_property_read_off_undefined_is_named_at_the_engine_face() {
    let mut eng = engine();
    let err = eng
        .run_once(
            Guest::Qjs("let o = {}; let f = o.missing; return f.name;"),
            None,
            "main",
            &[],
        )
        .expect_err("reading off undefined stops the script");
    assert!(
        matches!(&err, QjswasmError::PropertyOfNonObject(Some(key)) if key == "name"),
        "expected the key named, got {err:?}"
    );
    assert!(
        err.to_string()
            .starts_with("the script read `name` off a value that has no properties"),
        "{err}"
    );
}

/// `slice` itself answers since tinyvm 6b9464a: code-unit positions, negative
/// indices, the harness's truncation shape.
#[test]
fn slice_answers_on_code_units() {
    assert_eq!(
        returns("return \"abcdef\".slice(1, 3);"),
        JsValue::Str("bc".into())
    );
    assert_eq!(
        returns("return \"abcdef\".slice(-2);"),
        JsValue::Str("ef".into())
    );
    assert_eq!(
        returns("return \"caf\u{e9}x\".slice(3, 4);"),
        JsValue::Str("\u{e9}".into())
    );
}

#[test]
fn an_uncaught_throw_is_reported_as_a_throw_and_not_as_a_trap() {
    let mut eng = engine();
    let err = eng
        .run_once(Guest::Qjs("throw \"boom\";"), None, "main", &[])
        .expect_err("a script whose exception reaches the top does not complete");
    assert!(
        matches!(err, QjswasmError::UncaughtThrow(_)),
        "expected the throw to be named, got {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "the script threw and nothing caught it: boom"
    );
}

/// A caught `throw` is not a failure at all, and leaves nothing behind that
/// makes the *next* call look like one.
///
/// The fault word is one location in a persistent instance, so a stale code
/// from an earlier call is the obvious way this classification goes wrong:
/// every subsequent success would be readable as a throw. Upstream clears the
/// word on entry, and this is the assertion that keeps that guarantee honest
/// from the product's side -- one slot, called twice, throwing only the first
/// time.
#[test]
fn a_fault_word_from_an_earlier_call_does_not_taint_the_next_one() {
    let mut eng = engine();
    let slot = eng
        .spawn(Guest::Qjs("if ($0) { throw \"first\"; } return 7;"), None)
        .expect("spawn");

    let err = eng
        .call(slot, "main", &[Value::Js(JsValue::Bool(true))])
        .expect_err("the first call throws");
    assert!(
        matches!(err, QjswasmError::UncaughtThrow(_)),
        "expected the throw to be named, got {err:?}"
    );

    let outcome = eng
        .call(slot, "main", &[Value::Js(JsValue::Bool(false))])
        .expect("the second call does not throw, and must not inherit the first one's word");
    assert_eq!(outcome.values, vec![Value::Js(JsValue::Number(7.0))]);
}

/// A slot accepts values in the convention it was loaded under, and refuses the
/// other one rather than reinterpreting the bits.
///
/// Both directions are the same mistake seen from either side, and neither is
/// the guest's fault -- which is why both are `UnsupportedValue` and not
/// `Trap`. Without this, a `Value::I32(20)` handed to a `.qjs` entry point
/// would be a wasm arity mismatch reported as a trap, blaming a guest that did
/// nothing wrong.
#[test]
fn a_value_offered_in_the_wrong_convention_is_refused_at_the_face() {
    let mut eng = engine();
    let qjs = eng.spawn(Guest::Qjs("$0+1"), None).expect("spawn");
    let err = eng
        .call(qjs, "main", &[Value::I32(20)])
        .expect_err("a `.qjs` entry point does not take raw wasm numerics");
    assert!(
        matches!(err, QjswasmError::UnsupportedValue(_)),
        "got {err:?}"
    );

    let bytes =
        wat::parse_str("(module (func (export \"id\") (param i32) (result i32) local.get 0))")
            .expect("valid wat");
    let hand_written = eng.spawn(Guest::Wasm(&bytes), None).expect("spawn");
    let err = eng
        .call(hand_written, "id", &[number(1.0)])
        .expect_err("a hand-written module does not take JavaScript values");
    assert!(
        matches!(err, QjswasmError::UnsupportedValue(_)),
        "got {err:?}"
    );

    // A String *argument* is the one JS value the face cannot hand in: it would
    // have to be allocated in the guest's heap, and there is no door onto that
    // allocator. Refused for the same reason and in the same class -- never
    // faked with a pointer that means nothing.
    let err = eng
        .call(qjs, "main", &[Value::Js(JsValue::Str("x".into()))])
        .expect_err("a string argument has nowhere to live in the guest yet");
    assert!(
        matches!(err, QjswasmError::UnsupportedValue(_)),
        "got {err:?}"
    );
}

/// What the compiler emits clears *this* crate's load gate, under this crate's
/// budget -- not merely tinyvm's defaults.
///
/// `Budget` is agenterm's policy and can be tightened independently of the
/// compiler. A guest that only ever loads under `Limits::default()` would be
/// evidence about upstream's dials, not about the ones this engine ships.
///
/// This got sharper with the bump: a compiled module now declares a linear
/// memory (the string literal pool and the bump allocator live in it) and
/// carries an emitted runtime, so "does the product of the compiler fit through
/// the host's limits?" is a real question here for the first time.
#[test]
fn compiled_bytes_clear_this_crates_load_gate() {
    for source in [
        "0",
        "$0*($1+$2)-$3/$4",
        "-(-(-1))",
        "((1+2)*(3-4))/5",
        "return \"a string literal that occupies the pool\";",
        "function f(n) { if (n < 2) { return n; } return f(n-1); } return f(3);",
    ] {
        let bytes = compile_qjs(source).unwrap_or_else(|e| panic!("{source:?}: {e}"));
        assert_eq!(&bytes[..4], b"\0asm", "{source:?} is not a wasm module");
        validate_wasm(&bytes)
            .unwrap_or_else(|e| panic!("{source:?} rejected at the load gate: {e}"));
    }
    // And under a deliberately stingy budget, since that is the point of having
    // one: a module is checked against the limits it will actually run under.
    let tight = Budget {
        limits: tinyvm::Limits {
            max_memory_pages: 1,
            max_table_elems: 0,
            ..tinyvm::Limits::default()
        },
        ..Budget::default()
    };
    for source in ["$0+1", "return \"s\";"] {
        let bytes = compile_qjs(source).unwrap();
        agenterm_qjswasm::validate_wasm_with(&bytes, &tight)
            .unwrap_or_else(|e| panic!("{source:?} needs more than one page and no table: {e}"));
    }
}

/// Extension routing, which is this crate's and not the compiler's: `.qjs`
/// reaches the compiler, `.wasm` skips it, and nothing else is claimed.
#[test]
fn extension_routing_covers_exactly_two_extensions() {
    assert_eq!(guest_kind_for_path("scripts/x.qjs"), Some(GuestKind::Qjs));
    assert_eq!(guest_kind_for_path("scripts/x.wasm"), Some(GuestKind::Wasm));
    for path in ["scripts/x.js", "scripts/x.mjs", "scripts/x", "x.qjs.txt"] {
        assert_eq!(
            guest_kind_for_path(path),
            None,
            "{path} must not route here"
        );
    }
}

/// Compiling to bytes and loading those bytes back gives the *same* value as
/// running the source directly.
///
/// This is the acceptance criterion the `agenterm-qjs` CLI archive gate names
/// as its step zero (`plan/design-qjs-archive-gate.md` 4.3 and 9). Every
/// compile-once-run-later verb -- `pack`, a build cache, an artifact fetched
/// from anywhere -- is built on it, and before `Guest::CompiledQjs` existed it
/// was false: `Guest::Wasm(&compile_qjs(src))` loaded the same bytes under the
/// wasm convention, so `42` came back as the raw V1 pair `[I32(1),
/// I64(4631107791820423168)]` instead of `Number(42.0)`, and a string came back
/// as a pointer into a linear memory the caller was about to drop.
///
/// A `.wasm` file does not remember where it came from. The convention has to
/// be carried by whoever loads it, which is what the variant is for -- and the
/// third leg below is why: the same bytes named as plain wasm still speak the
/// pair, deliberately, so this is a choice the caller makes rather than a
/// property the engine guesses at.
#[test]
fn a_compiled_artifact_reloaded_gives_the_same_value_as_its_source() {
    for (source, want) in [
        ("return 42;", Value::Js(JsValue::Number(42.0))),
        ("return \"hello\";", Value::Js(JsValue::Str("hello".into()))),
        ("return true;", Value::Js(JsValue::Bool(true))),
        ("return null;", Value::Js(JsValue::Null)),
        ("let unused = 1;", Value::Js(JsValue::Undefined)),
    ] {
        let bytes = compile_qjs(source).unwrap_or_else(|e| panic!("{source:?}: {e}"));

        let direct = engine()
            .run_once(Guest::Qjs(source), None, "main", &[])
            .unwrap_or_else(|e| panic!("{source:?} direct: {e}"));
        let reloaded = engine()
            .run_once(Guest::CompiledQjs(&bytes), None, "main", &[])
            .unwrap_or_else(|e| panic!("{source:?} reloaded: {e}"));

        assert_eq!(direct.values, vec![want.clone()], "{source:?} direct");
        assert_eq!(reloaded.values, direct.values, "{source:?} reloaded");
    }

    // The same bytes under the wasm convention are a different answer, on
    // purpose: `Convention` is recorded, never inferred. If this ever starts
    // matching the two above, the engine has begun guessing at signatures.
    let bytes = compile_qjs("return 42;").expect("compiles");
    let raw = engine()
        .run_once(Guest::Wasm(&bytes), None, "main", &[])
        .expect("compiled bytes are ordinary wasm too");
    assert_eq!(
        raw.values,
        vec![Value::I32(1), Value::I64(42.0f64.to_bits() as i64)],
        "loading compiled bytes as plain wasm must still hand over the raw V1 pair"
    );
}

/// The pin printed by [`agenterm_qjswasm::identity`] is the pin the build
/// actually uses.
///
/// A version string is worth exactly as much as its accuracy, and this one
/// names an upstream revision that lives in a different file -- two literals
/// in `Cargo.toml`, which a bump edits and a constant does not. Reading the
/// manifest here is what makes printing the constant honest.
///
/// It also catches the half-bump: `tinyvm` and `tinyvm-qjs` are two
/// dependencies on one repository and must be the same revision. They have
/// never diverged, and this is what would say so if they did.
#[test]
fn the_printed_upstream_revision_is_the_one_this_build_pins() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("this crate's own Cargo.toml");

    let pins: Vec<&str> = manifest
        .lines()
        .filter(|line| line.contains("partnernetsoftware/tinyvm"))
        .map(|line| {
            let at = line.find("rev = \"").expect("a pinned rev") + "rev = \"".len();
            let rest = &line[at..];
            &rest[..rest.find('"').expect("a closing quote")]
        })
        .collect();

    assert_eq!(
        pins.len(),
        2,
        "expected both tinyvm crates pinned, got {pins:?}"
    );
    assert_eq!(
        pins[0], pins[1],
        "the two tinyvm crates are on different revisions"
    );
    assert_eq!(
        agenterm_qjswasm::UPSTREAM_TINYVM_REV,
        pins[0],
        "UPSTREAM_TINYVM_REV is stale; the build pins {}",
        pins[0]
    );

    let identity = agenterm_qjswasm::identity();
    assert!(identity.starts_with("agenterm-qjswasm "), "{identity}");
    assert!(identity.contains(pins[0]), "{identity}");
}

/// **`_start` is a name, not a convention** — and the entry-point question
/// PRD 02.36 carried as an open product decision is retired here by two
/// measurements rather than answered by a preference.
///
/// # What the question was
///
/// `agenterm-wasmcore`'s archive gate 2 (switch `.wasm` default routing to
/// this engine) was recorded as blocked on "does a hand-written `.wasm` guest
/// use WASI's `_start`, or an exported `main`?", called a product decision.
/// It is not a decision, because neither answer is reachable from where the
/// disagreement actually is.
///
/// # Fact one: this engine calls whatever name you ask for
///
/// There is no entry convention to choose. `Engine::call` takes the export's
/// name, `"main"` is only what the `.qjs` compiler happens to emit, and
/// `_start` is an ordinary export that works today. A "convention" that costs
/// nothing to satisfy either way is not a decision anyone has to make.
///
/// # Fact two: a WASI guest cannot reach the entry point at all
///
/// It is refused at the door, by import, before any export name is consulted.
/// That is the door discipline PRD 02.36 states -- *"能力全在门。门名单是
/// `agenterm.*`，不得把 WASI `fd_*` 做成第二扇 OS 面"* -- doing exactly what it
/// was written to do.
///
/// So what separates the two engines is the **import surface**, not the entry
/// name. A `wasmcore` guest is a Rust `std` program built for `wasm32-wasip1`:
/// its `_start` is `std::rt::lang_start` and it imports WASI *because std
/// does*, not because the product asked. Moving such a guest here is a
/// `no_std` rewrite of the guest, which is a guest-authoring cost that can be
/// measured -- not a convention the product has to pick.
#[test]
fn the_wasm_entry_point_is_a_name_and_the_wasi_surface_is_the_real_boundary() {
    let mut eng = engine();

    // A WASI command: imports `wasi_snapshot_preview1`, exports `_start`.
    let wasi_command = wat::parse_str(
        r#"(module
             (import "wasi_snapshot_preview1" "fd_write"
               (func $fd_write (param i32 i32 i32 i32) (result i32)))
             (memory (export "memory") 1)
             (func (export "_start")
               (drop (call $fd_write (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)))))"#,
    )
    .expect("fixture .wat must compile");

    let refused = eng
        .run_once(Guest::Wasm(&wasi_command), None, "_start", &[])
        .expect_err("a WASI guest has no door here");
    let refused = refused.to_string();
    assert!(
        refused.contains("wasi_snapshot_preview1.fd_write"),
        "the refusal must name the import it could not bind, got {refused:?}"
    );
    assert!(
        refused.contains("agenterm.*"),
        "and the surface it does offer, got {refused:?}"
    );
    assert!(
        !refused.contains("_start"),
        "the export name must not appear: it was never consulted, and saying it \
         would send a reader to rename their entry point. Got {refused:?}"
    );

    // The same module without the WASI import runs under that very name.
    let no_wasi = wat::parse_str(r#"(module (func (export "_start")))"#).expect("fixture");
    eng.run_once(Guest::Wasm(&no_wasi), None, "_start", &[])
        .expect("`_start` is an export like any other");

    // And so does any other name, which is what makes it not a convention.
    let named = wat::parse_str(r#"(module (func (export "anything") (result i32) (i32.const 7)))"#)
        .expect("fixture");
    let out = eng
        .run_once(Guest::Wasm(&named), None, "anything", &[])
        .expect("an export is reached by its name");
    assert_eq!(out.values, vec![Value::I32(7)]);
}

/// The PRD's stated pin is the pin.
///
/// `prd/PRD_02_36_agenterm_qjswasm.md` ends its revision chain with
/// ``**`<rev>`**（…当前 pin…）``. That line was found stale twice on
/// 2026-08-29 -- once by a human asking whether the work was done -- and both
/// times it was corrected by hand, which is not a gate. This is the gate: the
/// bolded rev in the chain must be the one `Cargo.toml` pins, so the PRD
/// cannot claim a revision the build does not have.
#[test]
fn the_prd_states_the_revision_this_build_pins() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("this crate's own Cargo.toml");
    let pinned = manifest
        .lines()
        .find(|line| line.contains("partnernetsoftware/tinyvm"))
        .and_then(|line| {
            let at = line.find("rev = \"")? + "rev = \"".len();
            let rest = &line[at..];
            Some(&rest[..rest.find('"')?])
        })
        .expect("a pinned tinyvm rev");

    let prd = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../prd/PRD_02_36_agenterm_qjswasm.md"
    ))
    .expect("the PRD beside this crate");
    let stated = prd
        .lines()
        .filter(|line| line.contains("当前 pin"))
        .find_map(|line| {
            let at = line.find("**`")? + "**`".len();
            let rest = &line[at..];
            Some(&rest[..rest.find('`')?])
        })
        .expect("the PRD's revision chain ends in a bolded current pin");

    assert_eq!(
        stated, pinned,
        "prd/PRD_02_36_agenterm_qjswasm.md says the current pin is `{stated}` but \
         Cargo.toml pins `{pinned}`; update the chain in the same change as the pin"
    );
}

/// A call on a non-function names the callee since tinyvm a4e12fd; it was
/// "guest trapped: unreachable executed", which is what a lint that wrote
/// `[...].concat(x)` -- a method this engine does not have -- died with.
/// A script that can catch gets the TypeError instead and never reaches the
/// engine face as an error.
#[test]
fn a_call_on_a_non_function_is_named_at_the_engine_face() {
    let mut eng = engine();
    let err = eng
        .run_once(
            Guest::Qjs("let a = [1]; return a.concat([2]).length;"),
            None,
            "main",
            &[],
        )
        .expect_err("calling a missing method stops the script");
    assert!(
        matches!(&err, QjswasmError::NotAFunction(Some(callee)) if callee == "concat"),
        "expected the callee named, got {err:?}"
    );
    assert!(
        err.to_string()
            .starts_with("the script called `concat`, which is not a function"),
        "{err}"
    );
    assert_eq!(
        returns("let a = [1]; try { a.concat([2]); } catch (e) { return e; } return \"ran\";"),
        JsValue::Str("TypeError: concat is not a function".into())
    );
}
