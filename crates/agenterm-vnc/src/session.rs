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
use crate::{Frame, MouseButtons, Tile, VncError};

/// How often the session asks the server for an incremental update.
///
/// RFB is request-driven: the server sends nothing until the client asks, so
/// this sets the ceiling on how often new pixels can arrive — ~60/s. It does
/// *not* gate how quickly they are handed on once they do; see the note on
/// `emit` in the session loop.
const REFRESH_INTERVAL: Duration = Duration::from_millis(16);

/// How long to keep collecting rects before handing a frame over.
///
/// Short enough to be invisible next to a network round trip, long enough that
/// the rects of one server update land in the same frame.
const FLUSH_DELAY: Duration = Duration::from_millis(2);

/// How long to wait for an answer before allowing another request.
///
/// An idle server may hold a request open indefinitely, which is legal: it
/// replies when something changes. This only bounds how long a lost or ignored
/// request can stall the session.
const IDLE_TIMEOUT: Duration = Duration::from_millis(500);

/// How many update requests may be outstanding at once.
///
/// RFB allows a request to be issued while an earlier answer is still
/// streaming, and on a server that spends hundreds of milliseconds encoding a
/// large screen that overlap is most of the available win: measured against
/// macOS Screen Sharing at 3840x2160, one at a time gave 0.6 frames a second
/// where four gave 1.7.
const MAX_INFLIGHT_REQUESTS: u32 = 4;

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
    // Shallow on purpose: one frame already carries a whole update's tiles, so
    // a backlog would only hold stale pixels the next frame supersedes.
    let (frame_tx, frame_rx) = mpsc::channel(4);
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
    // RFB is request/response: the server answers one update request with one
    // update. Firing on a timer regardless issues requests faster than a busy
    // server can answer them, and the backlog is what turns an idle desktop
    // into a continuous stream of redundant pixels. Ask again only once the
    // previous answer has arrived.
    let mut awaiting_update = false;
    let mut inflight: u32 = 0;
    // Held across iterations; see the flush arm below for why.
    let flush_at = tokio::time::sleep(FLUSH_DELAY);
    tokio::pin!(flush_at);
    let mut flush_armed = false;
    // Bounds how long a request that is never answered can stall the session.
    let mut last_request = std::time::Instant::now();
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
                            // Do not emit yet. A server tiles a large update
                            // into many small rects -- macOS sends 64x64, so a
                            // 2160x3840 screen is nearly two thousand of them
                            // -- and handing each one over separately turns a
                            // single refresh into two thousand round trips.
                            // Push the flush out instead: it fires once the
                            // rects stop arriving, which is the end of one
                            // server update.
                            // The first rect of an answer means the server has
                            // finished encoding and is now streaming. Asking
                            // for the next update here rather than after the
                            // flush lets its encoding -- which measured 37 to
                            // 453 ms on a 2160x3840 screen, and dominates the
                            // interaction delay -- overlap this one's decode.
                            if !flush_armed && inflight < MAX_INFLIGHT_REQUESTS {
                                if client.input(X11Event::Refresh).await.is_err() {
                                    break;
                                }
                                last_request = std::time::Instant::now();
                                inflight += 1;
                            }
                            flush_at.as_mut().reset(tokio::time::Instant::now() + FLUSH_DELAY);
                            flush_armed = true;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
            // Flush shortly after the last rect arrived. `poll_event` above is
            // ready continuously while a burst is being decoded, so this arm
            // only wins the select once the server pauses -- which is exactly
            // the end of one update. That keeps the batching from costing
            // latency: a lone rect still goes out within this interval, and a
            // burst goes out as one frame instead of hundreds.
            // Flush shortly after the last rect arrived, and treat that pause
            // as the end of the server's answer.
            //
            // `flush_at` is a single timer held across iterations. A `sleep`
            // constructed inside the select would restart on every loop, and
            // while a burst is decoding `poll_event` is ready continuously, so
            // it would never once elapse.
            () = &mut flush_at, if flush_armed => {
                flush_armed = false;
                awaiting_update = false;
                inflight = inflight.saturating_sub(1);
                if !emit(&frames, &framebuffer, &mut dirty) {
                    break;
                }
                // The next request already went out when this burst started,
                // so there is nothing to ask for here.
            }
            _ = ticker.tick(), if !awaiting_update || last_request.elapsed() > IDLE_TIMEOUT => {
                if client.input(X11Event::Refresh).await.is_err() {
                    break;
                }
                awaiting_update = true;
                last_request = std::time::Instant::now();
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
    if dirty.is_empty() {
        return true;
    }

    // Every pending rect travels in one frame. The alternative -- a frame per
    // rect -- is what a burst of two thousand tiles turns into otherwise, and
    // no consumer drains two thousand handoffs inside one refresh.
    let mut tiles = Vec::with_capacity(dirty.len());
    let mut rgba = Vec::new();
    for region in dirty.iter() {
        tiles.push(Tile {
            x: region.x,
            y: region.y,
            width: region.width,
            height: region.height,
            offset: rgba.len(),
        });
        rgba.extend_from_slice(&framebuffer.region_rgba(*region));
    }

    let frame = Frame { width: framebuffer.width(), height: framebuffer.height(), tiles, rgba };
    // A full channel keeps the rects pending rather than dropping them: unlike
    // a whole-surface frame, dropped rects would leave those pixels stale
    // forever, because nothing later redraws what the server already sent.
    match frames.try_send(frame) {
        Ok(()) => {
            dirty.clear();
            true
        }
        Err(mpsc::error::TrySendError::Full(_)) => true,
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}

/// The most regions tracked before they are folded into one.
///
/// A cap is needed because the pending list is drained by a consumer that may
/// be slower than the server: without one, a burst could grow without bound.
///
/// It has to be large. A 2160x3840 screen is nearly two thousand 64x64 tiles,
/// and a full repaint legitimately produces all of them at once; a cap of
/// eight turned that into a full-screen bounding box on every burst, which
/// measured 7.3 MB a frame where the tiles themselves are 16 KB. This bounds
/// memory without punishing the normal case.
const MAX_PENDING_REGIONS: usize = 4096;

/// Record a newly painted region.
///
/// Deliberately no merging. An earlier version folded touching regions into
/// their bounding box, on the theory that fewer, larger frames beat many small
/// ones. Measured against a real macOS server, which tiles every update into
/// 64x64 rects and never sends anything larger, that was 250 times worse:
///
/// ```text
/// merging      5 fps   75.8 MB/s   13,601 KB per frame
/// no merging  18 fps    0.3 MB/s        16 KB per frame
/// ```
///
/// The reason is that each merge widens the box, so the widened box then
/// touches regions it did not before, and a screen's worth of tiles chains
/// into one full-screen send. Any rule of the form "merge when they touch"
/// has this failure mode; the tiles genuinely are all touching.
///
/// Batching already collects a burst of rects into one flush, which is where
/// the win actually was. Sending them as separate regions costs a few
/// kilobytes of headers and nothing else.
fn accumulate(dirty: &mut Vec<Rect>, changed: Rect) {
    // Identical rects repeat when a server redraws the same tile; there is no
    // point queueing the same pixels twice.
    if dirty.last() == Some(&changed) {
        return;
    }
    dirty.push(changed);

    if dirty.len() > MAX_PENDING_REGIONS {
        // The consumer is not keeping up. Folding everything into one box is a
        // deliberate last resort: it costs bandwidth, never correctness, and
        // only happens when the alternative is unbounded growth.
        let all = dirty.drain(..).reduce(union).expect("the list is not empty");
        dirty.push(all);
    }
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
