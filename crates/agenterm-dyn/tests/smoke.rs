//! Platform native smoke tests — real `dlcall` into the host OS libraries.
//!
//! Each supported OS module uses [`agenterm_dyn::live_cell`] script data and
//! cross-checks results with a second `dlcall` where possible.

use agenterm_dyn::{CU_ADJACENT_PROBE_CATALOG, HostArch, HostOs, live_cell};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use agenterm_dyn::{Dyn, DynError, Value};

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn eval_native(env: &mut Dyn, source: &str) -> Result<Value, DynError> {
    // SAFETY: each smoke owns any bound storage and asserts the documented ABI.
    unsafe { env.eval_native(source) }
}

#[test]
fn cu_adjacent_catalog_has_six_cells() {
    assert_eq!(CU_ADJACENT_PROBE_CATALOG.len(), 6);
    assert!(
        CU_ADJACENT_PROBE_CATALOG
            .iter()
            .any(|cell| cell.os == HostOs::Linux && cell.arch == HostArch::X86_64)
    );
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use agenterm_dyn::{HostCell, LINUX_ATSPI_EXISTENCE_LIBS, SystemProbeStatus};

    fn cell() -> &'static HostCell {
        live_cell().expect("linux cell")
    }

    #[test]
    fn variadic_system_probes_are_catalogued_but_not_invoked() {
        for name in ["open_dev_null", "fcntl_stdin_getfd", "fcntl_stdin_getfl"] {
            let probe = cell()
                .system_probes
                .into_iter()
                .find(|probe| probe.name == name)
                .unwrap_or_else(|| panic!("missing system probe {name}"));
            assert!(matches!(probe.status, SystemProbeStatus::Placeholder));
        }
    }

    #[test]
    fn dlcall_x11_x_open_display_probe() {
        let row = CU_ADJACENT_PROBE_CATALOG
            .iter()
            .find(|c| c.os == HostOs::Linux && c.arch == HostArch::X86_64)
            .expect("linux x86_64 catalog row");
        let lib = row.window_list.lib;
        let sym = row.window_list.symbol;

        let mut env = Dyn::new();
        let script = format!(r#"(dlcall "{lib}" "{sym}" "ptr" "ptr" 0)"#);
        match eval_native(&mut env, &script) {
            Ok(Value::Ptr(display)) if display != 0 => {
                let close = eval_native(
                    &mut env,
                    &format!(r#"(dlcall "{lib}" "XCloseDisplay" "i32" "ptr" {display})"#),
                )
                .expect("XCloseDisplay dlcall");
                assert_eq!(close, Value::Int(0), "XCloseDisplay should succeed");
            }
            Ok(Value::Ptr(0)) | Ok(Value::Nil) => {}
            Err(DynError::Library(msg)) => {
                assert!(
                    msg.contains(lib),
                    "library load should name {lib}, got {msg}"
                );
            }
            other => panic!("unexpected XOpenDisplay probe outcome: {other:?}"),
        }
    }

    #[test]
    fn atspi_library_existence_probe() {
        let mut attempted = false;
        for name in LINUX_ATSPI_EXISTENCE_LIBS {
            attempted = true;
            // SAFETY: existence probe only; we never invoke resolved symbols.
            let _ = unsafe { libloading::Library::new(name) };
        }
        assert!(attempted, "should try at least one AT-SPI library name");
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use std::ffi::{CString, c_void};

    #[test]
    fn dlcall_getenv_display_probe() {
        let mut env = Dyn::new();
        let key = CString::new("DISPLAY").expect("DISPLAY key");
        env.bind("env_key", key.as_ptr().cast::<c_void>() as *mut c_void)
            .expect("bind env_key");
        match eval_native(
            &mut env,
            r#"(dlcall "ucrtbase.dll" "getenv" "ptr" "ptr" env_key)"#,
        ) {
            Ok(_) => {}
            Err(DynError::Library(_)) => {
                eval_native(
                    &mut env,
                    r#"(dlcall "msvcrt.dll" "getenv" "ptr" "ptr" env_key)"#,
                )
                .expect("getenv via msvcrt when ucrtbase is absent");
            }
            Err(other) => panic!("unexpected getenv probe error: {other:?}"),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported {
    use super::*;

    #[test]
    fn host_table_compiles_without_live_native_smoke() {
        assert!(live_cell().is_none());
        let _ = Dyn::new();
    }
}

#[test]
fn live_cell_present_on_supported_hosts() {
    if cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    )) {
        assert!(
            live_cell().is_some(),
            "supported OS should resolve a live host cell"
        );
    }
}

#[test]
fn non_live_cells_are_distinct_from_live() {
    use agenterm_dyn::ALL_CELLS;
    if let Some(live) = live_cell() {
        let others: Vec<_> = ALL_CELLS
            .iter()
            .filter(|c| c.os != live.os || c.arch != live.arch)
            .collect();
        assert_eq!(others.len(), 5, "five non-live placeholder rows");
    }
}
