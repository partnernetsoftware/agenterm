//! `pointer-move` / `pointer-scroll` / `pointer-position` / `drag`: the pointer verbs.

use super::*;
use agenterm_platform::input_inject::MAX_POINTER_SCROLL_DETENTS;

pub(super) fn pointer_move(x: i32, y: i32) -> Result<serde_json::Value, CuError> {
    pointer_move_with(x, y, |x, y| {
        mechanism::input_inject::pointer_move(x, y).map_err(map_mechanism_err)
    })
}

pub(super) fn pointer_position() -> Result<serde_json::Value, CuError> {
    pointer_position_with(|| mechanism::input_inject::pointer_position().map_err(map_mechanism_err))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
struct FocusedWindowIdentity {
    handle: isize,
    process_id: u32,
}

fn validate_pointer_scroll(dx: i32, dy: i32) -> Result<(), CuError> {
    if dx == 0 && dy == 0 {
        return Err(invalid_input(
            "pointer-scroll requires at least one non-zero axis".into(),
        ));
    }
    if dx.unsigned_abs() > MAX_POINTER_SCROLL_DETENTS
        || dy.unsigned_abs() > MAX_POINTER_SCROLL_DETENTS
    {
        return Err(invalid_input(format!(
            "pointer-scroll requires each axis to be within -{MAX_POINTER_SCROLL_DETENTS}..={MAX_POINTER_SCROLL_DETENTS}"
        )));
    }
    Ok(())
}

fn focused_window_identity() -> Result<Option<FocusedWindowIdentity>, CuError> {
    let mut windows =
        mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let stacking = mechanism::window_enumerate::stacking().unwrap_or_default();
    resolve_inventory_focus(&mut windows, &stacking);
    focused_window_identity_from(&windows)
}

fn focused_window_identity_from(
    windows: &[WindowInfo],
) -> Result<Option<FocusedWindowIdentity>, CuError> {
    let mut focused = windows.iter().filter(|row| row.focused);
    let identity = focused.next().map(|row| FocusedWindowIdentity {
        handle: row.handle,
        process_id: row.process_id,
    });
    if focused.next().is_some() {
        return Err(CuError::new(
            "focused_window_ambiguous",
            "more than one top-level window reports focused; pointer-scroll was not dispatched",
        ));
    }
    Ok(identity)
}

pub(super) fn pointer_scroll(
    dx: i32,
    dy: i32,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    pointer_scroll_with(
        dx,
        dy,
        receipts,
        || mechanism::input_inject::pointer_position().map_err(map_mechanism_err),
        focused_window_identity,
        |dx, dy| mechanism::input_inject::pointer_scroll(dx, dy).map_err(map_mechanism_err),
    )
}

fn pointer_scroll_with(
    dx: i32,
    dy: i32,
    receipts: &mut ReceiptLog,
    mut observe_pointer: impl FnMut() -> Result<(i32, i32), CuError>,
    mut observe_focus: impl FnMut() -> Result<Option<FocusedWindowIdentity>, CuError>,
    inject_once: impl FnOnce(i32, i32) -> Result<(), CuError>,
) -> Result<serde_json::Value, CuError> {
    validate_pointer_scroll(dx, dy)?;
    let pointer_before = observe_pointer()?;
    let focus_before = observe_focus()?;
    let ticket = receipts.reserve(
        "pointer-scroll",
        0,
        serde_json::json!({
            "action": "pointer-scroll",
            "addressing": "desktop-at-current-pointer",
            "delta": { "dx": dx, "dy": dy },
            "before": { "pointer": [pointer_before.0, pointer_before.1], "focused_window": focus_before },
        }),
    )?;
    let mechanism_error = inject_once(dx, dy).err();
    let pointer_after_result = observe_pointer();
    let focus_after_result = observe_focus();
    let pointer_after = pointer_after_result.as_ref().ok().copied();
    let focus_after = focus_after_result.as_ref().ok().copied().flatten();
    let pointer_unchanged = matches!(&pointer_after_result, Ok(value) if *value == pointer_before);
    let focus_unchanged = matches!(&focus_after_result, Ok(value) if *value == focus_before);
    let invariants_verified = mechanism_error.is_none() && pointer_unchanged && focus_unchanged;
    let failure_reason = if mechanism_error.is_some() {
        Some("mechanism_failed")
    } else if pointer_after.is_none() {
        Some("pointer_readback_unavailable")
    } else if focus_after_result.is_err() {
        Some("focused_window_readback_unavailable")
    } else if !pointer_unchanged {
        Some("pointer_moved")
    } else if !focus_unchanged {
        Some("focused_window_changed")
    } else {
        None
    };
    receipts.complete(
        &ticket,
        "pointer-scroll",
        0,
        invariants_verified,
        serde_json::json!({
            "performed": mechanism_error.is_none(),
            "invariants_verified": invariants_verified,
            "delivery_verified": false,
            "after": {
                "pointer": pointer_after.map(|(x, y)| [x, y]),
                "focused_window": focus_after,
            },
            "verification": {
                "pointer_unchanged": pointer_unchanged,
                "focused_window_unchanged": focus_unchanged,
                "reason": failure_reason,
            },
            "error": mechanism_error.as_ref().map(error_payload),
            "readback_error": pointer_after_result.as_ref().err().or_else(|| focus_after_result.as_ref().err()).map(error_payload),
        }),
    )?;
    let payload = serde_json::json!({
        "addressing": "desktop-at-current-pointer",
        "mechanism": "libagenterm",
        "delta": { "dx": dx, "dy": dy },
        "performed": mechanism_error.is_none(),
        "invariants_verified": invariants_verified,
        "delivery_verified": false,
        "pointer_before": [pointer_before.0, pointer_before.1],
        "pointer_after": pointer_after.map(|(x, y)| [x, y]),
        "focused_window_before": focus_before,
        "focused_window_after": focus_after,
        "receipt": ticket.json(),
    });
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
    }
    if !invariants_verified {
        return Err(CuError::new(
            "pointer_scroll_unverified",
            "the wheel event was accepted but pointer/focus invariants could not be verified",
        )
        .with_detail(serde_json::json!({
            "reason": failure_reason,
            "receipt": payload,
        })));
    }
    Ok(payload)
}

pub(super) fn pointer_position_with(
    observe_once: impl FnOnce() -> Result<(i32, i32), CuError>,
) -> Result<serde_json::Value, CuError> {
    let (x, y) = observe_once()?;
    Ok(serde_json::json!({
        "effect": "observed",
        "addressing": "absolute-screen-coordinates",
        "coords": [x, y],
        "mechanism": "libagenterm",
    }))
}

pub(super) fn pointer_move_with(
    x: i32,
    y: i32,
    move_once: impl FnOnce(i32, i32) -> Result<(), CuError>,
) -> Result<serde_json::Value, CuError> {
    move_once(x, y)?;
    Ok(serde_json::json!({
        "effect": "committed",
        "addressing": "absolute-screen-coordinates",
        "coords": [x, y],
        "mechanism": "libagenterm",
        "button_effect": "none",
    }))
}

/// Default intermediate moves between the press and the release. Enough
/// that a drag-aware view sees a gesture rather than a teleport, few
/// enough that the whole thing stays one bounded burst of events.
pub(super) const DEFAULT_DRAG_STEPS: u32 = 12;
pub(super) const MAX_DRAG_STEPS: u32 = 64;

/// `--steps` validation, before any event is created.
pub(super) fn validate_drag_steps(steps: Option<u32>) -> Result<u32, String> {
    match steps {
        None => Ok(DEFAULT_DRAG_STEPS),
        Some(0) => Err("--steps must be at least 1".to_owned()),
        Some(value) if value > MAX_DRAG_STEPS => Err(format!(
            "--steps must be at most {MAX_DRAG_STEPS}, got {value}"
        )),
        Some(value) => Ok(value),
    }
}

/// Whether this host can deliver a drag **into a window** without moving
/// the user's own cursor.
///
/// No adapter can today, and macOS provably cannot: mouse events posted to
/// a pid arrive with no window for AppKit to route them through, so the
/// only working path is the global one that moves the real pointer (see
/// the macOS input-injection adapter's module documentation). This is a
/// host capability question, not a policy: the day an adapter gains a
/// window-local path, this returns true there and `--degraded` stops being
/// required on that host.
pub(super) fn window_local_drag_available() -> bool {
    false
}

/// `drag --window H --from X,Y --to X,Y`: one press, a bounded series of
/// moves, one release.
#[allow(clippy::too_many_arguments)]
pub(super) fn drag_payload(
    window: isize,
    from: [i32; 2],
    to: [i32; 2],
    button: PointerButton,
    steps: Option<u32>,
    degraded: bool,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "drag requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    let steps = validate_drag_steps(steps).map_err(invalid_input)?;
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let Some(row) = windows.iter().find(|item| item.handle == window) else {
        return Err(CuError::new(
            "window_not_found",
            format!("no top-level window with handle {window}"),
        ));
    };
    let bounds = row.bounds;
    let inside = |point: [i32; 2]| {
        let right = bounds.x.saturating_add(bounds.width as i32);
        let bottom = bounds.y.saturating_add(bounds.height as i32);
        point[0] >= bounds.x && point[0] < right && point[1] >= bounds.y && point[1] < bottom
    };
    if !inside(from) {
        return Err(CuError::new(
            "drag_outside_window",
            format!(
                "drag --from {},{} is outside window {window} ({}x{} at {},{}); the press must start on the window it addresses",
                from[0], from[1], bounds.width, bounds.height, bounds.x, bounds.y
            ),
        )
        .with_detail(serde_json::json!({
            "from": from,
            "to": to,
            "window_bounds": bounds,
            "effect": "not_performed",
        })));
    }
    // The degraded opt-in is the same contract `click --coords` carries: a
    // path that moves the user's real cursor is never taken implicitly.
    let window_local = window_local_drag_available();
    if !window_local && !degraded {
        return Err(CuError::new(
            "invalid_input",
            "this host can only drag by moving the real pointer (there is no window-local pointer injection); pass --degraded to admit that path explicitly",
        )
        .with_detail(serde_json::json!({
            "path": "degraded-global-pointer",
            "window_local_available": false,
            "effect": "not_performed",
        })));
    }
    let path = if window_local {
        "window-local"
    } else {
        "degraded-global-pointer"
    };
    let pointer_before = mechanism::input_inject::pointer_position().ok();
    let before_tree = mechanism::tree_for_window(Some(window)).ok();
    let ticket = receipts.reserve(
        "drag",
        window,
        serde_json::json!({
            "action": "drag",
            "path": path,
            "from": from,
            "to": to,
            "button": button,
            "steps": steps,
            "before": { "pointer": pointer_before.map(|(x, y)| [x, y]) },
        }),
    )?;
    let inject_button = match button {
        PointerButton::Left => mechanism::input_inject::PointerButton::Left,
        PointerButton::Right => mechanism::input_inject::PointerButton::Right,
        PointerButton::Middle => mechanism::input_inject::PointerButton::Middle,
    };
    let mechanism_error = mechanism::input_inject::pointer_drag(
        (from[0], from[1]),
        (to[0], to[1]),
        inject_button,
        steps,
    )
    .err()
    .map(map_mechanism_err);
    // The read-back for a gesture is where the pointer ended up: the
    // release happened at `to`, so the pointer must be there.
    let pointer_after = mechanism::input_inject::pointer_position().ok();
    let landed = pointer_after == Some((to[0], to[1]));
    let after_tree = mechanism::tree_for_window(Some(window)).ok();
    let tree_changed = match (&before_tree, &after_tree) {
        (Some(was), Some(is)) => Some(observe::tree_changed(was, is)),
        _ => None,
    };
    let verified = landed && mechanism_error.is_none();
    let verification = serde_json::json!({
        "method": "pointer-position-readback",
        "reason": if mechanism_error.is_some() {
            Some("mechanism_failed")
        } else if !landed {
            Some("pointer_not_at_target")
        } else {
            None
        },
    });
    receipts.complete(
        &ticket,
        "drag",
        window,
        verified,
        serde_json::json!({
            "after": { "pointer": pointer_after.map(|(x, y)| [x, y]) },
            "verification": verification,
            "tree_changed": tree_changed,
            "error": mechanism_error.as_ref().map(error_payload),
        }),
    )?;
    let payload = serde_json::json!({
        "addressing": "degraded-coordinates",
        "mechanism": "libagenterm",
        "path": path,
        "window_local_available": window_local,
        "degraded": !window_local,
        "window": window,
        "from": from,
        "to": to,
        "to_inside_window": inside(to),
        "button": button,
        "steps": steps,
        "performed": mechanism_error.is_none(),
        "verified": verified,
        "verification": verification,
        "pointer_before": pointer_before.map(|(x, y)| [x, y]),
        "pointer_after": pointer_after.map(|(x, y)| [x, y]),
        "tree_changed": tree_changed,
        "receipt": ticket.json(),
    });
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
    }
    if !landed {
        return Err(CuError::new(
            "unverified",
            format!(
                "the drag was delivered but the pointer reads {:?}, not the release point {to:?}",
                pointer_after.map(|(x, y)| [x, y])
            ),
        )
        .with_detail(
            serde_json::json!({ "reason": "pointer_not_at_target", "receipt": payload }),
        ));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focused(handle: isize, process_id: u32) -> Option<FocusedWindowIdentity> {
        Some(FocusedWindowIdentity { handle, process_id })
    }

    #[test]
    fn pointer_move_calls_only_move_once_and_returns_bounded_typed_reply() {
        let mut calls = Vec::new();
        let reply = pointer_move_with(-320, 1440, |x, y| {
            calls.push((x, y));
            Ok(())
        })
        .expect("pointer move");
        assert_eq!(calls, [(-320, 1440)]);
        assert_eq!(reply["effect"], "committed");
        assert_eq!(reply["coords"], serde_json::json!([-320, 1440]));
        assert_eq!(reply["button_effect"], "none");
        assert_eq!(reply.as_object().expect("object").len(), 5);
    }

    #[test]
    fn pointer_position_observes_once_without_injection() {
        let mut calls = 0;
        let reply = pointer_position_with(|| {
            calls += 1;
            Ok((-17, 2048))
        })
        .expect("pointer position");
        assert_eq!(calls, 1);
        assert_eq!(reply["effect"], "observed");
        assert_eq!(reply["coords"], serde_json::json!([-17, 2048]));
        assert_eq!(reply.as_object().expect("object").len(), 4);
    }

    #[test]
    fn drag_steps_are_bounded_before_any_event_is_created() {
        assert_eq!(validate_drag_steps(None), Ok(DEFAULT_DRAG_STEPS));
        assert_eq!(validate_drag_steps(Some(1)), Ok(1));
        assert_eq!(
            validate_drag_steps(Some(MAX_DRAG_STEPS)),
            Ok(MAX_DRAG_STEPS)
        );
        assert!(validate_drag_steps(Some(0)).is_err());
        assert!(validate_drag_steps(Some(MAX_DRAG_STEPS + 1)).is_err());
    }

    #[test]
    fn pointer_scroll_closes_receipt_after_invariant_readback() {
        let scratch = audit_scratch("pointer-scroll-success");
        let directory = scratch.parent().expect("scratch parent");
        let mut receipts = ReceiptLog::open_in(directory, TargetRef::Current).expect("receipt");
        let mut pointers = [(40, 80), (40, 80)].into_iter();
        let mut focuses = [focused(7, 99), focused(7, 99)].into_iter();
        let mut injected = Vec::new();
        let reply = pointer_scroll_with(
            25,
            -100,
            &mut receipts,
            || Ok(pointers.next().expect("bounded pointer observation")),
            || Ok(focuses.next().expect("bounded focus observation")),
            |dx, dy| {
                injected.push((dx, dy));
                Ok(())
            },
        )
        .expect("bounded scroll");
        assert_eq!(injected, [(25, -100)]);
        assert_eq!(reply["performed"], true);
        assert_eq!(reply["invariants_verified"], true);
        assert_eq!(reply["delivery_verified"], false);
        let (lines, total) = receipt::list_file(receipts.path(), None, 10).expect("list");
        assert_eq!(total, 2);
        assert_eq!(lines[0]["phase"], "reserved");
        assert_eq!(lines[1]["phase"], "completed");
        drop(receipts);
        remove_audit_scratch(&scratch);
    }

    #[test]
    fn pointer_scroll_closes_failed_receipt_when_pointer_moves() {
        let scratch = audit_scratch("pointer-scroll-unverified");
        let directory = scratch.parent().expect("scratch parent");
        let mut receipts = ReceiptLog::open_in(directory, TargetRef::Current).expect("receipt");
        let mut pointers = [(40, 80), (41, 80)].into_iter();
        let mut focuses = [focused(7, 99), focused(7, 99)].into_iter();
        let error = pointer_scroll_with(
            0,
            -1,
            &mut receipts,
            || Ok(pointers.next().expect("bounded pointer observation")),
            || Ok(focuses.next().expect("bounded focus observation")),
            |_, _| Ok(()),
        )
        .expect_err("moved pointer is unverified");
        assert_eq!(error.code, "pointer_scroll_unverified");
        let (lines, total) = receipt::list_file(receipts.path(), None, 10).expect("list");
        assert_eq!(total, 2);
        assert_eq!(lines[1]["phase"], "failed");
        assert_eq!(lines[1]["verification"]["reason"], "pointer_moved");
        drop(receipts);
        remove_audit_scratch(&scratch);
    }

    #[test]
    fn pointer_scroll_rejects_unbounded_or_empty_delta_before_injection() {
        let before = mechanism::write_ledger::attempts();
        for (dx, dy) in [(0, 0), (101, 0), (0, -101), (i32::MIN, 1)] {
            let reply = actuate_executor().execute(&Command::PointerScroll {
                target: TargetRef::Current,
                dx,
                dy,
            });
            assert!(!reply.ok);
            assert_eq!(reply.error.expect("typed refusal").code, "invalid_input");
        }
        assert_eq!(mechanism::write_ledger::attempts(), before);
    }

    /// Every refusal `drag` can make happens before the mechanism, so the
    /// write ledger proves the pointer was never touched.
    #[test]
    fn every_drag_refusal_is_typed_and_reaches_no_injection() {
        let executor = actuate_executor();
        let drag = |window: isize, steps: Option<u32>, degraded: bool| Command::Drag {
            target: TargetRef::Current,
            window,
            from: [10, 10],
            to: [20, 20],
            button: PointerButton::Left,
            steps,
            degraded,
        };
        let before = mechanism::write_ledger::attempts();
        let no_window = executor.execute(&drag(0, None, true));
        assert_eq!(no_window.command, "drag");
        assert_eq!(
            no_window.error.as_ref().expect("typed").code,
            "invalid_input"
        );
        let bad_steps = executor.execute(&drag(7, Some(0), true));
        assert_eq!(
            bad_steps.error.as_ref().expect("typed").code,
            "invalid_input"
        );
        let too_many = executor.execute(&drag(7, Some(MAX_DRAG_STEPS + 1), true));
        assert_eq!(
            too_many.error.as_ref().expect("typed").code,
            "invalid_input"
        );
        // Without --degraded the host that can only move the real pointer
        // refuses and says which path it would have taken.
        let not_degraded = executor.execute(&drag(7, None, false));
        let error = not_degraded.error.as_ref().expect("typed");
        assert_ne!(error.code, "usage");
        if !window_local_drag_available() {
            // Window 7 is not a real handle, so either the degraded
            // refusal or the window lookup answers first; neither may
            // inject anything.
            assert!(
                matches!(error.code.as_str(), "invalid_input" | "window_not_found"),
                "{}",
                error.code
            );
        }
        assert_eq!(
            mechanism::write_ledger::attempts(),
            before,
            "a refused drag must not reach the injection mechanism"
        );
    }

    #[test]
    fn drag_requires_the_actuate_grant() {
        let reply = Executor::new(Authorization::new(Default::default())).execute(&Command::Drag {
            target: TargetRef::Current,
            window: 7,
            from: [0, 0],
            to: [1, 1],
            button: PointerButton::Left,
            steps: None,
            degraded: true,
        });
        assert!(!reply.ok);
        assert_eq!(reply.error.expect("typed refusal").code, "refused");
    }

    #[test]
    fn pointer_move_requires_actuate_and_refusal_moves_nothing() {
        let command = Command::PointerMove {
            target: TargetRef::Current,
            x: 10,
            y: 20,
        };
        let reply = Executor::new(Authorization::new(Default::default())).execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.expect("typed refusal").code, "refused");
    }
}
