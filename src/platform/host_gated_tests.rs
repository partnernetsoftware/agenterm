//! Host-specific product regression fixtures belong under the platform boundary.

#[cfg(unix)]
#[test]
fn instance_registry_refuses_a_preplanted_directory_symlink() {
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    let root = std::env::current_dir()
        .expect("repository root")
        .join("target")
        .join("instances-tests")
        .join(format!(
            "agenterm-instance-symlink-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
    let planted = root.join("planted");
    let registry = root.join("instances");
    std::fs::create_dir_all(&planted).unwrap();
    symlink(&planted, &registry).unwrap();

    assert!(crate::instances::discover_instances_in(&registry).is_err());
    assert!(
        crate::instances::mark_intentional_shutdown_in(&registry, "fixture", None, None, 42)
            .is_err()
    );
    assert!(std::fs::read_dir(&planted).unwrap().next().is_none());

    std::fs::remove_file(registry).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(feature = "script-qjswasm", unix))]
#[test]
fn qjs_native_non_finite_float_is_refused_instead_of_becoming_null() {
    use crate::script_engine::{
        QjswasmEngineBackend, ScriptEngineBackend, ScriptInvocationOptions,
    };

    #[cfg(target_os = "linux")]
    let library = "libm.so.6";
    #[cfg(target_os = "macos")]
    let library = "";
    let source = format!(
        r#"
import * as native from "agenterm:native";
return native.call("{library}|sqrt|f64(f64)", [-1]);
"#
    );
    let options = ScriptInvocationOptions {
        native_door_contained: true,
        ..ScriptInvocationOptions::default()
    };
    let error = QjswasmEngineBackend
        .execute(&source, &options, None)
        .expect_err("NaN has no JSON value and must not become null");
    assert!(
        error.message.contains("native_result_not_finite"),
        "the refusal keeps its stable code: {error:?}"
    );
}
