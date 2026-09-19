//! Milestone 1 boundary gate: the `#[no_mangle]` export set in `src/lib.rs`
//! must exactly match `exports.txt` (the single source of truth for generated
//! `.def` / version scripts / `-exported_symbols_list`). One extra or one
//! missing symbol fails this test.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn expected_exports() -> BTreeSet<String> {
    let raw = fs::read_to_string(manifest().join("exports.txt"))
        .expect("exports.txt must exist next to the crate root");
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn rust_u16_constants(source: &str) -> BTreeMap<String, String> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("pub ").unwrap_or(line.trim());
            let rest = line.strip_prefix("const ")?;
            let (name, value) = rest.split_once(": u16 = ")?;
            Some((
                name.trim().to_owned(),
                value.trim_end_matches(';').trim().to_owned(),
            ))
        })
        .collect()
}

fn resolve_u16(constants: &BTreeMap<String, String>, name: &str) -> u16 {
    let mut value = constants
        .get(name)
        .unwrap_or_else(|| panic!("missing u16 constant {name}"))
        .as_str();
    for _ in 0..constants.len() {
        if let Ok(number) = value.parse::<u16>() {
            return number;
        }
        value = constants
            .get(value)
            .unwrap_or_else(|| panic!("{name} refers to unknown u16 constant {value}"));
    }
    panic!("cycle while resolving u16 constant {name}");
}

fn abi_version(source: &str) -> (u16, u16) {
    let tail = source
        .split_once("abi_version!(")
        .expect("src/lib.rs must declare abi_version!(major, minor)")
        .1;
    let args = tail
        .split_once(')')
        .expect("abi_version! declaration must close")
        .0;
    let (major, minor) = args
        .split_once(',')
        .expect("abi_version! must contain major and minor");
    (
        major.trim().parse().expect("ABI major must be a u16"),
        minor.trim().parse().expect("ABI minor must be a u16"),
    )
}

fn header_abi_version(header: &str) -> (u16, u16) {
    let define = |name: &str| {
        header
            .lines()
            .find_map(|line| line.trim().strip_prefix(&format!("#define {name} ")))
            .unwrap_or_else(|| panic!("header is missing {name}"))
            .trim()
            .parse::<u16>()
            .unwrap_or_else(|_| panic!("header {name} must be a u16 literal"))
    };
    (define("AGT_ABI_MAJOR"), define("AGT_ABI_MINOR"))
}

fn required_runtime_symbols(cu_source: &str) -> BTreeSet<String> {
    let body = cu_source
        .split_once("const REQUIRED_RUNTIME_SYMBOLS: &[&[u8]] = &[")
        .expect("agenterm-cu must declare REQUIRED_RUNTIME_SYMBOLS")
        .1
        // Windows checkouts may use CRLF; the array terminator is the Rust
        // syntax, not a particular checkout's line ending.
        .split_once("];")
        .expect("REQUIRED_RUNTIME_SYMBOLS must be a closed array")
        .0;
    body.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("b\"")
                .and_then(|line| line.strip_suffix("\","))
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn runtime_symbol_inventory_accepts_both_checkout_line_endings() {
    let source = "const REQUIRED_RUNTIME_SYMBOLS: &[&[u8]] = &[\n    b\"agt_abi_version\",\n];\n";
    let expected = BTreeSet::from(["agt_abi_version".to_owned()]);
    assert_eq!(required_runtime_symbols(source), expected);
    assert_eq!(
        required_runtime_symbols(&source.replace('\n', "\r\n")),
        expected
    );
}

/// Extract the set of `#[unsafe(no_mangle)] pub extern "C" fn NAME` declarations
/// from `src/lib.rs`.
fn actual_no_mangle_exports() -> BTreeSet<String> {
    let src = fs::read_to_string(manifest().join("src/lib.rs"))
        .expect("src/lib.rs must exist in the crate");
    let lines: Vec<&str> = src.lines().collect();
    let mut set = BTreeSet::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line == "#[unsafe(no_mangle)]" || line == "#[no_mangle]" {
            // Consume any attribute lines before the fn keyword.
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().starts_with('#') {
                j += 1;
            }
            let decl = lines[j].trim();
            if let Some(rest) = decl.strip_prefix("pub extern \"C\" fn ")
                && let Some(name) = rest.split('(').next()
            {
                let name = name.trim();
                assert!(
                    name.starts_with("agt_"),
                    "exported symbol must be agt_-prefixed: {name}"
                );
                set.insert(name.to_owned());
            }
            i = j;
        }
        i += 1;
    }
    set
}

#[test]
fn exports_set_matches_exports_txt() {
    let expected = expected_exports();
    let actual = actual_no_mangle_exports();
    assert_eq!(
        expected, actual,
        "export set mismatch: expected={expected:?} actual={actual:?}"
    );
}

#[test]
fn exports_txt_is_not_empty() {
    let exports = expected_exports();
    assert!(
        !exports.is_empty(),
        "exports.txt must list at least one symbol"
    );
}

/// Extract the set of C function declarations from `include/agenterm.h`:
/// every occurrence of `agt_<lowercase...>(` on a non-comment line. Type
/// names (`agt_pty_t`, `agt_status`, `agt_pty_spawn`) never match because they
/// are not immediately followed by `(`.
fn declared_header_functions(header_text: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for line in header_text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("/*") || t.starts_with('*') || t.starts_with("//") {
            continue;
        }
        let bytes = t.as_bytes();
        let mut i = 0;
        while i + 4 <= bytes.len() {
            if bytes[i..].starts_with(b"agt_") {
                let mut j = i + 4;
                while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                let name = &t[i..j];
                // Only lowercase tails (excludes AGT_CAP_* constants) directly
                // followed by `(` (excludes type names like `agt_pty_t`).
                let tail_starts_lower = name.as_bytes().get(4).is_some_and(u8::is_ascii_lowercase);
                let followed_by_paren = t[j..].trim_start().starts_with('(');
                if tail_starts_lower && followed_by_paren {
                    set.insert(name.to_owned());
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    set
}

/// Boundary gate 2 (§14.5): `include/agenterm.h` must declare exactly the
/// exported symbol set — every export is declared, and no extra agt_ function
/// is declared that the library does not export.
#[test]
fn header_declares_exactly_the_exported_symbols() {
    let manifest = manifest();
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    let header = repo_root.join("include/agenterm.h");
    let text = fs::read_to_string(&header)
        .unwrap_or_else(|e| panic!("include/agenterm.h not found at {}: {e}", header.display()));
    let declared = declared_header_functions(&text);
    let expected = expected_exports();
    assert_eq!(
        expected, declared,
        "header/export set mismatch: exports.txt={expected:?} header={declared:?}"
    );
}

/// The delivered library, public header and dynamic CU consumer are separate
/// crates/artifacts, so compilation cannot keep their version and symbol
/// contracts aligned. Derive every side from its owning source instead of
/// copying a release number or a second symbol list into packaging scripts.
#[test]
fn cu_runtime_requirements_fit_the_exported_abi() {
    let manifest = manifest();
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    let abi_source = fs::read_to_string(manifest.join("src/lib.rs")).expect("read ABI source");
    let header = fs::read_to_string(repo_root.join("include/agenterm.h")).expect("read ABI header");
    let cu_source = fs::read_to_string(repo_root.join("crates/agenterm-cu/src/dynlib.rs"))
        .expect("read CU dynamic ABI consumer");

    let supplied = abi_version(&abi_source);
    assert_eq!(
        supplied,
        header_abi_version(&header),
        "Rust ABI version and public C header drifted"
    );

    let cu_constants = rust_u16_constants(&cu_source);
    let required = (
        resolve_u16(&cu_constants, "EXPECTED_ABI_MAJOR"),
        resolve_u16(&cu_constants, "REQUIRED_ABI_MINOR"),
    );
    assert!(
        supplied.0 == required.0 && supplied.1 >= required.1,
        "libagenterm ABI {}.{} is older than agenterm-cu requirement {}.{}",
        supplied.0,
        supplied.1,
        required.0,
        required.1
    );

    let missing: Vec<_> = required_runtime_symbols(&cu_source)
        .difference(&expected_exports())
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "agenterm-cu requires symbols absent from exports.txt: {missing:?}"
    );
}

/// Boundary gate: `include/agenterm.h` is a public C header compiled by
/// external consumers, so it must be pure ASCII. A non-ASCII byte (e.g. an
/// em dash, arrow, or section sign) triggers MSVC C4819 under CJK code pages
/// and breaks `/WX` builds. Fails on the first offending byte, naming its
/// line number and the line content for easy location.
#[test]
fn header_is_pure_ascii() {
    let manifest = manifest();
    let repo_root = manifest.parent().unwrap().parent().unwrap();
    let header = repo_root.join("include/agenterm.h");
    let bytes = fs::read(&header)
        .unwrap_or_else(|e| panic!("include/agenterm.h not found at {}: {e}", header.display()));
    let mut line_no = 1usize;
    let mut line_start = 0usize;
    for (idx, &b) in bytes.iter().enumerate() {
        if b < 0x80 {
            if b == b'\n' {
                line_no += 1;
                line_start = idx + 1;
            }
            continue;
        }
        let line_end = bytes[idx..]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(bytes.len(), |p| idx + p);
        let line = String::from_utf8_lossy(&bytes[line_start..line_end]);
        panic!("include/agenterm.h contains non-ASCII byte 0x{b:02x} at line {line_no}: {line}");
    }
}

/// The third boundary gate from plan §14 ("产品名闸"): an export must carry
/// the `agt_` prefix and must name an OS MECHANISM, never a product concept.
///
/// The mechanism layer's whole premise is that it knows nothing about the
/// products above it, and export names are where that leaks first: once
/// `agt_tab_activate` exists, the boundary has already moved and every
/// consumer inherits agenterm-con's vocabulary. The other two gates
/// (exports.txt as the single source of truth, header/implementation drift)
/// were built long ago; this one had not been.
///
/// Today's exports are all mechanisms -- pty, window, screenshot, process,
/// a11y, clipboard, parent_console, runtime, input, screen, native_window --
/// so this gate is green on arrival. That is the point: it pins a discipline
/// that currently holds, so the first violation is the one that turns red.
///
/// Matching is per underscore-separated SEGMENT, never substring. A substring
/// check would reject `agt_parent_console_write_stdout` for containing "con",
/// which is exactly the sort of false positive that gets a gate deleted.
#[test]
fn exports_name_mechanisms_not_products() {
    /// Product names and product-layer vocabulary. Each entry is a concept
    /// that belongs to a consumer, not to an OS mechanism.
    const PRODUCT_WORDS: &[&str] = &[
        // Product names.
        "agenterm",
        "con",
        "cu",
        // agenterm-con's own vocabulary: a terminal multiplexer's concepts,
        // not something an operating system offers.
        "tab",
        "session",
        "workspace",
        "pane",
        "split",
        // Presentation choices that belong to a product's UI.
        "theme",
        "palette",
        "layout",
        "profile",
        "prompt",
        // "terminal" is deliberately here: the mechanism the OS provides is a
        // PTY, and `agt_pty_*` already names it. An `agt_terminal_*` export
        // would mean a product concept had been pushed down.
        "terminal",
        // The window-placement catalog is a cu-level concept; the mechanism
        // is agt_native_window_move / _rect.
        "spectacle",
    ];
    const MECHANISM_EXCEPTIONS: &[&str] = &[
        // Public since ABI 1.32. Here "workspace desktop" names the EWMH
        // `_NET_WM_DESKTOP` mechanism, not an AgenTerm workspace.
        "agt_native_window_workspace_desktop",
    ];

    let mut violations: Vec<String> = Vec::new();
    for name in expected_exports() {
        if MECHANISM_EXCEPTIONS.contains(&name.as_str()) {
            continue;
        }
        let Some(rest) = name.strip_prefix("agt_") else {
            violations.push(format!("{name}: missing the agt_ prefix"));
            continue;
        };
        for segment in rest.split('_') {
            if PRODUCT_WORDS.contains(&segment) {
                violations.push(format!(
                    "{name}: segment {segment:?} is a product concept, not an OS mechanism"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "{} export name(s) carry product vocabulary across the mechanism \
         boundary:\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}
