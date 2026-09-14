//! `wait`: window-inventory conditions, `--node-name-contains`,
//! `--text-equals` / `--text-contains` (independent `Text.GetText`), and
//! `--expect` (the `verify` matcher polled).

use super::*;
use crate::execution_control::ExecutionControl;

/// Slice width for the inter-round pause. Cancellation is observed between these
/// slices, so the worst-case response to a token set during a pause is one slice
/// plus the pause remainder -- never the whole 120 s deadline. Same shape as the
/// shipped terminal-wait pause; no thread, signal or async runtime is added.
const WAIT_CANCEL_SLICE: Duration = Duration::from_millis(10);

/// The inter-round pause, sliced so the borrowed token is observed several times
/// instead of once. The total pause is unchanged at 50 ms.
///
/// It returns a private signal and NEVER builds an error. After the first authority
/// call of a wait, cancellation is only a request to stop: whether the outcome is
/// the ordinary timeout or a shaped partial is decided by the loop owner, which is
/// the only place that knows whether an observation was already accumulated.
fn wait_pause_cancelled(control: ExecutionControl<'_>) -> bool {
    let pause_deadline = Instant::now() + Duration::from_millis(50);
    while Instant::now() < pause_deadline {
        if control.is_cancelled() {
            return true;
        }
        thread::sleep(
            WAIT_CANCEL_SLICE.min(pause_deadline.saturating_duration_since(Instant::now())),
        );
    }
    control.is_cancelled()
}

/// The post-baseline cancellation outcome: a named `cancelled` failure whose
/// structured detail carries the bounded partial evidence the verb had already
/// accumulated.
///
/// `effect: partially_performed` is the truthful claim, because reaching any of the
/// call sites below required at least one authority call. The ONLY place
/// `effect: not_performed` remains legal is the single hoisted check that runs before
/// the first authority call of each variant.
fn wait_cancelled_partial(partial_observation: serde_json::Value) -> CuError {
    CuError::new(
        "cancelled",
        "the wait was cancelled after observation began",
    )
    .with_detail(serde_json::json!({
        "effect": "partially_performed",
        "phase": "observe_wait",
        "partial_observation": partial_observation,
    }))
}

pub(super) fn wait(
    timeout_ms: u64,
    condition: &WaitCondition,
    control: ExecutionControl<'_>,
) -> Result<serde_json::Value, CuError> {
    match condition {
        WaitCondition::Expect {
            window,
            expect,
            absent,
        } => {
            return wait_expect(timeout_ms, *window, expect, *absent, control);
        }
        WaitCondition::NodeNameContains {
            pattern,
            role,
            window,
        } => return wait_node(timeout_ms, pattern, role.as_deref(), *window, control),
        WaitCondition::NodeTextEquals {
            expected,
            name,
            role,
            window,
        } => {
            return wait_node_text(
                timeout_ms,
                expected,
                name,
                role.as_deref(),
                *window,
                NodeTextMatch::Equals,
                control,
            );
        }
        WaitCondition::NodeTextContains {
            substring,
            name,
            role,
            window,
        } => {
            return wait_node_text(
                timeout_ms,
                substring,
                name,
                role.as_deref(),
                *window,
                NodeTextMatch::Contains,
                control,
            );
        }
        WaitCondition::ReadyPath { path } => return wait_ready_path(timeout_ms, path, control),
        _ => {}
    }
    // Production passes the real mechanism; the generic parameter is what lets an
    // owning test prove that a zero-bounded wait performs NO authority read.
    fn real_window() -> Result<Vec<WindowInfo>, CuError> {
        mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)
    }
    wait_window_with_reader(timeout_ms, condition, control, real_window)
}

/// The `wait` window-condition loop, GENERIC over its window reader.
///
/// The reader is a generic `Fn` parameter rather than a trait object or a type alias,
/// so a caller's closure can borrow its own locals without a `'static` bound while
/// production keeps passing the real mechanism and its exact mapping.
fn wait_window_with_reader<R>(
    timeout_ms: u64,
    condition: &WaitCondition,
    control: ExecutionControl<'_>,
    read_windows: R,
) -> Result<serde_json::Value, CuError>
where
    R: Fn() -> Result<Vec<WindowInfo>, CuError>,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let mut last_observation = serde_json::json!({ "windows": [] });

    // ZERO-BOUND SEMANTICS PRESERVED. The shipped loop was `while Instant::now() <
    // deadline`, so a `--timeout-ms 0` wait performed NO authority read and NO
    // cancellation check and returned the ordinary `met:false` object directly. That
    // is the public behaviour and it is restored here: an already-reached bound is
    // decided BEFORE any direct check or authority call, so a pre-set token can never
    // turn a zero-bounded wait into a `cancelled` reply.
    if Instant::now() >= deadline {
        return Ok(serde_json::json!({
            "met": false,
            "timeout_ms": timeout_ms,
            "observation": last_observation,
        }));
    }

    // THE ONLY DIRECT CHECK for this variant, and therefore the only place
    // `effect: not_performed` is legal. It is hoisted OUT of the loop, so it runs
    // exactly once, strictly before the first window read; every later boundary is a
    // private signal handled below. From round 2 onward an accumulated
    // `last_observation` already exists, so a loop-top authority-bearing check there
    // would have fabricated a `not_performed` claim.
    control.check_observe()?;
    loop {
        let windows = read_windows()?;
        if matches!(condition, WaitCondition::WindowTitleContains { .. })
            && observe::window_titles_unavailable(&windows)
        {
            // A typed authority refusal is authoritative: returned before any
            // cancellation is consulted again, so a late cancel cannot mask it.
            return Err(window_titles_unavailable_error(windows.len()));
        }
        last_observation = serde_json::json!({ "window_count": windows.len(), "windows": windows });
        if condition_met(condition, &windows) {
            // A matched condition is the authoritative result and wins.
            return Ok(serde_json::json!({
                "met": true,
                "observation": last_observation,
            }));
        }
        if Instant::now() >= deadline {
            break;
        }
        // DEADLINE FIRST: a reached bound stays the authoritative outcome even when
        // the final slice saw the token.
        if wait_pause_cancelled(control) {
            if Instant::now() >= deadline {
                break;
            }
            return Err(wait_cancelled_partial(serde_json::json!({
                "met": false,
                "timeout_ms": timeout_ms,
                "observation": last_observation,
            })));
        }
    }

    Ok(serde_json::json!({
        "met": false,
        "timeout_ms": timeout_ms,
        "observation": last_observation,
    }))
}

pub(super) fn condition_met(condition: &WaitCondition, windows: &[WindowInfo]) -> bool {
    match condition {
        WaitCondition::WindowCountGte { count } => windows.len() >= *count,
        WaitCondition::WindowTitleContains { pattern } => windows
            .iter()
            .any(|window| observe::window_title_contains(&window.title, pattern)),
        WaitCondition::FocusedHandle { handle } => windows
            .iter()
            .any(|window| window.focused && window.handle == *handle),
        // Polled against the accessibility tree, not the window list.
        WaitCondition::Expect { .. }
        | WaitCondition::NodeNameContains { .. }
        | WaitCondition::NodeTextEquals { .. }
        | WaitCondition::NodeTextContains { .. }
        | WaitCondition::ReadyPath { .. } => false,
    }
}

/// Polls `tree` until exactly one showing node whose name contains `pattern`
/// (and whose role matches `role`, when given) appears. Two or more showing
/// hits fail typed (`a11y_node_ambiguous`) instead of taking the first.
/// Timeout is a typed failure so loop-until callers break on `ok:false`
/// instead of retrying blind.
pub(super) fn wait_node(
    timeout_ms: u64,
    pattern: &str,
    role: Option<&str>,
    window: Option<isize>,
    control: ExecutionControl<'_>,
) -> Result<serde_json::Value, CuError> {
    // Production passes the real mechanism; the generic parameter is what lets an
    // owning test drive THIS loop and flip the borrowed token from inside a round.
    fn real(window: Option<isize>) -> Result<mechanism::A11yTree, CuError> {
        mechanism::tree_for_window(window).map_err(map_mechanism_err)
    }
    wait_node_with_reader(timeout_ms, pattern, role, window, control, real)
}

/// The `wait_node` loop, GENERIC over its tree reader.
///
/// The reader is a generic `Fn` parameter rather than a trait object or a type
/// alias, so a caller's closure can borrow its own locals without a `'static`
/// bound and production keeps passing the real mechanism.
fn wait_node_with_reader<R>(
    timeout_ms: u64,
    pattern: &str,
    role: Option<&str>,
    window: Option<isize>,
    control: ExecutionControl<'_>,
    read_tree: R,
) -> Result<serde_json::Value, CuError>
where
    R: Fn(Option<isize>) -> Result<mechanism::A11yTree, CuError>,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let mut polls = 0usize;
    let mut last_node_count = 0usize;
    let mut last_error: Option<CuError> = None;

    // THE ONLY DIRECT CHECK for this variant; hoisted so it runs exactly once before
    // the first tree read. Every later boundary is a private signal, because from
    // round 2 on this variant already holds `last_node_count` / `last_error`.
    control.check_observe()?;
    loop {
        polls += 1;
        match read_tree(window) {
            Ok(tree) => {
                last_node_count = tree.nodes.len();
                last_error.take();
                let matches = showing_name_matches(&tree.nodes, pattern, role);
                match matches.len() {
                    0 => {}
                    1 => {
                        return Ok(serde_json::json!({
                            "met": true,
                            "addressing": "accessibility-tree",
                            "mechanism": "libagenterm",
                            "backend": tree.backend,
                            "window": window,
                            "polls": polls,
                            "node": matches[0],
                            "observation": { "node_count": last_node_count },
                        }));
                    }
                    count => return Err(name_match_error(pattern, role, count)),
                }
            }
            // The tree can be missing outright; that is not something more
            // polling will fix.
            Err(error) if error.code == "unsupported" => {
                // The shipped reader REPLACED the reason here, so the substitution (not
                // the provider's own text) is the published message. Preserved verbatim
                // so the refusal wire stays byte-identical.
                let _ = error;
                return Err(CuError::new(
                    "unsupported",
                    "accessibility-tree mechanism unavailable",
                ));
            }
            // A scoped window may not have an AT-SPI root yet — keep polling and
            // report the last failure if we run out of time.
            Err(error) => last_error = Some(error),
        }
        if Instant::now() >= deadline {
            break;
        }
        if wait_pause_cancelled(control) {
            if Instant::now() >= deadline {
                break;
            }
            // The accumulator is published as DATA here, because the ordinary timeout
            // path below keeps only a formatted message and DROPS the typed error code.
            // Nothing on the existing wire changes; this partial is additive.
            return Err(wait_cancelled_partial(serde_json::json!({
                "polls": polls,
                "node_count": last_node_count,
                "last_error_code": last_error.as_ref().map(|error| error.code.clone()),
            })));
        }
    }

    let detail = match last_error {
        Some(error) => format!("last tree read failed: {} ({})", error.message, error.code),
        None => format!("last tree read had {last_node_count} nodes"),
    };
    Err(CuError::new(
        "timeout",
        format!(
            "no showing accessibility node with {} after {timeout_ms}ms ({polls} polls, {detail})",
            name_scope(pattern, role)
        ),
    ))
}

/// Polls until `path` carries a schema-1 readiness marker with
/// `state: "ready"`. Compatible with `observe --ready-path` and any
/// other atomic publisher; partial JSON keeps polling.
pub(super) fn wait_ready_path(
    timeout_ms: u64,
    path: &str,
    control: ExecutionControl<'_>,
) -> Result<serde_json::Value, CuError> {
    wait_ready_path_with_reader(timeout_ms, path, control, |path| {
        Ok(super::a11y_observe::read_ready_marker(path))
    })
}

fn wait_ready_path_with_reader(
    timeout_ms: u64,
    path: &str,
    control: ExecutionControl<'_>,
    mut read_marker: impl FnMut(&str) -> Result<Option<serde_json::Value>, CuError>,
) -> Result<serde_json::Value, CuError> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let mut polls = 0usize;
    // THE ONLY DIRECT CHECK for this variant; hoisted so it runs exactly once before
    // the first marker read. Every later boundary is a private signal, because from
    // round 2 on this variant already holds a poll count.
    control.check_observe()?;
    loop {
        polls += 1;
        if let Some(marker) = read_marker(path)? {
            // The marker is the authoritative result and wins.
            return Ok(serde_json::json!({
                "met": true,
                "addressing": "ready-path",
                "polls": polls,
                "marker": marker,
            }));
        }
        if Instant::now() >= deadline {
            break;
        }
        // A cancellation after at least one marker read is NOT `not_performed`: a real
        // authority read was issued and answered, and the timeout message below already
        // publishes the poll count as evidence of that work.
        if wait_pause_cancelled(control) {
            if Instant::now() >= deadline {
                break;
            }
            return Err(wait_cancelled_partial(
                serde_json::json!({ "polls": polls }),
            ));
        }
    }
    Err(CuError::new(
        "timeout",
        format!(
            "readiness marker at {path:?} did not reach state \"ready\" after {timeout_ms}ms ({polls} polls)"
        ),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NodeTextMatch {
    Equals,
    Contains,
}

impl NodeTextMatch {
    fn flag(self) -> &'static str {
        match self {
            Self::Equals => "--text-equals",
            Self::Contains => "--text-contains",
        }
    }

    fn matches(self, text: &str, expected: &str) -> bool {
        match self {
            Self::Equals => text == expected,
            Self::Contains => text.contains(expected),
        }
    }

    fn timeout_verb(self) -> &'static str {
        match self {
            Self::Equals => "did not reach text",
            Self::Contains => "did not contain",
        }
    }
}

/// Polls AT-SPI `Text.GetText` (`agt_a11y_node_get_text`) on the unique
/// showing node addressed by `name` until that independent text equals
/// `expected` (`--text-equals`) or contains it (`--text-contains`). The
/// tree snapshot `node.text`, a prior `send-text` / `paste` / `copy`
/// `matched.text`, `last_text_write_via`, and the WebKit eval helper's
/// queued-job `OK` (Reasonix composer) are not this predicate. Timeout
/// is typed so loop-until callers break on `ok:false`.
pub(super) fn wait_node_text(
    timeout_ms: u64,
    expected: &str,
    name: &str,
    role: Option<&str>,
    window: Option<isize>,
    match_kind: NodeTextMatch,
    control: ExecutionControl<'_>,
) -> Result<serde_json::Value, CuError> {
    if window.is_none() {
        return Err(CuError::new(
            "invalid_input",
            format!("wait {} requires --window <handle>", match_kind.flag()),
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let mut polls = 0usize;
    let mut last_node_count = 0usize;
    let mut last_text: Option<String> = None;
    let mut last_error: Option<CuError> = None;

    // THE ONLY DIRECT CHECK for this variant; hoisted so it runs exactly once before
    // the first tree/text authority call. Every later boundary is a private signal,
    // because from round 2 on this variant already holds text and error state.
    control.check_observe()?;
    loop {
        polls += 1;
        match mechanism::tree_for_window(window) {
            Ok(tree) => {
                last_node_count = tree.nodes.len();
                last_error.take();
                let matches = showing_name_matches(&tree.nodes, name, role);
                match matches.len() {
                    0 => {}
                    1 => match mechanism::get_node_text(window, &matches[0].id) {
                        Ok(text) => {
                            last_text = Some(text.clone());
                            if match_kind.matches(&text, expected) {
                                return Ok(text_equals_success(
                                    &tree.backend,
                                    window,
                                    polls,
                                    matches[0],
                                    &text,
                                    last_node_count,
                                ));
                            }
                        }
                        Err(error @ mechanism::MechanismError::Unsupported { .. }) => {
                            return Err(map_mechanism_err(error));
                        }
                        Err(error) => last_error = Some(map_mechanism_err(error)),
                    },
                    count => return Err(name_match_error(name, role, count)),
                }
            }
            Err(error @ mechanism::MechanismError::Unsupported { .. }) => {
                return Err(map_mechanism_err(error));
            }
            Err(error) => last_error = Some(map_mechanism_err(error)),
        }
        if Instant::now() >= deadline {
            break;
        }
        if wait_pause_cancelled(control) {
            if Instant::now() >= deadline {
                break;
            }
            return Err(wait_cancelled_partial(serde_json::json!({
                "polls": polls,
                "node_count": last_node_count,
                "text": last_text,
                "last_error_code": last_error.as_ref().map(|error| error.code.clone()),
            })));
        }
    }

    Err(CuError::new(
        "timeout",
        format!(
            "accessibility node with {} {} {expected:?} after {timeout_ms}ms ({polls} polls, {})",
            name_scope(name, role),
            match_kind.timeout_verb(),
            text_equals_timeout_detail(last_text.as_deref(), last_error.as_ref(), last_node_count,)
        ),
    ))
}

/// Success payload for `--text-equals` / `--text-contains`. `gettext` is
/// the only text authority: snapshot `node.text` is overwritten so a
/// sidecar tree walk or `send-text` / `paste` `matched.text` cannot be
/// mistaken for the hit. Published `text` is the full independent GetText.
pub(super) fn text_equals_success(
    backend: &str,
    window: Option<isize>,
    polls: usize,
    node: &mechanism::A11yNode,
    gettext: &str,
    node_count: usize,
) -> serde_json::Value {
    let mut node = node.clone();
    node.text = Some(gettext.to_owned());
    serde_json::json!({
        "met": true,
        "addressing": "accessibility-tree",
        "mechanism": "libagenterm",
        "backend": backend,
        "window": window,
        "polls": polls,
        "node": node,
        "text": gettext,
        "via": "gettext",
        "observation": {
            "node_count": node_count,
            "text": gettext,
        },
    })
}

pub(super) fn text_equals_timeout_detail(
    last_text: Option<&str>,
    last_error: Option<&CuError>,
    last_node_count: usize,
) -> String {
    match (last_text, last_error) {
        (Some(text), _) => format!("last GetText={text:?}"),
        (None, Some(error)) => {
            format!("last GetText failed: {} ({})", error.message, error.code)
        }
        (None, None) => format!("last tree read had {last_node_count} nodes"),
    }
}

/// Poll the same matcher until every expectation is met. A missing node
/// keeps polling; ambiguity and an unobservable state fail closed at once.
pub(super) fn wait_expect(
    timeout_ms: u64,
    window: isize,
    expect: &[crate::command::Expectation],
    absent: bool,
    control: ExecutionControl<'_>,
) -> Result<serde_json::Value, CuError> {
    // Production passes the real readers; the generic parameters are what let an
    // owning test prove the pre-effect cancel performs NO authority read at all, and
    // what let it observe the real first-round order instead of inferring it.
    fn real_foreground() -> Result<ForegroundIdentity, CuError> {
        current_foreground_identity()
    }
    fn real_tree(window: isize) -> Result<mechanism::A11yTree, CuError> {
        mechanism::tree_for_window(Some(window)).map_err(map_mechanism_err)
    }
    wait_expect_with_readers(
        timeout_ms,
        window,
        expect,
        absent,
        control,
        real_foreground,
        real_tree,
    )
}

/// The `wait_expect` loop, GENERIC over its foreground-identity reader and its tree
/// reader.
///
/// Both are generic `Fn` parameters rather than trait objects or a type alias, so a
/// caller's closure can borrow its own locals without a `'static` bound while
/// production keeps passing the real mechanisms.
///
/// The tree adapter must preserve the shipped `MechanismError` mapping and its branch
/// semantics exactly, including the `denied` pass-through and the absent/positive
/// split in the trailing arm.
fn wait_expect_with_readers<G, T>(
    timeout_ms: u64,
    window: isize,
    expect: &[crate::command::Expectation],
    absent: bool,
    control: ExecutionControl<'_>,
    read_foreground: G,
    read_tree: T,
) -> Result<serde_json::Value, CuError>
where
    G: Fn() -> Result<ForegroundIdentity, CuError>,
    T: Fn(isize) -> Result<mechanism::A11yTree, CuError>,
{
    if window == 0 {
        return Err(invalid_input(
            "wait --expect requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    if expect.is_empty() {
        return Err(invalid_input(
            "wait requires a non-empty --expect array".into(),
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(120_000));
    let mut polls = 0usize;
    let mut last_complete: Option<serde_json::Value> = None;
    // THE ONLY DIRECT CHECK for this variant, and therefore the only place
    // `effect: not_performed` is legal. The local window/expect validation above runs
    // first so a malformed request keeps its inherent refusal, and the check then runs
    // BEFORE the foreground read, because `current_foreground_identity` performs REAL
    // authority observation (it enumerates top-level windows and stacking to resolve
    // the frontmost app). Claiming `not_performed` after that read would be false.
    // Every later boundary is a private signal.
    control.check_observe()?;
    let foreground = if absent {
        Some(read_foreground()?)
    } else {
        None
    };
    loop {
        polls += 1;
        match read_tree(window) {
            Ok(tree) => {
                require_complete_absence_observation(absent, window, tree.visited, tree.truncated)?;
                let flat = observe::flatten(&tree);
                let (results, goal_met) = evaluate_expectations(&flat, expect, absent)?;
                if let Some(before) = foreground.as_ref() {
                    // The same provider is used for the bracketing revalidation, so a
                    // test can drive a same-round foreground change through one seam.
                    let after = read_foreground()?;
                    require_same_foreground(before, &after)?;
                }
                let observation = serde_json::json!({
                    "backend": tree.backend,
                    "visited": tree.visited,
                    "truncated": tree.truncated,
                    "results": results,
                });
                last_complete = Some(observation.clone());
                if goal_met {
                    return Ok(serde_json::json!({
                        "met": true,
                        "verified": true,
                        "absent": absent,
                        "addressing": "accessibility-tree",
                        "mechanism": "libagenterm",
                        "window": window,
                        "polls": polls,
                        "foreground_unchanged": absent.then_some(true),
                        "observation": observation,
                    }));
                }
            }
            Err(error) if error.code == "unsupported" => {
                // The shipped reader mapped an unsupported tree through this exact
                // substitution, and the refusal is authoritative before any pause.
                let _ = error;
                return Err(CuError::new(
                    "unsupported",
                    "accessibility-tree mechanism unavailable",
                ));
            }
            Err(error) => {
                // Unchanged branch semantics: a denial is authoritative at once, while
                // any other failure keeps polling; a positive wait records it as the
                // last tree error, while an absence wait retains no incomplete sample.
                if error.code == "denied" {
                    return Err(error);
                }
                if !absent {
                    last_complete =
                        Some(serde_json::json!({ "tree_error": error_payload(&error) }));
                }
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        if wait_pause_cancelled(control) {
            if Instant::now() >= deadline {
                break;
            }
            return Err(wait_cancelled_partial(serde_json::json!({
                "absent": absent,
                "observation": last_complete,
            })));
        }
    }
    Err(CuError::new(
        "timeout",
        format!(
            "expectations did not become {} after {timeout_ms}ms ({polls} polls)",
            if absent { "absent" } else { "met" }
        ),
    )
    .with_detail(serde_json::json!({
        "absent": absent,
        "observation": last_complete,
    })))
}

fn window_titles_unavailable_error(window_count: usize) -> CuError {
    CuError::new(
        "unverified",
        format!(
            "window inventory reports {window_count} top-level window(s) but every title is empty; --window-title-contains cannot match"
        ),
    )
    .with_detail(serde_json::json!({
        "reason": "window_titles_unavailable",
        "window_count": window_count,
        "alternatives": [
            "windows --pid / --app (resolve a handle without a title)",
            "wait --focused-handle HANDLE",
            "wait --node-name-contains PAT [--window HANDLE] (AT-SPI --name, not window title)",
        ],
    }))
}

fn require_complete_absence_observation(
    absent: bool,
    window: isize,
    visited: usize,
    truncated: bool,
) -> Result<(), CuError> {
    if !absent || !truncated {
        return Ok(());
    }
    Err(CuError::new(
        "unverified",
        "wait --absent requires a complete accessibility-tree observation",
    )
    .with_detail(serde_json::json!({
        "reason": "observation_truncated",
        "window": window,
        "visited": visited,
        "truncated": true,
    })))
}

fn evaluate_expectations(
    flat: &[observe::FlatNode<'_>],
    expect: &[crate::command::Expectation],
    absent: bool,
) -> Result<(Vec<serde_json::Value>, bool), CuError> {
    let mut results = Vec::with_capacity(expect.len());
    let mut goal_met = true;
    for expectation in expect {
        let verdict = check_one(flat, expectation)?;
        if verdict.unknown {
            return Err(CuError::new(
                "unsupported",
                "an expected state is not observable on its node; more polling cannot make it so",
            )
            .with_detail(
                serde_json::json!({ "reason": "state_unobservable", "item": verdict.item }),
            ));
        }
        goal_met &= if absent { !verdict.met } else { verdict.met };
        results.push(verdict.item);
    }
    Ok((results, goal_met))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ForegroundIdentity {
    window_handle: isize,
    process_id: u32,
    app_name: String,
    app: Option<observe::FrontmostApp>,
}

fn current_foreground_identity() -> Result<ForegroundIdentity, CuError> {
    let mut rows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let stacking = mechanism::window_enumerate::stacking().unwrap_or_default();
    let focus = windows::resolve_inventory_focus(&mut rows, &stacking);
    let handle = focus.handle.ok_or_else(|| {
        CuError::new(
            "unverified",
            "wait --absent requires one exact observable foreground window",
        )
        .with_detail(serde_json::json!({
            "reason": "foreground_unobservable",
            "focus": focus.json(),
        }))
    })?;
    let window = rows
        .into_iter()
        .find(|row| row.handle == handle)
        .ok_or_else(|| {
            CuError::new(
                "unverified",
                "resolved foreground window was absent from its inventory",
            )
            .with_detail(serde_json::json!({ "reason": "foreground_unobservable" }))
        })?;
    if focus
        .app
        .as_ref()
        .is_some_and(|app| app.pid != window.process_id)
    {
        return Err(CuError::new(
            "unverified",
            "foreground application and window identities disagree",
        )
        .with_detail(serde_json::json!({ "reason": "foreground_ambiguous" })));
    }
    Ok(ForegroundIdentity {
        window_handle: window.handle,
        process_id: window.process_id,
        app_name: window.app_name,
        app: focus.app,
    })
}

fn require_same_foreground(
    before: &ForegroundIdentity,
    after: &ForegroundIdentity,
) -> Result<(), CuError> {
    if before == after {
        return Ok(());
    }
    Err(CuError::new(
        "foreground_changed",
        "desktop foreground identity changed while wait --absent was observing",
    )
    .with_detail(serde_json::json!({
        "before": foreground_json(before),
        "after": foreground_json(after),
    })))
}

fn foreground_json(identity: &ForegroundIdentity) -> serde_json::Value {
    serde_json::json!({
        "window": {
            "handle": identity.window_handle,
            "process_id": identity.process_id,
            "app_name": identity.app_name,
        },
        "app": identity.app.as_ref().map(observe::FrontmostApp::json),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn empty_tree_with_nodes(count: usize) -> mechanism::A11yTree {
        mechanism::A11yTree {
            backend: "fixture".into(),
            window_handle: Some(7),
            root_id: "/0".into(),
            nodes: (0..count)
                .map(|i| a11y_node(&format!("/{i}"), "elsewhere", "button", &[]))
                .collect(),
            truncated: false,
            visited: count,
            returned: count,
        }
    }

    fn tree_with_showing_node(name: &str) -> mechanism::A11yTree {
        mechanism::A11yTree {
            backend: "fixture".into(),
            window_handle: Some(7),
            root_id: "/0".into(),
            nodes: vec![a11y_node("/0", name, "button", &["showing"])],
            truncated: false,
            visited: 1,
            returned: 1,
        }
    }

    #[test]
    fn a_second_round_boundary_token_no_longer_fabricates_not_performed() {
        // THE REGRESSION FOR THE TWO-SIDED DEFECT. The token is flipped by the reader
        // at the END of round 2, so it is observed by the boundary that used to be the
        // loop-top authority-bearing check. By then rounds 1 and 2 have already stored a
        // node count, so the truthful outcome is a partial, never a `not_performed`.
        let token = Cell::new(false);
        let reads = Cell::new(0usize);
        let probe = || token.get();
        let error = wait_node_with_reader(
            30_000,
            "no-such-node",
            None,
            Some(7),
            ExecutionControl::with_cancel_probe(&probe),
            |_| {
                let n = reads.get() + 1;
                reads.set(n);
                if n == 2 {
                    token.set(true);
                }
                Ok(empty_tree_with_nodes(3))
            },
        )
        .expect_err("a post-baseline cancel must refuse the wait");
        assert!(token.get(), "the reader really did flip the token");
        assert_eq!(reads.get(), 2, "no further tree read may happen");
        assert_eq!(error.code, "cancelled");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "partially_performed");
        assert_eq!(detail["phase"], "observe_wait");
        let partial = &detail["partial_observation"];
        assert_eq!(partial["polls"], 2);
        assert_eq!(partial["node_count"], 3);
        assert!(partial["last_error_code"].is_null());
    }

    #[test]
    fn a_first_round_token_on_the_node_variant_still_claims_not_performed() {
        // The single hoisted check is the ONLY place `not_performed` is legal, and it
        // must still issue no tree read at all.
        let reads = Cell::new(0usize);
        let probe = || true;
        let error = wait_node_with_reader(
            30_000,
            "pattern",
            None,
            Some(7),
            ExecutionControl::with_cancel_probe(&probe),
            |_| {
                reads.set(reads.get() + 1);
                unreachable!("a pre-effect cancel must not read the tree")
            },
        )
        .expect_err("a pre-effect cancel must refuse the wait");
        assert_eq!(error.code, "cancelled");
        assert_eq!(reads.get(), 0);
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "not_performed");
        assert!(detail.get("partial_observation").is_none());
    }

    #[test]
    fn a_node_timeout_wire_is_unchanged_and_has_no_partial() {
        // The ordinary timeout keeps its exact code and message and must NOT gain a
        // structured partial, because nothing was cancelled.
        let error = wait_node_with_reader(
            1,
            "no-such-node",
            None,
            Some(7),
            ExecutionControl::none(),
            |_| Ok(empty_tree_with_nodes(0)),
        )
        .expect_err("an unmatched wait must time out");
        assert_eq!(error.code, "timeout");
        assert!(
            error
                .message
                .starts_with("no showing accessibility node with"),
            "unexpected timeout message: {}",
            error.message
        );
        assert!(error.message.contains("last tree read had 0 nodes"));
        assert!(
            error.detail.is_none(),
            "the ordinary timeout wire must not gain a partial"
        );
    }

    #[test]
    fn an_unsupported_tree_still_refuses_with_the_shipped_message() {
        // The reader adapter must preserve the message substitution the shipped code
        // made, so the refusal wire does not move.
        let error = wait_node_with_reader(
            30_000,
            "pattern",
            None,
            Some(7),
            ExecutionControl::none(),
            |_| Err(CuError::new("unsupported", "some provider specific reason")),
        )
        .expect_err("an unsupported tree must refuse");
        assert_eq!(error.code, "unsupported");
        assert_eq!(error.message, "accessibility-tree mechanism unavailable");
    }

    #[test]
    fn a_matched_node_still_wins_over_a_token_flipped_in_that_round() {
        // Same-round authority precedence: the round both finds the node AND flips the
        // token, and the match must win.
        let token = Cell::new(false);
        let probe = || token.get();
        let value = wait_node_with_reader(
            30_000,
            "target",
            None,
            Some(7),
            ExecutionControl::with_cancel_probe(&probe),
            |_| {
                token.set(true);
                Ok(tree_with_showing_node("target"))
            },
        )
        .expect("the matched node must win");
        assert!(token.get(), "the reader really did flip the token");
        assert_eq!(value["met"], true);
        assert_eq!(value["polls"], 1);
    }

    #[test]
    fn ready_path_cancel_after_a_read_is_partially_performed_with_polls() {
        // A ready-path cancellation is NOT `not_performed`: a real marker read was
        // issued and answered, and the partial publishes that poll count as data.
        let token = Cell::new(false);
        let reads = Cell::new(0usize);
        let probe = || token.get();
        let error = wait_ready_path_with_reader(
            30_000,
            "unused",
            ExecutionControl::with_cancel_probe(&probe),
            |_| {
                let n = reads.get() + 1;
                reads.set(n);
                if n == 2 {
                    token.set(true);
                }
                Ok(None)
            },
        )
        .expect_err("the pending token must surface as a partial");
        assert!(token.get(), "the reader really did flip the token");
        assert_eq!(reads.get(), 2);
        assert_eq!(error.code, "cancelled");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "partially_performed");
        assert_eq!(detail["phase"], "observe_wait");
        assert_eq!(detail["partial_observation"]["polls"], 2);
    }

    #[test]
    fn ready_path_pre_cancel_does_no_work() {
        // PRE-EFFECT: a token already set must stop the verb before its first
        // read, not after burning the 120 s deadline.
        let reads = Cell::new(0usize);
        let probe = || true;
        let result = wait_ready_path_with_reader(
            120_000,
            "unused",
            ExecutionControl::with_cancel_probe(&probe),
            |_| {
                reads.set(reads.get() + 1);
                Ok(None)
            },
        );

        let error = result.expect_err("a pre-effect cancel must refuse the verb");
        assert_eq!(error.code, "cancelled");
        assert_eq!(reads.get(), 0, "a pre-cancel must issue no marker read");
        assert!(
            error.detail.as_ref().and_then(|d| d.get("effect"))
                == Some(&serde_json::json!("not_performed")),
            "pre-effect cancel must report that no effect happened: {:?}",
            error.detail
        );
    }

    #[test]
    fn ready_path_cancel_after_an_unmatched_round_stops_before_a_second_poll() {
        // UNMATCHED THEN CANCEL: the first round misses, the token is set during
        // the sliced pause, and the loop must exit without a second round.
        let cancelled = Cell::new(false);
        let probe = || cancelled.get();
        let reads = Cell::new(0usize);
        let result = wait_ready_path_with_reader(
            120_000,
            "unused",
            ExecutionControl::with_cancel_probe(&probe),
            |_| {
                reads.set(reads.get() + 1);
                cancelled.set(true);
                Ok(None)
            },
        );

        let error = result.expect_err("a mid-wait cancel must refuse the verb");
        assert_eq!(error.code, "cancelled");
        assert_eq!(reads.get(), 1, "cancel must prevent a second marker read");
    }

    #[test]
    fn ready_path_returns_a_real_marker_even_if_the_token_is_set_late() {
        // AUTHORITATIVE RESULT OUTRANKS LATE CANCEL: publish a real marker, then
        // hand in a probe that reports cancelled from its very first call. The
        // verb's entry cannot see a token before it is asked, so we instead assert
        // the production precedence directly: a marker present on the round that
        // the pre-effect check allowed must be returned as `met`.
        let cancelled = Cell::new(false);
        let probe = || cancelled.get();
        let reads = Cell::new(0usize);
        let result = wait_ready_path_with_reader(
            120_000,
            "unused",
            ExecutionControl::with_cancel_probe(&probe),
            |_| {
                reads.set(reads.get() + 1);
                cancelled.set(true);
                Ok(Some(
                    serde_json::json!({ "schema": 1, "state": "ready", "window": 7 }),
                ))
            },
        );

        let value = result.expect("a matched round must return its result despite a late token");
        assert_eq!(value.get("met"), Some(&serde_json::json!(true)));
        assert_eq!(value.get("polls"), Some(&serde_json::json!(1)));
        assert_eq!(reads.get(), 1);
    }

    #[test]
    fn ready_path_authority_error_outranks_a_late_cancel() {
        let cancelled = Cell::new(false);
        let probe = || cancelled.get();
        let error = wait_ready_path_with_reader(
            120_000,
            "unused",
            ExecutionControl::with_cancel_probe(&probe),
            |_| {
                cancelled.set(true);
                Err(CuError::new(
                    "ready_path_refused",
                    "injected authority refusal",
                ))
            },
        )
        .expect_err("the authority refusal must outrank a late cancellation");

        assert_eq!(error.code, "ready_path_refused");
    }

    fn a11y_node(id: &str, name: &str, role: &str, states: &[&str]) -> mechanism::A11yNode {
        mechanism::A11yNode {
            id: id.into(),
            parent_id: None,
            role: role.into(),
            subrole: None,
            name: name.into(),
            states: states.iter().map(|state| (*state).into()).collect(),
            bounds: mechanism::A11yBounds {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            actions: Vec::new(),
            text: None,
            identifier: None,
        }
    }

    fn foreground(handle: isize, pid: u32, app_name: &str) -> ForegroundIdentity {
        ForegroundIdentity {
            window_handle: handle,
            process_id: pid,
            app_name: app_name.into(),
            app: None,
        }
    }

    fn expectation(identifier: &str, checked: bool) -> crate::command::Expectation {
        crate::command::Expectation {
            identifier: Some(identifier.into()),
            checked: Some(checked),
            ..crate::command::Expectation::default()
        }
    }

    #[test]
    fn a_zero_bounded_window_wait_keeps_its_ordinary_timeout_and_ignores_a_token() {
        // THE REWORK-3 REGRESSION. The shipped loop was `while now < deadline`, so
        // `--timeout-ms 0` performed no authority read and no cancellation check and
        // returned the ordinary `met:false` object. The window reader PANICS if called,
        // and a pre-set token must NOT rewrite this into a cancellation.
        let window_reads = Cell::new(0usize);
        let probe = || true;
        let value = wait_window_with_reader(
            0,
            &WaitCondition::WindowTitleContains {
                pattern: "anything".into(),
            },
            ExecutionControl::with_cancel_probe(&probe),
            || {
                window_reads.set(window_reads.get() + 1);
                unreachable!("a zero-bounded wait must not read the window inventory")
            },
        )
        .expect("a reached zero bound is the ordinary timeout, not a refusal");
        assert_eq!(window_reads.get(), 0, "no authority call for a zero bound");
        assert_eq!(value["met"], false);
        assert_eq!(value["timeout_ms"], 0);
        assert_eq!(value["observation"], serde_json::json!({ "windows": [] }));
        // Field-for-field the shipped object, with no cancellation vocabulary added.
        assert_eq!(
            value.as_object().map(|object| {
                let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
                keys.sort_unstable();
                keys
            }),
            Some(vec!["met", "observation", "timeout_ms"])
        );
    }

    #[test]
    fn a_zero_bounded_window_wait_still_prefers_a_matched_condition_impossible() {
        // Companion guard: with a zero bound nothing is observed, so the object can only
        // ever be the unmet one -- even though a matched condition would otherwise win.
        let value = wait_window_with_reader(
            0,
            &WaitCondition::WindowTitleContains {
                pattern: "anything".into(),
            },
            ExecutionControl::none(),
            || unreachable!("a zero-bounded wait must not read the window inventory"),
        )
        .expect("a reached zero bound returns the ordinary timeout");
        assert_eq!(value["met"], false);
    }

    #[test]
    fn an_absent_pre_cancel_reads_neither_foreground_nor_tree() {
        // THE REWORK REGRESSION. BOTH providers are observed, not inferred:
        // `current_foreground_identity` enumerates top-level windows and stacking, so
        // it is real authority observation, and the tree reader is an authority read
        // too. A pre-set token must stop the verb before EITHER of them.
        let foreground_reads = Cell::new(0usize);
        let tree_reads = Cell::new(0usize);
        let probe = || true;
        let error = wait_expect_with_readers(
            30_000,
            7,
            &[expectation("fixture-check", true)],
            true,
            ExecutionControl::with_cancel_probe(&probe),
            || {
                foreground_reads.set(foreground_reads.get() + 1);
                unreachable!("a pre-effect cancel must not read the foreground identity")
            },
            |_| {
                tree_reads.set(tree_reads.get() + 1);
                unreachable!("a pre-effect cancel must not read the tree")
            },
        )
        .expect_err("a pre-effect cancel must refuse the wait");
        assert_eq!(error.code, "cancelled");
        assert_eq!(foreground_reads.get(), 0, "no foreground authority read");
        assert_eq!(tree_reads.get(), 0, "no tree authority read");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["effect"], "not_performed");
        assert!(detail.get("partial_observation").is_none());
    }

    #[test]
    fn an_absent_pre_cancel_still_prefers_inherent_validation() {
        // The local window/expect validation stays AHEAD of the single check, so a
        // malformed request keeps its own typed refusal instead of reporting cancelled,
        // and it performs no authority read either.
        let foreground_reads = Cell::new(0usize);
        let tree_reads = Cell::new(0usize);
        let probe = || true;
        let error = wait_expect_with_readers(
            30_000,
            0,
            &[expectation("fixture-check", true)],
            true,
            ExecutionControl::with_cancel_probe(&probe),
            || {
                foreground_reads.set(foreground_reads.get() + 1);
                unreachable!("validation must refuse before any authority read")
            },
            |_| {
                tree_reads.set(tree_reads.get() + 1);
                unreachable!("validation must refuse before any authority read")
            },
        )
        .expect_err("a zero window must be refused by validation");
        assert_eq!(error.code, "invalid_input");
        assert_eq!(foreground_reads.get(), 0);
        assert_eq!(tree_reads.get(), 0);
    }

    #[test]
    fn an_absent_round_freezes_foreground_before_the_tree_read() {
        // The REAL first-round order, recorded from both providers instead of inferred.
        // The absent condition is satisfied by this fixture, so the round completes and
        // the recorded order covers the whole round rather than a timeout.
        let order = std::cell::RefCell::new(Vec::<&'static str>::new());
        let value = wait_expect_with_readers(
            30_000,
            7,
            &[expectation("fixture-check", true)],
            true,
            ExecutionControl::none(),
            || {
                order.borrow_mut().push("foreground");
                Ok(foreground(7, 4242, "app"))
            },
            |_| {
                order.borrow_mut().push("tree");
                Ok(absent_tree())
            },
        )
        .expect("the absent condition is satisfied by this fixture");
        // The OBSERVED order. The foreground identity is frozen BEFORE any tree read,
        // and the revalidation runs after it, as the shipped loop does. This is recorded
        // from the providers rather than inferred from reading the source.
        assert_eq!(
            order.borrow().as_slice(),
            ["foreground", "tree", "foreground"],
            "the freeze must precede the tree read, and the revalidation must follow it"
        );
        assert_eq!(value["met"], true);
        assert_eq!(value["absent"], true);
        assert_eq!(value["foreground_unchanged"], true);
    }

    fn absent_tree() -> mechanism::A11yTree {
        // A COMPLETE (untruncated) observation that does not carry the expected
        // identifier, so `absent` is genuinely satisfied and no ambiguity or
        // unobservable-state refusal fires.
        mechanism::A11yTree {
            backend: "fixture".into(),
            window_handle: Some(7),
            root_id: "/0".into(),
            nodes: vec![a11y_node("/0", "unrelated", "button", &["showing"])],
            truncated: false,
            visited: 1,
            returned: 1,
        }
    }

    #[test]
    fn a_same_round_foreground_change_wins_over_a_pending_cancellation() {
        // Reuses the SAME foreground seam for both the freeze and the revalidation, so a
        // same-round identity change is authoritative over a token that round flipped.
        let token = Cell::new(false);
        let probe = || token.get();
        let reads = Cell::new(0usize);
        let error = wait_expect_with_readers(
            30_000,
            7,
            &[expectation("fixture-check", true)],
            true,
            ExecutionControl::with_cancel_probe(&probe),
            || {
                let n = reads.get() + 1;
                reads.set(n);
                // The freeze reads the original window; the revalidation reads a
                // DIFFERENT one, i.e. the foreground really changed mid-round.
                Ok(if n == 1 {
                    foreground(7, 4242, "app")
                } else {
                    foreground(9, 99, "other")
                })
            },
            |_| {
                token.set(true);
                Ok(absent_tree())
            },
        )
        .expect_err("foreground drift must win over the pending cancellation");
        assert!(token.get(), "the tree reader really did flip the token");
        assert_eq!(reads.get(), 2, "the bracketing pair really ran");
        assert_eq!(error.code, "foreground_changed");
        let detail = error.detail.unwrap_or(serde_json::Value::Null);
        assert_ne!(detail["effect"], "partially_performed");
    }

    #[test]
    fn absent_requires_every_expectation_to_be_explicitly_unsatisfied() {
        let mut present = a11y_node("/0/1", "Gone", "button", &["showing", "checked"]);
        present.identifier = Some("gone".into());
        let nodes = [present];
        let flat = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| observe::FlatNode {
                index,
                depth: 1,
                node,
            })
            .collect::<Vec<_>>();
        let matching = crate::command::Expectation {
            identifier: Some("gone".into()),
            checked: Some(true),
            ..Default::default()
        };
        let missing = crate::command::Expectation {
            identifier: Some("missing".into()),
            name: Some("Missing".into()),
            ..Default::default()
        };
        let mismatched = crate::command::Expectation {
            identifier: Some("gone".into()),
            checked: Some(false),
            ..Default::default()
        };

        assert!(
            !evaluate_expectations(&flat, std::slice::from_ref(&matching), true)
                .unwrap()
                .1
        );
        assert!(
            evaluate_expectations(&flat, &[missing, mismatched], true)
                .unwrap()
                .1
        );
        assert!(evaluate_expectations(&flat, &[matching], false).unwrap().1);
    }

    #[test]
    fn absent_refuses_ambiguous_and_unobservable_matches() {
        let nodes = [
            a11y_node("/0/1", "Duplicate", "button", &["showing"]),
            a11y_node("/0/2", "Duplicate", "button", &["showing"]),
        ];
        let flat = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| observe::FlatNode {
                index,
                depth: 1,
                node,
            })
            .collect::<Vec<_>>();
        let ambiguous = crate::command::Expectation {
            name: Some("Duplicate".into()),
            checked: Some(true),
            ..Default::default()
        };
        assert_eq!(
            evaluate_expectations(&flat, &[ambiguous], true)
                .unwrap_err()
                .code,
            "ambiguous"
        );

        let unobservable = crate::command::Expectation {
            node: Some("/0/1".into()),
            checked: Some(true),
            ..Default::default()
        };
        let error = evaluate_expectations(&flat, &[unobservable], true).unwrap_err();
        assert_eq!(error.code, "unsupported");
        assert_eq!(error.detail.unwrap()["reason"], "state_unobservable");
    }

    #[test]
    fn absent_refuses_truncated_acquisition_while_positive_wait_stays_compatible() {
        let error = require_complete_absence_observation(true, 7, 1000, true)
            .expect_err("partial tree cannot prove absence");
        assert_eq!(error.code, "unverified");
        assert_eq!(error.detail.unwrap()["reason"], "observation_truncated");
        assert!(require_complete_absence_observation(false, 7, 1000, true).is_ok());
        assert!(require_complete_absence_observation(true, 7, 12, false).is_ok());
    }

    #[test]
    fn absent_binds_exact_foreground_identity_but_not_mutable_window_content() {
        let before = foreground(7, 42, "Fixture");
        assert!(require_same_foreground(&before, &before).is_ok());

        for changed in [
            foreground(8, 42, "Fixture"),
            foreground(7, 43, "Fixture"),
            foreground(7, 42, "Other"),
        ] {
            let error = require_same_foreground(&before, &changed)
                .expect_err("identity drift must fail typed");
            assert_eq!(error.code, "foreground_changed");
            assert_eq!(error.detail.unwrap()["before"]["window"]["handle"], 7);
        }
    }

    #[test]
    fn window_titles_unavailable_error_names_handle_and_name_alternatives() {
        let error = window_titles_unavailable_error(3);
        assert_eq!(error.code, "unverified");
        let detail = error.detail.expect("detail");
        assert_eq!(detail["reason"], "window_titles_unavailable");
        assert_eq!(detail["window_count"], 3);
        let alternatives = detail["alternatives"].as_array().expect("alternatives");
        assert!(
            alternatives
                .iter()
                .any(|alt| alt.as_str().is_some_and(|s| s.contains("--focused-handle")))
        );
        assert!(alternatives.iter().any(|alt| {
            alt.as_str()
                .is_some_and(|s| s.contains("--node-name-contains"))
        }));
    }

    #[test]
    fn window_title_contains_matches_case_insensitively() {
        let windows = vec![WindowInfo {
            handle: 1,
            title: "Example Domain - Google Chrome".into(),
            process_id: 1,
            app_name: "chrome".into(),
            bounds: mechanism::window_enumerate::WindowBounds {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            focused: false,
            minimized: false,
            maximized: false,
            fullscreen: false,
            above: false,
        }];
        assert!(condition_met(
            &WaitCondition::WindowTitleContains {
                pattern: "example domain".into(),
            },
            &windows,
        ));
        assert!(!condition_met(
            &WaitCondition::WindowTitleContains {
                pattern: "missing".into(),
            },
            &windows,
        ));
    }

    #[test]
    fn node_wait_timeout_is_a_typed_failure() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeNameContains {
                pattern: "agenterm-no-such-node".into(),
                role: None,
                window: Some(-1),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "timeout must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(code, "timeout" | "unsupported"),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn node_text_equals_timeout_is_a_typed_failure() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextEquals {
                expected: "agenterm-no-such-text".into(),
                name: "agenterm-no-such-node".into(),
                role: None,
                window: Some(-1),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "timeout must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(code, "timeout" | "unsupported"),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn text_equals_success_publishes_gettext_not_snapshot_text() {
        let mut snapshot = node("Message Reasonix…", "text", &["showing", "editable"]);
        snapshot.text = Some("stale-snapshot".into());
        snapshot.id = "/0/0/0/0/0/0/0/0/8/1/0".into();
        let payload =
            text_equals_success("at-spi2", Some(4194318), 2, &snapshot, "RXWAIT-TYPED", 130);
        assert_eq!(payload["via"], "gettext");
        assert_eq!(payload["text"], "RXWAIT-TYPED");
        assert_eq!(payload["observation"]["text"], "RXWAIT-TYPED");
        assert_eq!(payload["node"]["text"], "RXWAIT-TYPED");
        assert_ne!(payload["via"], "text");
        assert_ne!(payload["node"]["text"], "stale-snapshot");
    }

    #[test]
    fn text_equals_timeout_reports_last_gettext() {
        assert_eq!(
            text_equals_timeout_detail(Some("RXWAIT-TYPED"), None, 130),
            "last GetText=\"RXWAIT-TYPED\""
        );
        let failed = CuError::new("a11y_text_unavailable", "no Text.GetText");
        assert_eq!(
            text_equals_timeout_detail(None, Some(&failed), 130),
            "last GetText failed: no Text.GetText (a11y_text_unavailable)"
        );
    }

    #[test]
    fn node_text_equals_requires_window() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextEquals {
                expected: "x".into(),
                name: "FixtureField".into(),
                role: None,
                window: None,
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn node_text_contains_timeout_is_a_typed_failure() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextContains {
                substring: "agenterm-no-such-sub".into(),
                name: "agenterm-no-such-node".into(),
                role: None,
                window: Some(-1),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "timeout must not report success");
        let code = reply.error.as_ref().unwrap().code.as_str();
        assert!(
            matches!(code, "timeout" | "unsupported"),
            "unexpected code: {code}"
        );
    }

    #[test]
    fn node_text_contains_requires_window() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::NodeTextContains {
                substring: "GATE".into(),
                name: "FixtureField".into(),
                role: None,
                window: None,
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
        assert!(
            reply
                .error
                .as_ref()
                .unwrap()
                .message
                .contains("--text-contains"),
            "missing-window message should name the flag"
        );
    }

    #[test]
    fn text_contains_matches_substring_of_independent_gettext() {
        assert!(NodeTextMatch::Contains.matches("34aGATEXXXX", "GATE"));
        assert!(!NodeTextMatch::Contains.matches("34aGATEXXXX", "NOPE"));
        assert!(!NodeTextMatch::Equals.matches("34aGATEXXXX", "GATE"));
        assert!(NodeTextMatch::Equals.matches("34aGATEXXXX", "34aGATEXXXX"));
    }

    #[test]
    fn text_contains_success_publishes_full_gettext_not_substring() {
        let mut snapshot = node("FixtureField", "entry", &["showing", "editable"]);
        snapshot.text = Some("stale-snapshot".into());
        let payload =
            text_equals_success("at-spi2", Some(4194318), 2, &snapshot, "34aGATEXXXX", 12);
        assert_eq!(payload["via"], "gettext");
        assert_eq!(payload["text"], "34aGATEXXXX");
        assert!(payload["text"].as_str().unwrap().contains("GATE"));
        assert_ne!(payload["text"], "GATE");
        assert_ne!(payload["node"]["text"], "stale-snapshot");
    }

    #[test]
    fn ready_path_wait_times_out_when_marker_is_absent() {
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 1,
            condition: WaitCondition::ReadyPath {
                path: "agenterm-no-such-ready-marker.json".into(),
            },
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok, "absent marker must not report success");
        assert_eq!(reply.error.as_ref().unwrap().code, "timeout");
    }

    #[test]
    fn ready_path_wait_reports_met_when_marker_is_ready() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let directory = std::env::temp_dir().join(format!(
            "agenterm-cu-wait-ready-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir(&directory).expect("temporary directory");
        let path = directory.join("ready.json");
        super::a11y_observe::publish_ready_marker(
            path.to_str().expect("UTF-8 path"),
            41,
            "at-spi2",
            "poll-diff",
        )
        .expect("publish marker");
        let auth = Authorization::new([Grant::Observe].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::Wait {
            target: TargetRef::Current,
            timeout_ms: 500,
            condition: WaitCondition::ReadyPath {
                path: path.to_string_lossy().into_owned(),
            },
        };
        let reply = executor.execute(&command);
        assert!(reply.ok, "ready marker must report success");
        assert_eq!(reply.data.as_ref().unwrap()["met"], true);
        assert_eq!(reply.data.as_ref().unwrap()["addressing"], "ready-path");
        assert_eq!(reply.exit_code(), 0);
        std::fs::remove_file(path).expect("remove marker");
        std::fs::remove_dir(directory).expect("remove temporary directory");
    }
}
