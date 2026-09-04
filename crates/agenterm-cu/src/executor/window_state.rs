//! Window-level state verbs: `raise`, `minimize`, `restore`.
//!
//! These move a whole **window** — its place in its application's stacking
//! order, or whether it is in the dock — through the window's own
//! affordances. None of them activates an application, brings one to the
//! foreground, or sends a keystroke, and each reads its postcondition back
//! before claiming anything.
//!
//! `raise` is deliberately not `focus`: `focus` gives one accessibility
//! *node* inside a window the keyboard focus and never touches stacking;
//! `raise` moves a whole window in front of its siblings and never moves
//! the accessibility focus.

use super::*;

use crate::observe::FrontmostApp;

/// How long a window-state postcondition polls before it is called unmet.
const STATE_READBACK: Duration = Duration::from_millis(1_500);
const STATE_READBACK_POLL: Duration = Duration::from_millis(25);
/// `raise` is a restack request to the window server; it lands fast or not
/// at all, so it polls for less time than a minimize animation needs.
const RAISE_READBACK: Duration = Duration::from_millis(500);

#[cfg(target_os = "macos")]
fn frontmost_app_now() -> Option<FrontmostApp> {
    crate::macos_focus::frontmost_app()
}

#[cfg(not(target_os = "macos"))]
fn frontmost_app_now() -> Option<FrontmostApp> {
    None
}

fn frontmost_json(app: Option<&FrontmostApp>) -> serde_json::Value {
    app.map(FrontmostApp::json)
        .unwrap_or(serde_json::Value::Null)
}

/// The window's place among **its own application's** windows, front to
/// back: `Some(0)` is the application's topmost window. `None` means this
/// host reports no stacking order, which is not the same as "it did not
/// move".
struct AppOrder {
    rank: Option<usize>,
    of: usize,
    reason: Option<String>,
}

impl AppOrder {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({ "rank": self.rank, "of": self.of, "reason": self.reason })
    }

    fn is_front(&self) -> bool {
        self.rank == Some(0)
    }
}

/// Rank `window` inside the front-to-back order of the windows owned by
/// `pid`. The desktop-wide z-index answers a different question (`orderwin`
/// asks that one): raising inside an application must not be judged by
/// whether the whole application came forward, because it deliberately
/// does not.
fn app_order(window: isize, pid: u32) -> AppOrder {
    let windows = match mechanism::window_enumerate::enumerate_top_level() {
        Ok(rows) => rows,
        Err(error) => {
            return AppOrder {
                rank: None,
                of: 0,
                reason: Some(mechanism_reason(error)),
            };
        }
    };
    let stacking = match mechanism::window_enumerate::stacking() {
        Ok(rows) => rows,
        Err(error) => {
            return AppOrder {
                rank: None,
                of: 0,
                reason: Some(mechanism_reason(error)),
            };
        }
    };
    let mut owned: Vec<(u32, isize)> = windows
        .iter()
        .filter(|row| row.process_id == pid)
        .filter_map(|row| {
            stacking
                .iter()
                .find(|place| place.handle == row.handle)
                .map(|place| (place.z_index, row.handle))
        })
        .collect();
    owned.sort_by_key(|(z, _)| *z);
    let of = owned.len();
    let rank = owned.iter().position(|(_, handle)| *handle == window);
    AppOrder {
        rank,
        of,
        reason: rank
            .is_none()
            .then(|| "the window is not in this application's stacking order".to_owned()),
    }
}

fn mechanism_reason(error: mechanism::MechanismError) -> String {
    match error {
        mechanism::MechanismError::Unsupported { reason } => reason,
        mechanism::MechanismError::Failed { code, message } => format!("{code}: {message}"),
    }
}

/// `raise --window H`: lift one window inside its own application's
/// z-order, with the frontmost application measured before and after.
pub(super) fn raise_payload(
    window: isize,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    if window == 0 {
        return Err(invalid_input(
            "raise requires --window <handle> (a non-zero handle from `windows`)".into(),
        ));
    }
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let Some(row) = windows.iter().find(|item| item.handle == window) else {
        return Err(CuError::new(
            "window_not_found",
            format!("no top-level window with handle {window}"),
        ));
    };
    let pid = row.process_id;
    let identity = serde_json::json!({
        "handle": window,
        "pid": pid,
        "app_name": row.app_name,
        "title": row.title,
    });
    let front_before = frontmost_app_now();
    let before = app_order(window, pid);
    let ticket = receipts.reserve(
        "raise",
        window,
        serde_json::json!({
            "action": "raise",
            "window_identity": identity,
            "postcondition": "app-topmost",
            "before": before.json(),
            "frontmost_app": frontmost_json(front_before.as_ref()),
        }),
    )?;
    let mechanism_error = mechanism::window_op::show(window, mechanism::window_op::SHOW)
        .err()
        .map(map_mechanism_err);
    // Restacking is a request the window server applies asynchronously, so
    // sending it is not evidence: poll the order back.
    let started = Instant::now();
    let mut after = app_order(window, pid);
    let mut polls = 1usize;
    while !after.is_front() && mechanism_error.is_none() && started.elapsed() < RAISE_READBACK {
        thread::sleep(STATE_READBACK_POLL);
        polls += 1;
        after = app_order(window, pid);
    }
    let front_after = frontmost_app_now();
    let front_pid_before = front_before.as_ref().map(|app| app.pid);
    let front_pid_after = front_after.as_ref().map(|app| app.pid);
    let foreground_unchanged = front_pid_before == front_pid_after;
    let unverifiable = after.rank.is_none() && after.reason.is_some();
    let verified =
        after.is_front() && foreground_unchanged && mechanism_error.is_none() && !unverifiable;
    let reason = if mechanism_error.is_some() {
        Some("mechanism_failed")
    } else if !foreground_unchanged {
        Some("foreground_changed")
    } else if unverifiable {
        Some("stacking_unreadable")
    } else if !after.is_front() {
        Some("order_unchanged")
    } else {
        None
    };
    let verification = serde_json::json!({
        "method": "app-stacking-readback",
        "reason": reason,
        "polls": polls,
        "elapsed_ms": started.elapsed().as_millis(),
    });
    receipts.complete(
        &ticket,
        "raise",
        window,
        verified,
        serde_json::json!({
            "after": after.json(),
            "frontmost_app": frontmost_json(front_after.as_ref()),
            "verification": verification,
            "error": mechanism_error.as_ref().map(error_payload),
        }),
    )?;
    let payload = serde_json::json!({
        "addressing": "window-handle",
        "mechanism": "libagenterm",
        "via": "ax-raise",
        "window": window,
        "target": identity,
        "action": "raise",
        "scope": "within-application",
        "postcondition": "app-topmost",
        "performed": mechanism_error.is_none(),
        "verified": verified,
        "verification": verification,
        "before": before.json(),
        "after": after.json(),
        "frontmost_app_before": frontmost_json(front_before.as_ref()),
        "frontmost_app_after": frontmost_json(front_after.as_ref()),
        "frontmost_app_unchanged": foreground_unchanged,
        "activated_application": false,
        "receipt": ticket.json(),
    });
    if let Some(error) = mechanism_error {
        return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
    }
    if !foreground_unchanged {
        return Err(CuError::new(
            "foreground_changed",
            format!(
                "raising window {window} moved the system frontmost application from {:?} to {:?}; raise must not activate anything",
                front_pid_before, front_pid_after
            ),
        )
        .with_detail(serde_json::json!({ "reason": "foreground_changed", "receipt": payload })));
    }
    if unverifiable {
        // The host cannot report a stacking order, so nothing here can
        // confirm or deny the move; say that rather than letting the
        // absence of a contradiction read as success.
        return Ok(payload);
    }
    if !after.is_front() {
        return Err(CuError::new(
            "window_order_not_applied",
            format!(
                "window {window} is still behind {} of its application's windows after {polls} polls",
                after.rank.unwrap_or_default()
            ),
        )
        .with_detail(serde_json::json!({ "reason": "order_unchanged", "receipt": payload })));
    }
    Ok(payload)
}

/// `minimize` / `restore`: the desired minimized state, its gate, and the
/// read-back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WindowState {
    Minimized,
    Restored,
}

impl WindowState {
    fn verb(self) -> &'static str {
        match self {
            Self::Minimized => "minimize",
            Self::Restored => "restore",
        }
    }

    /// The one word `--expect` must carry.
    fn postcondition(self) -> &'static str {
        match self {
            Self::Minimized => "minimized",
            Self::Restored => "restored",
        }
    }

    fn wants_minimized(self) -> bool {
        matches!(self, Self::Minimized)
    }

    fn show_state(self) -> i32 {
        match self {
            Self::Minimized => crate::dynlib::AGT_NATIVE_WINDOW_MINIMIZE,
            Self::Restored => crate::dynlib::AGT_NATIVE_WINDOW_RESTORE,
        }
    }
}

/// The gate `minimize` / `restore` carry: an exact target and a checkable
/// postcondition. It is `close`'s gate without the prior snapshot —
/// neither verb destroys anything, so there is nothing to snapshot for
/// recovery, but both change what the user sees and so still refuse to run
/// on a guess.
pub(super) fn window_state_gate(
    state: WindowState,
    window: isize,
    expect: Option<&str>,
) -> Result<(), CuError> {
    let mut missing = Vec::new();
    if window == 0 {
        missing.push("target");
    }
    let wanted = state.postcondition();
    match expect.map(str::trim) {
        Some(value) if value == wanted => {}
        _ => missing.push("postcondition"),
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(CuError::new(
        "refused",
        format!(
            "{} changes what the user sees: it needs an exact target (--window HANDLE) and a \
             checkable postcondition (--expect {wanted}); nothing was performed",
            state.verb()
        ),
    )
    .with_detail(serde_json::json!({
        "reason": "destructive_gate",
        "missing": missing,
        "required": {
            "target": "--window HANDLE",
            "postcondition": format!("--expect {wanted}"),
        },
        "effect": "not_performed",
    })))
}

/// Read the window's minimized state, mapping the mechanism failure into
/// the caller's "nothing was performed" framing.
fn read_minimized(window: isize) -> Result<bool, CuError> {
    mechanism::window_op::minimized(window).map_err(map_mechanism_err)
}

fn inventory_present(window: isize) -> Option<bool> {
    mechanism::window_enumerate::enumerate_top_level()
        .ok()
        .map(|rows| rows.iter().any(|row| row.handle == window))
}

pub(super) fn window_state_payload(
    state: WindowState,
    window: isize,
    expect: Option<&str>,
    receipts: &mut ReceiptLog,
) -> Result<serde_json::Value, CuError> {
    window_state_gate(state, window, expect)?;
    let not_performed = |error: CuError| {
        let mut detail = error.detail.clone().unwrap_or(serde_json::json!({}));
        detail["effect"] = serde_json::json!("not_performed");
        error.with_detail(detail)
    };
    // The state is read before anything else: an already-minimized window
    // must be a verified no-op, not a second minimize.
    let was_minimized = read_minimized(window).map_err(not_performed)?;
    let was_present = inventory_present(window);
    let wanted = state.wants_minimized();
    let performed = was_minimized != wanted;
    let front_before = frontmost_app_now();
    let before = serde_json::json!({
        "minimized": was_minimized,
        "inventory_present": was_present,
    });
    let ticket = receipts.reserve(
        state.verb(),
        window,
        serde_json::json!({
            "action": state.verb(),
            "postcondition": state.postcondition(),
            "performed": performed,
            "before": before,
            "frontmost_app": frontmost_json(front_before.as_ref()),
        }),
    )?;
    let mut mechanism_error = None;
    if performed {
        mechanism_error = mechanism::window_op::show(window, state.show_state())
            .err()
            .map(map_mechanism_err);
    }
    // Read the state back rather than trusting the request: the minimize
    // is animated, so the attribute write returns before the window has
    // gone anywhere.
    let started = Instant::now();
    let mut polls = 0usize;
    let mut now = was_minimized;
    let mut readback_error = None;
    loop {
        polls += 1;
        match read_minimized(window) {
            Ok(value) => now = value,
            Err(error) => {
                readback_error = Some(error);
                break;
            }
        }
        if now == wanted || mechanism_error.is_some() || started.elapsed() >= STATE_READBACK {
            break;
        }
        thread::sleep(STATE_READBACK_POLL);
    }
    let is_present = inventory_present(window);
    let front_after = frontmost_app_now();
    let front_pid_before = front_before.as_ref().map(|app| app.pid);
    let front_pid_after = front_after.as_ref().map(|app| app.pid);
    let foreground_unchanged = front_pid_before == front_pid_after;
    let verified = now == wanted
        && foreground_unchanged
        && mechanism_error.is_none()
        && readback_error.is_none();
    let reason = if mechanism_error.is_some() {
        Some("mechanism_failed")
    } else if readback_error.is_some() {
        Some("readback_failed")
    } else if !foreground_unchanged {
        Some("foreground_changed")
    } else if now != wanted {
        Some("state_mismatch")
    } else if !performed {
        Some(match state {
            WindowState::Minimized => "already_minimized",
            WindowState::Restored => "already_restored",
        })
    } else {
        None
    };
    let verification = serde_json::json!({
        "method": "window-minimized-readback",
        "reason": reason,
        "polls": polls,
        "elapsed_ms": started.elapsed().as_millis(),
    });
    let after = serde_json::json!({
        "minimized": now,
        "inventory_present": is_present,
    });
    receipts.complete(
        &ticket,
        state.verb(),
        window,
        verified,
        serde_json::json!({
            "performed": performed && mechanism_error.is_none(),
            "after": after,
            "verification": verification,
            "error": mechanism_error.as_ref().or(readback_error.as_ref()).map(error_payload),
        }),
    )?;
    let payload = serde_json::json!({
        "addressing": "window-handle",
        "mechanism": "libagenterm",
        "via": "window-minimize-affordance",
        "window": window,
        "action": state.verb(),
        "postcondition": state.postcondition(),
        "performed": performed && mechanism_error.is_none(),
        "verified": verified,
        "verification": verification,
        "before": before,
        "after": after,
        "frontmost_app_before": frontmost_json(front_before.as_ref()),
        "frontmost_app_after": frontmost_json(front_after.as_ref()),
        "frontmost_app_unchanged": foreground_unchanged,
        "activated_application": false,
        "receipt": ticket.json(),
    });
    if let Some(error) = mechanism_error.or(readback_error) {
        return Err(error.with_detail(serde_json::json!({ "receipt": payload })));
    }
    if !foreground_unchanged {
        return Err(CuError::new(
            "foreground_changed",
            format!(
                "{} on window {window} moved the system frontmost application from {front_pid_before:?} to {front_pid_after:?}; it must not activate anything",
                state.verb()
            ),
        )
        .with_detail(serde_json::json!({ "reason": "foreground_changed", "receipt": payload })));
    }
    if now != wanted {
        return Err(CuError::new(
            "unverified",
            format!(
                "{} was delivered to window {window} but it reads minimized={now} after {polls} polls",
                state.verb()
            ),
        )
        .with_detail(serde_json::json!({ "reason": "state_mismatch", "receipt": payload })));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_names_every_missing_part_and_performs_nothing() {
        let executor = actuate_executor();
        // No target and no postcondition.
        let reply = executor.execute(&Command::Minimize {
            target: TargetRef::Current,
            window: 0,
            expect: None,
        });
        assert!(!reply.ok);
        assert_eq!(reply.command, "minimize");
        let error = reply.error.as_ref().expect("typed refusal");
        assert_eq!(error.code, "refused");
        let detail = error.detail.as_ref().expect("gate detail");
        assert_eq!(detail["reason"], "destructive_gate");
        assert_eq!(detail["effect"], "not_performed");
        assert_eq!(
            detail["missing"],
            serde_json::json!(["target", "postcondition"])
        );
        // The wrong postcondition word is still a missing postcondition:
        // `restore --expect minimized` must not run a restore.
        let swapped = executor.execute(&Command::Restore {
            target: TargetRef::Current,
            window: 4242,
            expect: Some("minimized".into()),
        });
        assert_eq!(
            swapped.error.as_ref().expect("typed").code,
            "refused",
            "restore only accepts --expect restored"
        );
        assert_eq!(
            swapped.error.as_ref().unwrap().detail.as_ref().unwrap()["missing"],
            serde_json::json!(["postcondition"])
        );
        // A complete gate gets past the refusal and reaches the mechanism,
        // where a handle that is not a window answers typed (never usage).
        let complete = executor.execute(&Command::Minimize {
            target: TargetRef::Current,
            window: 4242,
            expect: Some("minimized".into()),
        });
        assert!(!complete.ok);
        let code = complete.error.as_ref().expect("typed").code.clone();
        assert_ne!(code, "refused");
        assert_ne!(code, "usage");
    }

    #[test]
    fn the_gate_is_pure_and_accepts_only_its_own_word() {
        assert!(window_state_gate(WindowState::Minimized, 7, Some("minimized")).is_ok());
        assert!(window_state_gate(WindowState::Restored, 7, Some("restored")).is_ok());
        assert!(window_state_gate(WindowState::Restored, 7, Some(" restored ")).is_ok());
        for bad in [None, Some(""), Some("gone"), Some("restored")] {
            assert!(
                window_state_gate(WindowState::Minimized, 7, bad).is_err(),
                "{bad:?} must not satisfy minimize"
            );
        }
        assert!(window_state_gate(WindowState::Minimized, 0, Some("minimized")).is_err());
    }

    #[test]
    fn raise_requires_a_handle_and_never_reaches_the_mechanism_without_one() {
        let reply = actuate_executor().execute(&Command::Raise {
            target: TargetRef::Current,
            window: 0,
        });
        assert!(!reply.ok);
        assert_eq!(reply.command, "raise");
        assert_eq!(reply.error.as_ref().expect("typed").code, "invalid_input");
    }

    #[test]
    fn raise_and_minimize_require_the_actuate_grant() {
        for command in [
            Command::Raise {
                target: TargetRef::Current,
                window: 7,
            },
            Command::Minimize {
                target: TargetRef::Current,
                window: 7,
                expect: Some("minimized".into()),
            },
            Command::Restore {
                target: TargetRef::Current,
                window: 7,
                expect: Some("restored".into()),
            },
        ] {
            let reply = Executor::new(Authorization::new(Default::default())).execute(&command);
            assert!(!reply.ok, "{}", command.verb());
            assert_eq!(
                reply.error.expect("typed refusal").code,
                "refused",
                "{}",
                command.verb()
            );
        }
    }

    #[test]
    fn an_app_order_without_stacking_is_unreadable_not_front() {
        let order = AppOrder {
            rank: None,
            of: 0,
            reason: Some("no stacking".into()),
        };
        assert!(!order.is_front());
        assert_eq!(order.json()["rank"], serde_json::Value::Null);
        assert_eq!(order.json()["reason"], "no stacking");
        let front = AppOrder {
            rank: Some(0),
            of: 3,
            reason: None,
        };
        assert!(front.is_front());
        assert_eq!(front.json()["of"], 3);
    }
}
