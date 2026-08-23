//! The Tauri product shell over [`agenterm_vnc`].
//!
//! This layer owns exactly three things: the single-session state, the command
//! surface the webview calls, and the frame pump that turns session frames
//! into `frame-update` events. Protocol and pixel work stays in the mechanism
//! crate; nothing here parses RFB.

use std::sync::Mutex;

use agenterm_vnc::{ConnectOptions, MouseButtons, SessionHandle};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Event name carrying one composited surface to the canvas.
const FRAME_EVENT: &str = "frame-update";
/// Event name announcing that the session ended, with a reason.
const CLOSED_EVENT: &str = "session-closed";

/// The app holds at most one session; connecting again replaces it.
#[derive(Default)]
struct AppState {
    session: Mutex<Option<SessionHandle>>,
}

/// What `connect` reports back so the UI can size its canvas immediately.
#[derive(Debug, Clone, Serialize)]
struct Connected {
    width: u16,
    height: u16,
}

/// Frame payload: one changed region, not the whole screen.
///
/// `rgba` rides Tauri's IPC as a byte array rather than base64, which avoids
/// inflating every update by a third. Sending only the dirty region matters
/// more still: a cursor moving across a 4K desktop is a few kilobytes here
/// rather than the ~33MB a full surface would cost, sixty times a second.
#[derive(Clone, Serialize)]
struct FramePayload {
    width: u16,
    height: u16,
    x: u16,
    y: u16,
    #[serde(rename = "regionWidth")]
    region_width: u16,
    #[serde(rename = "regionHeight")]
    region_height: u16,
    rgba: Vec<u8>,
}

/// Open a session, replacing any existing one.
///
/// Returns the initial resolution. The server may still resize later, and the
/// UI must honour the dimensions carried on each frame rather than caching
/// these — `DesktopSizePseudo` makes mid-session changes legal.
#[tauri::command]
async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
) -> Result<Connected, String> {
    // Tear down any prior session first so two sockets never race to emit
    // frames into the same canvas.
    disconnect_current(&state).await;

    // An empty box means "no credential offered", not "the empty credential":
    // a blank password selects the `None` security type, and a blank username
    // keeps the connection off the Apple Remote Management path.
    let password = password.filter(|value| !value.is_empty());
    let username = username.filter(|value| !value.is_empty());
    let mut options = ConnectOptions::new(host, port, password);
    options.username = username;
    let (session, mut frames) = agenterm_vnc::connect(options)
        .await
        .map_err(|error| error.to_string())?;

    // The first frame establishes the resolution; waiting for it means the UI
    // never has to render against a placeholder size.
    let first = frames
        .recv()
        .await
        .ok_or_else(|| "the server closed the session before sending a frame".to_string())?;
    let connected = Connected { width: first.width, height: first.height };

    state.session.lock().expect("session lock").replace(session);

    let pump = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut frame = Some(first);
        while let Some(current) = frame {
            let payload = FramePayload {
                width: current.width,
                height: current.height,
                x: current.x,
                y: current.y,
                region_width: current.region_width,
                region_height: current.region_height,
                rgba: current.rgba,
            };
            if pump.emit(FRAME_EVENT, payload).is_err() {
                break;
            }
            frame = frames.recv().await;
        }
        // The channel closing is the session task's only exit path, so this is
        // where an unexpected drop (server hangup, auth revoked) surfaces.
        let _ = pump.emit(CLOSED_EVENT, "the VNC session ended");
    });

    Ok(connected)
}

/// Close the current session, if any. Idempotent.
#[tauri::command]
async fn disconnect(state: State<'_, AppState>) -> Result<(), String> {
    disconnect_current(&state).await;
    Ok(())
}

/// Send a pointer position plus RFB button mask.
#[tauri::command]
fn send_mouse(state: State<'_, AppState>, x: u16, y: u16, buttons: u8) -> Result<(), String> {
    with_session(&state, |session| {
        session.send_mouse(x, y, MouseButtons::from_bits(buttons))
    })
}

/// Send one X11 keysym transition.
#[tauri::command]
fn send_key(state: State<'_, AppState>, keysym: u32, down: bool) -> Result<(), String> {
    with_session(&state, |session| session.send_key(keysym, down))
}

/// Ask the server to retransmit the whole framebuffer.
#[tauri::command]
fn request_full_refresh(state: State<'_, AppState>) -> Result<(), String> {
    with_session(&state, SessionHandle::request_full_refresh)
}

fn with_session<F>(state: &State<'_, AppState>, action: F) -> Result<(), String>
where
    F: FnOnce(&SessionHandle) -> Result<(), agenterm_vnc::VncError>,
{
    let guard = state.session.lock().expect("session lock");
    let session = guard.as_ref().ok_or_else(|| "not connected".to_string())?;
    action(session).map_err(|error| error.to_string())
}

/// Take the session out of the state and shut it down.
///
/// The handle is moved out before the await so the mutex guard never crosses a
/// suspension point — holding a `std::sync::Mutex` across `.await` would risk
/// deadlocking the runtime.
async fn disconnect_current(state: &State<'_, AppState>) {
    let existing = state.session.lock().expect("session lock").take();
    if let Some(session) = existing {
        session.disconnect().await;
    }
}

/// Build and run the application. Shared by the desktop binary and mobile.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(AppState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            send_mouse,
            send_key,
            request_full_refresh
        ])
        .run(tauri::generate_context!())
        .expect("the AgenTerm VNC app failed to start");
}
