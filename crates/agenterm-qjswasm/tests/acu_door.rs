use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use agenterm_qjswasm::{
    AcuBridgeFn, Budget, Engine, Guest, HostBridges, JsValue, QjswasmError, Value, compile_qjs,
    compile_qjs_without_door,
};

fn bridges(acu: Option<AcuBridgeFn>) -> HostBridges {
    HostBridges { fleet: None, acu }
}

fn run(source: &str, acu: Option<AcuBridgeFn>) -> Result<agenterm_qjswasm::Outcome, QjswasmError> {
    Engine::new().run_once_with_bridges(Guest::Qjs(source), bridges(acu), "main", &[])
}

fn returned_string(outcome: agenterm_qjswasm::Outcome) -> String {
    match outcome.values.as_slice() {
        [Value::Js(JsValue::Str(value))] => value.clone(),
        other => panic!("wanted one string, got {other:?}"),
    }
}

#[test]
fn absent_bridge_is_status_2_without_a_fallback() {
    let answer = returned_string(
        run(
            r#"let s = acu_call("{}"); return s + ":" + acu_result();"#,
            None,
        )
        .expect("script continues after no bridge"),
    );
    assert_eq!(
        answer,
        "2:agenterm: no ACU bridge is installed in this slot"
    );
}

#[test]
fn command_and_complete_reply_round_trip() {
    let acu: AcuBridgeFn = Arc::new(|command, _, _, _| {
        assert_eq!(command, r#"{"verb":"capabilities","target":"current"}"#);
        Ok(r#"{"ok":false,"target":"current","command":"capabilities","error":{"code":"refused","message":"no"}}"#.to_owned())
    });
    let answer = returned_string(
        run(
            r#"let s = acu_call("{\"verb\":\"capabilities\",\"target\":\"current\"}"); if (s !== 0) { return "bad"; } return acu_result();"#,
            Some(acu),
        )
        .expect("ACU reply is data"),
    );
    assert!(answer.contains(r#""ok":false"#));
}

#[test]
fn success_and_failure_reply_bytes_are_billed_equally() {
    fn cost_for(answer: Result<String, String>) -> (u64, u64) {
        let outcome = run(
            r#"acu_call("{}"); return acu_result();"#,
            Some(Arc::new(move |_, _, _, _| answer.clone())),
        )
        .expect("the bridge reply remains guest-readable");
        (outcome.host_ops, outcome.host_bytes)
    }

    let success = cost_for(Ok("same".to_owned()));
    let failure = cost_for(Err("same".to_owned()));
    assert_eq!(success.0, 1);
    assert_eq!(failure.0, 1);
    assert_eq!(success.1, 2 + 4, "request plus reply bytes");
    assert_eq!(success.1, failure.1);

    let mut engine = Engine::new();
    let error = engine
        .run_once_with_bridges(
            Guest::Qjs(r#"acu_call("{}"); return acu_result();"#),
            bridges(Some(Arc::new(|_, _, _, _| -> Result<String, String> {
                panic!("not a bridge reply")
            }))),
            "main",
            &[],
        )
        .expect_err("a bridge panic remains a door fault");
    assert!(matches!(error, QjswasmError::Door(_)));
    let cost = engine
        .take_failed_cost()
        .expect("the attempted call is billed");
    assert_eq!(cost.host_ops, 1);
    assert_eq!(cost.host_bytes, 2, "panic text never crossed as a reply");

    let outcome = Engine::with_budget(Budget {
        max_bridge_result_bytes: 3,
        ..Budget::default()
    })
    .run_once_with_bridges(
        Guest::Qjs(r#"acu_call("{}"); return acu_result();"#),
        bridges(Some(Arc::new(|_, _, _, _| Ok("oversized".to_owned())))),
        "main",
        &[],
    )
    .expect("an oversized reply becomes a readable bounded refusal");
    let host_bytes = outcome.host_bytes;
    let refusal = returned_string(outcome);
    assert!(refusal.contains("ACU result exceeds"));
    assert_eq!(host_bytes, 2 + refusal.len() as u64);
}

#[test]
fn bad_guest_bytes_are_status_1_and_oob_traps_before_dispatch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_bridge = Arc::clone(&calls);
    let acu: AcuBridgeFn = Arc::new(move |_, _, _, _| {
        calls_for_bridge.fetch_add(1, Ordering::Relaxed);
        Ok("{}".to_owned())
    });
    let invalid_utf8 = wat::parse_str(
        r#"(module
          (import "agenterm" "acu_call" (func $call (param i32 i32) (result i32)))
          (memory 1) (data (i32.const 0) "\ff")
          (func (export "main") (result i32) i32.const 0 i32.const 1 call $call))"#,
    )
    .expect("wat");
    let outcome = Engine::new()
        .run_once_with_bridges(
            Guest::Wasm(&invalid_utf8),
            bridges(Some(Arc::clone(&acu))),
            "main",
            &[],
        )
        .expect("bad utf8 is recoverable");
    assert_eq!(outcome.values, vec![Value::I32(1)]);

    let oob = wat::parse_str(
        r#"(module
          (import "agenterm" "acu_call" (func $call (param i32 i32) (result i32)))
          (memory 1)
          (func (export "main") (result i32) i32.const 65535 i32.const 2 call $call))"#,
    )
    .expect("wat");
    let error = Engine::new()
        .run_once_with_bridges(Guest::Wasm(&oob), bridges(Some(acu)), "main", &[])
        .expect_err("OOB must trap");
    assert!(matches!(error, QjswasmError::Trap(_)));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[test]
fn oversize_host_op_cancel_and_panic_are_contained() {
    let budget = Budget {
        max_bridge_result_bytes: 3,
        ..Budget::default()
    };
    let acu: AcuBridgeFn = Arc::new(|_, _, _, _| Ok("1234".to_owned()));
    let answer = returned_string(
        Engine::with_budget(budget)
            .run_once_with_bridges(
                Guest::Qjs(r#"let s=acu_call("{}"); return s + ":" + acu_result();"#),
                bridges(Some(acu)),
                "main",
                &[],
            )
            .expect("oversize is status 1"),
    );
    assert!(answer.starts_with("1:agenterm: ACU result exceeds"));

    let budget = Budget {
        max_host_ops: 0,
        ..Budget::default()
    };
    let error = Engine::with_budget(budget)
        .run_once_with_bridges(
            Guest::Qjs(r#"return acu_call("{}");"#),
            bridges(Some(Arc::new(|_, _, _, _| Ok("{}".to_owned())))),
            "main",
            &[],
        )
        .expect_err("host operation cap");
    assert!(matches!(error, QjswasmError::Budget("max_host_ops")));

    let cancelled = Arc::new(AtomicBool::new(true));
    let budget = Budget {
        cancel: Some(cancelled),
        ..Budget::default()
    };
    let error = Engine::with_budget(budget)
        .run_once_with_bridges(
            Guest::Qjs(r#"return acu_call("{}");"#),
            bridges(Some(Arc::new(|_, _, _, _| Ok("{}".to_owned())))),
            "main",
            &[],
        )
        .expect_err("cancel");
    assert!(matches!(error, QjswasmError::Cancelled));

    let error = run(
        r#"return acu_call("{}");"#,
        Some(Arc::new(|_, _, _, _| -> Result<String, String> {
            panic!("boom")
        })),
    )
    .expect_err("panic is contained");
    assert!(
        matches!(error, QjswasmError::Door(message) if message.contains("ACU bridge panicked") && message.contains("boom"))
    );
}

#[test]
fn only_a_cooperatively_acknowledged_acu_cancel_discards_the_reply() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let raised = Arc::clone(&cancelled);
    let budget = Budget {
        cancel: Some(Arc::clone(&cancelled)),
        ..Budget::default()
    };
    let error = Engine::with_budget(budget)
        .run_once_with_bridges(
            Guest::Qjs(r#"acu_call("{}"); return acu_result();"#),
            bridges(Some(Arc::new(move |_, token, observed, acknowledged| {
                assert!(token.is_some());
                raised.store(true, Ordering::Release);
                observed.store(true, Ordering::Release);
                acknowledged.store(true, Ordering::Release);
                Ok("cancelled observe reply".to_owned())
            }))),
            "main",
            &[],
        )
        .expect_err("an acknowledged pre-effect cancellation is not script-catchable data");
    assert!(matches!(error, QjswasmError::Cancelled));
    assert!(
        cancelled.load(Ordering::Acquire),
        "an acknowledged cancellation remains raised rather than entering the authoritative arm"
    );
}

#[test]
fn a_late_cancel_does_not_hide_an_authoritative_acu_reply() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let raised = Arc::clone(&cancelled);
    let budget = Budget {
        cancel: Some(Arc::clone(&cancelled)),
        ..Budget::default()
    };
    let outcome = Engine::with_budget(budget)
        .run_once_with_bridges(
            Guest::Qjs(
                r#"
acu_call("{}");
const answer = acu_result();
let i = 0;
while (i < 4096) { i = i + 1; }
return answer;
"#,
            ),
            bridges(Some(Arc::new(move |_, _, observed, _| {
                raised.store(true, Ordering::Release);
                observed.store(true, Ordering::Release);
                Ok("authoritative reply".to_owned())
            }))),
            "main",
            &[],
        )
        .expect("an unacknowledged late cancel preserves the bridge reply");
    assert_eq!(returned_string(outcome), "authoritative reply");
    assert!(
        !cancelled.load(Ordering::Acquire),
        "the authoritative round consumes its late one-shot cancellation"
    );
}

#[test]
fn an_unobserved_new_cancel_is_not_consumed_with_the_reply() {
    let cancelled = Arc::new(AtomicBool::new(false));
    let raised = Arc::clone(&cancelled);
    let budget = Budget {
        cancel: Some(Arc::clone(&cancelled)),
        ..Budget::default()
    };
    let error = Engine::with_budget(budget)
        .run_once_with_bridges(
            Guest::Qjs(
                r#"
acu_call("{}");
const answer = acu_result();
let i = 0;
while (i < 4096) { i = i + 1; }
return answer;
"#,
            ),
            bridges(Some(Arc::new(move |_, _, _observed, _acknowledged| {
                // Models a worker Cancel/owner-loss store after the provider's
                // final cancellation probe: the round did not observe it.
                raised.store(true, Ordering::Release);
                Ok("completed reply".to_owned())
            }))),
            "main",
            &[],
        )
        .expect_err("the unobserved new request remains live for the VM poll");
    assert!(matches!(error, QjswasmError::Cancelled));
    assert!(
        cancelled.load(Ordering::Acquire),
        "a concurrent new cancel or owner-loss signal must not be cleared"
    );
}

#[test]
fn slots_keep_bridges_and_results_isolated() {
    let bytes = compile_qjs(r#"acu_call("{}"); return acu_result();"#).expect("compile");
    let mut engine = Engine::new();
    let first = engine
        .spawn_with_bridges(
            Guest::CompiledQjs(&bytes),
            bridges(Some(Arc::new(|_, _, _, _| Ok("first".to_owned())))),
        )
        .expect("first slot");
    let second = engine
        .spawn_with_bridges(
            Guest::CompiledQjs(&bytes),
            bridges(Some(Arc::new(|_, _, _, _| Ok("second".to_owned())))),
        )
        .expect("second slot");
    assert_eq!(
        returned_string(engine.call(first, "main", &[]).expect("first")),
        "first"
    );
    assert_eq!(
        returned_string(engine.call(second, "main", &[]).expect("second")),
        "second"
    );
}

#[test]
fn check_execute_share_bytes_and_unused_acu_costs_zero_guest_bytes() {
    let pure = "return 1 + 1;";
    assert_eq!(
        compile_qjs(pure).expect("door"),
        compile_qjs_without_door(pure).expect("pure")
    );

    let source = r#"acu_call("{}"); return acu_result();"#;
    let bytes = compile_qjs(source).expect("check compile");
    let acu: AcuBridgeFn = Arc::new(|_, _, _, _| Ok("same".to_owned()));
    let direct = returned_string(run(source, Some(Arc::clone(&acu))).expect("source"));
    let artifact = returned_string(
        Engine::new()
            .run_once_with_bridges(Guest::CompiledQjs(&bytes), bridges(Some(acu)), "main", &[])
            .expect("artifact"),
    );
    assert_eq!(direct, artifact);
}
