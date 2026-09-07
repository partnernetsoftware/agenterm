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
    let acu: AcuBridgeFn = Arc::new(|command| {
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
fn bad_guest_bytes_are_status_1_and_oob_traps_before_dispatch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_for_bridge = Arc::clone(&calls);
    let acu: AcuBridgeFn = Arc::new(move |_| {
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
    let acu: AcuBridgeFn = Arc::new(|_| Ok("1234".to_owned()));
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
            bridges(Some(Arc::new(|_| Ok("{}".to_owned())))),
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
            bridges(Some(Arc::new(|_| Ok("{}".to_owned())))),
            "main",
            &[],
        )
        .expect_err("cancel");
    assert!(matches!(error, QjswasmError::Cancelled));

    let error = run(
        r#"return acu_call("{}");"#,
        Some(Arc::new(|_| -> Result<String, String> { panic!("boom") })),
    )
    .expect_err("panic is contained");
    assert!(
        matches!(error, QjswasmError::Door(message) if message.contains("ACU bridge panicked") && message.contains("boom"))
    );
}

#[test]
fn slots_keep_bridges_and_results_isolated() {
    let bytes = compile_qjs(r#"acu_call("{}"); return acu_result();"#).expect("compile");
    let mut engine = Engine::new();
    let first = engine
        .spawn_with_bridges(
            Guest::CompiledQjs(&bytes),
            bridges(Some(Arc::new(|_| Ok("first".to_owned())))),
        )
        .expect("first slot");
    let second = engine
        .spawn_with_bridges(
            Guest::CompiledQjs(&bytes),
            bridges(Some(Arc::new(|_| Ok("second".to_owned())))),
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
    let acu: AcuBridgeFn = Arc::new(|_| Ok("same".to_owned()));
    let direct = returned_string(run(source, Some(Arc::clone(&acu))).expect("source"));
    let artifact = returned_string(
        Engine::new()
            .run_once_with_bridges(Guest::CompiledQjs(&bytes), bridges(Some(acu)), "main", &[])
            .expect("artifact"),
    );
    assert_eq!(direct, artifact);
}
