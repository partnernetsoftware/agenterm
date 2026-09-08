use std::borrow::Cow;
use std::time::{Duration, Instant};

use crate::contract::activation::{ActivationError, ActivationRequest, NativeWindowHandle};
use crate::contract::window_op::WindowShowState;

pub(crate) fn post_application_wake(_window: NativeWindowHandle) -> Result<(), ActivationError> {
    Err(ActivationError::Unsupported {
        reason: Cow::Borrowed("native-window-wake-is-unavailable"),
    })
}

fn failed(code: &'static str, message: impl Into<String>) -> ActivationError {
    ActivationError::Failed {
        code: Cow::Borrowed(code),
        message: message.into(),
    }
}

fn map_window_op(error: crate::contract::window_op::WindowOpError) -> ActivationError {
    match error {
        crate::contract::window_op::WindowOpError::Unsupported { reason } => {
            ActivationError::Unsupported { reason }
        }
        crate::contract::window_op::WindowOpError::Failed { code, message } => {
            ActivationError::Failed { code, message }
        }
    }
}

fn verify_foreground(handle: isize) -> Result<(), ActivationError> {
    let deadline = Instant::now() + Duration::from_millis(1_500);
    while Instant::now() < deadline {
        match crate::window_enumerate::enumerate_top_level() {
            Ok(rows) if rows.iter().any(|row| row.handle == handle && row.focused) => {
                return Ok(());
            }
            Ok(_) => {}
            Err(error) => {
                return Err(failed(
                    "activation_readback_failed",
                    format!("window inventory read-back failed: {error:?}"),
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Err(failed(
        "foreground_activation_not_observed",
        "native window did not read back as the focused desktop window",
    ))
}

pub(crate) fn apply(
    window: NativeWindowHandle,
    request: ActivationRequest,
) -> Result<(), ActivationError> {
    let handle = window.raw();
    match request {
        ActivationRequest::ShowWithoutActivation => {
            crate::window_op::show(handle, WindowShowState::Show).map_err(map_window_op)
        }
        ActivationRequest::ShowNewAndRequestActivation => {
            crate::window_op::show(handle, WindowShowState::Show).map_err(map_window_op)?;
            crate::window_op::activate(handle).map_err(map_window_op)?;
            verify_foreground(handle)
        }
        ActivationRequest::RestoreAndActivate => {
            crate::window_op::show(handle, WindowShowState::Restore).map_err(map_window_op)?;
            crate::window_op::activate(handle).map_err(map_window_op)?;
            verify_foreground(handle)
        }
    }
}

pub trait WindowAttributesActivationExt: Sized {
    fn with_platform_activation(self, no_activate: bool) -> Self;
}

impl WindowAttributesActivationExt for winit::window::WindowAttributes {
    fn with_platform_activation(self, no_activate: bool) -> Self {
        self.with_active(!no_activate)
    }
}

pub trait EventLoopActivationExt {
    fn configure_platform_activation(&mut self, no_activate: bool);
}

impl<T> EventLoopActivationExt for winit::event_loop::EventLoopBuilder<T> {
    fn configure_platform_activation(&mut self, _no_activate: bool) {}
}
