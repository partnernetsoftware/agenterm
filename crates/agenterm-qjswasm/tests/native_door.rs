#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::AtomicBool;

use agenterm_qjswasm::native::{
    native_invocation_stub_cardinality, native_register_pattern_cardinality,
};
use agenterm_qjswasm::{
    Budget, Engine, Guest, QjswasmError, Value, door_declarations, native_door_declarations,
};

fn run_wat(source: &str, budget: Budget) -> Result<i64, QjswasmError> {
    let wasm = wat::parse_str(source).expect("native-door fixture is valid WAT");
    let outcome =
        Engine::with_native_door(budget).run_once(Guest::Wasm(&wasm), None, "main", &[])?;
    match outcome.values.as_slice() {
        [Value::I64(bits)] => Ok(*bits),
        other => panic!("native fixture returned {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn the_same_native_guest_is_refused_by_default_and_runs_only_when_opted_in() {
    let wasm = wat::parse_str(include_str!("fixtures/native/getpid.wat"))
        .expect("native-door fixture is valid WAT");
    let error = Engine::new()
        .run_once(Guest::Wasm(&wasm), None, "main", &[])
        .expect_err("the default engine does not install the native door");
    assert!(
        matches!(&error, QjswasmError::Door(message)
            if message.contains("agenterm.native_call")
                && message.contains("with_native_door")),
        "expected the closed-door diagnostic, got {error:?}"
    );

    assert_eq!(
        Engine::with_native_door(Budget::default())
            .run_once(Guest::Wasm(&wasm), None, "main", &[])
            .expect("the explicitly opened native door runs")
            .values,
        [Value::I64(std::process::id() as i64)]
    );
}

#[test]
fn native_opt_in_composes_with_the_tool_door_without_losing_arguments() {
    let mut engine = Engine::with_tool_door(Budget::default()).enable_native_door();
    assert!(engine.has_tool_door());
    assert!(engine.has_native_door());
    engine.set_tool_args(vec!["preserved".to_owned()]);
    let outcome = engine
        .run_once(
            Guest::Qjs(
                r#"
                if (arg_count() !== 1) { return "wrong count"; }
                if (arg(0) !== 0) { return "arg failed"; }
                return tool_result();
                "#,
            ),
            None,
            "main",
            &[],
        )
        .expect("the combined tool/native engine runs a tool guest");
    assert!(matches!(
        outcome.values.as_slice(),
        [Value::Js(agenterm_qjswasm::JsValue::Str(value))] if value == "preserved"
    ));
}

#[test]
fn default_discovery_does_not_advertise_the_native_opt_in() {
    assert!(
        door_declarations()
            .iter()
            .all(|declaration| !declaration.field.starts_with("native_"))
    );
    let native = native_door_declarations();
    assert_eq!(native.len(), 3);
    assert!(
        native
            .iter()
            .all(|declaration| declaration.module == "agenterm")
    );
    assert_eq!(
        native
            .iter()
            .map(|declaration| declaration.field.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["native_call", "native_invoke", "native_result"])
    );
}

/// The door's counts are **sets at different layers**, not one number read three
/// ways: the host signature inventory (raw, `host.rs`, eleven entries), the
/// compiler's default declarations (everything except the native family), and what
/// the opt-in adds. This pins the relation rather than a bare number, so a
/// different raw inventory cannot be mistaken for drift in the compiler-visible
/// set.
#[test]
fn the_native_opt_in_adds_exactly_three_declarations_to_the_default_door() {
    use std::collections::BTreeSet;
    let default: BTreeSet<(String, String)> = door_declarations()
        .into_iter()
        .map(|declaration| {
            (
                declaration.module.to_string(),
                declaration.field.to_string(),
            )
        })
        .collect();
    let native: BTreeSet<(String, String)> = native_door_declarations()
        .into_iter()
        .map(|declaration| {
            (
                declaration.module.to_string(),
                declaration.field.to_string(),
            )
        })
        .collect();
    assert_eq!(
        default.len(),
        5,
        "the default door declaration set is five entries"
    );
    assert_eq!(
        native.len(),
        3,
        "the native opt-in is exactly three entries"
    );
    assert!(
        default.is_disjoint(&native),
        "native declarations must not already be in the default set"
    );
    let mut opt_in = default.clone();
    opt_in.extend(native.iter().cloned());
    assert_eq!(
        opt_in.len(),
        8,
        "the opt-in set is the default five plus three"
    );
    let added: BTreeSet<(String, String)> = opt_in.difference(&default).cloned().collect();
    assert_eq!(
        added,
        BTreeSet::from([
            ("agenterm".to_string(), "native_call".to_string()),
            ("agenterm".to_string(), "native_invoke".to_string()),
            ("agenterm".to_string(), "native_result".to_string()),
        ]),
        "the opt-in's exact difference is the raw ABI plus its QJS adapter"
    );
}

fn wat_for(spec: &str, spec_ptr: i32, spec_len: i32, block_ptr: i32, block_len: i32) -> String {
    let quoted = spec.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"(module
          (import "agenterm" "native_call"
            (func $native_call (param i32 i32 i32 i32) (result i32)))
          (memory 1)
          (data (i32.const 0) "{quoted}")
          (func (export "main") (result i64)
            (i32.store (i32.const 128) (i32.const 1))
            (i32.store (i32.const 132) (i32.const 0))
            (i64.store (i32.const 136) (i64.const 0))
            (drop (call $native_call
              (i32.const {spec_ptr}) (i32.const {spec_len})
              (i32.const {block_ptr}) (i32.const {block_len})))
            (i64.load (i32.const 136))))"#
    )
}

fn wat_for_scalar_args(spec: &str, arguments: &[u64]) -> String {
    let quoted = spec.replace('\\', "\\\\").replace('"', "\\\"");
    let stores = arguments
        .iter()
        .enumerate()
        .map(|(index, bits)| {
            let record = 144 + index * 16;
            format!(
                "(i32.store (i32.const {record}) (i32.const 0))\n\
                 (i32.store (i32.const {}) (i32.const 0))\n\
                 (i64.store (i32.const {}) (i64.const {bits}))",
                record + 4,
                record + 8,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let block_len = 16 + arguments.len() * 16;
    format!(
        r#"(module
          (import "agenterm" "native_call"
            (func $native_call (param i32 i32 i32 i32) (result i32)))
          (memory 1)
          (data (i32.const 0) "{quoted}")
          (func (export "main") (result i64)
            (i32.store (i32.const 128) (i32.const 1))
            (i32.store (i32.const 132) (i32.const {arity}))
            (i64.store (i32.const 136) (i64.const 0))
            {stores}
            (drop (call $native_call
              (i32.const 0) (i32.const {spec_len})
              (i32.const 128) (i32.const {block_len})))
            (i64.load (i32.const 136))))"#,
        arity = arguments.len(),
        spec_len = spec.len(),
    )
}

#[cfg(unix)]
fn host_page_size() -> i64 {
    let output = std::process::Command::new("getconf")
        .arg("PAGESIZE")
        .output()
        .expect("the POSIX getconf oracle runs");
    assert!(output.status.success(), "getconf PAGESIZE must succeed");
    String::from_utf8(output.stdout)
        .expect("getconf emits UTF-8 digits")
        .trim()
        .parse()
        .expect("getconf emits an integer page size")
}

#[cfg(target_os = "macos")]
const HOST_SC_PAGESIZE: i32 = 29;
#[cfg(target_os = "linux")]
const HOST_SC_PAGESIZE: i32 = 30;

/// The parent pid as the host reports it: `std`'s own process fact, not the
/// guest's answer and not the `native_call` path under test.
#[cfg(unix)]
fn host_parent_pid() -> i64 {
    i64::from(std::os::unix::process::parent_id())
}

/// Ask the host's standard identity utility for the real uid. This does not
/// share the guest's `native_call` path and, unlike file ownership, does not
/// silently substitute the effective uid.
#[cfg(unix)]
fn host_real_uid() -> u32 {
    let output = std::process::Command::new("id")
        .arg("-ru")
        .output()
        .expect("the Unix identity oracle runs");
    assert!(output.status.success(), "id -ru must succeed");
    String::from_utf8(output.stdout)
        .expect("id -ru emits UTF-8 digits")
        .trim()
        .parse()
        .expect("id -ru emits a u32")
}

#[cfg(unix)]
#[test]
fn three_real_read_only_native_capabilities_cross_the_eighth_door() {
    let pid = run_wat(
        include_str!("fixtures/native/getpid.wat"),
        Budget::default(),
    )
    .expect("getpid runs") as u32;
    assert_eq!(pid, std::process::id());

    let parent = run_wat(
        include_str!("fixtures/native/getppid.wat"),
        Budget::default(),
    )
    .expect("getppid runs");
    assert_eq!(
        parent,
        host_parent_pid(),
        "getppid must report this process's parent"
    );

    let uid = run_wat(
        include_str!("fixtures/native/getuid.wat"),
        Budget::default(),
    )
    .expect("getuid runs");
    assert_eq!(
        uid as u32,
        host_real_uid(),
        "getuid must match the host real-uid oracle"
    );
}

#[cfg(unix)]
#[test]
fn a_fourth_capability_is_only_an_additional_wat_guest() {
    let directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/native/additions");
    let mut additions = std::fs::read_dir(directory)
        .expect("native addition fixtures exist")
        .collect::<Result<Vec<_>, _>>()
        .expect("native addition fixtures are readable");
    additions.sort_by_key(std::fs::DirEntry::file_name);
    assert!(!additions.is_empty(), "the fourth capability is present");
    for addition in additions {
        let source = std::fs::read_to_string(addition.path()).expect("addition is UTF-8 WAT");
        assert_eq!(
            run_wat(&source, Budget::default()).expect("addition runs"),
            1,
            "each added WAT returns its own boolean proof"
        );
    }
}

/// The effective group id as an independent POSIX process reports it: `id -g`
/// is a second program, so this compares two facts rather than one path with
/// itself.
#[cfg(unix)]
fn host_effective_gid_from_id() -> u32 {
    let output = std::process::Command::new("id")
        .arg("-g")
        .output()
        .expect("the POSIX id command runs");
    assert!(output.status.success(), "id -g must succeed");
    String::from_utf8(output.stdout)
        .expect("id -g prints utf-8")
        .trim()
        .parse()
        .expect("id -g prints a numeric gid")
}

/// A fifth read-only capability costs one more guest and no production code:
/// the door's own raw import table stays at eleven, so the addition is a new
/// comparable marginal point rather than a new mechanism.
#[cfg(unix)]
#[test]
fn a_fifth_read_only_capability_is_another_wat_guest_with_no_production_change() {
    // Five ordinary declarations plus three native declarations remain
    // unchanged. The latter are the language-neutral raw call and the QJS
    // request/result adapter; two-pass results add private length imports.
    assert_eq!(
        door_declarations().len() + native_door_declarations().len(),
        8,
        "adding a capability must not change the door's import table"
    );
    let egid = run_wat(
        include_str!("fixtures/native/getegid.wat"),
        Budget::default(),
    )
    .expect("getegid runs");
    assert_eq!(
        egid as u32,
        host_effective_gid_from_id(),
        "getegid must report the group id an independent process sees"
    );
}

#[cfg(unix)]
#[test]
fn exact_nonzero_arity_gp_and_f64_signatures_use_their_own_abi_families() {
    assert_eq!(
        run_wat(
            include_str!("fixtures/native/abs_minus_seven.wat"),
            Budget::default(),
        )
        .expect("abs(-7) runs through an exact i32 stub"),
        7
    );
    let source = include_str!("fixtures/native/cos_zero.wat").to_owned();
    #[cfg(target_os = "linux")]
    let source = source
        .replace("|cos|f64(f64)", "libm.so.6|cos|f64(f64)")
        .replace(
            "(i32.const 13) (i32.const 128)",
            "(i32.const 22) (i32.const 128)",
        );
    let bits = run_wat(source.as_str(), Budget::default())
        .expect("cos(0) runs through an exact f64 stub") as u64;
    assert_eq!(f64::from_bits(bits), 1.0);
}

#[cfg(unix)]
#[test]
fn a_mixed_sysconf_signature_reaches_the_dyn_fixed_core() {
    let source = wat_for_scalar_args("|sysconf|isize(i32)", &[HOST_SC_PAGESIZE as i64 as u64]);
    let page_size = run_wat(&source, Budget::default())
        .expect("sysconf(_SC_PAGESIZE) runs through the fixed mixed prototype");
    assert_eq!(page_size, host_page_size());
}

#[test]
fn exact_stubs_are_not_confused_with_register_class_patterns() {
    assert_eq!(native_invocation_stub_cardinality(), 7 * 7);
    assert_eq!(native_register_pattern_cardinality(), 381);
    assert!(native_invocation_stub_cardinality() < native_register_pattern_cardinality());
}

#[test]
fn missing_library_symbol_unsupported_signature_and_oob_are_distinct() {
    let cases = [
        (
            wat_for(
                "agenterm-native-library-that-does-not-exist|f|i32()",
                0,
                51,
                128,
                16,
            ),
            "native_library_load_failed",
        ),
        (
            wat_for(
                "|agenterm_native_symbol_that_does_not_exist|i32()",
                0,
                49,
                128,
                16,
            ),
            "native_symbol_load_failed",
        ),
        (
            wat_for("|getpid|i16()", 0, 13, 128, 16),
            "native_invocation_signature_unsupported",
        ),
        (
            wat_for("|getpid|i32()", 65_530, 13, 128, 16),
            "native_span_out_of_bounds",
        ),
    ];
    for (source, code) in cases {
        let error = run_wat(&source, Budget::default()).expect_err(code);
        assert!(
            matches!(&error, QjswasmError::Door(message) if message.contains(code)),
            "expected Door({code}), got {error:?}"
        );
    }
}

/// This is the WAT-side replacement for the old dyn Lisp court that required
/// complete ABI validation before argument evaluation or dynamic loading. WAT
/// has no Lisp argument expressions to mutate, so the observable invariant is
/// narrower and stronger: an invalid scalar or unsupported complete signature
/// wins over the deliberately missing library named by the same request.
#[test]
fn wat_native_calls_validate_arguments_and_the_complete_signature_before_loading() {
    let missing_library = "agenterm-native-library-that-does-not-exist";
    let cases = [
        (
            wat_for_scalar_args(&format!("{missing_library}|unused|u128()"), &[]),
            "native_type_unknown",
        ),
        (
            wat_for_scalar_args(&format!("{missing_library}|unused|f32()"), &[]),
            "native_type_unsupported",
        ),
        (
            wat_for_scalar_args(&format!("{missing_library}|unused|i32(u32)"), &[0]),
            "native_invocation_signature_unsupported",
        ),
        (
            wat_for_scalar_args(
                &format!("{missing_library}|unused|i8(i8)"),
                &[i8::MAX as u64],
            ),
            "native_invocation_signature_unsupported",
        ),
        (
            wat_for_scalar_args(
                &format!("{missing_library}|unused|i32(i32)"),
                &[u64::from(u32::MAX) + 1],
            ),
            "native_scalar_not_canonical",
        ),
        (
            wat_for_scalar_args(&format!("{missing_library}|unused|isize(i64)"), &[0]),
            "native_invocation_signature_unsupported",
        ),
    ];

    for (source, expected_code) in cases {
        let error = run_wat(&source, Budget::default()).expect_err(expected_code);
        assert!(
            matches!(&error, QjswasmError::Door(message)
                if message.contains(expected_code)
                    && !message.contains("native_library_load_failed")),
            "expected pre-load Door({expected_code}), got {error:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn native_calls_share_the_host_operation_budget_and_cancel_source() {
    let source = include_str!("fixtures/native/getpid.wat");
    let error = run_wat(
        source,
        Budget {
            max_host_ops: 0,
            ..Budget::default()
        },
    )
    .expect_err("zero host operations refuses the native call");
    assert!(matches!(error, QjswasmError::Budget("max_host_ops")));

    let cancel = Arc::new(AtomicBool::new(true));
    let error = run_wat(
        source,
        Budget {
            cancel: Some(cancel),
            ..Budget::default()
        },
    )
    .expect_err("the shared cancellation flag refuses before dlsym");
    assert!(matches!(error, QjswasmError::Cancelled));
}
