//! Parity checks for agenterm's script-engine "L2 facade" — the fleet.*
//! scripting API surface that every engine exposes over
//! `__host.fleet_call(operation_id, params_json)`.
//!
//! Nothing else in the tree asserts these surfaces stay aligned, so drift
//! between engines would otherwise be silent. This file parses the actual
//! source artifacts (plain text/regex, no engine execution) and compares
//! the extracted (function/path -> operation_id) catalogs:
//!
//! 1. lua vs qjs: `scripts/lua/lib/fleet.lua` vs `scripts/qjs/lib/fleet.js`
//! 2. every facade vs host: `src/operations.rs` `OPERATION_CATALOG` (the
//!    authoritative, dispatchable operation-id list)
//!
//! A second comparison -- rh's compiled-in `SHIPPED_SURFACE_PATHS` (76
//! `fleet.*` entries, a strict superset of the 29 here) against lua/qjs and
//! against the host catalog -- lived here until the rh engine left the
//! repository on 2026-08-29 (partnernetsoftware/rh). Its findings, including
//! the 2026-08-25 correction below, stay as history.
//!
//! Investigation findings baked into this file:
//!
//! - lua and qjs are IN SYNC today: identical 29-entry (function_name ->
//!   operation_id) maps. Test 1 locks full equality.
//! - rh's `SHIPPED_SURFACE_PATHS` declared 76 `fleet.*` surface paths — a
//!   strict superset of lua/qjs's 29 — and every one of them was in the host
//!   catalog: the two catalogs agreed exactly (historical, see above).
//! - `src/operations.rs::OPERATION_CATALOG` is the authoritative,
//!   *dispatchable* operation-id list (`operation_by_id` looks entries up
//!   there). It has 77 entries, 76 of them `fleet.*`. Every lua/qjs
//!   operation_id is in it.
//!
//! # Correction, 2026-08-25: the "33 undispatchable rh surfaces" were not real
//!
//! This file used to report that 32 (the allowlist held 33) of rh's declared
//! `fleet.*` surfaces had no entry in `OPERATION_CATALOG`, and named the
//! families: `ui.settings.*`, `ui.modal.*`, `ui.font.*`,
//! `ui.instance-picker.*`, `ui.window.*`, `ui.tab.new`, and so on. The figure
//! reached PRD 02.10 as an open product finding.
//!
//! It was an artefact of how this file read the catalog. The extractor scanned
//! `src/operations.rs` for lines starting `id:`, and those 33 entries are
//! built by the `nullary_ui_action()` const constructor on one line with no
//! `id:` on it. The extractor saw 44 of 77; the `>= 40` sanity floor passed;
//! nothing went red. A test that reads a source file with a regex can be wrong
//! in exactly this direction — quietly, and in the *reporting*, not the
//! assertion.
//!
//! `host_operation_catalog_ids()` now links `OPERATION_CATALOG` instead of
//! parsing it, the allowlist is empty, and the assertion that used to pin the
//! false gap now pins its absence. `tests/fleet_catalog_conformance.rs`
//! (2026-08-25) links the catalog the same way and covers the direction this
//! file does not: whether each binding's *parameters* match the spec.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path: PathBuf = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Extracts `(function_name, operation_id)` pairs from a lua/qjs fleet
/// wrapper file. Both files share the same shape: each exported leaf
/// function's body contains exactly one `call("<operation_id>"` (or
/// `call("<operation_id>", ...`) invocation, and is declared as either
/// `function fleet.a.b.c(...)` (lua) or `fleet.a.b.c = function (...)`
/// (qjs). We key on the *fully-qualified* dotted path (`a.b.c`) rather than
/// just the leaf name so that e.g. `tabs.list` and (hypothetically)
/// `events.list` can't collide.
fn extract_script_facade(source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pending_path: Option<String> = None;
    for raw_line in source.lines() {
        let line = raw_line.trim();

        if let Some(rest) = line.strip_prefix("function fleet.") {
            // lua: `function fleet.tabs.set_note(tab_id, note)`
            if let Some(paren) = rest.find('(') {
                pending_path = Some(rest[..paren].to_owned());
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("fleet.") {
            // qjs: `fleet.tabs.set_note = function (tabId, note) {`
            if let Some(eq) = rest.find(" = function") {
                pending_path = Some(rest[..eq].to_owned());
                continue;
            }
        }

        if let Some(call_start) = line.find("call(\"") {
            let after = &line[call_start + "call(\"".len()..];
            if let Some(end) = after.find('"') {
                let operation_id = after[..end].to_owned();
                if let Some(path) = pending_path.take() {
                    out.push((path, operation_id));
                }
            }
        }
    }
    out
}

fn lua_facade() -> Vec<(String, String)> {
    extract_script_facade(&read("scripts/lua/lib/fleet.lua"))
}

fn qjs_facade() -> Vec<(String, String)> {
    extract_script_facade(&read("scripts/qjs/lib/fleet.js"))
}

/// The `.qjs` binding: the same catalog, bound for the `agenterm-qjswasm`
/// engine. Same file shape as `fleet.js`, so the same extractor reads it.
fn qjswasm_facade() -> Vec<(String, String)> {
    extract_script_facade(&read("scripts/qjs/lib/fleet.qjs"))
}

/// The authoritative, host-dispatchable operation-id set, read from the
/// linked `OPERATION_CATALOG` itself.
///
/// # This used to parse the Rust source, and that made the file lie
///
/// The previous version scanned `src/operations.rs` for lines beginning
/// `id:`. That shape reaches only entries written out longhand. Thirty-three
/// of the catalog's entries are built by the `nullary_ui_action()` const
/// constructor instead -- `nullary_ui_action(UI_TAB_NEW, "fleet.ui.tab.new",
/// "new-tab")`, one line, no `id:` on it -- so the extractor could not see
/// them, and the `ids.len() >= 40` floor below passed on the 44 it did see.
///
/// Nothing failed. What happened instead is worse: this file *reported* those
/// thirty-three operations as absent from the host catalog, in
/// `rh_surfaces_missing_from_host_catalog()`, described as surfaces "rh claims
/// support for [that] the host's authoritative catalog does not (yet)
/// implement/dispatch". They are all present and all dispatchable. The figure
/// was copied from here into PRD prose, which is how a text-parsing test turns
/// into a documented fact about the product.
///
/// Linking the catalog cannot drift that way: a new construction shape is
/// counted the moment it compiles, and a renamed constant is a compile error
/// rather than a silently smaller set.
fn host_operation_catalog_ids() -> BTreeSet<String> {
    let ids: BTreeSet<String> = agenterm::operations::OPERATION_CATALOG
        .iter()
        .map(|spec| spec.id.to_owned())
        .collect();
    assert_eq!(
        ids.len(),
        agenterm::operations::OPERATION_CATALOG.len(),
        "two catalog entries share an operation id"
    );
    ids
}

// ── Test 1: lua vs qjs — the strong parity check ───────────────────────
//
// Investigation result: the two facades are IN SYNC today (29/29 identical
// function-path -> operation_id pairs). This locks full equality; any
// future edit to either file that adds, removes, or renames an operation
// without mirroring it in the other engine will fail this test.

#[test]
fn lua_and_qjs_fleet_facades_expose_identical_operation_catalogs() {
    let lua = lua_facade();
    let qjs = qjs_facade();

    assert!(
        !lua.is_empty(),
        "lua facade extraction found zero entries — parser likely broken"
    );
    assert!(
        !qjs.is_empty(),
        "qjs facade extraction found zero entries — parser likely broken"
    );

    let lua_map: std::collections::BTreeMap<_, _> = lua.into_iter().collect();
    let qjs_map: std::collections::BTreeMap<_, _> = qjs.into_iter().collect();

    assert_eq!(
        lua_map, qjs_map,
        "lua and qjs fleet facades have drifted: expected identical \
         (function_path -> operation_id) maps. Diff the two sides above."
    );

    // Sanity-anchor the count so a silent parser regression that still
    // happens to produce equal (but empty, or partial) maps is caught too.
    assert_eq!(
        lua_map.len(),
        29,
        "expected the known-good count of 29 fleet operations in lua/qjs; \
         if this changed intentionally, update this anchor alongside the \
         module doc comment counts"
    );
}

// ── Test 1b: qjs vs qjswasm — the archive gate, as a test ──────────────
//
// `scripts/qjs/lib/fleet.qjs` binds the same catalog for agenterm's own
// engine, and "equivalent to `fleet.js`" is the stated gate for retiring
// `agenterm-qjs` (PRD 02.36). A gate checked by reading two files once is
// not a gate; this is the same equality Test 1 makes between lua and qjs,
// so all three bindings are now held to each other rather than two of them
// being held and the third being described.
//
// It was a 8-of-29 partial port until 2026-08-25, for a reason that was true
// at the time and is not any more: the engine could not build an object
// literal or stringify one, so nineteen operations had no way to express
// their params. Both landed upstream at tinyvm `f21f0f2`.

#[test]
fn qjs_and_qjswasm_fleet_facades_are_the_same_binding() {
    let qjs: std::collections::BTreeMap<_, _> = qjs_facade().into_iter().collect();
    let qjswasm: std::collections::BTreeMap<_, _> = qjswasm_facade().into_iter().collect();

    assert!(
        !qjswasm.is_empty(),
        "qjswasm facade extraction found zero entries — parser likely broken"
    );
    assert_eq!(
        qjs, qjswasm,
        "scripts/qjs/lib/fleet.js and scripts/qjs/lib/fleet.qjs have drifted: the two \
         engines would produce different Fleet operations for the same script."
    );
}

#[test]
fn lua_qjs_operation_ids_are_a_subset_of_the_host_operation_catalog() {
    let catalog = host_operation_catalog_ids();
    let lua_ids: BTreeSet<String> = lua_facade().into_iter().map(|(_, id)| id).collect();

    let missing: Vec<&String> = lua_ids.difference(&catalog).collect();
    assert!(
        missing.is_empty(),
        "lua/qjs fleet facade references operation_ids the host's \
         OPERATION_CATALOG (src/operations.rs) does not implement: {missing:?}"
    );
}
