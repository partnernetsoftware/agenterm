//! The live RFB session: one socket, one task, one command channel.
//!
//! Ownership rule: only the session task touches the `vnc-rs` client. Callers
//! interact through [`SessionHandle`], which is `Send + Sync` and cheap to
//! clone, so a GUI layer can hold it behind whatever lock it already uses.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use vnc::{PixelFormat, VncConnector, VncEvent, X11Event};

use crate::ard;
use crate::framebuffer::{Framebuffer, Rect};
use crate::preflight::{self, Credentials, PreflightError};
use crate::{Frame, MouseButtons, VncError};

/// How often the session asks the server for an incremental update.
///
/// RFB is request-driven: the server sends nothing until the client asks. This
/// interval is therefore the frame ceiling, not a polling overhead — ~60/s.
const REFRESH_INTERVAL: Duration = Duration::from_millis(16);

/// Commands the handle sends to the session task.
enum Command {
    Pointer { x: u16, y: u16, buttons: u8 },
    Key { keysym: u32, down: bool },
    /// Force a non-incremental update, e.g. when the UI reattaches.
    FullRefresh,
    Disconnect(oneshot::Sender<()>),
}

/// Parameters for opening a session.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub host: String,
    pub port: u16,
    /// Only Apple Remote Management (macOS Screen Sharing against a real
    /// account) uses a username; leave it `None` for password-only servers.
    pub username: Option<String>,
    /// `None` selects the `None` security type; `Some` selects VNC Auth.
    pub password: Option<String>,
    /// Share the desktop rather than displacing other clients.
    pub shared: bool,
}

impl ConnectOptions {
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16, password: Option<String>) -> Self {
        Self { host: host.into(), port, username: None, password, shared: true }
    }
}

/// A cloneable control surface for a running session.
#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::Sender<Command>,
    alive: Arc<AtomicBool>,
}

impl SessionHandle {
    /// Whether the session task is still running.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Send a pointer position and button mask to the server.
    pub fn send_mouse(&self, x: u16, y: u16, buttons: MouseButtons) -> Result<(), VncError> {
        self.dispatch(Command::Pointer { x, y, buttons: buttons.bits() })
    }

    /// Send an X11 keysym press or release.
    pub fn send_key(&self, keysym: u32, down: bool) -> Result<(), VncError> {
        self.dispatch(Command::Key { keysym, down })
    }

    /// Ask the server for a complete framebuffer rather than a delta.
    pub fn request_full_refresh(&self) -> Result<(), VncError> {
        self.dispatch(Command::FullRefresh)
    }

    /// Close the session and wait for the task to finish its shutdown.
    ///
    /// A closed channel means the task already exited, which is the desired
    /// end state, so that case resolves successfully rather than erroring.
    pub async fn disconnect(&self) {
        let (tx, rx) = oneshot::channel();
        if self.commands.send(Command::Disconnect(tx)).await.is_ok() {
            let _ = rx.await;
        }
        self.alive.store(false, Ordering::SeqCst);
    }

    fn dispatch(&self, command: Command) -> Result<(), VncError> {
        self.commands.try_send(command).map_err(|_| VncError::Disconnected)
    }
}

/// Open a session, returning a control handle and the stream of frames.
///
/// The returned receiver is the only frame path; dropping it makes the session
/// task shut down on its next send, which is how a closed UI stops the work.
pub async fn connect(
    options: ConnectOptions,
) -> Result<(SessionHandle, mpsc::Receiver<Frame>), VncError> {
    let address = format!("{}:{}", options.host, options.port);

    // Validate the handshake on a throwaway connection first. `vnc-rs` reacts
    // to a malformed security result with a non-unwinding abort, so a server
    // that is not well-formed RFB has to be rejected before it gets there.
    let credentials = Credentials {
        username: options.username.as_deref(),
        password: options.password.as_deref(),
    };
    preflight::probe(&address, credentials)
        .await
        .map_err(|error| match error {
        PreflightError::NotRfb => VncError::NotRfbServer { address: address.clone() },
        PreflightError::Rejected(reason) => VncError::Rejected(reason),
        PreflightError::UnsupportedSecurity(types) => VncError::UnsupportedSecurity(types),
        PreflightError::PasswordNotAccepted => VncError::PasswordNotAccepted,
        PreflightError::PasswordRequired => VncError::PasswordRequired,
        PreflightError::UsernameNotAccepted => VncError::UsernameNotAccepted,
        PreflightError::UsernameRequired => VncError::UsernameRequired,
        PreflightError::Io(reason) => VncError::Handshake(reason),
        })?;

    let stream = tokio::net::TcpStream::connect(&address)
        .await
        .map_err(|source| VncError::Connect { address: address.clone(), source })?;
    // Nagle batches small writes, which is exactly wrong for input events:
    // a click would wait behind the delayed-ack timer.
    let _ = stream.set_nodelay(true);

    let password = options.password.clone();
    let mut connector = VncConnector::new(stream)
        .set_auth_method(async move { Ok(password.unwrap_or_default()) });

    // Apple Remote Management: the vendored engine does the RFB framing and
    // calls back here for the Diffie-Hellman and AES work.
    if let Some(username) = options.username.clone() {
        let password = options.password.clone().unwrap_or_default();
        connector = connector.set_ard_handler(Box::new(move |challenge| {
            let private_key = ard::random_private_key(challenge.prime.len());
            let response = ard::respond(
                &challenge.generator,
                &challenge.prime,
                &challenge.server_public_key,
                &private_key,
                &username,
                &password,
                ard::random_padding(),
            )
            .map_err(|error| match error {
                ard::ArdError::CredentialTooLong => {
                    "the username and password must each be 63 bytes or fewer".to_owned()
                }
                ard::ArdError::BadParameters(reason) => reason.to_owned(),
            })?;
            Ok((response.ciphertext, response.public_key))
        }));
    }

    let client = connector
        .add_encoding(vnc::VncEncoding::Tight)
        .add_encoding(vnc::VncEncoding::Zrle)
        .add_encoding(vnc::VncEncoding::CopyRect)
        .add_encoding(vnc::VncEncoding::Raw)
        .add_encoding(vnc::VncEncoding::DesktopSizePseudo)
        .allow_shared(options.shared)
        .set_pixel_format(PixelFormat::bgra())
        .build()
        .map_err(VncError::Protocol)?
        .try_start()
        .await
        .map_err(VncError::Protocol)?
        .finish()
        .map_err(VncError::Protocol)?;

    let (command_tx, command_rx) = mpsc::channel(256);
    // Depth 2: frames are whole surfaces, so a backlog wastes memory and shows
    // stale pixels. A slow consumer should skip ahead, not accumulate.
    let (frame_tx, frame_rx) = mpsc::channel(2);
    let alive = Arc::new(AtomicBool::new(true));

    tokio::spawn(run_session(client, command_rx, frame_tx, Arc::clone(&alive)));

    Ok((SessionHandle { commands: command_tx, alive }, frame_rx))
}

async fn run_session(
    client: vnc::VncClient,
    mut commands: mpsc::Receiver<Command>,
    frames: mpsc::Sender<Frame>,
    alive: Arc<AtomicBool>,
) {
    let mut framebuffer = Framebuffer::new(0, 0);
    let mut dirty = false;
    let mut ticker = tokio::time::interval(REFRESH_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut shutdown_ack: Option<oneshot::Sender<()>> = None;

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                let event = match command {
                    Command::Pointer { x, y, buttons } => {
                        X11Event::PointerEvent((x, y, buttons).into())
                    }
                    Command::Key { keysym, down } => {
                        X11Event::KeyEvent((keysym, down).into())
                    }
                    Command::FullRefresh => X11Event::FullRefresh,
                    Command::Disconnect(ack) => {
                        shutdown_ack = Some(ack);
                        break;
                    }
                };
                if client.input(event).await.is_err() {
                    break;
                }
            }
            event = client.poll_event() => {
                match event {
                    Ok(Some(event)) => {
                        if apply_event(&mut framebuffer, event) {
                            dirty = true;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
            _ = ticker.tick() => {
                if client.input(X11Event::Refresh).await.is_err() {
                    break;
                }
                if dirty && framebuffer.width() > 0 {
                    let frame = Frame {
                        width: framebuffer.width(),
                        height: framebuffer.height(),
                        rgba: framebuffer.as_rgba().to_vec(),
                    };
                    // `try_send` on a full channel drops this frame on
                    // purpose: the next tick composites a newer one, so
                    // skipping beats queueing stale pixels behind a slow UI.
                    match frames.try_send(frame) {
                        Ok(()) => dirty = false,
                        Err(mpsc::error::TrySendError::Full(_)) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                    }
                }
            }
        }
    }

    alive.store(false, Ordering::SeqCst);
    let _ = client.close().await;
    if let Some(ack) = shutdown_ack {
        let _ = ack.send(());
    }
}

/// Fold one protocol event into the surface; returns whether pixels changed.
fn apply_event(framebuffer: &mut Framebuffer, event: VncEvent) -> bool {
    match event {
        VncEvent::SetResolution(screen) => {
            framebuffer.resize(screen.width, screen.height);
            // Sizing the surface is not painting it. Reporting this as dirty
            // would publish a blank frame ahead of the server's first rect,
            // which the UI then shows as a black flash on connect.
            false
        }
        VncEvent::RawImage(rect, data) => {
            framebuffer.blit_bgra(convert_rect(rect), &data);
            true
        }
        VncEvent::Copy(dst, src) => {
            framebuffer.copy_rect(convert_rect(dst), convert_rect(src));
            true
        }
        // Cursor and clipboard events carry no framebuffer pixels the canvas
        // consumer can use; the server also paints the cursor into the
        // framebuffer itself, so ignoring these leaves no visible gap.
        _ => false,
    }
}

fn convert_rect(rect: vnc::Rect) -> Rect {
    Rect { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
}
