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
use crate::resolve;
use crate::{Frame, MouseButtons, VncError};

/// How often the session asks the server for an incremental update.
///
/// RFB is request-driven: the server sends nothing until the client asks, so
/// this sets the ceiling on how often new pixels can arrive — ~60/s. It does
/// *not* gate how quickly they are handed on once they do; see the note on
/// `emit` in the session loop.
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
    /// How much colour to ask the server for.
    pub colour: ColourDepth,
}

/// How many bits of colour to negotiate.
///
/// This is not a free choice between two independent axes. Halving the bits
/// halves what an *uncompressed* rect costs, but the Tight encoding -- which
/// compresses far better than that, and which macOS Screen Sharing leans on --
/// only operates on 32-bit formats. Asking for sixteen bits therefore also
/// gives up Tight, and on a photographic desktop Tight wins by more than the
/// factor of two that sixteen bits saves.
///
/// So the default stays at millions of colours with Tight available, and
/// [`Self::Thousands`] exists for the case it is actually good at: a link slow
/// enough that raw bytes dominate, with content flat enough that Tight's
/// advantage is small.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColourDepth {
    /// 32-bit BGRA, and Tight stays available. The right default.
    #[default]
    Millions,
    /// 16-bit RGB565: half the raw bytes, but no Tight, so ZRLE and Raw only.
    Thousands,
}

impl ConnectOptions {
    #[must_use]
    pub fn new(host: impl Into<String>, port: u16, password: Option<String>) -> Self {
        Self {
            host: host.into(),
            port,
            username: None,
            password,
            shared: true,
            colour: ColourDepth::default(),
        }
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

    let targets = resolve::resolve(&options.host, options.port)
        .map_err(|source| VncError::Connect { address: address.clone(), source })?;
    let stream = tokio::net::TcpStream::connect(&targets[..])
        .await
        .map_err(|source| VncError::Connect { address: address.clone(), source })?;
    // Nagle batches small writes, which is exactly wrong for input events:
    // a click would wait behind the delayed-ack timer.
    let _ = stream.set_nodelay(true);

    let password = options.password.clone();
    // Tight cannot decode a 16-bit format, so requesting both would leave the
    // server free to send rects this client must then refuse.
    let encodings: &[vnc::VncEncoding] = match options.colour {
        ColourDepth::Millions => &[
            vnc::VncEncoding::Tight,
            vnc::VncEncoding::Zrle,
            vnc::VncEncoding::CopyRect,
            vnc::VncEncoding::Raw,
        ],
        ColourDepth::Thousands => &[
            vnc::VncEncoding::Zrle,
            vnc::VncEncoding::CopyRect,
            vnc::VncEncoding::Raw,
        ],
    };

    let mut connector = VncConnector::new(stream)
        .set_auth_method(async move { Ok(password.unwrap_or_default()) });
    for encoding in encodings {
        connector = connector.add_encoding(*encoding);
    }
    let mut connector = connector;

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
        .add_encoding(vnc::VncEncoding::DesktopSizePseudo)
        // Lets a server close an update without having declared a rect count
        // up front, which Apple's does. Without it the engine waits for rects
        // that never come.
        .add_encoding(vnc::VncEncoding::LastRectPseudo)
        .allow_shared(options.shared)
        .set_pixel_format(match options.colour {
            ColourDepth::Thousands => PixelFormat::rgb565(),
            ColourDepth::Millions => PixelFormat::bgra(),
        })
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

    tokio::spawn(run_session(
        client,
        options.colour,
        command_rx,
        frame_tx,
        Arc::clone(&alive),
    ));

    Ok((SessionHandle { commands: command_tx, alive }, frame_rx))
}

async fn run_session(
    client: vnc::VncClient,
    colour: ColourDepth,
    mut commands: mpsc::Receiver<Command>,
    frames: mpsc::Sender<Frame>,
    alive: Arc<AtomicBool>,
) {
    let mut framebuffer = Framebuffer::new(0, 0);
    // Regions touched since the last frame went out, kept separate rather than
    // merged into one bounding box. Two small updates in opposite corners of a
    // 4K screen share a bounding box of the whole screen: 2 KB of real content
    // becomes a 31 MB send, which is the case a naive union gets badly wrong.
    let mut dirty: Vec<Rect> = Vec::new();
    // Decode failures reported by the engine. Collected rather than dropped so
    // a stream this client cannot read is diagnosable instead of merely ugly.
    let mut decode_errors: Vec<String> = Vec::new();
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
                        if let Some(changed) =
                            apply_event(&mut framebuffer, event, colour, &mut decode_errors)
                        {
                            accumulate(&mut dirty, changed);
                            // Hand pixels on the moment they are composited.
                            // Waiting for the next refresh tick would add up to
                            // a full interval of latency to every interaction,
                            // on top of the round trip that already happened.
                            // If the consumer is busy the region stays pending
                            // and merges into the next one, which is why this
                            // cannot outrun the UI.
                            if !emit(&frames, &framebuffer, &mut dirty) {
                                break;
                            }
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
                // Retry anything the consumer was too busy to take earlier.
                if !emit(&frames, &framebuffer, &mut dirty) {
                    break;
                }
            }
        }
    }

    alive.store(false, Ordering::SeqCst);
    if !decode_errors.is_empty() {
        // Repeats are the norm when one encoding is unsupported, so report the
        // distinct messages with a count rather than a flood of duplicates.
        decode_errors.sort();
        decode_errors.dedup();
        eprintln!(
            "agenterm-vnc: {} decode failure(s) during the session: {}",
            decode_errors.len(),
            decode_errors.join("; ")
        );
    }
    let _ = client.close().await;
    if let Some(ack) = shutdown_ack {
        let _ = ack.send(());
    }
}

/// Hand the pending region to the consumer, if there is one and it will take it.
///
/// Returns false only when the consumer is gone, which ends the session.
fn emit(
    frames: &mpsc::Sender<Frame>,
    framebuffer: &Framebuffer,
    dirty: &mut Vec<Rect>,
) -> bool {
    if framebuffer.width() == 0 {
        return true;
    }
    // Each pending region goes out on its own. A consumer that is busy stops
    // the drain here and keeps the rest pending, so nothing is ever lost.
    while let Some(&region) = dirty.first() {
        let frame = Frame {
            width: framebuffer.width(),
            height: framebuffer.height(),
            x: region.x,
            y: region.y,
            region_width: region.width,
            region_height: region.height,
            rgba: framebuffer.region_rgba(region),
        };
        // A full channel keeps the region pending rather than dropping it:
        // unlike a whole-surface frame, a dropped region would leave those
        // pixels stale forever, because nothing later redraws what the server
        // already sent.
        match frames.try_send(frame) {
            Ok(()) => {
                dirty.remove(0);
            }
            Err(mpsc::error::TrySendError::Full(_)) => return true,
            Err(mpsc::error::TrySendError::Closed(_)) => return false,
        }
    }
    true
}

/// The most regions tracked before they are merged into one.
///
/// A cap is needed because the pending list is drained by a consumer that may
/// be slower than the server: without one, a burst could grow it without
/// bound. Merging is the honest fallback -- it costs bandwidth, never
/// correctness -- and eight is comfortably above the handful of rects a normal
/// update produces.
const MAX_PENDING_REGIONS: usize = 8;

/// Record a newly painted region, keeping the pending set small.
///
/// Regions that overlap or touch are merged, since sending them separately
/// would transmit the shared pixels twice. Disjoint ones are kept apart, which
/// is the whole point: a bounding box around scattered updates can be orders
/// of magnitude larger than the updates themselves.
fn accumulate(dirty: &mut Vec<Rect>, changed: Rect) {
    let mut merged = changed;
    // Fold in everything the new rect meets, which may chain several together.
    let mut index = 0;
    while index < dirty.len() {
        if overlaps_or_touches(dirty[index], merged) {
            merged = union(dirty.swap_remove(index), merged);
            // Restart: the wider rect may now reach regions checked earlier.
            index = 0;
        } else {
            index += 1;
        }
    }
    dirty.push(merged);

    if dirty.len() > MAX_PENDING_REGIONS {
        let all = dirty.drain(..).reduce(union).expect("the list is not empty");
        dirty.push(all);
    }
}

/// Whether two rectangles intersect or share an edge.
fn overlaps_or_touches(a: Rect, b: Rect) -> bool {
    let a_right = a.x as u32 + a.width as u32;
    let a_bottom = a.y as u32 + a.height as u32;
    let b_right = b.x as u32 + b.width as u32;
    let b_bottom = b.y as u32 + b.height as u32;
    a.x as u32 <= b_right
        && b.x as u32 <= a_right
        && a.y as u32 <= b_bottom
        && b.y as u32 <= a_bottom
}

/// Fold one protocol event into the surface, returning the region it touched.
fn apply_event(
    framebuffer: &mut Framebuffer,
    event: VncEvent,
    colour: ColourDepth,
    decode_errors: &mut Vec<String>,
) -> Option<Rect> {
    match event {
        VncEvent::SetResolution(screen) => {
            framebuffer.resize(screen.width, screen.height);
            // Sizing the surface is not painting it. Reporting this as dirty
            // would publish a blank frame ahead of the server's first rect,
            // which the UI then shows as a black flash on connect.
            None
        }
        VncEvent::RawImage(rect, data) => {
            let rect = convert_rect(rect);
            // The decoders hand pixels back in whatever format was negotiated,
            // so this has to follow that choice rather than assume four bytes.
            match colour {
                ColourDepth::Thousands => framebuffer.blit_rgb565(rect, &data),
                ColourDepth::Millions => framebuffer.blit_bgra(rect, &data),
            }
            Some(rect)
        }
        VncEvent::Copy(dst, src) => {
            let dst = convert_rect(dst);
            framebuffer.copy_rect(dst, convert_rect(src));
            Some(dst)
        }
        // Tight sends most photographic and desktop content as JPEG. Dropping
        // these leaves whatever was underneath, which is why an unhandled
        // JPEG path renders as a grid of stale tiles rather than as nothing.
        VncEvent::JpegImage(rect, data) => {
            let rect = convert_rect(rect);
            match decode_jpeg(&data) {
                Some(pixels) => {
                    framebuffer.blit_rgb(rect, &pixels);
                    Some(rect)
                }
                // A rect we cannot decode is left alone rather than painted
                // with garbage; the next full refresh replaces it.
                None => None,
            }
        }
        // A decode failure. There is nothing to paint, but staying silent is
        // how a broken stream turns into "the screen looks wrong" with no
        // explanation, so it is surfaced for the caller to log.
        VncEvent::Error(message) => {
            decode_errors.push(message);
            None
        }
        // Cursor and clipboard events carry no framebuffer pixels the canvas
        // consumer can use; the server also paints the cursor into the
        // framebuffer itself, so ignoring these leaves no visible gap.
        _ => None,
    }
}

/// Decode one JPEG rect to packed RGB, or `None` if it cannot be read.
///
/// RFB's Tight encoding carries baseline JPEG. A grayscale image decodes to
/// one byte per pixel, so it is expanded here rather than left for the blit to
/// misread as RGB.
fn decode_jpeg(data: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(data));
    let pixels = decoder.decode().ok()?;
    match decoder.info()?.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => Some(pixels),
        jpeg_decoder::PixelFormat::L8 => {
            Some(pixels.iter().flat_map(|&value| [value, value, value]).collect())
        }
        // CMYK and 16-bit grayscale do not occur in RFB Tight streams; a
        // future server that sends one gets a skipped rect, not garbage.
        _ => None,
    }
}

/// The smallest rectangle containing both inputs.
fn union(a: Rect, b: Rect) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    let right = (a.x + a.width).max(b.x + b.width);
    let bottom = (a.y + a.height).max(b.y + b.height);
    Rect { x, y, width: right - x, height: bottom - y }
}

fn convert_rect(rect: vnc::Rect) -> Rect {
    Rect { x: rect.x, y: rect.y, width: rect.width, height: rect.height }
}
