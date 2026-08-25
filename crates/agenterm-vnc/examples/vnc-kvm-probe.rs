//! Prove the out-of-band KVM loop against a live RFB server.
//!
//! This is the P1 harness of `plan/goal-local-six-cell.md`: keyboard and
//! pointer events go in over RFB, whole frames come back, and the frames are
//! composited and written as PNG. Running it against a QEMU guest is what makes
//! a VM's screen testable without trusting anything running inside the guest --
//! the framebuffer is the virtual GPU's, and the keystrokes are the virtual
//! keyboard's, so neither depends on the program under test being correct.
//!
//! ```text
//! qemu-system-aarch64 -machine virt -accel hvf -cpu host -m 512 \
//!     -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
//!     -device virtio-gpu-pci -device qemu-xhci -device usb-kbd -device usb-tablet \
//!     -vnc 127.0.0.1:1
//!
//! cargo run -p agenterm-vnc --example vnc-kvm-probe -- \
//!     127.0.0.1 5901 ./out [TEXT] [BASELINE.png] [MAX_CHANGED_PERCENT]
//! ```
//!
//! With a baseline the probe stops being a liveness check and becomes a visual
//! regression gate: the first run writes the baseline, later runs compare
//! against it and fail when the screen drifts past the tolerance. Two things
//! have to be pinned before that comparison means anything -- the framebuffer
//! size (a mode change would otherwise read as "everything moved") and whatever
//! the guest animates on its own. See the `settle` docs for why a live screen
//! is a sample rather than a state.
//!
//! `-device usb-tablet` is not optional: without an absolute pointing device
//! the guest reads pointer events as relative motion, and `send_mouse(x, y)`
//! stops meaning "go to this pixel".

use std::path::{Path, PathBuf};
use std::time::Duration;

use agenterm_vnc::{ConnectOptions, Frame, MouseButtons, SessionHandle};
use tokio::sync::mpsc::Receiver;

/// A full-screen RGBA surface assembled from tile updates.
///
/// A `Frame` carries only the rectangles that changed, so a single frame is
/// never a screenshot on its own. Compositing successive frames into one
/// surface is what turns the stream into something comparable across runs.
struct Canvas {
    width: u16,
    height: u16,
    rgba: Vec<u8>,
}

impl Canvas {
    fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        }
    }

    fn apply(&mut self, frame: &Frame) {
        if self.width != frame.width || self.height != frame.height {
            self.width = frame.width;
            self.height = frame.height;
            self.rgba = vec![0u8; frame.width as usize * frame.height as usize * 4];
        }
        let stride = self.width as usize * 4;
        for tile in &frame.tiles {
            let tile_stride = tile.width as usize * 4;
            for row in 0..tile.height as usize {
                let src = tile.offset + row * tile_stride;
                let dst = (tile.y as usize + row) * stride + tile.x as usize * 4;
                if src + tile_stride > frame.rgba.len() || dst + tile_stride > self.rgba.len() {
                    continue;
                }
                self.rgba[dst..dst + tile_stride]
                    .copy_from_slice(&frame.rgba[src..src + tile_stride]);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.rgba.is_empty()
    }

    fn write_png(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let file = std::fs::File::create(path)?;
        let mut encoder = png::Encoder::new(
            std::io::BufWriter::new(file),
            self.width as u32,
            self.height as u32,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&self.rgba)?;
        Ok(())
    }
}

/// Count pixels that differ between two surfaces of the same size.
///
/// Differing sizes count as a total change rather than an error: a guest is
/// allowed to switch modes, and that is still evidence the input landed.
fn read_png(path: &Path) -> Result<Canvas, Box<dyn std::error::Error>> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    let mut reader = decoder.read_info()?;
    let mut rgba = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut rgba)?;
    rgba.truncate(info.buffer_size());
    Ok(Canvas { width: info.width as u16, height: info.height as u16, rgba })
}

fn changed_pixels(before: &Canvas, after: &Canvas) -> usize {
    if before.width != after.width || before.height != after.height {
        return after.rgba.len() / 4;
    }
    before
        .rgba
        .chunks_exact(4)
        .zip(after.rgba.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count()
}

/// Drain frames into the canvas until the server goes quiet, or the budget runs out.
///
/// "Quiet" rather than "one frame" is the right stopping rule: a repaint arrives
/// as a burst of tile updates, and stopping at the first one captures a
/// half-drawn screen.
///
/// The overall budget is what makes it safe on a live desktop. A blinking
/// cursor -- or a clock, or any animation -- means the server *never* goes
/// quiet, and waiting for silence there hangs forever. Capturing such a screen
/// is inherently a sample rather than a settled state; the budget makes that
/// explicit instead of pretending the screen holds still.
async fn settle(
    canvas: &mut Canvas,
    frames: &mut Receiver<Frame>,
    quiet: Duration,
    budget: Duration,
) -> usize {
    let deadline = tokio::time::Instant::now() + budget;
    let mut received = 0;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let wait = quiet.min(remaining);
        match tokio::time::timeout(wait, frames.recv()).await {
            Ok(Some(frame)) => {
                canvas.apply(&frame);
                received += 1;
            }
            // Quiet window elapsed with nothing pending: the screen settled.
            Err(_) if wait == quiet => break,
            _ => break,
        }
    }
    received
}

async fn capture(
    session: &SessionHandle,
    frames: &mut Receiver<Frame>,
    canvas: &mut Canvas,
) -> Result<(), Box<dyn std::error::Error>> {
    session.request_full_refresh()?;
    settle(canvas, frames, Duration::from_millis(1200), Duration::from_secs(8)).await;
    Ok(())
}

/// Press and release one X11 keysym. RFB speaks keysyms, not scancodes, and for
/// printable ASCII the keysym *is* the character's code point.
async fn tap(session: &SessionHandle, keysym: u32) -> Result<(), Box<dyn std::error::Error>> {
    session.send_key(keysym, true)?;
    tokio::time::sleep(Duration::from_millis(40)).await;
    session.send_key(keysym, false)?;
    tokio::time::sleep(Duration::from_millis(60)).await;
    Ok(())
}

/// Type a literal string at whatever has focus.
///
/// Typing printable characters, rather than pressing a navigation key, is what
/// makes this probe portable across guests: a shell prompt, a text field, and a
/// terminal all echo them, whereas Escape or an arrow key is meaningful in some
/// and inert in others. An inert key is indistinguishable from a key that never
/// arrived, which would make a failure unreadable.
async fn type_text(session: &SessionHandle, text: &str) -> Result<(), Box<dyn std::error::Error>> {
    for character in text.chars() {
        tap(session, character as u32).await?;
    }
    Ok(())
}

/// One scripted interaction step.
///
/// Driving the UI by coordinate is the only way to test what a keyboard cannot
/// reach -- a toolbar button, a tab, a scrollbar. It is also the only way to
/// find out whether absolute pointer placement actually works: a guest without
/// a tablet device reads these as relative motion, and the clicks land
/// somewhere else entirely while every individual call still "succeeds".
enum Step {
    Click { x: u16, y: u16 },
    Type(String),
    Key(u32),
    Wait(u64),
}

fn parse_steps(script: &str) -> Result<Vec<Step>, Box<dyn std::error::Error>> {
    let mut steps = Vec::new();
    for raw in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (verb, rest) = raw.split_once(':').ok_or_else(|| format!("bad step {raw:?}"))?;
        steps.push(match verb {
            "click" => {
                let (x, y) = rest.split_once(',').ok_or_else(|| format!("bad click {rest:?}"))?;
                Step::Click { x: x.trim().parse()?, y: y.trim().parse()? }
            }
            "type" => Step::Type(rest.to_string()),
            "key" => Step::Key(u32::from_str_radix(rest.trim_start_matches("0x"), 16)?),
            "wait" => Step::Wait(rest.parse()?),
            other => return Err(format!("unknown step verb {other:?}").into()),
        });
    }
    Ok(steps)
}

async fn run_steps(
    session: &SessionHandle,
    frames: &mut Receiver<Frame>,
    canvas: &mut Canvas,
    steps: &[Step],
    out_dir: &Path,
) -> Result<usize, Box<dyn std::error::Error>> {
    let mut responded = 0;
    for (index, step) in steps.iter().enumerate() {
        let label = match step {
            Step::Click { x, y } => {
                // Press and release as separate events with the pointer already
                // parked: a click that moves and presses in one event is a drag
                // to anything watching motion.
                session.send_mouse(*x, *y, MouseButtons::NONE)?;
                tokio::time::sleep(Duration::from_millis(150)).await;
                session.send_mouse(*x, *y, MouseButtons::LEFT)?;
                tokio::time::sleep(Duration::from_millis(120)).await;
                session.send_mouse(*x, *y, MouseButtons::NONE)?;
                format!("click({x},{y})")
            }
            Step::Type(text) => {
                type_text(session, text).await?;
                format!("type({text:?})")
            }
            Step::Key(keysym) => {
                tap(session, *keysym).await?;
                format!("key(0x{keysym:04x})")
            }
            Step::Wait(ms) => {
                tokio::time::sleep(Duration::from_millis(*ms)).await;
                format!("wait({ms}ms)")
            }
        };

        let previous = Canvas {
            width: canvas.width,
            height: canvas.height,
            rgba: canvas.rgba.clone(),
        };
        capture(session, frames, canvas).await?;
        let changed = changed_pixels(&previous, canvas);
        let path = out_dir.join(format!("step{index}.png"));
        canvas.write_png(&path)?;
        if changed > 0 {
            responded += 1;
        }
        println!(
            "step {index}: {label} -> {changed} px changed, {}",
            path.display()
        );
    }
    Ok(responded)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut steps_script = None;
    let mut positional = Vec::new();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == "--steps" {
            index += 1;
            steps_script = Some(
                raw.get(index).ok_or("--steps needs a script")?.clone(),
            );
        } else {
            positional.push(raw[index].clone());
        }
        index += 1;
    }
    let mut argv = positional.into_iter();
    let host = argv.next().unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = argv.next().unwrap_or_else(|| "5901".into()).parse()?;
    let out_dir = PathBuf::from(argv.next().unwrap_or_else(|| "vnc-kvm-probe-out".into()));
    let text = argv.next().unwrap_or_else(|| "agenterm".into());
    let baseline = argv.next().map(PathBuf::from);
    // Default tolerance is derived from the smallest change worth catching, not
    // picked for roundness. One line of terminal text on a 1280x800 screen is
    // roughly 600 pixels, i.e. 0.06%; measured idle noise on a pinned guest
    // (solid root, screensaver and DPMS off) is 0.00%. A "reasonable-looking"
    // 0.5% is 5120 pixels and would sail straight past a whole line rendering
    // wrong -- measured, that exact value let a 16-character regression through.
    let tolerance: f64 = argv.next().unwrap_or_else(|| "0.02".into()).parse()?;
    std::fs::create_dir_all(&out_dir)?;

    println!("probe: connecting to {host}:{port}");
    let (session, mut frames) =
        agenterm_vnc::connect(ConnectOptions::new(host.clone(), port, None)).await?;

    let mut before = Canvas::new();
    capture(&session, &mut frames, &mut before).await?;
    if before.is_empty() {
        return Err("probe: server sent no frame; nothing to compare".into());
    }
    let before_path = out_dir.join("before.png");
    before.write_png(&before_path)?;
    println!(
        "probe: before {}x{} -> {}",
        before.width,
        before.height,
        before_path.display()
    );

    // Scripted mode: drive the UI and report what each action changed on screen.
    if let Some(script) = steps_script {
        let steps = parse_steps(&script)?;
        let mut canvas = before;
        let responded = run_steps(&session, &mut frames, &mut canvas, &steps, &out_dir).await?;
        session.disconnect().await;
        println!("probe: {responded}/{} steps produced a visible change", steps.len());
        if responded == 0 {
            return Err(
                "probe: no step changed the screen; the UI never saw these actions".into(),
            );
        }
        return Ok(());
    }

    // An empty TEXT means pure observation: capture and compare, send nothing.
    // A visual-regression run must not perturb the thing it is measuring, and
    // without this the probe's own keystrokes become part of the next baseline.
    if text.is_empty() {
        println!("probe: observation only (no input sent)");
        return finish(&session, &before, &before, baseline, tolerance, false).await;
    }

    // Pointer first. Absolute placement only means anything with a tablet
    // device attached, so this doubles as a check that one is present.
    let (mid_x, mid_y) = (before.width / 2, before.height / 2);
    session.send_mouse(mid_x, mid_y, MouseButtons::NONE)?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    session.send_mouse(mid_x, mid_y, MouseButtons::LEFT)?;
    tokio::time::sleep(Duration::from_millis(120)).await;
    session.send_mouse(mid_x, mid_y, MouseButtons::NONE)?;

    // Then keys. The echoed characters are the evidence: they appear only if the
    // keystrokes reached the guest's keyboard device and the guest redrew.
    println!("probe: typing {text:?}");
    type_text(&session, &text).await?;

    let mut after = Canvas::new();
    capture(&session, &mut frames, &mut after).await?;
    let after_path = out_dir.join("after.png");
    after.write_png(&after_path)?;

    println!(
        "probe: after  {}x{} -> {}",
        after.width,
        after.height,
        after_path.display()
    );

    finish(&session, &before, &after, baseline, tolerance, true).await
}

/// Report the input result, then optionally gate on a stored baseline.
///
/// The baseline is compared against the *pre-input* frame on purpose. What a
/// regression gate should answer is "does the idle screen still render the way
/// it did", and folding the probe's own keystrokes into that makes every run
/// drift from the last one for a reason that has nothing to do with rendering.
async fn finish(
    session: &SessionHandle,
    before: &Canvas,
    after: &Canvas,
    baseline: Option<PathBuf>,
    tolerance: f64,
    expect_input: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let total = (after.width as usize * after.height as usize).max(1);
    if expect_input {
        let changed = changed_pixels(before, after);
        let percent = changed as f64 * 100.0 / total as f64;
        println!("probe: changed pixels {changed}/{total} ({percent:.2}%)");
        if changed == 0 {
            session.disconnect().await;
            return Err("probe: input produced no visible change; KVM loop not proven".into());
        }
        println!("probe: PASS -- keyboard and pointer reached the guest, frames came back");
    }

    session.disconnect().await;

    if let Some(baseline_path) = baseline {
        let after = before;
        if !baseline_path.exists() {
            after.write_png(&baseline_path)?;
            println!(
                "probe: baseline written to {} ({}x{}) -- rerun to compare",
                baseline_path.display(),
                after.width,
                after.height
            );
            return Ok(());
        }
        let reference = read_png(&baseline_path)?;
        // Assert the geometry first. A resized framebuffer makes every later
        // number meaningless, and reporting it as "99% of pixels changed" hides
        // the actual cause behind a plausible-looking ratio.
        if reference.width != after.width || reference.height != after.height {
            return Err(format!(
                "probe: framebuffer is {}x{} but baseline is {}x{}; \
                 pin the resolution before comparing",
                after.width, after.height, reference.width, reference.height
            )
            .into());
        }
        let drift = changed_pixels(&reference, after);
        let drift_percent = drift as f64 * 100.0 / total as f64;
        println!(
            "probe: baseline drift {drift}/{total} ({drift_percent:.2}%), tolerance {tolerance:.2}%"
        );
        if drift_percent > tolerance {
            return Err(format!(
                "probe: visual regression -- {drift_percent:.2}% of pixels differ from {}",
                baseline_path.display()
            )
            .into());
        }
        println!("probe: PASS -- screen matches the baseline within tolerance");
    }
    Ok(())
}
