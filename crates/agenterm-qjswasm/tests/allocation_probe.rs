//! The allocation probe is an opt-in diagnostic on a persistent qjs slot.
//! Production compilation and hand-written Wasm expose no such state.

use agenterm_qjswasm::{
    Engine, Guest, Value, compile_qjs_with_modules, compile_qjs_with_modules_and_allocation_probe,
};

fn no_modules(_: &str) -> Option<String> {
    None
}

#[test]
fn ordinary_qjs_has_no_allocation_waterline_surface() {
    let bytes = compile_qjs_with_modules("return 1;", &no_modules).expect("compiles");
    let mut engine = Engine::new();
    let slot = engine
        .spawn(Guest::CompiledQjs(&bytes), None)
        .expect("loads");
    assert!(matches!(engine.allocation_waterline(slot), Ok(None)));
}

#[test]
fn diagnostic_slot_reports_repeatable_dead_json_growth() {
    let bytes = compile_qjs_with_modules_and_allocation_probe(
        "return JSON.parse(\"[{\\\"name\\\":\\\"alpha\\\"},{\\\"name\\\":\\\"beta\\\"}]\").length;",
        &no_modules,
    )
    .expect("compiles");
    let mut engine = Engine::new();
    let slot = engine
        .spawn(Guest::CompiledQjs(&bytes), None)
        .expect("loads");
    let mut waterlines = vec![
        engine
            .allocation_waterline(slot)
            .expect("probe runs")
            .expect("diagnostic module"),
    ];
    for _ in 0..3 {
        let outcome = engine.call(slot, "main", &[]).expect("main runs");
        assert_eq!(
            outcome.values,
            vec![Value::Js(agenterm_qjswasm::JsValue::Number(2.0))]
        );
        assert_eq!(outcome.heap_start_bytes, Some(waterlines[0]));
        assert!(outcome.heap_bytes.is_some_and(|end| end > waterlines[0]));
        assert!(outcome.json_parse_bytes.is_some_and(|bytes| bytes > 0));
        assert_eq!(outcome.json_stringify_bytes, Some(0));
        waterlines.push(
            engine
                .allocation_waterline(slot)
                .expect("probe runs")
                .expect("diagnostic module"),
        );
    }
    let deltas: Vec<usize> = waterlines
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect();
    assert_eq!(deltas[1], deltas[2], "warm calls have one stable slope");
    assert!(deltas[1] > 0, "the dead suffix is observable");
}
