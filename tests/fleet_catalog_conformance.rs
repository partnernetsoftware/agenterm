//! Drift gate: `src/operations.rs::OPERATION_CATALOG` vs the hand-written
//! script-side `fleet.*` bindings.
//!
//! The catalog is the declaring authority: each `OperationSpec` carries a
//! `script_surface` (`"fleet.tabs.set_note"`) alongside its dispatchable `id`
//! (`"tabs.set-note"`) and its `parameters`. The bindings —
//! `scripts/lua/lib/fleet.lua` and `scripts/qjs/lib/fleet.js` — are
//! hand-written re-implementations of that same catalog, kept aligned by
//! copy-and-compare. A third binding for `.qjs` (`agenterm-qjswasm`) is
//! coming, which makes "aligned by hand" a three-way problem.
//!
//! This file asserts the two directions that nothing else asserts:
//!
//!   forward   every `fleet.*` `script_surface` the catalog declares is
//!             implemented by every binding (today: a pinned 47-entry gap)
//!   backward  every function a binding exposes names a declared
//!             `script_surface`, forwards that surface's declared `id`, and
//!             sends the parameter names that surface declares
//!
//! # Why this reads the catalog through Rust, not through a text parse
//!
//! `tests/script_fleet_facade_parity.rs` extracts the catalog by scanning
//! `src/operations.rs` for `id:` lines. That scan is blind to the 33 entries
//! built by the `nullary_ui_action()` const constructor (`src/operations.rs:503`),
//! which have no `id:` line of their own. It therefore sees 44 of the 77
//! real entries, and its `rh_surfaces_missing_from_host_catalog()` allowlist
//! names 33 operations as "absent from OPERATION_CATALOG" that are in fact
//! present and dispatchable. That test is green while asserting a false
//! statement. This file links against `agenterm::operations::OPERATION_CATALOG`
//! directly so the Rust side cannot be misparsed at all; only the binding
//! files, which are not Rust, are parsed as text — and their parse is
//! checked for integrity before any comparison runs (see
//! `binding_parsers_see_every_definition_and_every_call`).
//!
//! Findings are recorded in two shapes. A *conformance* assertion states
//! what must be true (the fixed parameter objects below); a *pinned* set is
//! an allowlist of a known-wrong status quo, each with a comment saying what
//! would make it shrink.
//!
//! # How the runtime consequences here were established
//!
//! An earlier revision of this file said its `broker_invalid_arguments`
//! conclusion was "read off the dispatcher rather than executed end to end
//! (the broker path needs a live server)". That parenthetical was wrong, and
//! worth correcting because it is why the conclusion went unverified for so
//! long: `validate_fleet_parameters` runs in the CLI *parent* process, before
//! `fleet_mutation_command` and before any IPC, so every rejection in this
//! file is observable with no server running at all. Each claim below was
//! produced by running the real binding file through the real broker:
//!
//! ```text
//! cargo build --bin agenterm --features script-qjs
//! agenterm cli script run <scripts/qjs/lib/fleet.js + a try/catch probe>
//! ```
//!
//! with `AGENTERM_SCRIPT_BACKEND=qjs`. qjs, not lua: on the lua path a
//! failing `__host.fleet_call` raises `mlua::Error` through LuaJIT's
//! `lua_error`, which longjmps across a `extern "C-unwind"`-less Rust frame
//! and aborts the whole worker ("panic in a function that cannot unwind"), so
//! a Lua script cannot even observe the rejection it caused. qjs turns the
//! same error into an ordinary catchable JS exception carrying the broker's
//! message verbatim. That lua abort is a host defect independent of anything
//! this gate asserts; it is recorded in plan/design-fleet-binding-gaps.md §6.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use agenterm::operations::OPERATION_CATALOG;

// ── binding registry ────────────────────────────────────────────────────
//
// Adding the third binding is one line here plus, if its syntax differs
// from both flavours below, one `Flavor` arm. The `.qjs` binding cannot
// exist yet: property access and object literals are unsupported upstream,
// so its first form will be free functions (`fleet_tabs_list()`), which
// will need a `Flavor::FreeFunction` arm that maps `fleet_a_b_c` back to
// the dotted `fleet.a.b.c` surface.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Flavor {
    /// `fleet.tabs.list = function () { return call("tabs.list"); };`
    JsAssignment,
    /// `function fleet.tabs.list() return call("tabs.list") end`
    LuaFunction,
}

#[derive(Clone, Copy)]
struct Binding {
    name: &'static str,
    path: &'static str,
    flavor: Flavor,
}

const BINDINGS: &[Binding] = &[
    Binding {
        name: "lua",
        path: "scripts/lua/lib/fleet.lua",
        flavor: Flavor::LuaFunction,
    },
    Binding {
        name: "qjs-js",
        path: "scripts/qjs/lib/fleet.js",
        flavor: Flavor::JsAssignment,
    },
];

// ── parsing ─────────────────────────────────────────────────────────────

/// One binding function: the dotted surface it is reachable at, the
/// operation id it forwards to `__host.fleet_call`, and the parameter names
/// it puts in the params object.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Exposed {
    operation_id: String,
    param_names: BTreeSet<String>,
}

#[derive(Debug)]
struct Parse {
    /// surface path -> what it forwards
    functions: BTreeMap<String, Exposed>,
    /// every namespace-table assignment, in source order, duplicates kept
    namespace_decls: Vec<String>,
    /// how many function definitions the parser saw
    definitions: usize,
    /// how many `call("...")` sites the parser saw
    call_sites: usize,
}

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path: PathBuf = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

/// Pulls the key names out of a single-line object/table literal, i.e. the
/// `{ tab_id: tabId, note: note }` of a `JSON.stringify(...)` call or the
/// `{ tab_id = tab_id, note = note }` of a `std.json.stringify(...)` call.
/// Returns `None` when the line has no literal at all (a no-params call).
fn param_names(line: &str, key_sep: char) -> Option<BTreeSet<String>> {
    let open = line.find('{')?;
    let close = line.rfind('}')?;
    if close < open {
        return None;
    }
    let inner = &line[open + 1..close];
    let mut names = BTreeSet::new();
    for field in inner.split(',') {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let Some(sep) = field.find(key_sep) else {
            panic!(
                "params literal field {field:?} has no `{key_sep}` separator \
                 — the params parser does not understand this line: {line:?}"
            );
        };
        names.insert(field[..sep].trim().to_owned());
    }
    Some(names)
}

fn parse(source: &str, flavor: Flavor) -> Parse {
    let mut functions = BTreeMap::new();
    let mut namespace_decls = Vec::new();
    let mut definitions = 0usize;
    let mut call_sites = 0usize;
    let mut pending: Option<String> = None;

    let (comment, key_sep) = match flavor {
        Flavor::JsAssignment => ("//", ':'),
        Flavor::LuaFunction => ("--", '='),
    };

    for raw in source.lines() {
        let line = raw.trim();
        if line.starts_with(comment) {
            continue;
        }

        // function definition
        match flavor {
            Flavor::JsAssignment => {
                if let Some(rest) = line.strip_prefix("fleet.")
                    && let Some(eq) = rest.find(" = function")
                {
                    definitions += 1;
                    let path = format!("fleet.{}", &rest[..eq]);
                    assert!(
                        pending.is_none(),
                        "definition of `{path}` follows `{}` without an \
                         intervening call(\"...\") — parser lost a body",
                        pending.as_deref().unwrap_or("<none>")
                    );
                    pending = Some(path);
                    continue;
                }
            }
            Flavor::LuaFunction => {
                if let Some(rest) = line.strip_prefix("function fleet.")
                    && let Some(paren) = rest.find('(')
                {
                    definitions += 1;
                    let path = format!("fleet.{}", &rest[..paren]);
                    assert!(
                        pending.is_none(),
                        "definition of `{path}` follows `{}` without an \
                         intervening call(\"...\") — parser lost a body",
                        pending.as_deref().unwrap_or("<none>")
                    );
                    pending = Some(path);
                    continue;
                }
            }
        }

        // namespace table assignment: `fleet.ui.tabs = {}` / `= {};`
        if let Some(rest) = line.strip_prefix("fleet.")
            && let Some(eq) = rest.find(" = {}")
        {
            namespace_decls.push(format!("fleet.{}", &rest[..eq]));
            continue;
        }
        if line == "const fleet = {};" || line == "local fleet = {}" {
            namespace_decls.push("fleet".to_owned());
            continue;
        }

        // `return call("op.id"[, stringify({ ... })])`
        if let Some(idx) = line.find("call(\"") {
            let after = &line[idx + "call(\"".len()..];
            let Some(end) = after.find('"') else {
                panic!("unterminated operation-id string literal on line: {line:?}");
            };
            call_sites += 1;
            let operation_id = after[..end].to_owned();
            let params = param_names(&after[end..], key_sep).unwrap_or_default();
            let Some(path) = pending.take() else {
                panic!(
                    "call(\"{operation_id}\") at line {line:?} is not inside \
                     any `fleet.*` function the parser recognised"
                );
            };
            let previous = functions.insert(
                path.clone(),
                Exposed {
                    operation_id,
                    param_names: params,
                },
            );
            assert!(
                previous.is_none(),
                "binding defines `{path}` twice — the later definition wins \
                 at runtime and the gate would only see one of them"
            );
        }
    }

    assert!(
        pending.is_none(),
        "binding ends with `{}` defined but never calling call(\"...\")",
        pending.as_deref().unwrap_or("<none>")
    );

    Parse {
        functions,
        namespace_decls,
        definitions,
        call_sites,
    }
}

fn parse_binding(binding: &Binding) -> Parse {
    parse(&read(binding.path), binding.flavor)
}

// ── catalog views ───────────────────────────────────────────────────────

/// `script_surface -> (operation id, declared parameter names)` for every
/// entry whose surface lives in the `fleet.*` namespace.
fn catalog_fleet_surfaces() -> BTreeMap<&'static str, (&'static str, BTreeSet<&'static str>)> {
    OPERATION_CATALOG
        .iter()
        .filter(|spec| spec.script_surface.starts_with("fleet."))
        .map(|spec| {
            (
                spec.script_surface,
                (
                    spec.id,
                    spec.parameters
                        .iter()
                        .map(|p| p.name)
                        .collect::<BTreeSet<_>>(),
                ),
            )
        })
        .collect()
}

// ── 0. parse integrity — never compare against a silently empty parse ───

#[test]
fn binding_parsers_see_every_definition_and_every_call() {
    for binding in BINDINGS {
        let source = read(binding.path);
        let parsed = parse_binding(binding);

        assert!(
            parsed.definitions > 0,
            "{}: parser found zero function definitions in {} — the parse \
             broke, it did not find an empty binding",
            binding.name,
            binding.path
        );

        // Ground truth taken straight off the raw text, independent of the
        // line-oriented state machine above: every `call("` occurrence in
        // the file must have been paired with a definition.
        let raw_call_sites = source.matches("call(\"").count();
        assert_eq!(
            parsed.call_sites, raw_call_sites,
            "{}: parser paired {} call sites but {} occurrences of `call(\"` \
             exist in {} — the parser is skipping lines",
            binding.name, parsed.call_sites, raw_call_sites, binding.path
        );
        assert_eq!(
            parsed.definitions,
            parsed.functions.len(),
            "{}: {} definitions collapsed into {} distinct surfaces",
            binding.name,
            parsed.definitions,
            parsed.functions.len()
        );
        assert_eq!(
            parsed.call_sites,
            parsed.functions.len(),
            "{}: {} call sites for {} functions — a body has zero or two \
             fleet_call sites and the gate's 1:1 assumption is broken",
            binding.name,
            parsed.call_sites,
            parsed.functions.len()
        );

        for path in parsed.functions.keys() {
            assert!(
                path.starts_with("fleet.")
                    && !path.contains(char::is_whitespace)
                    && !path.ends_with('.'),
                "{}: parsed a malformed surface path {path:?}",
                binding.name
            );
        }
    }
}

/// The Rust side is read through the linked crate, so it cannot be
/// misparsed — but the *reason* a text parse of `src/operations.rs` is
/// wrong is worth pinning, because a sibling test still does it and is
/// green while under-counting. The invariant below holds whichever way the
/// catalog is written: literal entries plus constructor-built entries must
/// account for every entry.
#[test]
fn catalog_length_is_literal_entries_plus_constructor_entries() {
    let source = read("src/operations.rs");
    let start = source
        .find("pub const OPERATION_CATALOG:")
        .expect("OPERATION_CATALOG declaration not found");
    let body = &source[start..];
    let open = body
        .find("= &[")
        .expect("OPERATION_CATALOG opener not found");
    let after_open = &body[open + "= &[".len()..];
    let close = after_open
        .find("\n];")
        .expect("OPERATION_CATALOG closer not found");
    let block = &after_open[..close];

    let literal_entries = block
        .lines()
        .filter(|line| line.trim().starts_with("id:"))
        .count();
    let constructed_entries = block.matches("nullary_ui_action(").count();

    assert!(
        literal_entries > 0,
        "catalog block extraction broke: zero `id:` lines found"
    );
    assert_eq!(
        literal_entries + constructed_entries,
        OPERATION_CATALOG.len(),
        "OPERATION_CATALOG has {} entries but src/operations.rs shows {} \
         written out longhand plus {} built by nullary_ui_action(). A third \
         construction shape has appeared: any test that reads this catalog \
         by scanning text (tests/script_fleet_facade_parity.rs does) is now \
         under-counting it and may be green on a false premise.",
        OPERATION_CATALOG.len(),
        literal_entries,
        constructed_entries
    );
    assert_eq!(
        constructed_entries, 33,
        "the number of nullary_ui_action() entries changed; \
         tests/script_fleet_facade_parity.rs::rh_surfaces_missing_from_host_catalog() \
         is a stale list of exactly these operations and needs revisiting"
    );
}

// ── 1. backward direction — bindings must not invent surfaces ───────────

#[test]
fn every_binding_function_names_a_declared_script_surface() {
    let catalog = catalog_fleet_surfaces();
    let mut violations = Vec::new();

    for binding in BINDINGS {
        for path in parse_binding(binding).functions.keys() {
            if !catalog.contains_key(path.as_str()) {
                violations.push(format!("{}: {path}", binding.name));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "binding functions with no matching `script_surface` in \
         src/operations.rs::OPERATION_CATALOG (the binding exposes an API \
         the host does not declare): {violations:#?}"
    );
}

#[test]
fn every_binding_function_forwards_the_operation_id_its_surface_declares() {
    let catalog = catalog_fleet_surfaces();
    let mut violations = Vec::new();

    for binding in BINDINGS {
        for (path, exposed) in parse_binding(binding).functions {
            let Some((declared_id, _)) = catalog.get(path.as_str()) else {
                continue; // reported by the test above
            };
            if *declared_id != exposed.operation_id {
                violations.push(format!(
                    "{}: {path} forwards {:?} but the catalog declares {declared_id:?}",
                    binding.name, exposed.operation_id
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "binding forwards an operation id that does not match its surface's \
         declared id: {violations:#?}"
    );
}

// ── 2. forward direction — catalog surfaces must be implemented ─────────

#[test]
fn every_catalog_surface_is_implemented_by_every_binding() {
    let catalog = catalog_fleet_surfaces();
    let expected_gap = unimplemented_surfaces();

    for binding in BINDINGS {
        let implemented: BTreeSet<String> = parse_binding(binding).functions.into_keys().collect();
        let gap: BTreeSet<&str> = catalog
            .keys()
            .copied()
            .filter(|surface| !implemented.contains(*surface))
            .collect();

        assert_eq!(
            gap, expected_gap,
            "the set of catalog `fleet.*` surfaces the {} binding does not \
             implement changed. Grown: a new OperationSpec landed with no \
             binding — port it, or add it here with a reason. Shrunk: a \
             binding gained one of the pinned gaps — delete that line here.",
            binding.name
        );
    }
}

/// The 47 `fleet.*` surfaces `OPERATION_CATALOG` declares that neither
/// hand-written binding implements, measured 2026-08-25. Both bindings are
/// missing exactly the same 47, which is what "kept aligned by
/// copy-and-compare" buys and also what it costs: the two files agree with
/// each other and disagree with the catalog by 62% of its surface.
///
/// 33 of these are the `nullary_ui_action()` entries (`ui.settings.*`,
/// `ui.font.*`, `ui.modal.*`, `ui.window.{close,maximize,minimize,restore}`,
/// `ui.instance_picker.{cancel,confirm,next,prev}`, `ui.tab.editor.*`,
/// `ui.tab.new`, `ui.locale.toggle`, `ui.new_terminal.open`,
/// `terminal.copy_selection`, `ui.window_close.keep_server_running`); the
/// remaining 14 are longhand entries. Shrinking this list is binding work,
/// deliberately out of this gate's remit.
fn unimplemented_surfaces() -> BTreeSet<&'static str> {
    [
        "fleet.terminal.copy_selection",
        "fleet.terminal.mouse",
        "fleet.ui.cwd_editor.open",
        "fleet.ui.cwd_editor.prepare",
        "fleet.ui.cwd_editor.prepare_append",
        "fleet.ui.cwd_editor.prepare_replace",
        "fleet.ui.cwd_editor.send_now",
        "fleet.ui.font.decrease",
        "fleet.ui.font.increase",
        "fleet.ui.input.key",
        "fleet.ui.instance_picker.cancel",
        "fleet.ui.instance_picker.confirm",
        "fleet.ui.instance_picker.next",
        "fleet.ui.instance_picker.open",
        "fleet.ui.instance_picker.prev",
        "fleet.ui.instance_picker.select",
        "fleet.ui.locale.toggle",
        "fleet.ui.modal.cancel",
        "fleet.ui.modal.confirm",
        "fleet.ui.new_terminal.open",
        "fleet.ui.server_strip.select",
        "fleet.ui.settings.apply",
        "fleet.ui.settings.inherit.font",
        "fleet.ui.settings.inherit.size",
        "fleet.ui.settings.inherit.theme",
        "fleet.ui.settings.open",
        "fleet.ui.settings.preset.classic_day",
        "fleet.ui.settings.preset.classic_night",
        "fleet.ui.settings.preset.fancy_day",
        "fleet.ui.settings.preset.fancy_night",
        "fleet.ui.settings.reset_overrides",
        "fleet.ui.settings.scope.current",
        "fleet.ui.settings.scope.defaults",
        "fleet.ui.settings.theme.dark",
        "fleet.ui.settings.theme.light",
        "fleet.ui.tab.close",
        "fleet.ui.tab.edit",
        "fleet.ui.tab.editor.cancel",
        "fleet.ui.tab.editor.save",
        "fleet.ui.tab.new",
        "fleet.ui.window.close",
        "fleet.ui.window.maximize",
        "fleet.ui.window.minimize",
        "fleet.ui.window.resize",
        "fleet.ui.window.restore",
        "fleet.ui.window_close.keep_server_running",
        "fleet.ui.window_close.stop_server_and_exit",
    ]
    .into_iter()
    .collect()
}

// ── 3. parameter conformance ────────────────────────────────────────────
//
// The catalog does not only name a surface, it declares that surface's
// parameter names — and `src/client/mod.rs:2649` (`validate_fleet_parameters`,
// reached from `__host.fleet_call` via `src/script_worker.rs:616` ->
// broker `"fleet.call"` -> `src/client/mod.rs:2593`) rejects any key the
// spec does not list and any required key the caller omits. So a binding
// whose params object disagrees with the spec is not a documentation nit:
// it is a call the host refuses.
//
// Two classes are asserted; a third is deliberately not:
//
//   unknown           binding sends a key the spec does not declare
//   missing_required  binding omits a key the spec marks required
//   missing_optional  binding omits an optional key — fine, that is what
//                     "optional" means, and hand-written sugar legitimately
//                     leaves optionals off the signature. Not asserted.

#[derive(Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
struct ParamDrift {
    unknown: BTreeSet<String>,
    missing_required: BTreeSet<String>,
}

fn param_drift(binding: &Binding) -> BTreeMap<String, ParamDrift> {
    let catalog: BTreeMap<&str, &'static [agenterm::operations::OperationParameterSpec]> =
        OPERATION_CATALOG
            .iter()
            .map(|spec| (spec.script_surface, spec.parameters))
            .collect();

    let mut drift = BTreeMap::new();
    for (path, exposed) in parse_binding(binding).functions {
        let Some(declared) = catalog.get(path.as_str()) else {
            continue; // reported by the backward-direction test
        };
        let mut entry = ParamDrift::default();
        for sent in &exposed.param_names {
            if !declared.iter().any(|spec| spec.name == sent.as_str()) {
                entry.unknown.insert(sent.clone());
            }
        }
        for spec in declared.iter().filter(|spec| spec.required) {
            if !exposed.param_names.contains(spec.name) {
                entry.missing_required.insert(spec.name.to_owned());
            }
        }
        if entry != ParamDrift::default() {
            drift.insert(path, entry);
        }
    }
    drift
}

#[test]
fn binding_params_objects_conform_to_the_catalog_parameter_spec() {
    let expected = expected_parameter_drift();

    for binding in BINDINGS {
        let actual = param_drift(binding);
        assert_eq!(
            actual, expected,
            "the {} binding's params objects drifted against the catalog's \
             declared `parameters`. This compares the full detail, not just \
             which surfaces are affected: a new unknown key on an \
             already-listed surface fails here too. Growing this map means a \
             call the host will refuse; shrinking it means someone fixed a \
             binding — delete the entry.",
            binding.name
        );
    }
}

/// The parameter disagreements that remain between the two hand-written
/// bindings and `OPERATION_CATALOG`. Both bindings drift **identically** on
/// every one — the copy-and-compare kept them consistent with each other and
/// inconsistent with the declaring catalog, which is precisely the failure
/// mode a binding-to-binding diff cannot see.
///
/// Nine surfaces were measured on 2026-08-25. Two were **pure bugs** — a
/// wrong wire key behind an otherwise correct signature — and are fixed;
/// they are asserted positively by
/// `fixed_bindings_send_exactly_the_declared_parameter_names` and are gone
/// from this map. The seven below are **product decisions**: repairing each
/// one changes the binding's own argument list, so every existing caller
/// would have to change. Deciding that is not this gate's call, and silently
/// "fixing" one by changing a published signature is exactly what this map
/// exists to make visible.
///
/// Payloads are the literal JSON `scripts/lua/lib/fleet.lua` emits, captured
/// by overriding `__host.fleet_call` inside a hosted lua invocation; the
/// broker messages are the literal strings the real `agenterm cli script run`
/// answered with (see the module doc for the method). Nothing here is
/// inferred from the regex above or read off the dispatcher.
///
/// ```text
/// surface                  emits                 broker answers
/// fleet.terminal.paste     {"text":"abc"}        does not accept parameter text
/// fleet.ui.composer.send   {"text":"hi"}         does not accept parameter text
/// fleet.ui.input.wheel     {"delta":3}           does not accept parameter delta
/// fleet.ui.hello           {}                    requires parameter minimum
/// fleet.ui.deltas          {}                    requires parameter epoch
/// fleet.events.read        {}                    requires parameter epoch
/// fleet.events.wait        {"timeout_ms":100}    requires parameter epoch
/// ```
///
/// all prefixed `broker_invalid_arguments:`. What the seven decisions are,
/// one by one, is written down in plan/design-fleet-binding-gaps.md §4 —
/// `remaining_parameter_drift_is_documented_as_product_decisions` keeps that
/// document and this map from parting company.
fn expected_parameter_drift() -> BTreeMap<String, ParamDrift> {
    fn drift(surface: &str, unknown: &[&str], missing_required: &[&str]) -> (String, ParamDrift) {
        (
            surface.to_owned(),
            ParamDrift {
                unknown: unknown.iter().map(|s| (*s).to_owned()).collect(),
                missing_required: missing_required.iter().map(|s| (*s).to_owned()).collect(),
            },
        )
    }

    [
        drift("fleet.events.read", &[], &["after", "epoch"]),
        drift("fleet.events.wait", &[], &["after", "epoch", "kind"]),
        drift("fleet.terminal.paste", &["text"], &[]),
        drift("fleet.ui.composer.send", &["text"], &[]),
        drift("fleet.ui.deltas", &[], &["after", "epoch"]),
        drift("fleet.ui.hello", &[], &["maximum", "minimum"]),
        drift("fleet.ui.input.wheel", &["delta"], &["delta_y", "x", "y"]),
    ]
    .into_iter()
    .collect()
}

/// The two surfaces whose binding was a **pure bug**: the published
/// signature was right, only the JSON key it put on the wire was wrong, so
/// repairing them cost no caller anything.
///
/// | surface | was | is | observed before the fix |
/// |---|---|---|---|
/// | `fleet.tabs.set_note(tab_id, note)` | `{tab_id, note}` | `{tab, note}` | `broker_invalid_arguments: tabs.set-note does not accept parameter tab_id` |
/// | `fleet.ui.tab.select(id)` | `{id}` | `{tab}` | `broker_invalid_arguments: ui.tab.select does not accept parameter id` |
///
/// After the fix, `tabs.set-note` clears validation and reaches its adapter
/// (`broker_transport`, i.e. "no server", which is as far as an offline probe
/// can get). `ui.tab.select` clears validation and then hits
/// `broker_operation_unknown: no Fleet adapter exists for ui.tab.select` — a
/// second, host-side defect that this binding cannot fix and that
/// plan/design-fleet-binding-gaps.md §5 records.
///
/// This is asserted as an exact set, not merely as an absence from
/// `expected_parameter_drift`: that map ignores omitted *optional*
/// parameters, so it would stay silent if someone re-broke `set_note` by
/// dropping `note`, or "helpfully" widened `select` to send `tab` plus
/// something else the spec happens to list.
fn conformant_binding_parameters() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    [
        ("fleet.tabs.set_note", ["note", "tab"].as_slice()),
        ("fleet.ui.tab.select", ["tab"].as_slice()),
    ]
    .into_iter()
    .map(|(surface, names)| (surface, names.iter().copied().collect()))
    .collect()
}

#[test]
fn fixed_bindings_send_exactly_the_declared_parameter_names() {
    let catalog = catalog_fleet_surfaces();

    for (surface, expected) in conformant_binding_parameters() {
        let (_, declared) = catalog
            .get(surface)
            .unwrap_or_else(|| panic!("{surface} is no longer a declared script_surface"));
        assert!(
            expected.is_subset(declared),
            "{surface}: the conformance expectation {expected:?} is not a subset of              the catalog's declared parameters {declared:?} — the catalog moved and              this expectation was not revisited"
        );

        for binding in BINDINGS {
            let functions = parse_binding(binding).functions;
            let exposed = functions.get(surface).unwrap_or_else(|| {
                panic!(
                    "{}: {surface} is gone from {} — it is a fixed, conformant                      surface and must stay bound",
                    binding.name, binding.path
                )
            });
            let sent: BTreeSet<&str> = exposed.param_names.iter().map(String::as_str).collect();
            assert_eq!(
                sent, expected,
                "{}: {surface} puts {sent:?} on the wire but must put exactly                  {expected:?}. These keys are the host contract, not a naming                  preference: `validate_fleet_parameters` (src/client/mod.rs)                  answers any other set with broker_invalid_arguments, which is                  what this surface used to do on every single call.",
                binding.name
            );
        }
    }
}

#[test]
fn remaining_parameter_drift_is_documented_as_product_decisions() {
    let design = read("plan/design-fleet-binding-gaps.md");
    for surface in expected_parameter_drift().keys() {
        assert!(
            design.contains(surface.as_str()),
            "plan/design-fleet-binding-gaps.md does not mention {surface}, but              expected_parameter_drift() still pins it. Every remaining entry is              a product decision — a binding whose published signature would have              to change — and the decision has to be written down somewhere a              reader can find it, or the pin degrades into an unexplained              allowlist."
        );
    }
    for surface in conformant_binding_parameters().keys() {
        assert!(
            design.contains(surface),
            "plan/design-fleet-binding-gaps.md no longer records the fix to              {surface}"
        );
    }
}

// ── 3b. a parameter spec the validator cannot satisfy at all ────────────

/// `validate_fleet_parameters` decides `valid_type` with a `match` over
/// `spec.value_type` whose fallback arm is `_ => false`. Any `value_type` the
/// catalog declares but that match does not name is therefore **impossible to
/// satisfy**: every value of every shape is rejected, and the operation is
/// unreachable no matter what a binding sends.
///
/// This was not hypothetical. The catalog declared `number` for pointer and
/// wheel coordinates and the validator's match had no `number` arm, so those
/// two surfaces were unreachable whatever any binding sent. Observed against
/// the real broker before the fix:
///
/// ```text
/// ui.input.pointer {"x":1,"y":2}          -> parameter x must be number
/// ui.input.pointer {"x":1.5,"y":2.5}      -> parameter x must be number
/// ui.input.pointer {"x":"1","y":"2"}      -> parameter x must be number
/// ui.input.wheel   {"x":1,"y":2,"delta_y":3} -> parameter x must be number
/// ```
///
/// `fleet.ui.input.pointer` was the sharp edge: it sends `{x, y, action}`, all
/// three declared, so it has **zero** parameter drift and this file's drift
/// test called it conformant — while the host refused every call it made.
/// "Conforms to the declared parameter names" and "works" are different
/// properties, and only the first one is cheap to check. That is why this test
/// asserts the *class* and not the instance.
///
/// `src/client/mod.rs` gained the `"number"` arm (commit `f801cf20`), so the
/// unsatisfiable set is now empty and stays that way: a new `value_type` the
/// validator does not name fails here, at the point someone adds it, rather
/// than silently at the point someone calls it.
///
/// `fleet.ui.input.wheel` stays in `expected_parameter_drift` all the same,
/// and for its own reason: its binding sends `{delta}` where the catalog
/// declares `{x, y, delta_y}`. That is a published signature, so repairing it
/// changes every caller — a product decision, exactly as the other six are.
/// What changed is that repairing it would now *work*; before, it would not
/// have.
#[test]
fn every_catalog_value_type_is_one_the_broker_validator_can_accept() {
    let source = read("src/client/mod.rs");
    let opener = "let valid_type = match spec.value_type {";
    let start = source
        .find(opener)
        .expect("validate_fleet_parameters' value_type match was not found in src/client/mod.rs");
    let body = &source[start + opener.len()..];
    let close = body
        .find(
            "
        };",
        )
        .expect("value_type match closer not found");
    let block = &body[..close];

    let mut accepted: BTreeSet<&str> = BTreeSet::new();
    let mut arms = 0usize;
    for line in block.lines() {
        let line = line.trim();
        if !line.starts_with('"') {
            continue;
        }
        let Some(head) = line.split("=>").next() else {
            continue;
        };
        arms += 1;
        for literal in head.split('"').skip(1).step_by(2) {
            accepted.insert(literal);
        }
    }
    assert!(
        arms >= 3 && accepted.len() >= 5,
        "the value_type match parse degenerated ({arms} arms, {accepted:?}) —          it did not find a permissive validator, it failed to read the match"
    );

    let mut unsatisfiable: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for spec in OPERATION_CATALOG {
        for parameter in spec.parameters {
            if !accepted.contains(parameter.value_type) {
                unsatisfiable
                    .entry(parameter.value_type)
                    .or_default()
                    .insert(spec.script_surface);
            }
        }
    }

    // Empty, and it must stay empty: every type the catalog declares has an
    // arm. The message below says what to do in either direction.
    let expected: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();

    assert_eq!(
        unsatisfiable, expected,
        "a catalog `value_type` has no arm in `validate_fleet_parameters`, so          every operation listed here is dead on arrival for every binding: no          value of any shape can satisfy it. Add the arm in src/client/mod.rs          rather than adding an entry here — an entry here would only record          that a surface cannot be called. Validator accepts: {accepted:?}."
    );
}

// ── 4. bindings must agree with each other ──────────────────────────────

#[test]
fn all_bindings_expose_the_same_surface_map() {
    let mut reference: Option<(&str, BTreeMap<String, Exposed>)> = None;

    for binding in BINDINGS {
        let functions = parse_binding(binding).functions;
        match &reference {
            None => reference = Some((binding.name, functions)),
            Some((first_name, first)) => assert_eq!(
                *first, functions,
                "binding `{}` and binding `{}` expose different \
                 (surface -> operation id + params) maps",
                first_name, binding.name
            ),
        }
    }

    let (_, functions) = reference.expect("BINDINGS is empty");
    assert!(
        !functions.is_empty(),
        "every binding parsed to an empty map"
    );
}

// ── 5. namespace hygiene ────────────────────────────────────────────────

#[test]
fn no_binding_declares_the_same_namespace_table_twice() {
    let mut duplicates: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();

    for binding in BINDINGS {
        let mut seen = BTreeSet::new();
        for decl in parse_binding(binding).namespace_decls {
            if !seen.insert(decl.clone()) {
                duplicates.entry(binding.name).or_default().insert(decl);
            }
        }
    }

    assert_eq!(
        duplicates,
        redundant_namespace_declarations(),
        "the set of namespace tables a binding assigns more than once \
         changed. A second `fleet.ui.tab = {{}}` silently discards whatever \
         the first one held; today every duplicate happens to precede all of \
         that namespace's functions, so nothing is lost — but the pattern is \
         one reordering away from erasing methods."
    );
}

/// `scripts/lua/lib/fleet.lua` assigns nine namespace tables twice each,
/// measured 2026-08-25 — dead lines left by copy-and-compare maintenance.
/// `scripts/qjs/lib/fleet.js`, ported from it, has none: the port dropped
/// the duplicates, which is direct evidence that the two files are
/// maintained by re-typing rather than by a shared source.
fn redundant_namespace_declarations() -> BTreeMap<&'static str, BTreeSet<String>> {
    let lua: BTreeSet<String> = [
        "fleet.control_center",
        "fleet.events",
        "fleet.protocol",
        "fleet.server",
        "fleet.ui.tab",
        "fleet.ui.tabs",
        "fleet.ui.tree",
        "fleet.ui.window",
        "fleet.workspace",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    [("lua", lua)].into_iter().collect()
}

// ── 6. the one surface outside the fleet.* namespace ────────────────────

#[test]
fn only_one_script_surface_sits_outside_the_fleet_namespace() {
    let outliers: BTreeMap<&str, &str> = OPERATION_CATALOG
        .iter()
        .filter(|spec| !spec.script_surface.starts_with("fleet."))
        .map(|spec| (spec.script_surface, spec.id))
        .collect();

    let expected: BTreeMap<&str, &str> = [("FleetTerminal.capture", "pane.capture")]
        .into_iter()
        .collect();

    assert_eq!(
        outliers, expected,
        "`script_surface` values outside the `fleet.*` namespace changed. \
         `pane.capture` is the sole legacy outlier: it is spelled as a \
         pseudo-class path (`FleetTerminal.capture`) rather than a dotted \
         `fleet.*` path, so no binding can implement it by the naming \
         convention every other entry follows, and the forward-direction \
         gate above cannot see it. A second outlier means the convention \
         is eroding — give the new entry a `fleet.*` surface."
    );
}
