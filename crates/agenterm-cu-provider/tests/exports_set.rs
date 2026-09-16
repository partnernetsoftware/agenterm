//! The one fixed-sibling artifact must carry both embedded-call and process-main
//! ABI families. A missing half makes either the host or thin launcher fail at
//! symbol resolution before product code runs.

use std::collections::BTreeSet;

const LIB_SOURCE: &str = include_str!("../src/lib.rs");
const PROCESS_MAIN_SOURCE: &str = include_str!("../src/process_main.rs");
const HEADER: &str = include_str!("../include/agenterm-cu-provider.h");

fn expected() -> BTreeSet<&'static str> {
    [
        "agenterm_cu_provider_abi_version",
        "agenterm_cu_provider_call",
        "agenterm_cu_provider_call_v2",
        "agenterm_cu_process_main_abi_version",
        "agenterm_cu_process_main_v1",
    ]
    .into_iter()
    .collect()
}

fn no_mangle_exports(source: &'static str) -> BTreeSet<&'static str> {
    let lines: Vec<&str> = source.lines().collect();
    let mut exports = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#[unsafe(no_mangle)]" {
            continue;
        }
        let declaration = lines
            .get(index + 1)
            .expect("no_mangle must be followed by a function")
            .trim();
        let declaration = declaration
            .strip_prefix("pub unsafe extern \"C\" fn ")
            .or_else(|| declaration.strip_prefix("pub extern \"C\" fn "))
            .expect("provider exports must use the public C ABI");
        exports.insert(
            declaration
                .split('(')
                .next()
                .expect("export declaration has a name"),
        );
    }
    exports
}

#[test]
fn fixed_sibling_exports_both_abi_families() {
    let actual = no_mangle_exports(LIB_SOURCE)
        .into_iter()
        .chain(no_mangle_exports(PROCESS_MAIN_SOURCE))
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected());
}

#[test]
fn public_header_declares_every_export() {
    for symbol in expected() {
        assert!(
            HEADER.contains(&format!("{symbol}(")),
            "public header is missing {symbol}"
        );
    }
    assert!(HEADER.contains("AGENTERM_CU_PROVIDER_ABI_VERSION UINT32_C(1)"));
    assert!(HEADER.contains("AGENTERM_CU_PROCESS_MAIN_ABI_VERSION UINT32_C(1)"));
}
