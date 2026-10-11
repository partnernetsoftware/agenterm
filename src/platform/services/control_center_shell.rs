//! Native Control Center shell product bridge over the platform crate.

use std::borrow::Cow;

pub(crate) use crate::platform::contract::control_center_shell::{
    ControlCenterFocusRequest, ControlCenterFrame, ControlCenterInputEvent, ControlCenterKey,
    ControlCenterPointerButton, ControlCenterShellError, ControlCenterShellHost,
    ControlCenterShellResult,
};

struct HostBridge {
    host: Box<dyn ControlCenterShellHost>,
}

impl minicon_platform::window::NativeTextWindowHost for HostBridge {
    fn title(&self) -> String {
        self.host.title()
    }

    fn lines(&self) -> Vec<String> {
        self.host.lines()
    }

    fn poll(&mut self) -> bool {
        self.host.poll()
    }

    fn close_requested(&self) -> bool {
        self.host.close_requested()
    }

    fn publish_native_window(
        &mut self,
        raw_handle: i64,
    ) -> Result<(), minicon_platform::window::NativeTextWindowError> {
        self.host
            .publish_native_window(raw_handle)
            .map_err(to_platform_error)
    }

    fn take_focus_request(&mut self) -> Option<minicon_platform::window::NativeTextWindowFocus> {
        self.host.take_focus_request().map(|request| match request {
            ControlCenterFocusRequest::Activate => {
                minicon_platform::window::NativeTextWindowFocus::Activate
            }
            ControlCenterFocusRequest::NoActivate => {
                minicon_platform::window::NativeTextWindowFocus::NoActivate
            }
        })
    }

    fn handle_input(
        &mut self,
        event: minicon_platform::window::NativeTextInputEvent,
    ) -> Result<bool, minicon_platform::window::NativeTextWindowError> {
        self.host
            .handle_input(from_platform_input(event).map_err(to_platform_error)?)
            .map_err(to_platform_error)
    }

    fn capture_requested_screenshot(
        &mut self,
        frame: Option<minicon_platform::window::NativeTextFrame<'_>>,
    ) -> Result<(), minicon_platform::window::NativeTextWindowError> {
        self.host
            .capture_requested_screenshot(frame.map(|frame| ControlCenterFrame {
                pixels: frame.pixels,
                width: frame.width,
                height: frame.height,
                scale_factor: frame.scale_factor,
            }))
            .map_err(to_platform_error)
    }
}

fn from_platform_input(
    event: minicon_platform::window::NativeTextInputEvent,
) -> ControlCenterShellResult<ControlCenterInputEvent> {
    Ok(match event {
        minicon_platform::window::NativeTextInputEvent::PointerPressed {
            button,
            physical_x,
            physical_y,
            line,
        } => ControlCenterInputEvent::PointerPressed {
            button: match button {
                minicon_platform::window::NativeTextPointerButton::Primary => {
                    ControlCenterPointerButton::Primary
                }
                minicon_platform::window::NativeTextPointerButton::Secondary => {
                    ControlCenterPointerButton::Secondary
                }
                minicon_platform::window::NativeTextPointerButton::Middle => {
                    ControlCenterPointerButton::Middle
                }
                _ => {
                    return Err(ControlCenterShellError::Unsupported {
                        reason: "unknown-native-pointer-button",
                    });
                }
            },
            physical_x,
            physical_y,
            line,
        },
        minicon_platform::window::NativeTextInputEvent::KeyPressed { key, repeat } => {
            ControlCenterInputEvent::KeyPressed {
                key: match key {
                    minicon_platform::window::NativeTextKey::ArrowUp => ControlCenterKey::ArrowUp,
                    minicon_platform::window::NativeTextKey::ArrowDown => {
                        ControlCenterKey::ArrowDown
                    }
                    minicon_platform::window::NativeTextKey::Home => ControlCenterKey::Home,
                    minicon_platform::window::NativeTextKey::End => ControlCenterKey::End,
                    minicon_platform::window::NativeTextKey::Enter => ControlCenterKey::Enter,
                    minicon_platform::window::NativeTextKey::Escape => ControlCenterKey::Escape,
                    _ => {
                        return Err(ControlCenterShellError::Unsupported {
                            reason: "unknown-native-text-key",
                        });
                    }
                },
                repeat,
            }
        }
        _ => {
            return Err(ControlCenterShellError::Unsupported {
                reason: "unknown-native-text-input-event",
            });
        }
    })
}

pub(crate) fn run_native_shell(
    host: Box<dyn ControlCenterShellHost>,
    no_activate: bool,
) -> ControlCenterShellResult<()> {
    minicon_platform::linux_xkb_startup::preflight().map_err(|message| {
        ControlCenterShellError::failed("linux_x11_preflight_failed", message)
    })?;
    minicon_platform::window::run_native_text_window(Box::new(HostBridge { host }), no_activate)
        .map_err(from_platform_error)
}

fn to_platform_error(
    error: ControlCenterShellError,
) -> minicon_platform::window::NativeTextWindowError {
    match error {
        ControlCenterShellError::Unsupported { reason } => {
            minicon_platform::window::NativeTextWindowError::Unsupported {
                reason: Cow::Borrowed(reason),
            }
        }
        ControlCenterShellError::Failed { code, message } => {
            minicon_platform::window::NativeTextWindowError::Failed {
                code: Cow::Borrowed(code),
                message,
            }
        }
    }
}

fn from_platform_error(
    error: minicon_platform::window::NativeTextWindowError,
) -> ControlCenterShellError {
    match error {
        minicon_platform::window::NativeTextWindowError::Unsupported { .. } => {
            ControlCenterShellError::Unsupported {
                reason: "native-text-window-unsupported",
            }
        }
        minicon_platform::window::NativeTextWindowError::Failed { code, message } => {
            ControlCenterShellError::Failed {
                code: product_error_code(code.as_ref()),
                message,
            }
        }
        _ => ControlCenterShellError::Failed {
            code: "control_center_native_window_failed",
            message: "native text window returned an unknown failure".to_owned(),
        },
    }
}

fn product_error_code(code: &str) -> &'static str {
    match code {
        "native_text_window_module_handle_failed" => "control_center_module_handle_failed",
        "native_text_window_class_register_failed" => "control_center_window_class_register_failed",
        "native_text_window_create_failed" => "control_center_window_create_failed",
        "native_text_window_timer_failed" => "control_center_window_timer_failed",
        "native_text_window_message_loop_failed" | "native_text_window_event_loop_failed" => {
            "control_center_message_loop_failed"
        }
        "native_text_window_surface_context_failed" => "control_center_surface_context_failed",
        "native_text_window_surface_create_failed" => "control_center_surface_create_failed",
        "native_text_window_surface_resize_failed" => "control_center_surface_resize_failed",
        "native_text_window_surface_buffer_failed" => "control_center_surface_buffer_failed",
        "native_text_window_surface_present_failed" => "control_center_surface_present_failed",
        _ => "control_center_native_window_failed",
    }
}
