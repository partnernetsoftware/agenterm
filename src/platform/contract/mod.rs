//! OS-neutral facade contracts and typed failures.

#[cfg(test)]
pub(crate) mod adapter;
pub(crate) mod control_center_shell;
pub(crate) mod ipc;
#[allow(unused_imports)]
pub(crate) use minicon_platform::contract::ipc_transport;
#[allow(unused_imports)]
pub(crate) use minicon_platform::contract::process;
#[allow(unused_imports)]
pub(crate) use minicon_platform::contract::pty;
#[allow(unused_imports)]
pub(crate) use minicon_platform::contract::runtime;
pub(crate) mod supervisor_audit;
pub(crate) mod ui_clipboard;
#[allow(unused_imports)]
pub(crate) use minicon_platform::contract::font as ui_font;
pub(crate) mod ui_screenshot;
