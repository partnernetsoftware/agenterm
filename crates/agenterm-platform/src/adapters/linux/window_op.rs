//! Linux X11 ConfigureWindow for foreign top-level windows.

use x11rb::{
    connection::Connection,
    protocol::xproto::{
        Atom, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask,
        StackMode, Window,
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

/// Raise, iconify, or restore a window through the window manager.
///
/// `ConfigureWindow(stack_mode = Above)` is the X11 primitive for exactly
/// raising without touching focus. Iconify and restore use ICCCM
/// `WM_CHANGE_STATE` / `MapWindow`, which is what the desktop means by
/// putting a window away and bringing it back; `_NET_WM_STATE` maximize
/// stays typed unsupported because guessing transitions a given WM may
/// ignore would report success for nothing.
pub(crate) fn show(
    handle: isize,
    state: crate::contract::window_op::WindowShowState,
) -> Result<(), WindowOpError> {
    use crate::contract::window_op::WindowShowState;
    let conn = connect()?;
    let window = window_id(handle)?;
    match state {
        WindowShowState::Show => raise(&conn, window),
        WindowShowState::Hide | WindowShowState::Minimize => set_iconified(&conn, window, true),
        WindowShowState::Restore => {
            set_maximized(&conn, window, false)?;
            set_iconified(&conn, window, false)
        }
        WindowShowState::Maximize => set_maximized(&conn, window, true),
    }
}

fn raise(
    conn: &x11rb::rust_connection::RustConnection,
    window: Window,
) -> Result<(), WindowOpError> {
    // A managed window is reparented into a window-manager frame, and
    // SubstructureRedirect means ConfigureWindow on the client window is
    // not an order to the X server -- it is a request the WM is free to
    // drop. Openbox drops it, so the raise silently did nothing.
    //
    // `_NET_RESTACK_WINDOW` is the EWMH message for this exact case: a
    // pager restacking a window it does not own, without touching focus
    // (unlike `_NET_ACTIVE_WINDOW`). Source indication 2 is "pager",
    // sibling 0 is "no sibling", and detail `Above` raises to the top.
    // ConfigureWindow stays as the fallback for an unmanaged window or a
    // WM that does not advertise the message.
    if wm_supports(conn, b"_NET_RESTACK_WINDOW").unwrap_or(false) {
        let restack = atom(conn, b"_NET_RESTACK_WINDOW")?;
        return send_root_message(
            conn,
            window,
            restack,
            [2, 0, u32::from(StackMode::ABOVE), 0, 0],
        );
    }
    let aux = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
    conn.configure_window(window, &aux)
        .map_err(|error| failed(format!("ConfigureWindow(raise) send failed: {error}")))?;
    sync(conn)
}

/// ICCCM `WM_CHANGE_STATE` to `IconicState`, or `MapWindow` to bring it back.
fn set_iconified(
    conn: &x11rb::rust_connection::RustConnection,
    window: Window,
    iconify: bool,
) -> Result<(), WindowOpError> {
    const ICONIC_STATE: u32 = 3;
    if iconify {
        let wm_change_state = atom(conn, b"WM_CHANGE_STATE")?;
        let event = ClientMessageEvent::new(32, window, wm_change_state, [ICONIC_STATE, 0, 0, 0, 0]);
        let root = root_of(conn)?;
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
            event,
        )
        .map_err(|error| failed(format!("WM_CHANGE_STATE send failed: {error}")))?;
    } else {
        conn.map_window(window)
            .map_err(|error| failed(format!("MapWindow send failed: {error}")))?;
    }
    sync(conn)
}

/// Whether `_NET_WM_STATE` lists both horizontal and vertical maximization.
fn window_is_maximized(
    conn: &x11rb::rust_connection::RustConnection,
    window: Window,
) -> Result<bool, WindowOpError> {
    let wm_state = atom(conn, b"_NET_WM_STATE")?;
    let horz = atom(conn, b"_NET_WM_STATE_MAXIMIZED_HORZ")?;
    let vert = atom(conn, b"_NET_WM_STATE_MAXIMIZED_VERT")?;
    let reply = conn
        .get_property(false, window, wm_state, AtomEnum::ATOM, 0, 32)
        .map_err(|error| failed(format!("_NET_WM_STATE request failed: {error}")))?
        .reply()
        .map_err(|error| failed(format!("_NET_WM_STATE reply failed: {error}")))?;
    let Some(mut states) = reply.value32() else {
        return Ok(false);
    };
    let mut has_horz = false;
    let mut has_vert = false;
    for state in states {
        if state == horz {
            has_horz = true;
        } else if state == vert {
            has_vert = true;
        }
    }
    Ok(has_horz && has_vert)
}

fn set_maximized(
    conn: &x11rb::rust_connection::RustConnection,
    window: Window,
    maximized: bool,
) -> Result<(), WindowOpError> {
    const NET_WM_STATE_REMOVE: u32 = 0;
    const NET_WM_STATE_ADD: u32 = 1;
    let wm_state = atom(conn, b"_NET_WM_STATE")?;
    let horz = atom(conn, b"_NET_WM_STATE_MAXIMIZED_HORZ")?;
    let vert = atom(conn, b"_NET_WM_STATE_MAXIMIZED_VERT")?;
    let action = if maximized {
        NET_WM_STATE_ADD
    } else {
        NET_WM_STATE_REMOVE
    };
    send_root_message(&conn, window, wm_state, [action, horz, vert, 2, 0])
}

/// Whether the window manager reports this window as not on screen.
fn window_is_iconified(
    conn: &x11rb::rust_connection::RustConnection,
    window: Window,
) -> Result<bool, WindowOpError> {
    let wm_state = atom(conn, b"_NET_WM_STATE")?;
    let state_hidden = atom(conn, b"_NET_WM_STATE_HIDDEN")?;
    let reply = conn
        .get_property(false, window, wm_state, AtomEnum::ATOM, 0, 32)
        .map_err(|error| failed(format!("_NET_WM_STATE request failed: {error}")))?
        .reply()
        .map_err(|error| failed(format!("_NET_WM_STATE reply failed: {error}")))?;
    Ok(reply
        .value32()
        .is_some_and(|mut states| states.any(|state| state == state_hidden)))
}

/// Whether the running window manager advertises `name` in `_NET_SUPPORTED`.
///
/// A WM that does not list a message is entitled to ignore it, so asking
/// first is what keeps the fallback path honest rather than sending both
/// and hoping.
fn wm_supports(
    conn: &x11rb::rust_connection::RustConnection,
    name: &[u8],
) -> Result<bool, WindowOpError> {
    let root = root_of(conn)?;
    let supported = atom(conn, b"_NET_SUPPORTED")?;
    let wanted = atom(conn, name)?;
    let reply = conn
        .get_property(false, root, supported, AtomEnum::ATOM, 0, 1024)
        .map_err(|error| failed(format!("_NET_SUPPORTED request failed: {error}")))?
        .reply()
        .map_err(|error| failed(format!("_NET_SUPPORTED reply failed: {error}")))?;
    Ok(reply
        .value32()
        .is_some_and(|mut atoms| atoms.any(|atom| atom == wanted)))
}

/// Wait until the server has actually processed everything sent so far.
///
/// `flush` only pushes bytes toward the socket. Every one of these calls
/// returns immediately afterwards, and the connection is dropped on the way
/// out -- and a request that the server has not processed by then is simply
/// lost. Measured on openbox: an `_NET_RESTACK_WINDOW` sent, flushed and
/// dropped never restacked anything, while the identical message on a
/// connection that stayed open did. A round trip (`GetInputFocus` is the
/// classic no-op for this) forces the server to have handled the request
/// before this function returns, which is also what makes it honest to
/// return `Ok`.
fn sync(conn: &x11rb::rust_connection::RustConnection) -> Result<(), WindowOpError> {
    conn.flush()
        .map_err(|error| failed(format!("X11 flush failed: {error}")))?;
    conn.get_input_focus()
        .map_err(|error| failed(format!("X11 sync request failed: {error}")))?
        .reply()
        .map_err(|error| failed(format!("X11 sync reply failed: {error}")))?;
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
    sync(conn)
}

/// The window manager's decoration thickness on the left and top, from
/// `_NET_FRAME_EXTENTS`. Absent (no window manager, or an undecorated
/// window) is zero, not an error: an undecorated window's frame *is* its
/// client rect.
fn frame_extents(conn: &x11rb::rust_connection::RustConnection, window: Window) -> (i32, i32) {
    let Ok(extents) = atom(conn, b"_NET_FRAME_EXTENTS") else {
        return (0, 0);
    };
    let Ok(cookie) = conn.get_property(false, window, extents, AtomEnum::CARDINAL, 0, 4) else {
        return (0, 0);
    };
    let Ok(reply) = cookie.reply() else {
        return (0, 0);
    };
    let Some(values) = reply.value32() else {
        return (0, 0);
    };
    let values: Vec<u32> = values.collect();
    // left, right, top, bottom
    match (values.first(), values.get(2)) {
        (Some(&left), Some(&top)) => (
            i32::try_from(left).unwrap_or(0),
            i32::try_from(top).unwrap_or(0),
        ),
        _ => (0, 0),
    }
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
    // A ConfigureRequest with the default NorthWest gravity names where the
    // *frame* goes, but every reader here reports the client rect (that is
    // what TranslateCoordinates answers). Asking for 400,300 therefore put
    // the client at 401,320 under openbox -- offset by the titlebar -- and
    // the read-back could never equal the request. Subtracting the frame's
    // own extents makes the client land where the caller asked, so the
    // verification is an equality rather than an approximation.
    let (frame_left, frame_top) = frame_extents(&conn, window);
    let aux = ConfigureWindowAux::new()
        .x(x - frame_left)
        .y(y - frame_top)
        .width(width.max(1))
        .height(height.max(1));
    conn.configure_window(window, &aux)
        .map_err(|error| failed(format!("ConfigureWindow send failed: {error}")))?;
    sync(&conn)
}

pub(crate) fn window_rect(handle: isize) -> Result<WindowBounds, WindowOpError> {
    let conn = connect()?;
    let window = u32::try_from(handle).map_err(|_| failed("window handle is not a valid XID"))?;
    let geom = conn
        .get_geometry(window)
        .map_err(|error| failed(format!("GetGeometry send failed: {error}")))?
        .reply()
        .map_err(|error| failed(format!("GetGeometry failed: {error}")))?;
    // GetGeometry answers in the *parent's* coordinates, and a reparenting
    // window manager makes the parent its own frame -- so the raw x/y is
    // the client's offset inside the titlebar (1, 20 under openbox), not a
    // screen position. Every other Linux reader here already translates to
    // the root; this one did not, so `window-place` reported a window as
    // sitting at (1, 20) no matter where on the screen it actually was.
    let root = root_of(&conn)?;
    let origin = conn
        .translate_coordinates(window, root, 0, 0)
        .map_err(|error| failed(format!("TranslateCoordinates send failed: {error}")))?
        .reply()
        .map_err(|error| failed(format!("TranslateCoordinates failed: {error}")))?;
    Ok(WindowBounds {
        x: i32::from(origin.dst_x),
        y: i32::from(origin.dst_y),
        width: u32::from(geom.width),
        height: u32::from(geom.height),
    })
}

/// Whether the window manager marks the window `_NET_WM_STATE_HIDDEN`.
pub(crate) fn minimized(handle: isize) -> Result<bool, WindowOpError> {
    let conn = connect()?;
    let window = window_id(handle)?;
    window_is_iconified(&conn, window)
}

/// Whether `_NET_WM_STATE` carries both maximized atoms.
pub(crate) fn maximized(handle: isize) -> Result<bool, WindowOpError> {
    let conn = connect()?;
    let window = window_id(handle)?;
    window_is_maximized(&conn, window)
}

/// `_NET_ACTIVE_WINDOW`: the explicit foreground-changing counterpart to
/// `show`/`_NET_RESTACK_WINDOW`. Source 2 identifies an automation pager;
/// the product layer owns the subsequent `_NET_ACTIVE_WINDOW` read-back.
pub(crate) fn activate(handle: isize) -> Result<(), WindowOpError> {
    let conn = connect()?;
    let window = window_id(handle)?;
    let active = atom(&conn, b"_NET_ACTIVE_WINDOW")?;
    send_root_message(&conn, window, active, [2, 0, 0, 0, 0])
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
