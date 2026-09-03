//! `window-place`: the Spectacle catalog and `frame` as one preflight ->
//! apply -> read-back -> history transaction with structured rollback.

use super::*;

/// What `window-place` was asked to do: a catalog action, or (PRD_02_32
/// `frame`, slice 4) an explicit rect that replaces the geometry step and
/// rides the same preflight / apply / read-back / history transaction.
#[derive(Clone, Copy, Debug)]
pub(super) enum PlaceRequest {
    Catalog(crate::place::PlaceAction),
    Frame(crate::place::Rect),
}

impl PlaceRequest {
    fn kebab(self) -> &'static str {
        match self {
            Self::Catalog(action) => action.kebab(),
            Self::Frame(_) => "frame",
        }
    }

    fn spectacle_id(self) -> &'static str {
        match self {
            Self::Catalog(action) => action.spectacle_id(),
            // Not a Spectacle constant: `frame` is agenterm's own closed id.
            Self::Frame(_) => "AgentermWindowActionFrame",
        }
    }

    fn history(self) -> Option<crate::place::PlaceAction> {
        match self {
            Self::Catalog(action) if action.is_history() => Some(action),
            _ => None,
        }
    }
}

pub(super) const FRAME_MAX_EXTENT: i32 = 32_768;

pub(super) fn window_place(
    action_raw: &str,
    window: Option<isize>,
    frame: Option<[i32; 4]>,
) -> Result<serde_json::Value, CuError> {
    let action = action_raw.trim();
    let windows = mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
    let screens = mechanism::window_enumerate::list_screens().map_err(map_mechanism_err)?;
    if screens.is_empty() {
        return Err(CuError::new("failed", "no screens available"));
    }
    let target_window = if let Some(handle) = window {
        windows
            .iter()
            .find(|item| item.handle == handle)
            .ok_or_else(|| CuError::new("failed", format!("window handle {handle} not found")))?
    } else {
        windows
            .iter()
            .find(|item| item.focused)
            .or_else(|| windows.first())
            .ok_or_else(|| CuError::new("failed", "no top-level window to place"))?
    };
    let request = if action == "frame" || action == "move" || action == "resize" {
        let Some([mut x, mut y, mut width, mut height]) = frame else {
            return Err(CuError::new(
                "invalid_input",
                match action {
                    "move" => "movewin requires --window H --x X --y Y",
                    "resize" => "resize requires --window H --width W --height H",
                    _ => "window-place --action frame requires --x X --y Y --width W --height H",
                },
            ));
        };
        if action == "move" {
            width = i32::try_from(target_window.bounds.width).unwrap_or(0);
            height = i32::try_from(target_window.bounds.height).unwrap_or(0);
        } else if action == "resize" {
            x = target_window.bounds.x;
            y = target_window.bounds.y;
        }
        if width <= 0 || height <= 0 {
            return Err(CuError::new(
                "invalid_input",
                format!("frame width and height must be positive, got {width}x{height}"),
            ));
        }
        if [x, y, width, height]
            .iter()
            .any(|value| value.abs() > FRAME_MAX_EXTENT)
        {
            return Err(CuError::new(
                "invalid_input",
                format!("frame coordinates must be within ±{FRAME_MAX_EXTENT}"),
            ));
        }
        PlaceRequest::Frame(crate::place::Rect::new(
            f64::from(x),
            f64::from(y),
            f64::from(width),
            f64::from(height),
        ))
    } else {
        if frame.is_some() {
            return Err(CuError::new(
                "invalid_input",
                format!("--x/--y/--width/--height belong to --action frame, not '{action_raw}'"),
            ));
        }
        PlaceRequest::Catalog(crate::place::PlaceAction::parse(action_raw).ok_or_else(|| {
            CuError::new(
                "invalid_input",
                format!("unknown window-place action '{action_raw}'"),
            )
        })?)
    };
    let history = crate::place::PlaceHistory::open()
        .map_err(|error| CuError::new("failed", format!("history: {error}")))?;
    window_place_transaction(
        request,
        target_window,
        &screens,
        history,
        &mut NativePlaceRuntime,
        &mut NativeHistoryCommitter,
    )
}

#[derive(Clone, Debug)]
pub(super) struct PlaceIdentity {
    handle: isize,
    process_id: u32,
    app_name: String,
}

pub(super) trait PlaceRuntime {
    fn read_rect(&mut self, handle: isize) -> Result<crate::place::Rect, CuError>;
    fn inspect_placement(
        &mut self,
        handle: isize,
        expected_pid: u32,
    ) -> Result<mechanism::window_placement::PlacementWindowInfo, CuError>;
    fn apply_rect(
        &mut self,
        handle: isize,
        target: crate::place::Rect,
        visible: crate::place::Rect,
    ) -> Result<(crate::place::Rect, bool, bool), CuError>;
    fn identity_matches(&mut self, identity: &PlaceIdentity) -> Result<bool, CuError>;
}

pub(super) struct NativePlaceRuntime;

impl PlaceRuntime for NativePlaceRuntime {
    fn read_rect(&mut self, handle: isize) -> Result<crate::place::Rect, CuError> {
        crate::place::read_rect(handle).map_err(map_mechanism_err)
    }

    fn inspect_placement(
        &mut self,
        handle: isize,
        expected_pid: u32,
    ) -> Result<mechanism::window_placement::PlacementWindowInfo, CuError> {
        mechanism::window_placement::inspect(handle, expected_pid).map_err(map_mechanism_err)
    }

    fn apply_rect(
        &mut self,
        handle: isize,
        target: crate::place::Rect,
        visible: crate::place::Rect,
    ) -> Result<(crate::place::Rect, bool, bool), CuError> {
        crate::place::apply_rect(handle, target, visible).map_err(map_mechanism_err)
    }

    fn identity_matches(&mut self, identity: &PlaceIdentity) -> Result<bool, CuError> {
        let windows =
            mechanism::window_enumerate::enumerate_top_level().map_err(map_mechanism_err)?;
        Ok(windows.iter().any(|window| {
            window.handle == identity.handle
                && window.process_id == identity.process_id
                && window.app_name == identity.app_name
        }))
    }
}

#[derive(Debug)]
pub(super) struct HistoryCommitFailure {
    message: String,
    published: bool,
}

pub(super) trait HistoryCommitter {
    fn commit(&mut self, history: &crate::place::PlaceHistory) -> Result<(), HistoryCommitFailure>;
}

pub(super) struct NativeHistoryCommitter;

impl HistoryCommitter for NativeHistoryCommitter {
    fn commit(&mut self, history: &crate::place::PlaceHistory) -> Result<(), HistoryCommitFailure> {
        history.save().map_err(|error| HistoryCommitFailure {
            message: error.to_string(),
            published: error.published(),
        })
    }
}

#[cfg(test)]
pub(super) fn window_place_resolved<R, H>(
    action: crate::place::PlaceAction,
    target_window: &mechanism::window_enumerate::WindowInfo,
    screens: &[mechanism::window_enumerate::ScreenInfo],
    history: crate::place::PlaceHistory,
    runtime: &mut R,
    committer: &mut H,
) -> Result<serde_json::Value, CuError>
where
    R: PlaceRuntime,
    H: HistoryCommitter,
{
    window_place_transaction(
        PlaceRequest::Catalog(action),
        target_window,
        screens,
        history,
        runtime,
        committer,
    )
}

pub(super) fn window_place_transaction<R, H>(
    request: PlaceRequest,
    target_window: &mechanism::window_enumerate::WindowInfo,
    screens: &[mechanism::window_enumerate::ScreenInfo],
    history: crate::place::PlaceHistory,
    runtime: &mut R,
    committer: &mut H,
) -> Result<serde_json::Value, CuError>
where
    R: PlaceRuntime,
    H: HistoryCommitter,
{
    let identity = PlaceIdentity {
        handle: target_window.handle,
        process_id: target_window.process_id,
        app_name: target_window.app_name.clone(),
    };
    let app_key = format!("{}:{}", identity.process_id, identity.app_name);
    let before = runtime.read_rect(identity.handle).map_err(|error| {
        CuError::new(
            "window_state_unavailable",
            format!(
                "could not read exact window bounds before placement: {error_message}",
                error_message = error.message
            ),
        )
        .with_detail(serde_json::json!({
            "stage": "read_before",
            "effect": "not_applied",
            "history": "unchanged",
            "window": identity.handle,
            "cause": error_payload(&error),
        }))
    })?;
    let geo_screens: Vec<_> = screens.iter().map(crate::place::screen_from_info).collect();

    let (requested_target, planned_history) = if let Some(action) = request.history() {
        let step = if matches!(action, crate::place::PlaceAction::Undo) {
            history.plan_undo(&app_key)
        } else {
            history.plan_redo(&app_key)
        };
        let Some((planned, rect)) = step else {
            return Err(CuError::new(
                "unsupported",
                format!("{} has no {} history", app_key, action.kebab()),
            ));
        };
        (rect, Some(planned))
    } else {
        let dest = match request {
            PlaceRequest::Frame(rect) => rect,
            PlaceRequest::Catalog(action) => crate::place::place(action, before, &geo_screens)
                .ok_or_else(|| CuError::new("failed", "could not compute destination rectangle"))?,
        };
        (dest, None)
    };

    let inspection = runtime
        .inspect_placement(identity.handle, identity.process_id)
        .map_err(|error| {
            CuError::new(error.code.clone(), error.message.clone()).with_detail(serde_json::json!({
                "stage": "placement_preflight",
                "effect": "not_applied",
                "history": "unchanged",
                "window": identity.handle,
                "app": app_key,
                "cause": error_payload(&error),
            }))
        })?;
    let constraint = placement_target(before, requested_target, inspection).map_err(|error| {
        CuError::new(error.code.clone(), error.message.clone()).with_detail(serde_json::json!({
            "stage": "placement_preflight",
            "effect": "not_applied",
            "history": "unchanged",
            "window": identity.handle,
            "app": app_key,
            "cause": error_payload(&error),
        }))
    })?;
    let after_target = constraint.target;

    let (screen_index, screen) = screen_for_rect(after_target, screens)
        .ok_or_else(|| CuError::new("failed", "could not resolve destination screen"))?;
    let visible = crate::place::rect_from_bounds(screen.visible);
    let rollback_visible = screen_for_rect(before, screens)
        .map(|(_, screen)| crate::place::rect_from_bounds(screen.visible))
        .unwrap_or(visible);
    match runtime.identity_matches(&identity) {
        Ok(true) => {}
        Ok(false) => {
            return Err(CuError::new(
                "window_identity_changed",
                "selected window identity changed before placement",
            )
            .with_detail(serde_json::json!({
                "stage": "identity_before_apply",
                "effect": "not_applied",
                "history": "unchanged",
                "window": identity.handle,
                "app": app_key,
            })));
        }
        Err(error) => {
            return Err(CuError::new(
                "window_identity_unavailable",
                format!(
                    "could not revalidate selected window identity: {}",
                    error.message
                ),
            )
            .with_detail(serde_json::json!({
                "stage": "identity_before_apply",
                "effect": "not_applied",
                "history": "unchanged",
                "window": identity.handle,
                "app": app_key,
                "cause": error_payload(&error),
            })));
        }
    }
    let current_before_apply = runtime.read_rect(identity.handle).map_err(|error| {
        CuError::new(
            "window_state_unavailable",
            format!(
                "could not revalidate window bounds before placement: {}",
                error.message
            ),
        )
        .with_detail(serde_json::json!({
            "stage": "state_before_apply",
            "effect": "not_applied",
            "history": "unchanged",
            "window": identity.handle,
            "app": app_key,
            "before": rect_payload(before),
            "cause": error_payload(&error),
        }))
    })?;
    if current_before_apply != before {
        return Err(CuError::new(
            "window_state_changed",
            "window bounds changed while placement was being prepared",
        )
        .with_detail(serde_json::json!({
            "stage": "state_before_apply",
            "effect": "not_applied",
            "history": "unchanged",
            "window": identity.handle,
            "app": app_key,
            "before": rect_payload(before),
            "observed": rect_payload(current_before_apply),
        })));
    }
    let (after, quantized, clamped) =
        match runtime.apply_rect(identity.handle, after_target, visible) {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(recover_after_place_failure(
                    error,
                    runtime,
                    PlaceRecovery {
                        stage: "actuation",
                        identity: &identity,
                        before,
                        intended: after_target,
                        expected_current: None,
                        rollback_visible,
                        history_state: "unchanged",
                    },
                ));
            }
        };
    let next_history = planned_history
        .unwrap_or_else(|| history.plan_record(&app_key, identity.handle, before, after));
    if let Err(error) = committer.commit(&next_history) {
        if error.published {
            return Err(CuError::new(
                "history_durability_uncertain",
                format!(
                    "history was published but its directory durability is uncertain: {}",
                    error.message
                ),
            )
            .with_detail(serde_json::json!({
                "stage": "history_commit",
                "effect": "committed",
                "history": "published_durability_uncertain",
                "window": identity.handle,
                "app": app_key,
                "action": request.kebab(),
                "before": rect_payload(before),
                "intended": rect_payload(after_target),
                "applied": rect_payload(after),
                "cause": { "code": "history_sync_failed", "message": error.message },
            })));
        }
        return Err(recover_after_place_failure(
            CuError::new("history_commit_failed", error.message),
            runtime,
            PlaceRecovery {
                stage: "history_commit",
                identity: &identity,
                before,
                intended: after_target,
                expected_current: Some(after),
                rollback_visible,
                history_state: "unchanged",
            },
        ));
    }

    let constraint_adjusted = constraint.adjusted
        || (constraint.mode == "application_enforced" && !after.almost_eq(after_target));
    Ok(serde_json::json!({
        "effect": "committed",
        "history": "committed",
        "action": request.kebab(),
        "spectacle_id": request.spectacle_id(),
        "window": identity.handle,
        "app": app_key,
        "screen": {
            "index": screen_index,
            "frame": screen.frame,
            "visible": screen.visible,
            "primary": screen.primary,
        },
        "before": { "x": before.x, "y": before.y, "width": before.width, "height": before.height },
        "after": { "x": after.x, "y": after.y, "width": after.width, "height": after.height },
        "quantized": quantized,
        "clamped": clamped,
        "constraint_mode": constraint.mode,
        "constraint_adjusted": constraint_adjusted,
    }))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlacementTarget {
    target: crate::place::Rect,
    mode: &'static str,
    adjusted: bool,
}

pub(super) fn placement_target(
    before: crate::place::Rect,
    requested: crate::place::Rect,
    inspection: mechanism::window_placement::PlacementWindowInfo,
) -> Result<PlacementTarget, CuError> {
    use mechanism::window_placement::{PlacementRole, SizeConstraints, Support};

    if !matches!(
        inspection.role,
        PlacementRole::Standard | PlacementRole::Dialog
    ) {
        return Err(CuError::new(
            "window_role_refused",
            format!(
                "window role {:?} is not eligible for placement",
                inspection.role
            ),
        ));
    }
    let (bx, by, bw, bh) = before.to_i32();
    let (rx, ry, rw, rh) = requested.to_i32();
    let moves = (bx, by) != (rx, ry);
    let resizes = (bw, bh) != (rw, rh);
    if moves && inspection.movable != Support::Yes {
        return Err(CuError::new(
            "window_not_movable",
            format!(
                "window movable support is {:?}, not Yes",
                inspection.movable
            ),
        ));
    }
    if resizes && inspection.resizable != Support::Yes {
        return Err(CuError::new(
            "window_not_resizable",
            format!(
                "window resizable support is {:?}, not Yes",
                inspection.resizable
            ),
        ));
    }
    match inspection.constraints {
        SizeConstraints::Unknown if resizes => Err(CuError::new(
            "window_constraints_unknown",
            "window size constraints are unknown; refusing resize",
        )),
        SizeConstraints::Unknown => Ok(PlacementTarget {
            target: requested,
            mode: "unknown",
            adjusted: false,
        }),
        SizeConstraints::ApplicationEnforced => Ok(PlacementTarget {
            target: requested,
            mode: "application_enforced",
            adjusted: false,
        }),
        SizeConstraints::Explicit {
            min,
            max,
            increment,
        } => {
            if !resizes {
                return Ok(PlacementTarget {
                    target: requested,
                    mode: "explicit",
                    adjusted: false,
                });
            }
            let normalize_axis = |value: u32,
                                  min: Option<u32>,
                                  max: Option<u32>,
                                  increment: Option<u32>|
             -> Result<u32, CuError> {
                let lower = min.unwrap_or(1);
                let upper = max.unwrap_or(u32::MAX);
                let mut normalized = value.clamp(lower, upper);
                if let Some(step) = increment {
                    let base = u64::from(min.unwrap_or(0));
                    let step = u64::from(step);
                    let lower_delta = u64::from(lower).saturating_sub(base);
                    let upper_delta = u64::from(upper).saturating_sub(base);
                    let first = lower_delta.div_ceil(step);
                    let last = upper_delta / step;
                    if first > last {
                        return Err(CuError::new(
                            "window_constraints_invalid",
                            "size increment has no value inside the min/max range",
                        ));
                    }
                    let desired_delta = u64::from(normalized).saturating_sub(base);
                    let nearest = (desired_delta + step / 2) / step;
                    let steps = nearest.clamp(first, last);
                    normalized = u32::try_from(base + steps * step).map_err(|_| {
                        CuError::new(
                            "window_constraints_invalid",
                            "normalized size exceeds the ABI dimension range",
                        )
                    })?;
                }
                Ok(normalized)
            };
            let width = normalize_axis(
                rw,
                min.map(|s| s.width),
                max.map(|s| s.width),
                increment.map(|s| s.width),
            )?;
            let height = normalize_axis(
                rh,
                min.map(|s| s.height),
                max.map(|s| s.height),
                increment.map(|s| s.height),
            )?;
            let target = crate::place::Rect::new(rx as f64, ry as f64, width as f64, height as f64);
            Ok(PlacementTarget {
                adjusted: !target.almost_eq(requested),
                target,
                mode: "explicit",
            })
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct PlaceRecovery<'a> {
    stage: &'a str,
    identity: &'a PlaceIdentity,
    before: crate::place::Rect,
    intended: crate::place::Rect,
    expected_current: Option<crate::place::Rect>,
    rollback_visible: crate::place::Rect,
    history_state: &'a str,
}

pub(super) fn recover_after_place_failure<R: PlaceRuntime>(
    cause: CuError,
    runtime: &mut R,
    recovery: PlaceRecovery<'_>,
) -> CuError {
    let PlaceRecovery {
        stage,
        identity,
        before,
        intended,
        expected_current,
        rollback_visible,
        history_state,
    } = recovery;
    let observed = match runtime.read_rect(identity.handle) {
        Ok(observed) if observed == before => {
            return CuError::new(
                if stage == "history_commit" {
                    "history_commit_failed"
                } else {
                    "window_place_failed"
                },
                format!("{}; window bounds remained unchanged", cause.message),
            )
            .with_detail(serde_json::json!({
                "stage": stage,
                "effect": "not_applied",
                "history": history_state,
                "rollback": "not_needed",
                "window": identity.handle,
                "app": format!("{}:{}", identity.process_id, identity.app_name),
                "before": rect_payload(before),
                "intended": rect_payload(intended),
                "observed": rect_payload(observed),
                "cause": error_payload(&cause),
            }));
        }
        Ok(observed) => {
            if expected_current.is_none() {
                return in_doubt_error(
                    stage,
                    history_state,
                    identity,
                    before,
                    intended,
                    Some(observed),
                    "skipped_unverified_apply_state",
                    &cause,
                    None,
                );
            }
            if expected_current.is_some_and(|expected| observed != expected) {
                return in_doubt_error(
                    stage,
                    history_state,
                    identity,
                    before,
                    intended,
                    Some(observed),
                    "skipped_external_change",
                    &cause,
                    None,
                );
            }
            observed
        }
        Err(read_error) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                None,
                "readback_failed",
                &cause,
                Some(&read_error),
            );
        }
    };
    match runtime.identity_matches(identity) {
        Ok(true) => {}
        Ok(false) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                Some(observed),
                "skipped_identity_changed",
                &cause,
                None,
            );
        }
        Err(identity_error) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                Some(observed),
                "identity_check_failed",
                &cause,
                Some(&identity_error),
            );
        }
    }
    match runtime.read_rect(identity.handle) {
        Ok(current) if current == observed => {}
        Ok(current) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                Some(current),
                "skipped_external_change",
                &cause,
                None,
            );
        }
        Err(read_error) => {
            return in_doubt_error(
                stage,
                history_state,
                identity,
                before,
                intended,
                None,
                "rollback_read_failed",
                &cause,
                Some(&read_error),
            );
        }
    }
    if let Err(rollback_error) = runtime.apply_rect(identity.handle, before, rollback_visible) {
        let current = runtime.read_rect(identity.handle).ok();
        return in_doubt_error(
            stage,
            history_state,
            identity,
            before,
            intended,
            current,
            "rollback_failed",
            &cause,
            Some(&rollback_error),
        );
    }
    match runtime.read_rect(identity.handle) {
        Ok(restored) if restored == before => CuError::new(
            if stage == "history_commit" {
                "history_commit_failed"
            } else {
                "window_place_failed"
            },
            format!("{}; window placement was rolled back", cause.message),
        )
        .with_detail(serde_json::json!({
            "stage": stage,
            "effect": "rolled_back",
            "history": history_state,
            "rollback": "verified",
            "window": identity.handle,
            "app": format!("{}:{}", identity.process_id, identity.app_name),
            "before": rect_payload(before),
            "intended": rect_payload(intended),
            "applied": rect_payload(observed),
            "observed": rect_payload(restored),
            "cause": error_payload(&cause),
        })),
        Ok(restored) => in_doubt_error(
            stage,
            history_state,
            identity,
            before,
            intended,
            Some(restored),
            "rollback_unverified",
            &cause,
            None,
        ),
        Err(read_error) => in_doubt_error(
            stage,
            history_state,
            identity,
            before,
            intended,
            None,
            "rollback_readback_failed",
            &cause,
            Some(&read_error),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn in_doubt_error(
    stage: &str,
    history_state: &str,
    identity: &PlaceIdentity,
    before: crate::place::Rect,
    intended: crate::place::Rect,
    observed: Option<crate::place::Rect>,
    rollback: &str,
    cause: &CuError,
    rollback_error: Option<&CuError>,
) -> CuError {
    let mut detail = serde_json::json!({
        "stage": stage,
        "effect": "possibly_applied",
        "history": history_state,
        "rollback": rollback,
        "window": identity.handle,
        "app": format!("{}:{}", identity.process_id, identity.app_name),
        "before": rect_payload(before),
        "intended": rect_payload(intended),
        "cause": error_payload(cause),
    });
    if let Some(observed) = observed {
        detail["observed"] = rect_payload(observed);
    }
    if let Some(error) = rollback_error {
        detail["rollback_error"] = error_payload(error);
    }
    CuError::new(
        "window_place_in_doubt",
        format!(
            "window placement may have changed the window and could not be verified or restored: {}",
            cause.message
        ),
    )
    .with_detail(detail)
}

pub(super) fn rect_payload(rect: crate::place::Rect) -> serde_json::Value {
    serde_json::json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height,
    })
}

pub(super) fn screen_for_rect(
    rect: crate::place::Rect,
    screens: &[mechanism::window_enumerate::ScreenInfo],
) -> Option<(usize, &mechanism::window_enumerate::ScreenInfo)> {
    let mut best: Option<(f64, usize, &mechanism::window_enumerate::ScreenInfo)> = None;
    for (index, screen) in screens.iter().enumerate() {
        let frame = crate::place::rect_from_bounds(screen.frame);
        if frame.contains(rect) {
            return Some((index, screen));
        }
        if let Some(hit) = rect.intersection(frame) {
            let proportion = hit.area() / rect.area().max(1.0);
            if best
                .as_ref()
                .map(|(current, _, _)| proportion > *current)
                .unwrap_or(true)
            {
                best = Some((proportion, index, screen));
            }
        }
    }
    best.map(|(_, index, screen)| (index, screen))
        .or_else(|| screens.first().map(|screen| (0, screen)))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{collections::VecDeque, sync::atomic::Ordering};

    enum FakeApply {
        Ok {
            actual: crate::place::Rect,
            quantized: bool,
            clamped: bool,
        },
        Err {
            error: CuError,
            observed: crate::place::Rect,
        },
    }

    struct FakePlaceRuntime {
        rect: crate::place::Rect,
        first_read_error: Option<CuError>,
        inspections: VecDeque<Result<mechanism::window_placement::PlacementWindowInfo, CuError>>,
        inspect_args: Vec<(isize, u32)>,
        identities: VecDeque<Result<bool, CuError>>,
        applies: VecDeque<FakeApply>,
        apply_handles: Vec<isize>,
    }

    impl FakePlaceRuntime {
        fn new(rect: crate::place::Rect, applies: impl IntoIterator<Item = FakeApply>) -> Self {
            Self {
                rect,
                first_read_error: None,
                inspections: VecDeque::from([Ok(placement_fixture())]),
                inspect_args: Vec::new(),
                identities: VecDeque::from([Ok(true), Ok(true)]),
                applies: applies.into_iter().collect(),
                apply_handles: Vec::new(),
            }
        }
    }

    impl PlaceRuntime for FakePlaceRuntime {
        fn read_rect(&mut self, _handle: isize) -> Result<crate::place::Rect, CuError> {
            if let Some(error) = self.first_read_error.take() {
                return Err(error);
            }
            Ok(self.rect)
        }

        fn inspect_placement(
            &mut self,
            handle: isize,
            expected_pid: u32,
        ) -> Result<mechanism::window_placement::PlacementWindowInfo, CuError> {
            self.inspect_args.push((handle, expected_pid));
            self.inspections
                .pop_front()
                .unwrap_or_else(|| Ok(placement_fixture()))
        }

        fn apply_rect(
            &mut self,
            handle: isize,
            _target: crate::place::Rect,
            _visible: crate::place::Rect,
        ) -> Result<(crate::place::Rect, bool, bool), CuError> {
            self.apply_handles.push(handle);
            match self.applies.pop_front().expect("scripted apply outcome") {
                FakeApply::Ok {
                    actual,
                    quantized,
                    clamped,
                } => {
                    self.rect = actual;
                    Ok((actual, quantized, clamped))
                }
                FakeApply::Err { error, observed } => {
                    self.rect = observed;
                    Err(error)
                }
            }
        }

        fn identity_matches(&mut self, _identity: &PlaceIdentity) -> Result<bool, CuError> {
            self.identities.pop_front().unwrap_or(Ok(true))
        }
    }

    struct SavingHistory;

    impl HistoryCommitter for SavingHistory {
        fn commit(
            &mut self,
            history: &crate::place::PlaceHistory,
        ) -> Result<(), HistoryCommitFailure> {
            history.save().map_err(|error| HistoryCommitFailure {
                message: error.to_string(),
                published: error.published(),
            })
        }
    }

    struct FailingHistory {
        published: bool,
    }

    impl HistoryCommitter for FailingHistory {
        fn commit(
            &mut self,
            _history: &crate::place::PlaceHistory,
        ) -> Result<(), HistoryCommitFailure> {
            Err(HistoryCommitFailure {
                message: "injected history commit failure".into(),
                published: self.published,
            })
        }
    }

    fn saga_scratch(label: &str) -> PathBuf {
        let sequence = NEXT_AUDIT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "agenterm-cu-saga-{label}-{}-{sequence}",
                std::process::id()
            ))
            .join("history.json")
    }

    fn saga_window(bounds: crate::place::Rect) -> mechanism::window_enumerate::WindowInfo {
        let (x, y, width, height) = bounds.to_i32();
        mechanism::window_enumerate::WindowInfo {
            handle: 7,
            title: "fixture".into(),
            process_id: 42,
            app_name: "fixture-app".into(),
            bounds: mechanism::window_enumerate::WindowBounds {
                x,
                y,
                width,
                height,
            },
            focused: true,
            minimized: false,
        }
    }

    fn saga_screen() -> mechanism::window_enumerate::ScreenInfo {
        let bounds = mechanism::window_enumerate::WindowBounds {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        mechanism::window_enumerate::ScreenInfo {
            frame: bounds,
            visible: bounds,
            primary: true,
        }
    }

    fn saga_rect(x: f64, y: f64, width: f64, height: f64) -> crate::place::Rect {
        crate::place::Rect::new(x, y, width, height)
    }

    fn placement_fixture() -> mechanism::window_placement::PlacementWindowInfo {
        use mechanism::window_placement::{
            PlacementRole, PlacementWindowInfo, SizeConstraints, Support,
        };
        PlacementWindowInfo {
            handle: 7,
            process_id: 42,
            role: PlacementRole::Standard,
            movable: Support::Yes,
            resizable: Support::Yes,
            constraints: SizeConstraints::Explicit {
                min: None,
                max: None,
                increment: None,
            },
        }
    }

    fn remove_saga_scratch(path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn placement_roles_and_unknown_support_fail_closed() {
        use mechanism::window_placement::{PlacementRole, Support};
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let moved = saga_rect(200.0, 100.0, 800.0, 600.0);
        for role in [
            PlacementRole::Sheet,
            PlacementRole::SystemDialog,
            PlacementRole::Other,
            PlacementRole::Unknown,
        ] {
            let mut info = placement_fixture();
            info.role = role;
            assert_eq!(
                placement_target(before, moved, info).unwrap_err().code,
                "window_role_refused"
            );
        }
        for role in [PlacementRole::Standard, PlacementRole::Dialog] {
            let mut info = placement_fixture();
            info.role = role;
            info.movable = Support::Unknown;
            assert_eq!(
                placement_target(before, moved, info).unwrap_err().code,
                "window_not_movable"
            );
            info.movable = Support::Yes;
            info.resizable = Support::Unknown;
            assert_eq!(
                placement_target(before, saga_rect(100.0, 100.0, 900.0, 600.0), info)
                    .unwrap_err()
                    .code,
                "window_not_resizable"
            );
        }
    }

    #[test]
    fn explicit_constraints_clamp_and_quantize_requested_size() {
        use mechanism::window_placement::{SizeConstraints, WindowSize};
        let before = saga_rect(10.0, 20.0, 400.0, 300.0);
        let mut info = placement_fixture();
        info.constraints = SizeConstraints::Explicit {
            min: Some(WindowSize {
                width: 300,
                height: 200,
            }),
            max: Some(WindowSize {
                width: 800,
                height: 700,
            }),
            increment: Some(WindowSize {
                width: 50,
                height: 20,
            }),
        };
        let result = placement_target(before, saga_rect(10.0, 20.0, 503.0, 407.0), info)
            .expect("explicit normalization");
        assert_eq!(result.mode, "explicit");
        assert!(result.adjusted);
        assert_eq!(result.target, saga_rect(10.0, 20.0, 500.0, 400.0));

        info.constraints = SizeConstraints::Unknown;
        assert_eq!(
            placement_target(before, saga_rect(10.0, 20.0, 500.0, 300.0), info)
                .unwrap_err()
                .code,
            "window_constraints_unknown"
        );
    }

    #[test]
    fn application_enforced_constraints_use_final_readback_and_expected_pid() {
        use mechanism::window_placement::SizeConstraints;
        let path = saga_scratch("application-enforced");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let actual = saga_rect(0.0, 0.0, 900.0, 1000.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [FakeApply::Ok {
                actual,
                quantized: false,
                clamped: false,
            }],
        );
        let mut info = placement_fixture();
        info.constraints = SizeConstraints::ApplicationEnforced;
        runtime.inspections = VecDeque::from([Ok(info)]);
        let reply = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect("application-enforced placement");
        assert_eq!(runtime.inspect_args, [(7, 42)]);
        assert_eq!(reply["constraint_mode"], "application_enforced");
        assert_eq!(reply["constraint_adjusted"], true);
        assert_eq!(reply["after"], rect_payload(actual));
        remove_saga_scratch(&path);
    }

    #[test]
    fn window_place_strict_read_failure_has_no_side_effect() {
        let path = saga_scratch("strict-read");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(before, []);
        runtime.first_read_error = Some(CuError::new("read_failed", "injected strict read"));
        let error = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect_err("strict read must fail");
        assert_eq!(error.code, "window_state_unavailable");
        assert_eq!(error.detail.as_ref().unwrap()["effect"], "not_applied");
        assert!(runtime.apply_handles.is_empty());
        assert!(!path.exists());
        remove_saga_scratch(&path);
    }

    #[test]
    fn history_commit_failure_rolls_window_back_and_retains_bytes() {
        let path = saga_scratch("commit-rollback");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let after = saga_rect(0.0, 0.0, 960.0, 1040.0);
        let history = crate::place::PlaceHistory::open_at(path.clone())
            .expect("history")
            .plan_record("seed", 1, before, before);
        history.save().expect("seed history");
        let old_bytes = std::fs::read(&path).expect("old history bytes");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [
                FakeApply::Ok {
                    actual: after,
                    quantized: false,
                    clamped: false,
                },
                FakeApply::Ok {
                    actual: before,
                    quantized: false,
                    clamped: false,
                },
            ],
        );
        let error = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut FailingHistory { published: false },
        )
        .expect_err("history commit must fail");
        assert_eq!(error.code, "history_commit_failed");
        assert_eq!(error.detail.as_ref().unwrap()["effect"], "rolled_back");
        assert!(runtime.rect.almost_eq(before));
        assert_eq!(runtime.apply_handles, [7, 7]);
        assert_eq!(std::fs::read(&path).expect("retained history"), old_bytes);
        remove_saga_scratch(&path);
    }

    #[test]
    fn partial_apply_failure_does_not_overwrite_unverified_window_state() {
        let path = saga_scratch("apply-rollback");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let partial = saga_rect(0.0, 0.0, 970.0, 1040.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [FakeApply::Err {
                error: CuError::new("readback_failed", "injected apply readback failure"),
                observed: partial,
            }],
        );
        let error = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect_err("partial apply must fail");
        assert_eq!(error.code, "window_place_in_doubt");
        assert_eq!(error.detail.as_ref().unwrap()["effect"], "possibly_applied");
        assert_eq!(
            error.detail.as_ref().unwrap()["rollback"],
            "skipped_unverified_apply_state"
        );
        assert!(runtime.rect.almost_eq(partial));
        assert_eq!(runtime.apply_handles, [7]);
        assert!(
            !path.exists(),
            "history must not commit after apply failure"
        );
        remove_saga_scratch(&path);
    }

    #[test]
    fn history_commit_and_rollback_failure_is_structured_in_doubt() {
        let path = saga_scratch("commit-in-doubt");
        let before = saga_rect(100.0, 100.0, 800.0, 600.0);
        let after = saga_rect(0.0, 0.0, 960.0, 1040.0);
        let partial = saga_rect(4.0, 4.0, 950.0, 1030.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [
                FakeApply::Ok {
                    actual: after,
                    quantized: false,
                    clamped: false,
                },
                FakeApply::Err {
                    error: CuError::new("rollback_failed", "injected rollback failure"),
                    observed: partial,
                },
            ],
        );
        let error = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut FailingHistory { published: false },
        )
        .expect_err("rollback failure must be in doubt");
        let detail = error.detail.as_ref().expect("structured detail");
        assert_eq!(error.code, "window_place_in_doubt");
        assert_eq!(detail["effect"], "possibly_applied");
        assert_eq!(detail["rollback"], "rollback_failed");
        assert_eq!(detail["observed"], rect_payload(partial));
        assert!(!path.exists(), "failed commit must not create history");
        remove_saga_scratch(&path);
    }

    #[test]
    fn undo_uses_current_validated_handle_not_stored_historical_handle() {
        let path = saga_scratch("stale-handle");
        let original = saga_rect(100.0, 100.0, 800.0, 600.0);
        let current = saga_rect(0.0, 0.0, 960.0, 1040.0);
        let history = crate::place::PlaceHistory::open_at(path.clone())
            .expect("history")
            .plan_record("42:fixture-app", 999, original, current);
        history.save().expect("seed history");
        let mut runtime = FakePlaceRuntime::new(
            current,
            [FakeApply::Ok {
                actual: original,
                quantized: false,
                clamped: false,
            }],
        );
        let reply = window_place_resolved(
            crate::place::PlaceAction::Undo,
            &saga_window(current),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect("undo");
        assert_eq!(reply["window"], 7);
        assert_eq!(runtime.apply_handles, [7]);
        let reopened = crate::place::PlaceHistory::open_at(path.clone()).expect("reopen");
        let (_, redo) = reopened.plan_redo("42:fixture-app").expect("redo remains");
        assert!(redo.almost_eq(current));
        remove_saga_scratch(&path);
    }

    #[test]
    fn quantized_final_readback_is_the_history_record() {
        let path = saga_scratch("quantized-final");
        let before = saga_rect(100.0, 100.0, 801.0, 601.0);
        let actual = saga_rect(0.0, 0.0, 958.0, 1038.0);
        let history = crate::place::PlaceHistory::open_at(path.clone()).expect("history");
        let mut runtime = FakePlaceRuntime::new(
            before,
            [FakeApply::Ok {
                actual,
                quantized: true,
                clamped: false,
            }],
        );
        let reply = window_place_resolved(
            crate::place::PlaceAction::LeftHalf,
            &saga_window(before),
            &[saga_screen()],
            history,
            &mut runtime,
            &mut SavingHistory,
        )
        .expect("place");
        assert_eq!(reply["quantized"], true);
        assert_eq!(reply["after"], rect_payload(actual));
        let reopened = crate::place::PlaceHistory::open_at(path.clone()).expect("reopen");
        let (undone, undo_target) = reopened.plan_undo("42:fixture-app").expect("undo");
        assert!(undo_target.almost_eq(before));
        let (_, redo_target) = undone.plan_redo("42:fixture-app").expect("redo");
        assert!(redo_target.almost_eq(actual));
        remove_saga_scratch(&path);
    }

    #[test]
    fn window_place_without_grant_is_refused() {
        let auth = Authorization::new(Default::default());
        let executor = Executor::new(auth);
        let command = Command::WindowPlace {
            target: TargetRef::Current,
            action: "left-half".into(),
            window: None,
            frame: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "refused");
    }

    #[test]
    fn window_place_unknown_action_is_invalid() {
        let auth = Authorization::new([Grant::Actuate].into_iter().collect());
        let executor = Executor::new(auth);
        let command = Command::WindowPlace {
            target: TargetRef::Current,
            action: "tile-magic".into(),
            window: None,
            frame: None,
        };
        let reply = executor.execute(&command);
        assert!(!reply.ok);
        assert_eq!(reply.error.as_ref().unwrap().code, "invalid_input");
    }

    #[test]
    fn window_place_resolves_the_destination_screen_for_clamp_and_reply() {
        use mechanism::window_enumerate::{ScreenInfo, WindowBounds};

        let screens = [
            ScreenInfo {
                frame: WindowBounds {
                    x: 0,
                    y: 0,
                    width: 1000,
                    height: 800,
                },
                visible: WindowBounds {
                    x: 0,
                    y: 40,
                    width: 1000,
                    height: 760,
                },
                primary: true,
            },
            ScreenInfo {
                frame: WindowBounds {
                    x: 1000,
                    y: 0,
                    width: 1200,
                    height: 900,
                },
                visible: WindowBounds {
                    x: 1000,
                    y: 0,
                    width: 1200,
                    height: 860,
                },
                primary: false,
            },
        ];

        let (index, screen) = screen_for_rect(
            crate::place::Rect::new(1300.0, 200.0, 500.0, 400.0),
            &screens,
        )
        .expect("destination screen");
        assert_eq!(index, 1);
        assert_eq!(screen.visible, screens[1].visible);

        let (index, _) = screen_for_rect(
            crate::place::Rect::new(850.0, 100.0, 500.0, 300.0),
            &screens,
        )
        .expect("largest intersection screen");
        assert_eq!(index, 1);
    }
}
