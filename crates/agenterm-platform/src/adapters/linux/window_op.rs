//! Linux X11 ConfigureWindow for foreign top-level windows.

use x11rb::{
    connection::Connection,
    protocol::xproto::{
        Atom, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask, StackMode,
        Window,
    },
};

use crate::CapabilityStatus;
use crate::contract::window_enumerate::WindowBounds;
use crate::contract::window_op::WindowOpError;

fn failed(message: impl ToString) -> WindowOpError {
    WindowOpError::failed("window_op_failed", message)
}

fn connect() -> Result<x11rb::rust_connection::RustConnection, WindowOpError> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("XDG_SESSION_TYPE").ok().as_deref() == Some("wayland")
        && std::env::var_os("DISPLAY").is_none()
    {
        return Err(WindowOpError::Unsupported {
            reason: "window-op requires X11; Wayland is unsupported".into(),
        });
    }
    x11rb::connect(None)
        .map(|(conn, _)| conn)
        .map_err(|error| failed(format!("X11 display could not be opened: {error}")))
}

pub(crate) fn capability_status() -> CapabilityStatus {
    if std::env::var_os("DISPLAY").is_some() {
        CapabilityStatus::Available
    } else {
        CapabilityStatus::Unsupported {
            reason: "window-op requires DISPLAY".into(),
        }
    }
}

/// Raise a window without touching focus.
///
/// `ConfigureWindow(stack_mode = Above)` is the X11 primitive for exactly
/// that: the window comes to the front and the keyboard focus stays where
/// the user left it, which is what `orderwin` means. The other show states
/// are window-manager policy (iconify, maximize) rather than a stacking
/// operation and stay typed: guessing at `_NET_WM_STATE` transitions that a
/// given WM may ignore would report success for nothing.
pub(crate) fn show(
    handle: isize,
    state: crate::contract::window_op::WindowShowState,
) -> Result<(), WindowOpError> {
    use crate::contract::window_op::WindowShowState;
    if state != WindowShowState::Show {
        return Err(WindowOpError::Unsupported {
            reason: "only the raise (Show) state is wired on Linux; iconify / maximize / restore are window-manager policy".into(),
        });
    }
    let conn = connect()?;
    let window = window_id(handle)?;
    let aux = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
    conn.configure_window(window, &aux)
        .map_err(|error| failed(format!("ConfigureWindow(raise) send failed: {error}")))?;
    conn.flush()
        .map_err(|error| failed(format!("ConfigureWindow(raise) flush failed: {error}")))?;
    Ok(())
}

fn window_id(handle: isize) -> Result<Window, WindowOpError> {
    u32::try_from(handle).map_err(|_| failed("window handle is not a valid XID"))
}

fn atom(conn: &x11rb::rust_connection::RustConnection, name: &[u8]) -> Result<Atom, WindowOpError> {
    conn.intern_atom(false, name)
        .map_err(|_| failed("an X11 atom request could not be sent"))?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|_| failed("an X11 atom request failed"))
}

fn root_of(conn: &x11rb::rust_connection::RustConnection) -> Result<Window, WindowOpError> {
    conn.setup()
        .roots
        .first()
        .map(|screen| screen.root)
        .ok_or_else(|| failed("the X11 display has no screen"))
}

/// Send one EWMH client message to the root window, which is how a pager
/// or automation tool asks the window manager to act on a window it does
/// not own.
fn send_root_message(
    conn: &x11rb::rust_connection::RustConnection,
    window: Window,
    message: Atom,
    data: [u32; 5],
) -> Result<(), WindowOpError> {
    let root = root_of(conn)?;
    let event = ClientMessageEvent::new(32, window, message, data);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
        event,
    )
    .map_err(|error| failed(format!("EWMH client message send failed: {error}")))?;
    conn.flush()
        .map_err(|error| failed(format!("EWMH client message flush failed: {error}")))?;
    Ok(())
}

pub(crate) fn move_window(
    handle: isize,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), WindowOpError> {
    let conn = connect()?;
    let window = u32::try_from(handle).map_err(|_| failed("window handle is not a valid XID"))?;
    let aux = ConfigureWindowAux::new()
        .x(x)
        .y(y)
        .width(width.max(1))
        .height(height.max(1));
    conn.configure_window(window, &aux)
        .map_err(|error| failed(format!("ConfigureWindow send failed: {error}")))?;
    conn.flush()
        .map_err(|error| failed(format!("ConfigureWindow flush failed: {error}")))?;
    Ok(())
}

pub(crate) fn window_rect(handle: isize) -> Result<WindowBounds, WindowOpError> {
    let conn = connect()?;
    let window = u32::try_from(handle).map_err(|_| failed("window handle is not a valid XID"))?;
    let geom = conn
        .get_geometry(window)
        .map_err(|error| failed(format!("GetGeometry send failed: {error}")))?
        .reply()
        .map_err(|error| failed(format!("GetGeometry failed: {error}")))?;
    Ok(WindowBounds {
        x: i32::from(geom.x),
        y: i32::from(geom.y),
        width: u32::from(geom.width),
        height: u32::from(geom.height),
    })
}

/// `_NET_WM_STATE` add/remove of `_NET_WM_STATE_ABOVE`.
///
/// 1 is `_NET_WM_STATE_ADD` and 0 is `_NET_WM_STATE_REMOVE`; the second
/// data word is the state atom and the last is the source indication
/// (2 = a pager, which is what this is).
pub(crate) fn set_topmost(handle: isize, topmost: bool) -> Result<(), WindowOpError> {
    let conn = connect()?;
    let window = window_id(handle)?;
    let wm_state = atom(&conn, b"_NET_WM_STATE")?;
    let above = atom(&conn, b"_NET_WM_STATE_ABOVE")?;
    send_root_message(
        &conn,
        window,
        wm_state,
        [u32::from(topmost), above, 0, 2, 0],
    )
}

/// `_NET_CLOSE_WINDOW`: ask the window manager to close the window the way
/// its own close button would, so the application still gets to run its
/// shutdown path and show a "save your work?" dialog.
///
/// This is a request, not a kill: a window that refuses to close stays
/// open, which is why cu's destructive gate reads the handle back
/// afterwards instead of trusting the call.
pub(crate) fn close(handle: isize) -> Result<(), WindowOpError> {
    let conn = connect()?;
    let window = window_id(handle)?;
    let close_window = atom(&conn, b"_NET_CLOSE_WINDOW")?;
    send_root_message(&conn, window, close_window, [0, 2, 0, 0, 0])
}
