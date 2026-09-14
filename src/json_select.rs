//! Field selection over a JSON document the product already built.
//!
//! A selector is the **caller's request for fewer bytes**, never an authority:
//!
//! * it is applied to a [`Value`] the caller could already receive in full —
//!   the producer's own document, before the product's existing pretty
//!   serializer runs — so it can only *remove* members and can never make a
//!   field visible that was not visible before;
//! * there is no per-command field list anywhere in the product. The caller
//!   names the paths it reads; the host projects whatever document that command
//!   was about to emit. A command without a selector emits exactly the bytes it
//!   emitted before this module existed;
//! * a path that is absent from the document is absent from the projection and
//!   is **not** an error, so a caller that mistypes a field sees precisely what
//!   it sees today (a missing member), and an optional member (`error.code`) is
//!   never invented and never reported as a denial.
//!
//! # Frozen grammar
//!
//! ```text
//! selector := path ("," path)*
//! path     := segment ("." segment)*
//! segment  := key | key "[]" | "[]"
//! ```
//!
//! `key` is a non-empty run of `A-Za-z0-9_-`. `[]` descends into every element
//! of the array held at that point, so `[].endpoint` addresses a root array and
//! `tabs[].render.text` addresses a member of every element. A terminal `[]`
//! keeps elements whole.
//!
//! There is deliberately no wildcard, no recursion, no regular expression, no
//! filter, no computed key and no union: the grammar is the smallest one the
//! three real journeys' read sets needed.
//!
//! # Limits (robustness, not permission)
//!
//! [`SELECTOR_MAX_BYTES`], [`SELECTOR_MAX_PATHS`] and [`SELECTOR_MAX_DEPTH`]
//! bound the work a single request can ask for. They are the same kind of
//! control as a buffer length: exceeding one is a typed malformed-selector
//! failure, never a policy decision.
//!
//! # Properties this module guarantees
//!
//! 1. **Value preservation by construction.** Retained members are moved
//!    unchanged into the projected document and rendered by the *same*
//!    serializer, so every retained value keeps its exact spelling and order.
//!    Nothing here parses text and re-serializes it.
//! 2. **Monotone size.** Removing a member cannot enlarge the pretty-printed
//!    document. A path that matches nothing invents nothing and therefore
//!    yields an empty projection at that level; a selector can never raise the
//!    encoded-byte budget.
//! 3. **No hidden state.** Parsing and projecting are pure functions of their
//!    arguments; nothing is remembered between calls.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::commands::{has_option, option_value};

/// The one flag that enables a selection.
///
/// `protocol-info` has always answered JSON, so `--json` does not change what
/// the command produces. It is the caller's statement that it wants *machine*
/// output, and therefore the declared precondition for naming fields: it keeps
/// `--select` from being attached to a call whose caller still expects text.
pub(crate) const SELECT_FLAG: &str = "--select";
/// The declared precondition for [`SELECT_FLAG`].
pub(crate) const SELECT_REQUIRES_FLAG: &str = "--json";
/// Stable code for a selection requested without its precondition.
pub(crate) const SELECT_REQUIRES_JSON_CODE: &str = "selection_requires_json";
/// Stable code for a selection that does not parse.
pub(crate) const SELECT_MALFORMED_CODE: &str = "selection_malformed";

/// Longest accepted selector text, in bytes.
pub(crate) const SELECTOR_MAX_BYTES: usize = 4096;
/// Most paths one selector may name.
pub(crate) const SELECTOR_MAX_PATHS: usize = 64;
/// Deepest accepted path, counting segments.
pub(crate) const SELECTOR_MAX_DEPTH: usize = 8;

/// A selector that could not be parsed. Every variant is a syntax or limit
/// problem in the request itself, never a judgement about the reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SelectorError {
    Empty,
    TooLong { bytes: usize, limit: usize },
    TooManyPaths { paths: usize, limit: usize },
    TooDeep { path: String, limit: usize },
    EmptySegment { path: String },
    UnsupportedCharacter { path: String, character: char },
}

/// A refused selection: the request is wrong, the reply is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionRefusal {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

/// Read the caller's selection out of `args` and apply it to `value`.
///
/// `Ok(None)` means the caller did not ask for a selection and the caller must
/// emit `value` exactly as it always did. `Ok(Some(value))` is the projected
/// document, still a [`Value`], ready for the same serializer. `Err` is a
/// typed refusal that the caller renders through its own failure channel.
///
/// Every command that accepts these two options shares this function, so the
/// contract cannot drift between the client-side and host-side producers of the
/// same document: the grammar, the limits, and the `--json` precondition have
/// exactly one implementation.
pub(crate) fn apply_selection_request(
    value: &Value,
    args: &[String],
) -> Result<Option<Value>, SelectionRefusal> {
    apply_selection_request_with_contract(value, args, true)
}

/// Apply the shared selector grammar to a command whose only output format is
/// already JSON. Such a command needs no invented `--json` precondition: the
/// presence of `--select` is the caller's opt-in to projection.
pub(crate) fn apply_inherent_json_selection_request(
    value: &Value,
    args: &[String],
) -> Result<Option<Value>, SelectionRefusal> {
    apply_selection_request_with_contract(value, args, false)
}

fn apply_selection_request_with_contract(
    value: &Value,
    args: &[String],
    requires_json_flag: bool,
) -> Result<Option<Value>, SelectionRefusal> {
    let Some(text) = option_value(args, SELECT_FLAG) else {
        return Ok(None);
    };
    if requires_json_flag && !has_option(args, SELECT_REQUIRES_FLAG) {
        return Err(SelectionRefusal {
            code: SELECT_REQUIRES_JSON_CODE,
            message: format!(
                "{SELECT_FLAG} requires {SELECT_REQUIRES_FLAG}: this command always answers \
                 JSON, and the machine-output flag is the contract that says the caller wants \
                 named fields rather than text"
            ),
        });
    }
    let selector = Selector::parse(text).map_err(|error| SelectionRefusal {
        code: error.code(),
        message: error.message(),
    })?;
    Ok(Some(selector.project(value)))
}

impl SelectorError {
    /// The stable code a caller switches on.
    pub(crate) fn code(&self) -> &'static str {
        SELECT_MALFORMED_CODE
    }

    /// Human-readable text naming the offending path and limit.
    pub(crate) fn message(&self) -> String {
        match self {
            SelectorError::Empty => "a --select list names at least one path".to_owned(),
            SelectorError::TooLong { bytes, limit } => {
                format!("--select is {bytes} bytes; the limit is {limit}. Shorten the path list.")
            }
            SelectorError::TooManyPaths { paths, limit } => {
                format!("--select names {paths} paths; the limit is {limit}. Ask for fewer fields.")
            }
            SelectorError::TooDeep { path, limit } => {
                format!("--select path `{path}` is deeper than {limit} segments.")
            }
            SelectorError::EmptySegment { path } => format!(
                "--select path `{path}` has an empty segment; a path is `key` or `key[]`, \
                 joined by single `.`"
            ),
            SelectorError::UnsupportedCharacter { path, character } => format!(
                "--select path `{path}` contains `{character}`, which is not part of a key \
                 (`A-Za-z0-9_-`) or of the `[]` element walk"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// A member, kept whole.
    Key(String),
    /// A member holding an array; the rest of the path applies to each element.
    Elements(String),
    /// The document itself is the array; the rest of the path applies to each
    /// element.
    RootElements,
}

/// What a parsed selector keeps at one point of the document.
///
/// One node can carry both a member set and an element walk, because a caller
/// may name `a.b` and `a[].c` for the same key; projection picks the form that
/// matches the value it is looking at. `leaf` means "keep this value exactly as
/// the producer built it" and always wins, which is how a shorter path absorbs
/// a deeper one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Node {
    /// Keep this value exactly as the producer built it.
    leaf: bool,
    /// Members to keep, by member name.
    members: BTreeMap<String, Node>,
    /// The value here is an array; project every element with this node.
    elements: Option<Box<Node>>,
}

impl Node {
    fn leaf() -> Self {
        Node {
            leaf: true,
            ..Node::default()
        }
    }
}

/// A parsed, bounded field selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selector {
    root: Node,
}

impl Selector {
    /// Parse `text` into a bounded selection.
    ///
    /// Duplicate paths collapse, and when one path is a prefix of another the
    /// shorter one wins and keeps that whole subtree: `a` promises `a` itself,
    /// so `a` plus `a.b` keeps `a` whole.
    pub(crate) fn parse(text: &str) -> Result<Self, SelectorError> {
        if text.len() > SELECTOR_MAX_BYTES {
            return Err(SelectorError::TooLong {
                bytes: text.len(),
                limit: SELECTOR_MAX_BYTES,
            });
        }
        if text.trim().is_empty() {
            return Err(SelectorError::Empty);
        }
        let mut parsed: Vec<Vec<Segment>> = Vec::new();
        for raw in text.split(',') {
            let path = raw.trim();
            if path.is_empty() {
                return Err(SelectorError::EmptySegment {
                    path: raw.to_owned(),
                });
            }
            let segments = parse_path(path)?;
            if segments.len() > SELECTOR_MAX_DEPTH {
                return Err(SelectorError::TooDeep {
                    path: path.to_owned(),
                    limit: SELECTOR_MAX_DEPTH,
                });
            }
            if !parsed.contains(&segments) {
                parsed.push(segments);
            }
        }
        if parsed.is_empty() {
            return Err(SelectorError::Empty);
        }
        if parsed.len() > SELECTOR_MAX_PATHS {
            return Err(SelectorError::TooManyPaths {
                paths: parsed.len(),
                limit: SELECTOR_MAX_PATHS,
            });
        }
        // Shorter first, so a prefix always installs its `Leaf` before a deeper
        // path tries to walk through it.
        parsed.sort_by_key(Vec::len);
        let mut root = Node::default();
        for segments in &parsed {
            insert(&mut root, segments);
        }
        Ok(Selector { root })
    }

    /// How many distinct paths this selector names. Used by the ownership
    /// court to prove that duplicates collapse and that a shorter path absorbs
    /// a deeper one.
    #[cfg(test)]
    pub(crate) fn path_count(&self) -> usize {
        fn count(node: &Node) -> usize {
            if node.leaf {
                return 1;
            }
            let members: usize = node.members.values().map(count).sum();
            let elements = node.elements.as_ref().map_or(0, |inner| count(inner));
            members + elements
        }
        count(&self.root)
    }

    /// Keep only the selected paths of `value`.
    ///
    /// A node whose kind does not match the document (`tabs[].id` where `tabs`
    /// is not an array) keeps that value whole rather than dropping it: the
    /// projection never guesses, and never removes something the caller may
    /// still need.
    pub(crate) fn project(&self, value: &Value) -> Value {
        project_node(value, &self.root)
    }
}

fn parse_path(path: &str) -> Result<Vec<Segment>, SelectorError> {
    let mut segments = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            return Err(SelectorError::EmptySegment {
                path: path.to_owned(),
            });
        }
        let (key, elements) = match part.strip_suffix("[]") {
            Some(head) => (head, true),
            None => (part, false),
        };
        if key.is_empty() {
            if !elements {
                return Err(SelectorError::EmptySegment {
                    path: path.to_owned(),
                });
            }
            segments.push(Segment::RootElements);
            continue;
        }
        for character in key.chars() {
            if !(character.is_ascii_alphanumeric() || character == '_' || character == '-') {
                return Err(SelectorError::UnsupportedCharacter {
                    path: path.to_owned(),
                    character,
                });
            }
        }
        segments.push(if elements {
            Segment::Elements(key.to_owned())
        } else {
            Segment::Key(key.to_owned())
        });
    }
    Ok(segments)
}

fn insert(root: &mut Node, segments: &[Segment]) {
    let mut current = root;
    for (index, segment) in segments.iter().enumerate() {
        let last = index + 1 == segments.len();
        match segment {
            Segment::Key(key) => {
                if last {
                    current.members.insert(key.clone(), Node::leaf());
                    return;
                }
                current = current.members.entry(key.clone()).or_default();
            }
            Segment::Elements(key) => {
                let entry = current.members.entry(key.clone()).or_default();
                if last {
                    // `key[]` keeps every element whole.
                    entry.elements = Some(Box::new(Node::leaf()));
                    return;
                }
                current = entry
                    .elements
                    .get_or_insert_with(|| Box::new(Node::default()));
            }
            Segment::RootElements => {
                if last {
                    current.elements = Some(Box::new(Node::leaf()));
                    return;
                }
                current = current
                    .elements
                    .get_or_insert_with(|| Box::new(Node::default()));
            }
        }
    }
}

fn project_node(value: &Value, node: &Node) -> Value {
    if node.leaf {
        return value.clone();
    }
    match value {
        Value::Array(items) if node.elements.is_some() => {
            let inner = node.elements.as_ref().expect("checked above");
            Value::Array(items.iter().map(|item| project_node(item, inner)).collect())
        }
        Value::Object(object) if !node.members.is_empty() => {
            let mut kept = Map::new();
            for (key, member) in object {
                if let Some(inner) = node.members.get(key) {
                    kept.insert(key.clone(), project_node(member, inner));
                }
            }
            Value::Object(kept)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::borrow::Cow;

    fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
        let mut current = value;
        for part in path.split('.') {
            let (key, elements) = match part.strip_suffix("[]") {
                Some(head) => (head, true),
                None => (part, false),
            };
            if !key.is_empty() {
                current = current.get(key)?;
            }
            if elements {
                current = current.as_array()?.first()?;
            }
        }
        Some(current)
    }

    fn document() -> Value {
        json!({
            "protocol_version": 1,
            "pid": 4242,
            "build_identity": { "git_commit": "abc", "profile": "dev", "extra": [1, 2, 3] },
            "ui_bridge": {
                "ownership_mode": "split_server_client",
                "server_executable": "agenterm.exe",
                "replaceable_ui": true,
                "nested": { "deep": { "deeper": "kept-whole" }, "other": 1 }
            },
            "command_catalog": { "schema_version": 1, "commands": [ { "id": "new-window", "usage": "x" } ] },
            "tabs": [
                { "id": "@1", "render": { "text": { "width": 3, "height": 4 } }, "note": "a" },
                { "id": "@2", "render": { "text": { "width": 5, "height": 6 } }, "note": "b" }
            ],
            "optional_gap": { "present": 1 }
        })
    }

    /// Research reporter for
    /// `plan/design-ui-snapshot-selection-boundary-experiment.md`.
    ///
    /// Ignored by default because elapsed time is evidence to record, never a
    /// unit-test assertion. The semantic and bound assertions remain hard so a
    /// timing sample cannot be emitted for the wrong projection.
    #[test]
    #[ignore = "explicit ui-snapshot selection boundary measurement"]
    fn measure_ui_snapshot_text_reparse_boundary() {
        const ONE_TAB_ENVELOPE: &str = include_str!(
            "../research/qjswasm-host-reply-wire-cost/replies/workbench-smoke/env-0000.txt"
        );
        const THREE_TAB_ENVELOPE: &str = include_str!(
            "../research/qjswasm-host-reply-wire-cost/replies/workbench-smoke/env-0009.txt"
        );
        const WARMUPS: usize = 100;
        const ITERATIONS: usize = 1_000;

        fn stdout_document(envelope: &str) -> String {
            let envelope: Value = serde_json::from_str(envelope).expect("frozen envelope JSON");
            envelope["stdout"]
                .as_str()
                .expect("frozen envelope stdout")
                .to_owned()
        }

        fn project_optional<'a>(source: &'a str, selector: Option<&Selector>) -> Cow<'a, str> {
            let Some(selector) = selector else {
                return Cow::Borrowed(source);
            };
            let value: Value = serde_json::from_str(source).expect("snapshot JSON");
            Cow::Owned(
                serde_json::to_string_pretty(&selector.project(&value))
                    .expect("selected snapshot JSON"),
            )
        }

        fn percentile(values: &mut [u128], numerator: usize, denominator: usize) -> u128 {
            values.sort_unstable();
            let index = (values.len() * numerator).div_ceil(denominator) - 1;
            values[index]
        }

        fn measure(name: &str, source: &str, selector_text: &str) -> Value {
            let selector = Selector::parse(selector_text).expect("frozen selector");
            assert!(matches!(project_optional(source, None), Cow::Borrowed(_)));
            assert_eq!(project_optional(source, None).as_ref(), source);

            let original: Value = serde_json::from_str(source).expect("snapshot JSON");
            let projected = selector.project(&original);
            let expected = serde_json::to_string_pretty(&projected).unwrap();
            assert_eq!(project_optional(source, Some(&selector)).as_ref(), expected);
            assert!(expected.len() <= source.len());

            for _ in 0..WARMUPS {
                std::hint::black_box(project_optional(source, Some(&selector)));
            }
            let mut samples = Vec::with_capacity(3);
            for _ in 0..3 {
                let mut nanos = Vec::with_capacity(ITERATIONS);
                for _ in 0..ITERATIONS {
                    let started = std::time::Instant::now();
                    std::hint::black_box(project_optional(source, Some(&selector)));
                    nanos.push(started.elapsed().as_nanos());
                }
                let mut median_values = nanos.clone();
                let median_ns = percentile(&mut median_values, 1, 2);
                let p95_ns = percentile(&mut nanos, 95, 100);
                samples.push(json!({"median_ns": median_ns, "p95_ns": p95_ns}));
            }
            json!({
                "name": name,
                "selector": selector_text,
                "full_bytes": source.len(),
                "selected_bytes": expected.len(),
                "selected_ratio": expected.len() as f64 / source.len() as f64,
                "samples": samples,
            })
        }

        let one_tab = stdout_document(ONE_TAB_ENVELOPE);
        let three_tabs = stdout_document(THREE_TAB_ENVELOPE);
        let report = json!({
            "schema_version": 1,
            "warmups": WARMUPS,
            "iterations_per_sample": ITERATIONS,
            "allocation_bytes": "未测定",
            "documents": [
                measure("rendered-one-tab", &one_tab, "event_position"),
                measure(
                    "rendered-three-tabs",
                    &three_tabs,
                    "tabs[].id,tabs[].render.text",
                ),
            ],
        });
        eprintln!("UI_SNAPSHOT_SELECTION_MEASUREMENT={report}");
    }

    #[test]
    fn a_selector_keeps_exactly_the_named_paths() {
        let selector =
            Selector::parse("pid,ui_bridge.ownership_mode,ui_bridge.replaceable_ui").unwrap();
        let projected = selector.project(&document());
        assert_eq!(at(&projected, "pid"), Some(&json!(4242)));
        assert_eq!(
            at(&projected, "ui_bridge.ownership_mode"),
            Some(&json!("split_server_client"))
        );
        assert_eq!(
            at(&projected, "ui_bridge.replaceable_ui"),
            Some(&json!(true))
        );
        // nothing else survives at the levels the selector addressed
        assert_eq!(at(&projected, "ui_bridge.server_executable"), None);
        assert_eq!(at(&projected, "ui_bridge.nested"), None);
        assert_eq!(at(&projected, "command_catalog"), None);
        assert_eq!(at(&projected, "build_identity"), None);
    }

    #[test]
    fn retained_values_are_byte_identical_under_the_product_serializer() {
        // "present" is absent at the root: the projection must not invent it
        let selector = Selector::parse("build_identity,ui_bridge.nested,present").unwrap();
        let original = document();
        let projected = selector.project(&original);
        assert_eq!(
            serde_json::to_string_pretty(projected.get("build_identity").unwrap()).unwrap(),
            serde_json::to_string_pretty(original.get("build_identity").unwrap()).unwrap()
        );
        assert_eq!(
            serde_json::to_string_pretty(projected["ui_bridge"].get("nested").unwrap()).unwrap(),
            serde_json::to_string_pretty(original["ui_bridge"].get("nested").unwrap()).unwrap()
        );
        assert!(
            serde_json::to_string_pretty(&projected).unwrap().len()
                <= serde_json::to_string_pretty(&original).unwrap().len()
        );
    }

    #[test]
    fn an_absent_path_stays_absent_and_is_not_an_error() {
        let selector = Selector::parse("ui_bridge.ownership_mode,error.code,wait").unwrap();
        let projected = selector.project(&document());
        assert_eq!(
            serde_json::to_string_pretty(&projected).unwrap(),
            "{\n  \"ui_bridge\": {\n    \"ownership_mode\": \"split_server_client\"\n  }\n}"
        );
    }

    #[test]
    fn element_walks_cover_root_arrays_and_nested_arrays() {
        let root = json!([
            { "endpoint": "tcp:127.0.0.1:1", "lease_nonce": "n1", "extra": "dropped" },
            { "endpoint": "unix:/tmp/x.sock", "lease_nonce": "n2", "extra": "dropped" }
        ]);
        let selector = Selector::parse("[].endpoint").unwrap();
        let projected = selector.project(&root);
        assert_eq!(projected.as_array().unwrap().len(), 2);
        assert_eq!(
            at(&projected, "[].endpoint"),
            Some(&json!("tcp:127.0.0.1:1"))
        );
        assert_eq!(at(&projected, "[].lease_nonce"), None);

        let tabs = document();
        let selector = Selector::parse("tabs[].id,tabs[].render.text.width").unwrap();
        let projected = selector.project(&tabs);
        assert_eq!(at(&projected, "tabs[].id"), Some(&json!("@1")));
        assert_eq!(at(&projected, "tabs[].render.text.width"), Some(&json!(3)));
        assert_eq!(at(&projected, "tabs[].render.text.height"), None);
        assert_eq!(at(&projected, "tabs[].note"), None);
    }

    #[test]
    fn a_type_mismatch_keeps_the_value_whole_rather_than_dropping_it() {
        // `tabs` is not an array here: the walk cannot descend, so the member is
        // kept as the producer wrote it.
        let value = json!({ "tabs": { "shape": "not-an-array" }, "pid": 1 });
        let selector = Selector::parse("tabs[].id").unwrap();
        let projected = selector.project(&value);
        assert_eq!(at(&projected, "tabs.shape"), Some(&json!("not-an-array")));
        assert_eq!(at(&projected, "pid"), None);
    }

    #[test]
    fn a_shorter_path_wins_over_a_deeper_one() {
        let selector = Selector::parse("ui_bridge.nested.deep,ui_bridge.nested").unwrap();
        let projected = selector.project(&document());
        assert_eq!(
            at(&projected, "ui_bridge.nested.other"),
            Some(&json!(1)),
            "the shorter path keeps the whole subtree"
        );
        assert_eq!(selector.path_count(), 1);
    }

    #[test]
    fn duplicate_paths_collapse() {
        let selector = Selector::parse("pid,pid , pid").unwrap();
        assert_eq!(selector.path_count(), 1);
    }

    #[test]
    fn malformed_selectors_are_named_and_refused() {
        assert_eq!(Selector::parse("").unwrap_err(), SelectorError::Empty);
        assert_eq!(
            Selector::parse("pid,").unwrap_err(),
            SelectorError::EmptySegment {
                path: String::new()
            }
        );
        assert_eq!(
            Selector::parse("pid..name").unwrap_err(),
            SelectorError::EmptySegment {
                path: "pid..name".to_owned()
            }
        );
        assert_eq!(
            Selector::parse("ui_bridge.*").unwrap_err(),
            SelectorError::UnsupportedCharacter {
                path: "ui_bridge.*".to_owned(),
                character: '*'
            }
        );
        assert_eq!(
            Selector::parse("a?b").unwrap_err(),
            SelectorError::UnsupportedCharacter {
                path: "a?b".to_owned(),
                character: '?'
            }
        );
        assert_eq!(
            Selector::parse("[1].id").unwrap_err(),
            SelectorError::UnsupportedCharacter {
                path: "[1].id".to_owned(),
                character: '['
            }
        );
        assert_eq!(
            Selector::parse("a.b.c.d.e.f.g.h.i").unwrap_err(),
            SelectorError::TooDeep {
                path: "a.b.c.d.e.f.g.h.i".to_owned(),
                limit: SELECTOR_MAX_DEPTH
            }
        );
        assert!(matches!(
            Selector::parse(&"x".repeat(SELECTOR_MAX_BYTES + 1)).unwrap_err(),
            SelectorError::TooLong { .. }
        ));
        let too_many = (0..SELECTOR_MAX_PATHS + 1)
            .map(|index| format!("key{index}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(matches!(
            Selector::parse(&too_many).unwrap_err(),
            SelectorError::TooManyPaths { .. }
        ));
    }

    #[test]
    fn a_selector_that_matches_nothing_yields_an_empty_object_and_is_not_an_error() {
        // Every named path is absent, so the projection is the empty object.
        // That is deliberately *not* byte-identical to the source: the caller
        // asked for fields and named none that exist. What it must never do is
        // fail, or invent `error.code` and turn a typo into a refusal.
        let selector = Selector::parse("nothing.here").unwrap();
        let projected = serde_json::to_string_pretty(&selector.project(&document())).unwrap();
        assert_eq!(projected, "{}");
    }

    #[test]
    fn a_retained_subtree_is_byte_identical_to_the_source_and_never_grows() {
        // Byte preservation is a real guarantee on what the caller keeps *whole*:
        // a path retained as a leaf keeps its exact spelling and order. A
        // partially-selected subtree is not equal to its source, which is the
        // point of selecting.
        let original = document();
        let source = serde_json::to_string_pretty(&original).unwrap();

        // `path` is named whole, so the projection must equal the source there.
        let whole = [
            ("build_identity", "build_identity"),
            ("tabs", "tabs"),
            ("pid", "pid"),
            ("ui_bridge.nested", "ui_bridge.nested"),
            ("tabs[].id", "tabs"),
            (
                "missing.one,ui_bridge.replaceable_ui",
                "ui_bridge.replaceable_ui",
            ),
        ];
        for (text, path) in whole {
            let projected = Selector::parse(text).unwrap().project(&original);
            let rendered = serde_json::to_string_pretty(&projected).unwrap();
            assert!(
                rendered.len() <= source.len(),
                "`{text}` grew the document: {} > {}",
                rendered.len(),
                source.len()
            );
            let kept = at(&projected, path);
            assert!(kept.is_some(), "`{text}` dropped `{path}`");
            // For element walks only the selected member survives, so compare
            // the member across elements rather than the whole array.
            if text.contains("[]") {
                continue;
            }
            assert_eq!(
                serde_json::to_string_pretty(kept.unwrap()).unwrap(),
                serde_json::to_string_pretty(at(&original, path).unwrap()).unwrap(),
                "`{text}` re-spelled `{path}`"
            );
        }

        // A selector that retains nothing still shrinks and never errors.
        let projected = Selector::parse("nothing.here").unwrap().project(&original);
        assert_eq!(serde_json::to_string_pretty(&projected).unwrap(), "{}");
    }

    #[test]
    fn a_selection_without_json_is_refused_and_one_with_json_is_applied() {
        let document = document();
        let named = |args: &[&str]| args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();

        assert_eq!(
            apply_selection_request(&document, &named(&["protocol-info"])),
            Ok(None),
            "no selection means the producer's own document"
        );
        assert_eq!(
            apply_selection_request(&document, &named(&["protocol-info", "--select", "pid"])),
            Err(SelectionRefusal {
                code: SELECT_REQUIRES_JSON_CODE,
                message: format!(
                    "{SELECT_FLAG} requires {SELECT_REQUIRES_FLAG}: this command always answers \
                     JSON, and the machine-output flag is the contract that says the caller \
                     wants named fields rather than text"
                ),
            })
        );
        let projected = apply_selection_request(
            &document,
            &named(&["protocol-info", "--json", "--select", "pid"]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(projected, json!({ "pid": 4242 }));
        assert_eq!(
            apply_selection_request(
                &document,
                &named(&["protocol-info", "--json", "--select", "pid.*"]),
            )
            .unwrap_err()
            .code,
            SELECT_MALFORMED_CODE
        );
    }

    #[test]
    fn inherent_json_selection_needs_no_format_flag_and_keeps_the_same_grammar() {
        let args = vec![
            "ui-snapshot".to_owned(),
            "--select".to_owned(),
            "event_position".to_owned(),
        ];
        let value = json!({"event_position": {"epoch": "e", "sequence": 7}, "tabs": [1]});
        assert_eq!(
            apply_inherent_json_selection_request(&value, &args).unwrap(),
            Some(json!({"event_position": {"epoch": "e", "sequence": 7}}))
        );

        let malformed = vec![
            "ui-snapshot".to_owned(),
            "--select".to_owned(),
            "tabs.*".to_owned(),
        ];
        assert_eq!(
            apply_inherent_json_selection_request(&value, &malformed)
                .unwrap_err()
                .code,
            SELECT_MALFORMED_CODE
        );
    }

    #[test]
    fn the_error_code_is_stable() {
        assert_eq!(
            Selector::parse("").unwrap_err().code(),
            "selection_malformed"
        );
    }
}
