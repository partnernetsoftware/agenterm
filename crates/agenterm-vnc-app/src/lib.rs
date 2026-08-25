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

/// Event name telling the canvas that a frame is waiting to be fetched.
///
/// The pixels do not travel on this event. Tauri serialises event payloads
/// with `serde_json`, so a `Vec<u8>` becomes a JSON array of decimal numbers:
/// measured at 4.6x the raw size, which for a full 2048x1536 surface is 56 MB
/// of text to move 12 MB of pixels, plus the parse on the other side. Commands
/// can return `tauri::ipc::Response`, which crosses as a real ArrayBuffer, so
/// the notification goes out as an event and the frontend pulls the bytes.
const FRAME_EVENT: &str = "frame-ready";
/// Event name announcing that the session ended, with a reason.
const CLOSED_EVENT: &str = "session-closed";

/// The app holds at most one session; connecting again replaces it.
#[derive(Default)]
struct AppState {
    session: Mutex<Option<SessionHandle>>,
    /// Everything painted since the frontend last collected, in draw order.
    ///
    /// One slot rather than a queue, but an accumulating one: frames that
    /// arrive before the previous is collected are appended, so a frontend
    /// that skips ahead skips round trips without skipping pixels.
    pending: Mutex<Option<agenterm_vnc::Frame>>,
}

/// What `connect` reports back so the UI can size its canvas immediately.
#[derive(Debug, Clone, Serialize)]
struct Connected {
    width: u16,
    height: u16,
}

/// Serialise the frame waiting to be drawn, if any.
///
/// This is served over a custom URI scheme rather than returned from a
/// command. A command's reply crosses as JSON unless the IPC happens to be
/// using the custom protocol, and a `Vec<u8>` in JSON is an array of decimal
/// numbers: measured on this app, fetching one frame took 560 ms against
/// 13 ms to draw it, or about 16 MB/s through an IPC on the same machine.
/// A protocol response is an HTTP body, so the bytes cross as bytes.
fn take_frame_body(state: &AppState) -> Vec<u8> {
    let frame = state.pending.lock().expect("pending lock").take();
    let Some(frame) = frame else {
        return Vec::new();
    };

    // Layout: screen width and height, the tile count, then that many tile
    // records of x, y, width, height, and finally every tile's pixels
    // concatenated in the same order.
    let mut body =
        Vec::with_capacity(HEADER_LEN + frame.tiles.len() * TILE_RECORD_LEN + frame.rgba.len());
    body.extend_from_slice(&frame.width.to_le_bytes());
    body.extend_from_slice(&frame.height.to_le_bytes());
    body.extend_from_slice(&(frame.tiles.len() as u32).to_le_bytes());
    for tile in &frame.tiles {
        for value in [tile.x, tile.y, tile.width, tile.height] {
            body.extend_from_slice(&value.to_le_bytes());
        }
    }
    body.extend_from_slice(&frame.rgba);
    body
}

/// Concatenate two frames, keeping the newer one's tiles last.
///
/// Drawing is in order, so a tile from `newer` overwrites an older tile
/// covering the same pixels. Both frames describe the same framebuffer, so the
/// screen size is taken from the newer one.
fn join(older: agenterm_vnc::Frame, newer: agenterm_vnc::Frame) -> agenterm_vnc::Frame {
    let offset = older.rgba.len();
    let mut tiles = older.tiles;
    tiles.extend(newer.tiles.into_iter().map(|tile| agenterm_vnc::Tile {
        offset: tile.offset + offset,
        ..tile
    }));
    let mut rgba = older.rgba;
    rgba.extend_from_slice(&newer.rgba);
    agenterm_vnc::Frame {
        width: newer.width,
        height: newer.height,
        tiles,
        rgba,
    }
}

/// Screen width, screen height, then the tile count.
const HEADER_LEN: usize = 8;
/// Four little-endian `u16` fields per tile.
const TILE_RECORD_LEN: usize = 8;

/// Diagnostic channel from the webview, so a frontend fault is visible in the
/// terminal instead of only in a devtools console nobody has open.
#[tauri::command]
fn log_from_ui(message: String) {
    eprintln!("[ui] {message}");
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
    let connected = Connected {
        width: first.width,
        height: first.height,
    };

    state.session.lock().expect("session lock").replace(session);

    let pump = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut frame = Some(first);
        while let Some(current) = frame {
            {
                let state = pump.state::<AppState>();
                let mut pending = state.pending.lock().expect("pending lock");
                match pending.take() {
                    // Skipping ahead must not skip pixels. An uncollected
                    // frame's tiles describe changes nothing will redraw, so
                    // they are appended rather than dropped; the newer tiles
                    // come after and win wherever the two overlap.
                    Some(previous) => *pending = Some(join(previous, current)),
                    None => *pending = Some(current),
                }
            }
            if pump.emit(FRAME_EVENT, ()).is_err() {
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
        .register_uri_scheme_protocol("vncframe", |ctx, _request| {
            let body = take_frame_body(&ctx.app_handle().state::<AppState>());
            tauri::http::Response::builder()
                .header("Content-Type", "application/octet-stream")
                // The page is served from a different origin than this scheme,
                // so `fetch` treats the request as cross-origin and drops the
                // response without this. The window is the only client.
                .header("Access-Control-Allow-Origin", "*")
                // The frame is consumed by this read, so a cached response
                // would hand back pixels that have already been drawn.
                .header("Cache-Control", "no-store")
                .body(body)
                .expect("a response with a byte body is always well formed")
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            log_from_ui,
            disconnect,
            send_mouse,
            send_key,
            request_full_refresh
        ])
        .run(tauri::generate_context!())
        .expect("the AgenTerm VNC app failed to start");
}
