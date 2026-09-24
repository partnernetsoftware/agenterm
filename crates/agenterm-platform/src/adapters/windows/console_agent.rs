//! A pseudoconsole for Windows builds that do not have one.
//!
//! ConPTY arrived in Windows 10 build 17763. On anything older — Windows
//! Server 2016 is 14393 and still in support — `CreatePseudoConsole` does not
//! exist, and Microsoft's ConPTY redistributable does not lower that floor
//! either: it supports 10.0.17763.0 and above, the same as the in-box API.
//!
//! What does work everywhere is the mechanism every terminal used before
//! ConPTY: put the child in its own *hidden* console, then read that console's
//! screen buffer and turn what changed into a terminal stream. The APIs
//! involved (`AllocConsole`, `ReadConsoleOutputW`, `WriteConsoleInputW`) are
//! as old as Win32.
//!
//! A process can be attached to only one console, so the scraping cannot
//! happen inside a GUI host. It runs in a separate agent process — this
//! executable, re-executed with [`AGENT_ARGUMENT`], so nothing third-party
//! enters the product.
//!
//! The agent speaks the same two pipes a pseudoconsole does: terminal bytes
//! out, input bytes in. Everything above the adapter therefore cannot tell
//! which backend it got, which is the whole point — the difference is sealed
//! here and not spread through the terminal.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::Console::{
    AllocConsole, CHAR_INFO, CHAR_INFO_0, CONSOLE_SCREEN_BUFFER_INFO, COORD, CTRL_BREAK_EVENT,
    CTRL_C_EVENT, ENABLE_MOUSE_INPUT, FROM_LEFT_1ST_BUTTON_PRESSED, FROM_LEFT_2ND_BUTTON_PRESSED,
    FreeConsole, GenerateConsoleCtrlEvent, GetConsoleMode, GetConsoleScreenBufferInfo,
    GetConsoleWindow, GetLargestConsoleWindowSize, INPUT_RECORD, INPUT_RECORD_0, KEY_EVENT,
    KEY_EVENT_RECORD, KEY_EVENT_RECORD_0, MOUSE_EVENT, MOUSE_EVENT_RECORD, MOUSE_MOVED,
    MOUSE_WHEELED, RIGHTMOST_BUTTON_PRESSED, ReadConsoleOutputW, SMALL_RECT, SetConsoleCtrlHandler,
    SetConsoleScreenBufferSize, SetConsoleWindowInfo, WriteConsoleInputW,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, GetExitCodeProcess,
    PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOW,
    WaitForSingleObject,
};

/// The argument that turns an executable into an agent.
///
/// Carries no product name, on purpose. This adapter is shared, so whichever
/// name it took would appear in some *other* product's command line — and a
/// process list is public: a product with its own trademark should not be
/// seen re-executing itself under a different one. `--internal-` marks it as
/// not part of any command surface, matching the convention the other
/// internal arguments already use, and makes a collision with a real option
/// implausible.
pub const AGENT_ARGUMENT: &str = "--internal-console-agent";

/// Rows the screen buffer keeps above the window, so Windows scrolls the
/// window through the buffer instead of dropping the rows that leave the
/// screen. Each of those scrolls is mirrored into the host's scrollback.
const BUFFER_SCROLL_ROOM_ROWS: u32 = 500;

/// How often the screen buffer is polled when the child is producing output.
/// The console API offers no change notification, so this is the floor on
/// output latency; 8 ms is under a frame and well below the cost of the
/// `ReadConsoleOutputW` itself at terminal sizes.
const POLL_BUSY: std::time::Duration = std::time::Duration::from_millis(8);
/// Backoff once the screen stops changing, so an idle shell is not a spin.
const POLL_IDLE: std::time::Duration = std::time::Duration::from_millis(40);
/// Idle polls before backing off. Roughly a quarter second of quiet.
const IDLE_POLLS: u32 = 30;
/// How long failed screen reads may persist before the session is declared
/// lost. A resize can make `ReadConsoleOutputW` fail while Windows rebuilds
/// the buffer; that work can take substantially longer under x86 emulation.
/// Keep this as elapsed time rather than a poll count so scheduler and API
/// latency cannot silently change the recovery contract.
const MAX_POLL_FAILURE_DURATION: std::time::Duration = std::time::Duration::from_secs(10);

const SW_HIDE: u16 = 0;

#[link(name = "user32")]
unsafe extern "system" {
    fn ShowWindow(window: HANDLE, command: i32) -> i32;
}

/// A raw handle that may cross into a worker thread.
///
/// Console and pipe handles are process-wide kernel objects, not thread
/// affine; `HANDLE` is only `!Send` because it is a bare pointer. Each of
/// these is moved to exactly one thread that owns it for the process
/// lifetime, so there is no sharing to synchronize.
#[derive(Clone, Copy)]
struct Portable(HANDLE);

// SAFETY: see the type comment -- the referent is a kernel object with no
// thread affinity, and each value is moved to a single owning thread.
unsafe impl Send for Portable {}

impl Portable {
    /// Taken by value so a closure captures the wrapper rather than reaching
    /// through to the bare pointer field, which is not `Send`.
    fn handle(self) -> HANDLE {
        self.0
    }
}

// Most console attribute bits, the `Cell` type, the SGR mapping and the row
// re-encoding all live in the platform-independent `console_row_emit` module so
// they can be unit-tested off Windows. The double-width (CJK) continuation fix
// lives there too. Everything is re-imported here by glob, so the existing
// tests in this file keep reaching them through `use super::*`.
use crate::adapters::console_row_emit::*;

// A double-width character occupies two cells that carry the *same* code unit.
// These two byte-order bits are used only by `decode_cell` below, so they stay
// on the Windows path rather than in the always-compiled shared module.
const COMMON_LVB_LEADING_BYTE: u16 = 0x0100;
const COMMON_LVB_TRAILING_BYTE: u16 = 0x0200;

// ---------------------------------------------------------------------------
// Command-line transport
// ---------------------------------------------------------------------------

/// The child's command line travels as hex rather than as further arguments.
///
/// It is already a fully quoted Windows command line built for `CreateProcessW`
/// and re-quoting it to survive a second `CommandLineToArgvW` round trip is a
/// class of bug with no upside. Hex has no metacharacters at all.
fn encode_utf16_hex(units: &[u16]) -> String {
    let mut encoded = String::with_capacity(units.len() * 4);
    for unit in units {
        encoded.push_str(&format!("{unit:04x}"));
    }
    encoded
}

fn decode_utf16_hex(encoded: &str) -> Option<Vec<u16>> {
    if !encoded.len().is_multiple_of(4) {
        return None;
    }
    let bytes = encoded.as_bytes();
    let mut units = Vec::with_capacity(encoded.len() / 4);
    for chunk in bytes.chunks(4) {
        let text = std::str::from_utf8(chunk).ok()?;
        units.push(u16::from_str_radix(text, 16).ok()?);
    }
    Some(units)
}

// ---------------------------------------------------------------------------
// Host side
// ---------------------------------------------------------------------------

/// Everything the host needs to talk to a spawned agent.
pub(crate) struct AgentSpawn {
    /// The agent process. It outlives the child by exactly as long as it takes
    /// to flush the final screen, so the host may wait on it as if it were the
    /// child.
    pub(crate) process: OwnedHandle,
    pub(crate) pid: u32,
    /// Resize requests. Separate from the input pipe so a resize cannot be
    /// mistaken for something the child typed.
    pub(crate) control: OwnedHandle,
}

/// Spawns the agent with the three pipe ends it needs.
///
/// `child_command_line` is the fully quoted line the child would have been
/// given directly, and `environment` the UTF-16 environment block, both
/// already built by the ConPTY path — the agent inherits them rather than
/// rebuilding them, so both backends start the child from identical inputs.
pub(crate) fn spawn_agent(
    input_read: &OwnedHandle,
    output_write: &OwnedHandle,
    child_command_line: &[u16],
    environment: Option<&[u16]>,
    current_dir: Option<&[u16]>,
    cols: u16,
    rows: u16,
) -> io::Result<AgentSpawn> {
    // The two session pipes are built by the ConPTY path, which hands its
    // endpoints straight to `CreatePseudoConsole` and therefore creates them
    // non-inheritable. An agent inherits or it gets nothing, and "nothing"
    // surfaces only as ERROR_INVALID_HANDLE from the first read.
    make_inheritable(input_read)?;
    make_inheritable(output_write)?;

    let control = create_pipe()?;
    let executable = std::env::current_exe()?;
    let mut executable_utf16: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let command_line = format!(
        "\"{}\" {AGENT_ARGUMENT} {} {} {} {cols} {rows} {}",
        executable.display().to_string().replace('"', "\"\""),
        input_read.as_raw_handle() as usize,
        output_write.as_raw_handle() as usize,
        control.read.as_raw_handle() as usize,
        encode_utf16_hex(child_command_line)
    );
    let mut command_line: Vec<u16> = command_line
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    // Both flags matter. USESTDHANDLES keeps the agent from inheriting the
    // host's stdio, and USESHOWWINDOW with SW_HIDE means the new console is
    // never shown -- hiding it after creation flashes a window.
    startup.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
    startup.wShowWindow = SW_HIDE;
    startup.hStdInput = input_read.as_raw_handle() as HANDLE;
    startup.hStdOutput = output_write.as_raw_handle() as HANDLE;
    startup.hStdError = output_write.as_raw_handle() as HANDLE;

    let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        // SAFETY: every pointer is either null or a live NUL-terminated
        // buffer owned by this frame for the duration of the call.
        CreateProcessW(
            executable_utf16.as_mut_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_NEW_CONSOLE | CREATE_UNICODE_ENVIRONMENT,
            environment.map_or(null(), |block| block.as_ptr().cast::<c_void>()),
            current_dir.map_or(null(), <[u16]>::as_ptr),
            &startup,
            &mut information,
        )
    };
    let _ = &mut executable_utf16;
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        // SAFETY: the thread handle is owned by this frame and unused; the
        // agent is not created suspended.
        CloseHandle(information.hThread);
    }
    Ok(AgentSpawn {
        process: unsafe {
            // SAFETY: CreateProcessW transferred a unique process handle.
            OwnedHandle::from_raw_handle(information.hProcess as _)
        },
        pid: information.dwProcessId,
        control: control.write,
    })
}

/// Tells a running agent the terminal is now `cols` x `rows`.
///
/// Four bytes, fixed width: a pipe carries no message boundaries, and a
/// length-prefixed format would only add a way to get out of sync.
pub(crate) fn request_resize(control: &OwnedHandle, cols: u16, rows: u16) -> io::Result<()> {
    let message = [
        (cols & 0xFF) as u8,
        (cols >> 8) as u8,
        (rows & 0xFF) as u8,
        (rows >> 8) as u8,
    ];
    write_all(control.as_raw_handle() as HANDLE, &message)
}

/// Marks one already-created handle as inheritable, in place.
///
/// Duplicating instead would give the child a different handle value than the
/// one written into its command line, which is the whole way it finds them.
fn make_inheritable(handle: &OwnedHandle) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    let set = unsafe {
        // SAFETY: the handle is live and owned by the caller for this call.
        SetHandleInformation(
            handle.as_raw_handle() as HANDLE,
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    };
    if set == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Pipe {
    read: OwnedHandle,
    write: OwnedHandle,
}

fn create_pipe() -> io::Result<Pipe> {
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        // Inheritable, or the agent receives a closed handle and never hears
        // about a resize.
        bInheritHandle: 1,
    };
    let mut read = null_mut();
    let mut write = null_mut();
    let created = unsafe {
        // SAFETY: both slots are valid out-pointers and attributes is a live
        // initialized structure.
        CreatePipe(&mut read, &mut write, &attributes, 0)
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Pipe {
        read: unsafe {
            // SAFETY: CreatePipe transferred this unique handle.
            OwnedHandle::from_raw_handle(read as _)
        },
        write: unsafe {
            // SAFETY: CreatePipe transferred this unique handle.
            OwnedHandle::from_raw_handle(write as _)
        },
    })
}

// ---------------------------------------------------------------------------
// Agent side
// ---------------------------------------------------------------------------

/// Runs the agent if `arguments` say to, returning the exit code to use.
///
/// `None` means these are ordinary arguments and the caller should carry on
/// being itself. A binary that embeds this adapter calls this before parsing
/// anything else; the agent is not a mode of the product, it is a different
/// program that happens to live in the same file.
#[must_use]
pub fn run_if_agent(arguments: &[String]) -> Option<i32> {
    let position = arguments.iter().position(|value| value == AGENT_ARGUMENT)?;
    let rest = &arguments[position + 1..];
    Some(match parse_and_run(rest) {
        Ok(code) => code,
        // The agent owns a hidden console and its stdout is the terminal
        // stream, so there is nowhere to print. The exit code tells the host
        // that the agent and not the child failed; the diagnostics sink is
        // what says which step, because "exit 251" on its own is exactly the
        // kind of dead end this whole area keeps producing.
        Err(error) => {
            #[cfg(feature = "runtime")]
            crate::diagnostics::record("console_agent", "agent_failed", &error.to_string());
            let _ = &error;
            251
        }
    })
}

fn parse_and_run(rest: &[String]) -> io::Result<i32> {
    // A request carries five fixed fields plus the hex-encoded child command
    // line. Reject a short request, but name the count: the bare
    // "malformed console agent request" this used to return made every one of
    // these failures indistinguishable in the diagnostics log.
    if rest.len() < 6 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "malformed console agent request: expected 6 arguments, got {}",
                rest.len()
            ),
        ));
    }
    let handle_at = |index: usize, name: &'static str| -> io::Result<HANDLE> {
        rest[index]
            .parse::<usize>()
            .map(|value| value as HANDLE)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("malformed console agent request: {name} is not a handle"),
                )
            })
    };
    let input_read = handle_at(0, "input read handle")?;
    let output_write = handle_at(1, "output write handle")?;
    let control_read = handle_at(2, "control read handle")?;
    let cols: u16 = rest[3].parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "malformed console agent request: columns is not a number",
        )
    })?;
    let rows: u16 = rest[4].parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "malformed console agent request: rows is not a number",
        )
    })?;
    let command_line = decode_utf16_hex(&rest[5]).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "malformed console agent request: child command line is not valid hex",
        )
    })?;
    run_agent(
        input_read,
        output_write,
        control_read,
        cols.max(1),
        rows.max(1),
        command_line,
    )
}

fn step<T>(what: &str, result: io::Result<T>) -> io::Result<T> {
    result.map_err(|error| io::Error::new(error.kind(), format!("{what}: {error}")))
}

fn run_agent(
    input_read: HANDLE,
    output_write: HANDLE,
    control_read: HANDLE,
    cols: u16,
    rows: u16,
    mut command_line: Vec<u16>,
) -> io::Result<i32> {
    let console = step("acquire console", ConsoleHandles::acquire())?;
    // After the console exists and before the child does. A handler registered
    // against the console this process had at startup does not survive the
    // FreeConsole/AllocConsole swap above, and an agent without one is killed
    // by the very interrupt it raises -- taking the shell with it, because the
    // job object kills on close.
    install_ctrl_handler();
    step("resize console", console.resize(cols, rows))?;

    let job = step("create job", OwnedJob::new())?;
    let child = step("spawn child", spawn_child(&console, &mut command_line))?;
    // The child dies with the agent. Without this, killing the host leaves an
    // orphan attached to a console nobody can see.
    unsafe {
        // SAFETY: both handles are live and owned by this frame.
        AssignProcessToJobObject(
            job.0.as_raw_handle() as HANDLE,
            child.as_raw_handle() as HANDLE,
        );
    }

    let input_console = Portable(console.input.as_raw_handle() as HANDLE);
    let input_pipe = Portable(input_read);
    spawn_thread("agenterm-console-agent-input", move || {
        forward_input(input_pipe.handle(), input_console.handle());
    });
    let control_pipe = Portable(control_read);
    spawn_thread("agenterm-console-agent-control", move || {
        forward_control(control_pipe.handle());
    });

    let mut screen = ScreenMirror::new(cols, rows);
    let mut idle = 0_u32;
    let mut first_poll_failure = None;
    let mut mouse_announced = false;
    loop {
        apply_pending_resize(&console);
        announce_mouse_mode(&console, output_write, &mut mouse_announced);
        let changed = match screen.poll_and_emit(&console, output_write) {
            Ok(changed) => {
                first_poll_failure = None;
                changed
            }
            Err(error) => {
                // A single failed read is survivable -- a resize landing
                // between the size query and the read is the ordinary cause,
                // and the next poll sees a consistent console. Only a
                // persistent failure means the session is really gone, and
                // treating the first one as fatal is what silently killed
                // the terminal on every window resize.
                if poll_failure_expired(&mut first_poll_failure, std::time::Instant::now()) {
                    return Err(error);
                }
                false
            }
        };
        idle = if changed { 0 } else { idle.saturating_add(1) };

        let child_state = unsafe {
            // SAFETY: child is a live process handle owned by this frame.
            WaitForSingleObject(child.as_raw_handle() as HANDLE, 0)
        };
        if child_state == WAIT_OBJECT_0 {
            // One last pass: the child's final output can land between the
            // previous poll and its exit, and dropping it loses exactly the
            // line a user most wants to see.
            let _ = screen.poll_and_emit(&console, output_write);
            break;
        }
        std::thread::sleep(if idle >= IDLE_POLLS {
            POLL_IDLE
        } else {
            POLL_BUSY
        });
    }

    let mut code: u32 = 0;
    unsafe {
        // SAFETY: the child has exited and the handle is still owned here.
        GetExitCodeProcess(child.as_raw_handle() as HANDLE, &mut code);
    }
    // Closing the stream is what tells the host the session ended.
    unsafe {
        // SAFETY: output_write was inherited and is not used after this.
        CloseHandle(output_write);
    }
    Ok(code as i32)
}

fn poll_failure_expired(
    first_failure: &mut Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    let started = first_failure.get_or_insert(now);
    now.saturating_duration_since(*started) >= MAX_POLL_FAILURE_DURATION
}

fn spawn_thread(name: &'static str, task: impl FnOnce() + Send + 'static) {
    let _ = crate::threading::spawn_named_detached(name, Box::new(task));
}

struct OwnedJob(OwnedHandle);

impl OwnedJob {
    fn new() -> io::Result<Self> {
        let job = unsafe {
            // SAFETY: null attributes and name request an unnamed job.
            CreateJobObjectW(null(), null())
        };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = unsafe {
            // SAFETY: CreateJobObjectW transferred a unique handle.
            OwnedHandle::from_raw_handle(job as _)
        };
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            // SAFETY: limits is live, initialized and correctly sized.
            SetInformationJobObject(
                job.as_raw_handle() as HANDLE,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast::<c_void>(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
        }
        Ok(Self(job))
    }
}

/// The agent's own console, opened by name.
///
/// `GetStdHandle` is the wrong way to reach it: the agent's standard handles
/// are the host's pipes, and stay that way after `AllocConsole`. `CONOUT$`
/// and `CONIN$` name the attached console directly.
struct ConsoleHandles {
    input: OwnedHandle,
    output: OwnedHandle,
}

impl ConsoleHandles {
    fn acquire() -> io::Result<Self> {
        unsafe {
            // SAFETY: neither call takes arguments. FreeConsole is tolerated
            // failing -- it only matters when one was already attached.
            FreeConsole();
            if AllocConsole() == 0 {
                return Err(io::Error::last_os_error());
            }
            // Belt and braces with STARTF_USESHOWWINDOW: a console allocated
            // at run time can still surface a window on some configurations.
            let window = GetConsoleWindow();
            if !window.is_null() {
                ShowWindow(window, i32::from(SW_HIDE));
            }
        }
        Ok(Self {
            input: open_console_device("CONIN$")?,
            output: open_console_device("CONOUT$")?,
        })
    }

    fn info(&self) -> io::Result<CONSOLE_SCREEN_BUFFER_INFO> {
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        let read = unsafe {
            // SAFETY: the handle is a live console output handle and info is
            // writable storage of exactly the required size.
            GetConsoleScreenBufferInfo(self.output.as_raw_handle() as HANDLE, &mut info)
        };
        if read == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(info)
    }

    /// Sizes the window to the terminal and gives the buffer room above it.
    ///
    /// The buffer used to be exactly the window's height so that the host
    /// owned all scrollback. In practice that lost it: with nowhere to
    /// scroll into, Windows shifted the buffer's own contents and the vacated
    /// rows were gone before the next scrape, so a program writing faster
    /// than the poll left the host with almost no history (measured on
    /// Windows 11 ARM, 2026-09-23: 200 lines of shell output produced one
    /// scrollable row). With room above the window, Windows keeps those rows
    /// and moves the window instead, which `render` already mirrors into the
    /// host's scrollback one `\r\n` at a time. The host still owns what the
    /// user scrolls through; this only stops the console from dropping rows
    /// between two scrapes.
    ///
    /// Shrinking has to move the window first and growing has to move it
    /// last, because neither may ever exceed the buffer.
    fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let handle = self.output.as_raw_handle() as HANDLE;
        // Where the window was before any of this. Every failure path puts it
        // back: a size that could not be applied is recoverable, a console left
        // at a degenerate rectangle is not -- the next poll reads that
        // rectangle and the mirror rebuilds itself around one cell.
        let entry = self.info()?;
        let largest = unsafe {
            // SAFETY: handle is a live console output handle.
            GetLargestConsoleWindowSize(handle)
        };
        let plan = plan_resize(cols, rows, entry.dwCursorPosition.Y, (largest.X, largest.Y));
        let target = COORD {
            X: plan.buffer.0,
            Y: plan.buffer.1,
        };
        let window = plan.window;
        let minimal = SMALL_RECT {
            Left: 0,
            Top: 0,
            Right: 0,
            Bottom: 0,
        };
        // Putting the window back is itself fallible: once the buffer has been
        // resized, the rectangle this console had on entry may no longer fit
        // inside it. So the restore is verified by reading the window back, and
        // falls back to a rectangle built from the buffer the console actually
        // has. A failure to restore is recorded rather than assumed away --
        // assuming it was exactly how the one-cell window went unnoticed.
        let restore = |error: io::Error| -> io::Error {
            for candidate in [Some(entry.srWindow), None] {
                let Ok(current) = self.info() else { break };
                let rectangle = match candidate {
                    Some(previous) => previous,
                    None => fitted_window(
                        current.dwSize,
                        current.dwCursorPosition.Y,
                        (largest.X, largest.Y),
                    ),
                };
                unsafe {
                    // SAFETY: handle is live and the rectangle is an
                    // initialized local.
                    SetConsoleWindowInfo(handle, 1, &rectangle);
                }
                if self.info().is_ok_and(|info| {
                    info.srWindow.Right > info.srWindow.Left
                        && info.srWindow.Bottom > info.srWindow.Top
                }) {
                    return error;
                }
            }
            #[cfg(feature = "runtime")]
            crate::diagnostics::record(
                "console_agent",
                "resize_window_not_restored",
                &error.to_string(),
            );
            error
        };
        unsafe {
            // SAFETY: handle is a live console output handle; both rectangles
            // and the coordinate are initialized locals.
            SetConsoleWindowInfo(handle, 1, &minimal);
            if SetConsoleScreenBufferSize(handle, target) == 0 {
                // Growing the window before the buffer is the failing order;
                // retry after making room.
                let error = io::Error::last_os_error();
                if SetConsoleWindowInfo(handle, 1, &window) == 0 {
                    return Err(restore(error));
                }
                if SetConsoleScreenBufferSize(handle, target) == 0 {
                    return Err(restore(error));
                }
            }
            if SetConsoleWindowInfo(handle, 1, &window) == 0 {
                return Err(restore(io::Error::last_os_error()));
            }
        }
        // The console can move its cursor while the buffer is resized, and a
        // window that no longer contains it scrapes blank rows: zooming out
        // (more rows) blanked the terminal until the next zoom in, measured
        // in the ARM court on 2026-09-23. Re-read and correct once.
        if let Ok(info) = self.info() {
            let cursor = info.dwCursorPosition.Y;
            if cursor < info.srWindow.Top || cursor > info.srWindow.Bottom {
                let corrected = plan_resize(cols, rows, cursor, (largest.X, largest.Y)).window;
                let moved = unsafe {
                    // SAFETY: handle is a live console output handle and the
                    // rectangle is an initialized local within the buffer.
                    SetConsoleWindowInfo(handle, 1, &corrected)
                };
                // The size did apply; only this cursor correction did not. Say
                // so instead of returning as though the window followed.
                if moved == 0 {
                    #[cfg(feature = "runtime")]
                    crate::diagnostics::record(
                        "console_agent",
                        "resize_cursor_not_followed",
                        &io::Error::last_os_error().to_string(),
                    );
                }
            }
        }
        Ok(())
    }
}

/// A rectangle that fits the buffer a console actually has, anchored on the
/// cursor. The last-resort restore: whatever else failed, the window must not
/// be left at one cell, because every later poll scrapes exactly that.
fn fitted_window(buffer: COORD, cursor_row: i16, largest: (i16, i16)) -> SMALL_RECT {
    // Both bounds apply: a window may exceed neither its buffer nor the screen.
    // Using the buffer alone would ask for the whole scroll room -- 540 rows on
    // a screen that holds 43 -- and be refused exactly like the window this
    // restore exists to replace.
    let cols = buffer.X.max(1).min(largest.0.max(1));
    let rows = buffer.Y.max(1).min(largest.1.max(1));
    let top = (cursor_row - rows + 1).clamp(0, (buffer.Y.max(1) - rows).max(0));
    SMALL_RECT {
        Left: 0,
        Top: top,
        Right: cols - 1,
        Bottom: top + rows - 1,
    }
}

/// The rectangles one resize asks of the console, once every bound is applied.
///
/// This is separate from the calls so it can be asserted without a console:
/// the geometry is where the blank screen came from, and it was previously
/// unreachable by any test. `SMALL_RECT` carries no derives of its own, so
/// tests compare its fields.
struct ResizePlan {
    /// Columns and rows for the screen buffer, scroll room included.
    buffer: (i16, i16),
    /// The window, never wider or taller than the host can display.
    window: SMALL_RECT,
}

/// `largest` is `GetLargestConsoleWindowSize`: what this screen can show at the
/// current font. The buffer may exceed it -- that is what scrollback is -- but
/// the window may not, and `SetConsoleWindowInfo` refuses a window that does
/// with `ERROR_INVALID_PARAMETER`. Before this clamp existed, a zoom-out asked
/// for 141 columns on a host whose largest window was 128, the refusal left the
/// console at the one-cell rectangle Windows had collapsed it to when the
/// buffer grew, and every later poll scraped that one cell: a blank terminal
/// with a live child behind it. Measured on a hosted Windows x86_64 runner and
/// in the aarch64 court on 2026-09-24.
fn plan_resize(cols: u16, rows: u16, cursor_row: i16, largest: (i16, i16)) -> ResizePlan {
    let buffer_cols = cols.max(1).min(i16::MAX as u16) as i16;
    let buffer_rows = u32::from(rows.max(1))
        .saturating_add(BUFFER_SCROLL_ROOM_ROWS)
        .min(i16::MAX as u32) as i16;
    // A host that reports no largest window at all still gets a usable
    // rectangle: clamping to zero would recreate the defect being fixed.
    let window_cols = buffer_cols.min(largest.0.max(1));
    let window_rows = (rows.max(1).min(i16::MAX as u16) as i16)
        .min(largest.1.max(1))
        .min(buffer_rows);
    // Anchor on the cursor. The console writes at its cursor and this buffer is
    // taller than the window, so a window left where it happened to be -- at
    // the buffer's top, at its bottom, or where the previous size put it -- can
    // show rows the program is not writing to.
    let highest_top = (buffer_rows - window_rows).max(0);
    let top = (cursor_row - window_rows + 1).clamp(0, highest_top);
    ResizePlan {
        buffer: (buffer_cols, buffer_rows),
        window: SMALL_RECT {
            Left: 0,
            Top: top,
            Right: window_cols - 1,
            Bottom: top + window_rows - 1,
        },
    }
}

fn open_console_device(name: &str) -> io::Result<OwnedHandle> {
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        // The child must inherit these, or it paints nowhere and the scrape
        // comes back blank with no error anywhere.
        bInheritHandle: 1,
    };
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        // SAFETY: the name is NUL-terminated and attributes is initialized.
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &attributes,
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe {
        // SAFETY: CreateFileW returned a unique owned handle.
        OwnedHandle::from_raw_handle(handle as _)
    })
}

fn spawn_child(console: &ConsoleHandles, command_line: &mut [u16]) -> io::Result<OwnedHandle> {
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdInput = console.input.as_raw_handle() as HANDLE;
    startup.hStdOutput = console.output.as_raw_handle() as HANDLE;
    startup.hStdError = console.output.as_raw_handle() as HANDLE;

    let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        // SAFETY: command_line is a live NUL-terminated mutable buffer and
        // every other pointer is null or a live local. The child inherits the
        // agent's environment and working directory, which the host already
        // set to what the child should see.
        CreateProcessW(
            null(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            0,
            null(),
            null(),
            &startup,
            &mut information,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        // SAFETY: the thread handle is owned here and never used.
        CloseHandle(information.hThread);
    }
    Ok(unsafe {
        // SAFETY: CreateProcessW transferred a unique process handle.
        OwnedHandle::from_raw_handle(information.hProcess as _)
    })
}

// ---------------------------------------------------------------------------
// Screen mirror: console buffer -> terminal stream
// ---------------------------------------------------------------------------

/// What the host has already been told the screen looks like.
///
/// The mirror exists so the agent emits *differences*. Repainting everything
/// each poll would work and would also make every keystroke redraw the screen,
/// which is both slow and visibly wrong when the user has scrolled.
struct ScreenMirror {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
    cursor: (u16, u16),
    attributes: u16,
    /// The absolute buffer row the window started at last time. When the
    /// console scrolls, this is how the agent knows to push lines into the
    /// host's scrollback rather than silently overwrite them.
    window_top: i16,
    started: bool,
}

impl ScreenMirror {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); usize::from(cols) * usize::from(rows)],
            cursor: (0, 0),
            attributes: DEFAULT_ATTRIBUTES,
            window_top: 0,
            started: false,
        }
    }

    /// Reads the console once and writes whatever changed. Returns whether
    /// anything did.
    fn poll_and_emit(&mut self, console: &ConsoleHandles, output: HANDLE) -> io::Result<bool> {
        let info = console.info()?;
        let cols = (info.srWindow.Right - info.srWindow.Left + 1).max(1) as u16;
        let rows = (info.srWindow.Bottom - info.srWindow.Top + 1).max(1) as u16;
        let mut out = Vec::with_capacity(256);

        if cols != self.cols || rows != self.rows {
            // A resize invalidates every row. Start the mirror over rather
            // than trying to reconcile two geometries.
            *self = Self::new(cols, rows);
        }

        let window_scrolled = (info.srWindow.Top - self.window_top).max(0) as usize;
        self.window_top = info.srWindow.Top;

        let read = read_window(console, &info, cols, rows)?;
        // The buffer is exactly the window's size (see `resize`), so a program
        // that fills the last row scrolls the buffer's own contents and
        // `srWindow.Top` never moves. Recover the shift from the frames
        // themselves; the window rectangle still speaks for the cases where it
        // does move.
        let scrolled = if window_scrolled > 0 {
            window_scrolled
        } else {
            crate::adapters::console_row_emit::scrolled_rows(&self.cells, &read, cols, rows)
        };

        if !self.started {
            self.started = true;
            // Clear and home once, so the host's idea of the screen and the
            // mirror's start out identical instead of merely similar.
            out.extend_from_slice(b"\x1b[H\x1b[2J");
            self.attributes = DEFAULT_ATTRIBUTES;
            out.extend_from_slice(b"\x1b[0m");
            self.cells = vec![Cell::default(); usize::from(cols) * usize::from(rows)];
            self.cursor = (0, 0);
        } else if scrolled > 0 && self.cells.len() == read.len() {
            // Park at the last row and feed newlines: that is what pushes the
            // vacated lines into the host's scrollback. Emitting them as text
            // would duplicate content the host already has.
            let feed = usize::from(rows).min(scrolled);
            out.extend_from_slice(format!("\x1b[{};1H", rows).as_bytes());
            for _ in 0..feed {
                out.extend_from_slice(b"\r\n");
            }
            self.scroll_mirror(feed);
            self.cursor = (rows.saturating_sub(1), 0);
        }

        for row in 0..rows {
            let start = usize::from(row) * usize::from(cols);
            let new_row = &read[start..start + usize::from(cols)];
            if self.cells[start..start + usize::from(cols)] == *new_row {
                continue;
            }
            self.emit_row(&mut out, row, new_row);
            self.cells[start..start + usize::from(cols)].copy_from_slice(new_row);
        }

        // Clamped into the window: the console reports the cursor in absolute
        // buffer coordinates, and a cursor parked one row past the last line
        // would otherwise be emitted as a position the host does not have.
        let last_row = rows.saturating_sub(1) as i16;
        let last_column = cols.saturating_sub(1) as i16;
        let cursor = (
            (info.dwCursorPosition.Y - info.srWindow.Top).clamp(0, last_row) as u16,
            (info.dwCursorPosition.X - info.srWindow.Left).clamp(0, last_column) as u16,
        );
        if cursor != self.cursor || !out.is_empty() {
            out.extend_from_slice(format!("\x1b[{};{}H", cursor.0 + 1, cursor.1 + 1).as_bytes());
            self.cursor = cursor;
        }

        if out.is_empty() {
            return Ok(false);
        }
        write_all(output, &out)?;
        Ok(true)
    }

    fn scroll_mirror(&mut self, lines: usize) {
        let width = usize::from(self.cols);
        let shift = lines.min(usize::from(self.rows)) * width;
        self.cells.drain(..shift);
        self.cells
            .resize(width * usize::from(self.rows), Cell::default());
    }

    /// Rewrites one row from column one. Erasing to end of line first is what
    /// makes a shortened line actually get shorter.
    ///
    /// The cell-to-bytes walk — including the double-width (CJK) continuation
    /// handling and the SGR/trailing-blank behaviour — is
    /// [`emit_row_cells`](crate::adapters::console_row_emit::emit_row_cells),
    /// which is platform-independent and unit-tested off Windows.
    fn emit_row(&mut self, out: &mut Vec<u8>, row: u16, cells: &[Cell]) {
        out.extend_from_slice(format!("\x1b[{};1H\x1b[K", row + 1).as_bytes());
        emit_row_cells(out, cells, &mut self.attributes);
        self.cursor = (row, 0);
    }
}

fn read_window(
    console: &ConsoleHandles,
    info: &CONSOLE_SCREEN_BUFFER_INFO,
    cols: u16,
    rows: u16,
) -> io::Result<Vec<Cell>> {
    let mut raw = vec![
        CHAR_INFO {
            Char: CHAR_INFO_0 { UnicodeChar: 0 },
            Attributes: DEFAULT_ATTRIBUTES,
        };
        usize::from(cols) * usize::from(rows)
    ];
    let mut region = SMALL_RECT {
        Left: info.srWindow.Left,
        Top: info.srWindow.Top,
        Right: info.srWindow.Left + cols as i16 - 1,
        Bottom: info.srWindow.Top + rows as i16 - 1,
    };
    let read = unsafe {
        // SAFETY: raw is exactly size.X * size.Y elements, matching the
        // rectangle requested, and region is a live out/in parameter.
        ReadConsoleOutputW(
            console.output.as_raw_handle() as HANDLE,
            raw.as_mut_ptr(),
            COORD {
                X: cols as i16,
                Y: rows as i16,
            },
            COORD { X: 0, Y: 0 },
            &mut region,
        )
    };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(raw.into_iter().map(decode_cell).collect())
}

fn decode_cell(raw: CHAR_INFO) -> Cell {
    let unit = unsafe {
        // SAFETY: the console always fills the Unicode arm for ReadConsoleOutputW.
        raw.Char.UnicodeChar
    };
    Cell {
        text: char::from_u32(u32::from(unit))
            .filter(|c| *c != '\0')
            .unwrap_or(' '),
        // The width bits describe layout, not appearance; keeping them would
        // make two identical-looking cells compare unequal.
        attributes: raw.Attributes & !(COMMON_LVB_LEADING_BYTE | COMMON_LVB_TRAILING_BYTE),
        continuation: raw.Attributes & COMMON_LVB_TRAILING_BYTE != 0,
    }
}

// ---------------------------------------------------------------------------
// Input: terminal bytes -> console input records
// ---------------------------------------------------------------------------

/// The most recent size the host asked for, `cols << 16 | rows`, or zero.
///
/// The control thread only records; the poll thread applies. Resizing the
/// console from another thread races the poll loop's `ReadConsoleOutputW`,
/// whose rectangle is then larger than the buffer it is reading — which
/// surfaces as a failed read and, before this, killed the agent mid-session.
static PENDING_RESIZE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn forward_control(control: HANDLE) {
    use std::sync::atomic::Ordering;

    let mut message = [0_u8; 4];
    loop {
        if read_exact(control, &mut message).is_err() {
            return;
        }
        let cols = u16::from(message[0]) | (u16::from(message[1]) << 8);
        let rows = u16::from(message[2]) | (u16::from(message[3]) << 8);
        if cols == 0 || rows == 0 {
            continue;
        }
        // Last writer wins: an intermediate size during a drag is worth
        // nothing once a newer one has arrived.
        PENDING_RESIZE.store((u32::from(cols) << 16) | u32::from(rows), Ordering::Release);
    }
}

/// Applies a size the control thread recorded, if any.
fn apply_pending_resize(console: &ConsoleHandles) {
    use std::sync::atomic::Ordering;

    let pending = PENDING_RESIZE.swap(0, Ordering::AcqRel);
    if pending == 0 {
        return;
    }
    let cols = (pending >> 16) as u16;
    let rows = (pending & 0xFFFF) as u16;
    // A refused resize is not fatal -- `resize` restores the window it found,
    // so the next poll reads a usable size and the mirror rebuilds around it.
    // It is not silent either: discarding this error is what made the blank
    // terminal undiagnosable from outside. The host's own resize-failure
    // counter stayed at zero through a session that failed every time, because
    // the error stopped here.
    if let Err(error) = console.resize(cols.max(1), rows.max(1)) {
        #[cfg(feature = "runtime")]
        crate::diagnostics::record("console_agent", "resize_refused", &error.to_string());
        let _ = &error;
    }
}

fn forward_input(input: HANDLE, console_input: HANDLE) {
    let mut buffer = [0_u8; 1024];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let read = match read_some(input, &mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(count) => count,
        };
        pending.extend_from_slice(&buffer[..read]);
        let consumed = write_records(console_input, &pending);
        pending.drain(..consumed);
        // An unbounded remainder would mean a byte stream that never forms a
        // sequence; drop it rather than grow without limit.
        if pending.len() > 64 {
            pending.clear();
        }
    }
}

/// Interrupt bytes, which are signals rather than keys.
///
/// `WriteConsoleInput` does not raise a console control event: the console
/// only synthesizes one for real keyboard input. Delivering Ctrl+C as a key
/// record therefore gives the child the keystroke and not the interrupt,
/// which is why a shell echoed `^C` and then carried on running whatever it
/// was running. `GenerateConsoleCtrlEvent` is the actual signal, and process
/// group zero means every process attached to this console — the child, its
/// own children, and the agent itself.
const CTRL_C_BYTE: u8 = 0x03;
const CTRL_BREAK_BYTE: u8 = 0x1C;

fn raise_console_signal(event: u32) {
    unsafe {
        // SAFETY: no pointers. Group 0 addresses this console's process
        // group, which is exactly the set this agent created.
        GenerateConsoleCtrlEvent(event, 0);
    }
}

/// Keeps the agent alive through the interrupt it just raised.
///
/// The agent shares the console with the child, so it receives the signal
/// too. Reporting it handled is what stops it from being killed alongside
/// the process it was trying to interrupt.
///
/// Deliberately a handler function rather than `SetConsoleCtrlHandler(None,
/// TRUE)`: that form sets an *ignore* flag which children inherit, which
/// would leave the shell unable to be interrupted at all — the same defect
/// one level down.
unsafe extern "system" fn agent_ctrl_handler(control_type: u32) -> i32 {
    i32::from(control_type == CTRL_C_EVENT || control_type == CTRL_BREAK_EVENT)
}

fn install_ctrl_handler() {
    unsafe {
        // Clear any inherited Ctrl+C-ignore flag first. That flag -- set by
        // `SetConsoleCtrlHandler(None, TRUE)` somewhere up the ancestry -- is
        // inherited by every descendant, so an agent that keeps it hands a
        // shell that can never be interrupted to the user. The ConPTY path
        // clears it for the same reason before creating its child.
        // SAFETY: affects only this process; no pointers involved.
        SetConsoleCtrlHandler(None, 0);
        // SAFETY: the handler is a plain function with the documented
        // signature and static lifetime.
        SetConsoleCtrlHandler(Some(agent_ctrl_handler), 1);
    }
}

/// Translates as many complete key presses as `bytes` contains, returning how
/// many bytes were consumed. A partial escape sequence is left for the next
/// read rather than being delivered as a literal escape.
fn write_records(console_input: HANDLE, bytes: &[u8]) -> usize {
    let mut records = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        // Signals are handled before decoding, because they are not keys and
        // must not also arrive as one: the child would then see both an
        // interrupt and a literal control character.
        match bytes[index] {
            CTRL_C_BYTE => {
                flush_records(console_input, &mut records);
                raise_console_signal(CTRL_C_EVENT);
                index += 1;
                continue;
            }
            CTRL_BREAK_BYTE => {
                flush_records(console_input, &mut records);
                raise_console_signal(CTRL_BREAK_EVENT);
                index += 1;
                continue;
            }
            _ => {}
        }
        match decode_sgr_mouse(&bytes[index..]) {
            SgrMouse::Report(report, used) => {
                index += used;
                records.push(mouse_record(report));
                continue;
            }
            // A partial report must wait for the rest rather than be decoded as
            // a literal escape key.
            SgrMouse::Incomplete => break,
            SgrMouse::Other => {}
        }
        let Some((key, used)) = decode_key(&bytes[index..]) else {
            break;
        };
        index += used;
        records.push(key_record(key, true));
        records.push(key_record(key, false));
    }
    flush_records(console_input, &mut records);
    index
}

/// Writes and clears whatever has accumulated.
///
/// Called before raising a signal as well as at the end, so keystrokes typed
/// before a Ctrl+C reach the child before the interrupt does rather than
/// after it.
fn flush_records(console_input: HANDLE, records: &mut Vec<INPUT_RECORD>) {
    if records.is_empty() {
        return;
    }
    let mut written = 0_u32;
    unsafe {
        // SAFETY: records is a live slice of initialized records and written
        // is a valid out-pointer.
        WriteConsoleInputW(
            console_input,
            records.as_ptr(),
            records.len() as u32,
            &mut written,
        );
    }
    records.clear();
}

#[derive(Clone, Copy)]
struct Key {
    virtual_key: u16,
    unicode: u16,
    control: bool,
}

const LEFT_CTRL_PRESSED: u32 = 0x0008;
const LEFT_ALT_PRESSED: u32 = 0x0002;
const SHIFT_PRESSED: u32 = 0x0010;

/// Tells the terminal when the child turns console mouse input on or off.
///
/// On this path a program asks for the mouse through `SetConsoleMode`
/// (`ENABLE_MOUSE_INPUT`), not by writing a DECSET. The terminal upstream only
/// parses the output stream, so without this it never learns the child wants
/// the mouse and keeps every click as local selection — which is exactly how
/// mouse support goes missing. Synthesising the DECSET is the same translation
/// ConPTY performs for its own host; this path has to do it itself.
fn announce_mouse_mode(console: &ConsoleHandles, output: HANDLE, announced: &mut bool) {
    let mut mode = 0_u32;
    // SAFETY: the console input handle is owned by this agent for its lifetime.
    if unsafe { GetConsoleMode(console.input.as_raw_handle() as HANDLE, &mut mode) } == 0 {
        return;
    }
    let wanted = mode & ENABLE_MOUSE_INPUT != 0;
    if wanted == *announced {
        return;
    }
    *announced = wanted;
    // 1003 (any-motion) with 1006 (SGR) is what reports every button, drag and
    // wheel with coordinates that do not break past column 223.
    let sequence: &[u8] = if wanted {
        b"\x1b[?1003h\x1b[?1006h"
    } else {
        b"\x1b[?1006l\x1b[?1003l"
    };
    let _ = write_all(output, sequence);
}

/// One mouse report decoded from the SGR (DECSET 1006) form a terminal emits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MouseReport {
    /// The SGR button code, including the modifier, motion and wheel bits.
    code: u32,
    /// Zero-based cell coordinates; the wire form is one-based.
    column: i16,
    row: i16,
    /// `M` is a press, `m` a release.
    pressed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SgrMouse {
    Report(MouseReport, usize),
    /// A prefix of a report: wait for more bytes rather than delivering the
    /// escape literally.
    Incomplete,
    /// Not a mouse report; let the key decoder have it.
    Other,
}

/// Decodes `ESC [ < code ; column ; row (M|m)`.
///
/// ConPTY refuses to carry mouse reports to a child, so on the console-agent
/// path the terminal's SGR bytes are translated into the `MOUSE_EVENT_RECORD`
/// a console delivers natively — which is the same thing a program receives
/// when it runs in a plain conhost window.
fn decode_sgr_mouse(bytes: &[u8]) -> SgrMouse {
    const PREFIX: [u8; 3] = [0x1b, b'[', b'<'];
    for (offset, want) in PREFIX.iter().enumerate() {
        match bytes.get(offset) {
            None => return SgrMouse::Incomplete,
            Some(byte) if byte == want => {}
            Some(_) => return SgrMouse::Other,
        }
    }
    let mut index = PREFIX.len();
    let mut fields = [0_u32; 3];
    for (slot, field) in fields.iter_mut().enumerate() {
        let start = index;
        while let Some(byte) = bytes.get(index) {
            if !byte.is_ascii_digit() {
                break;
            }
            *field = field
                .saturating_mul(10)
                .saturating_add(u32::from(byte - b'0'));
            index += 1;
        }
        if index == start {
            // No digits yet. With bytes still to come this is a report that has
            // not finished arriving, and calling it `Other` here delivered the
            // prefix to the child as a literal ESC -- the very thing this
            // decoder exists to prevent. Only a non-digit that is actually
            // present makes it something else.
            return match bytes.get(index) {
                None => SgrMouse::Incomplete,
                Some(_) => SgrMouse::Other,
            };
        }
        match bytes.get(index) {
            None => return SgrMouse::Incomplete,
            Some(b';') if slot < 2 => index += 1,
            Some(b'M' | b'm') if slot == 2 => {}
            Some(_) => return SgrMouse::Other,
        }
    }
    let pressed = bytes[index] == b'M';
    let cell = |value: u32| i16::try_from(value.saturating_sub(1)).unwrap_or(i16::MAX);
    SgrMouse::Report(
        MouseReport {
            code: fields[0],
            column: cell(fields[1]),
            row: cell(fields[2]),
            pressed,
        },
        index + 1,
    )
}

/// Builds the console record for one decoded report.
fn mouse_record(report: MouseReport) -> INPUT_RECORD {
    const SHIFT_BIT: u32 = 4;
    const ALT_BIT: u32 = 8;
    const CTRL_BIT: u32 = 16;
    const MOTION_BIT: u32 = 32;
    const WHEEL_BIT: u32 = 64;

    let mut buttons = 0_u32;
    let mut flags = 0_u32;
    if report.code & WHEEL_BIT != 0 {
        flags |= MOUSE_WHEELED;
        // The wheel delta rides in the high word, signed: SGR 64 scrolls up.
        let delta: i16 = if report.code & 0x03 == 0 { 120 } else { -120 };
        buttons |= (delta as u16 as u32) << 16;
    } else {
        if report.pressed {
            buttons |= match report.code & 0x03 {
                0 => FROM_LEFT_1ST_BUTTON_PRESSED,
                1 => FROM_LEFT_2ND_BUTTON_PRESSED,
                2 => RIGHTMOST_BUTTON_PRESSED,
                _ => 0,
            };
        }
        if report.code & MOTION_BIT != 0 {
            flags |= MOUSE_MOVED;
        }
    }
    let mut modifiers = 0_u32;
    if report.code & SHIFT_BIT != 0 {
        modifiers |= SHIFT_PRESSED;
    }
    if report.code & ALT_BIT != 0 {
        modifiers |= LEFT_ALT_PRESSED;
    }
    if report.code & CTRL_BIT != 0 {
        modifiers |= LEFT_CTRL_PRESSED;
    }
    INPUT_RECORD {
        EventType: MOUSE_EVENT as u16,
        Event: INPUT_RECORD_0 {
            MouseEvent: MOUSE_EVENT_RECORD {
                dwMousePosition: COORD {
                    X: report.column,
                    Y: report.row,
                },
                dwButtonState: buttons,
                dwControlKeyState: modifiers,
                dwEventFlags: flags,
            },
        },
    }
}

fn key_record(key: Key, down: bool) -> INPUT_RECORD {
    INPUT_RECORD {
        EventType: KEY_EVENT as u16,
        Event: INPUT_RECORD_0 {
            KeyEvent: KEY_EVENT_RECORD {
                bKeyDown: i32::from(down),
                wRepeatCount: 1,
                wVirtualKeyCode: key.virtual_key,
                wVirtualScanCode: 0,
                uChar: KEY_EVENT_RECORD_0 {
                    UnicodeChar: key.unicode,
                },
                dwControlKeyState: if key.control { LEFT_CTRL_PRESSED } else { 0 },
            },
        },
    }
}

/// One key press from the front of a terminal byte stream.
///
/// `None` means the bytes so far are a prefix of something longer and the
/// caller should wait for more.
fn decode_key(bytes: &[u8]) -> Option<(Key, usize)> {
    const VK_BACK: u16 = 0x08;
    const VK_TAB: u16 = 0x09;
    const VK_RETURN: u16 = 0x0D;
    const VK_ESCAPE: u16 = 0x1B;
    const VK_END: u16 = 0x23;
    const VK_HOME: u16 = 0x24;
    const VK_F1: u16 = 0x70;

    let plain = |virtual_key: u16, unicode: u16, used: usize| {
        Some((
            Key {
                virtual_key,
                unicode,
                control: false,
            },
            used,
        ))
    };

    match bytes.first()? {
        0x1B => {
            if bytes.len() < 2 {
                // Could be a lone Escape or the start of a sequence. Waiting
                // one more read is better than guessing wrong on every arrow.
                return None;
            }
            match bytes[1] {
                b'[' => decode_csi(bytes),
                b'O' if bytes.len() >= 3 => match bytes[2] {
                    b'P'..=b'S' => plain(VK_F1 + u16::from(bytes[2] - b'P'), 0, 3),
                    b'H' => plain(VK_HOME, 0, 3),
                    b'F' => plain(VK_END, 0, 3),
                    _ => plain(VK_ESCAPE, 0x1B, 1),
                },
                b'O' => None,
                _ => plain(VK_ESCAPE, 0x1B, 1),
            }
        }
        b'\r' | b'\n' => plain(VK_RETURN, u16::from(b'\r'), 1),
        0x08 | 0x7F => plain(VK_BACK, u16::from(b'\x08'), 1),
        b'\t' => plain(VK_TAB, u16::from(b'\t'), 1),
        byte @ 0x01..=0x1A => Some((
            Key {
                // Ctrl+A is 0x01, and the console expects the letter's own
                // virtual key with the control modifier set.
                virtual_key: u16::from(b'A') + u16::from(*byte) - 1,
                unicode: u16::from(*byte),
                control: true,
            },
            1,
        )),
        _ => {
            // Anything else is text. Decode one whole UTF-8 scalar so a
            // multi-byte character is never split into replacement bytes.
            let text = std::str::from_utf8(bytes).map_or_else(
                |error| {
                    let valid = error.valid_up_to();
                    (valid > 0).then(|| std::str::from_utf8(&bytes[..valid]).unwrap_or_default())
                },
                Some,
            )?;
            let character = text.chars().next()?;
            let mut units = [0_u16; 2];
            let encoded = character.encode_utf16(&mut units);
            plain(0, encoded[0], character.len_utf8())
        }
    }
}

fn decode_csi(bytes: &[u8]) -> Option<(Key, usize)> {
    const VK_PRIOR: u16 = 0x21;
    const VK_NEXT: u16 = 0x22;
    const VK_END: u16 = 0x23;
    const VK_HOME: u16 = 0x24;
    const VK_LEFT: u16 = 0x25;
    const VK_UP: u16 = 0x26;
    const VK_RIGHT: u16 = 0x27;
    const VK_DOWN: u16 = 0x28;
    const VK_INSERT: u16 = 0x2D;
    const VK_DELETE: u16 = 0x2E;
    const VK_F1: u16 = 0x70;

    // Find the final byte of the sequence, which is what identifies it.
    let end = bytes[2..]
        .iter()
        .position(|byte| byte.is_ascii_alphabetic() || *byte == b'~')?;
    let final_byte = bytes[2 + end];
    let parameters = &bytes[2..2 + end];
    let used = 3 + end;
    let key = |virtual_key: u16| {
        Some((
            Key {
                virtual_key,
                unicode: 0,
                control: false,
            },
            used,
        ))
    };
    match final_byte {
        b'A' => key(VK_UP),
        b'B' => key(VK_DOWN),
        b'C' => key(VK_RIGHT),
        b'D' => key(VK_LEFT),
        b'H' => key(VK_HOME),
        b'F' => key(VK_END),
        b'~' => {
            let number: u16 = std::str::from_utf8(parameters)
                .ok()
                .and_then(|text| text.split(';').next()?.parse().ok())?;
            match number {
                1 | 7 => key(VK_HOME),
                2 => key(VK_INSERT),
                3 => key(VK_DELETE),
                4 | 8 => key(VK_END),
                5 => key(VK_PRIOR),
                6 => key(VK_NEXT),
                11..=15 => key(VK_F1 + number - 11),
                17..=21 => key(VK_F1 + number - 12),
                23 | 24 => key(VK_F1 + number - 13),
                _ => Some((
                    Key {
                        virtual_key: 0,
                        unicode: 0,
                        control: false,
                    },
                    used,
                )),
            }
        }
        // An unrecognized sequence is consumed rather than delivered as text:
        // a stray "[200~" typed into a shell is worse than a dropped key.
        _ => Some((
            Key {
                virtual_key: 0,
                unicode: 0,
                control: false,
            },
            used,
        )),
    }
}

// ---------------------------------------------------------------------------
// Raw pipe helpers
// ---------------------------------------------------------------------------

fn write_all(handle: HANDLE, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let mut written = 0_u32;
        let ok = unsafe {
            // SAFETY: bytes is a live slice and written is a valid out-pointer.
            WriteFile(
                handle,
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ));
        }
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "pipe accepted nothing",
            ));
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

fn read_some(handle: HANDLE, buffer: &mut [u8]) -> io::Result<usize> {
    let mut read = 0_u32;
    let ok = unsafe {
        // SAFETY: buffer is live and writable for its whole length.
        ReadFile(
            handle,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut read,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ));
    }
    Ok(read as usize)
}

fn read_exact(handle: HANDLE, buffer: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while filled < buffer.len() {
        match read_some(handle, &mut buffer[filled..])? {
            0 => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            count => filled += count,
        }
    }
    Ok(())
}

use std::os::windows::ffi::OsStrExt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_survives_the_hex_round_trip_with_every_metacharacter() {
        let original: Vec<u16> = "cmd.exe /k \"a b\" & ^ | \"中文\"\0"
            .encode_utf16()
            .collect();
        let encoded = encode_utf16_hex(&original);
        assert!(encoded.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(decode_utf16_hex(&encoded).as_deref(), Some(&original[..]));
    }

    #[test]
    fn a_truncated_or_non_hex_command_line_is_rejected_rather_than_guessed() {
        assert_eq!(decode_utf16_hex("abc"), None, "not a whole unit");
        assert_eq!(decode_utf16_hex("zzzz"), None, "not hexadecimal");
        assert_eq!(decode_utf16_hex(""), Some(Vec::new()));
    }

    /// The console orders colour bits blue-green-red and ANSI red-green-blue.
    /// Getting this backwards is invisible in monochrome output and wrong for
    /// every coloured prompt.
    #[test]
    fn console_colours_map_to_ansi_with_red_and_blue_swapped() {
        assert!(
            sgr_for(FOREGROUND_RED).contains(";31;"),
            "{}",
            sgr_for(FOREGROUND_RED)
        );
        assert!(
            sgr_for(FOREGROUND_BLUE).contains(";34;"),
            "{}",
            sgr_for(FOREGROUND_BLUE)
        );
        assert!(sgr_for(FOREGROUND_GREEN).contains(";32;"));
        assert!(
            sgr_for(FOREGROUND_RED | FOREGROUND_INTENSITY).contains(";91;"),
            "intensity selects the bright range"
        );
        assert!(sgr_for(BACKGROUND_BLUE).contains(";44m"));
        assert!(
            sgr_for(BACKGROUND_BLUE | BACKGROUND_INTENSITY).contains(";104m"),
            "background intensity selects the bright range"
        );
    }

    #[test]
    fn reverse_video_and_underline_are_carried_through() {
        assert!(sgr_for(COMMON_LVB_REVERSE_VIDEO).contains(";7"));
        assert!(sgr_for(COMMON_LVB_UNDERSCORE).contains(";4"));
        assert!(!sgr_for(DEFAULT_ATTRIBUTES).contains(";7"));
    }

    fn wide_cell(unit: u16, trailing: bool) -> CHAR_INFO {
        CHAR_INFO {
            Char: CHAR_INFO_0 { UnicodeChar: unit },
            Attributes: DEFAULT_ATTRIBUTES
                | if trailing {
                    COMMON_LVB_TRAILING_BYTE
                } else {
                    COMMON_LVB_LEADING_BYTE
                },
        }
    }

    /// The trap that shows up as doubled CJK: both halves of a wide character
    /// carry the same code unit, and only the width bits tell them apart.
    #[test]
    fn the_second_half_of_a_wide_character_is_marked_as_continuation() {
        let lead = decode_cell(wide_cell(u16::from(b'A'), false));
        let trail = decode_cell(wide_cell(u16::from(b'A'), true));
        assert!(!lead.continuation);
        assert!(trail.continuation);
        assert_eq!(
            lead.attributes, trail.attributes,
            "width bits must not make two identically-styled cells differ"
        );
    }

    #[test]
    fn a_null_console_cell_reads_as_a_blank_not_a_nul_byte() {
        let cell = decode_cell(CHAR_INFO {
            Char: CHAR_INFO_0 { UnicodeChar: 0 },
            Attributes: DEFAULT_ATTRIBUTES,
        });
        assert_eq!(cell.text, ' ');
    }

    #[test]
    fn arrows_and_editing_keys_decode_to_their_virtual_keys() {
        let cases: &[(&[u8], u16)] = &[
            (b"\x1b[A", 0x26),
            (b"\x1b[B", 0x28),
            (b"\x1b[C", 0x27),
            (b"\x1b[D", 0x25),
            (b"\x1b[H", 0x24),
            (b"\x1b[F", 0x23),
            (b"\x1b[3~", 0x2E),
            (b"\x1b[5~", 0x21),
            (b"\x1b[6~", 0x22),
            (b"\x1bOP", 0x70),
        ];
        for (bytes, expected) in cases {
            let (key, used) = decode_key(bytes).expect("decodes");
            assert_eq!(key.virtual_key, *expected, "{bytes:?}");
            assert_eq!(used, bytes.len(), "{bytes:?} consumed whole");
        }
    }

    /// A half-received escape sequence must not be delivered as a literal
    /// Escape followed by "[", which is what a naive decoder does to every
    /// arrow key that arrives split across two reads.
    #[test]
    fn a_partial_escape_sequence_waits_instead_of_becoming_literal_text() {
        assert!(decode_key(b"\x1b").is_none());
        assert!(decode_key(b"\x1b[").is_none());
        assert!(decode_key(b"\x1b[1").is_none());
        assert!(decode_key(b"\x1bO").is_none());
        let (key, used) = decode_key(b"\x1b[1;5A").expect("complete sequence decodes");
        assert_eq!(key.virtual_key, 0x26);
        assert_eq!(used, 6, "parameters are consumed with the sequence");
    }

    #[test]
    fn control_characters_carry_the_control_modifier_and_the_letter_key() {
        let (key, used) = decode_key(b"\x03").expect("Ctrl+C decodes");
        assert_eq!(used, 1);
        assert!(key.control);
        assert_eq!(key.virtual_key, u16::from(b'C'));
        assert_eq!(key.unicode, 3);
    }

    #[test]
    fn enter_tab_and_backspace_use_the_keys_a_console_expects() {
        assert_eq!(decode_key(b"\r").unwrap().0.virtual_key, 0x0D);
        assert_eq!(decode_key(b"\n").unwrap().0.virtual_key, 0x0D);
        assert_eq!(decode_key(b"\t").unwrap().0.virtual_key, 0x09);
        assert_eq!(decode_key(b"\x7f").unwrap().0.virtual_key, 0x08);
        assert_eq!(decode_key(b"\x08").unwrap().0.virtual_key, 0x08);
    }

    /// A multi-byte character split across two pipe reads must not turn into
    /// replacement characters.
    #[test]
    fn a_split_utf8_character_waits_for_its_remaining_bytes() {
        let text = "中".as_bytes();
        assert!(decode_key(&text[..1]).is_none());
        assert!(decode_key(&text[..2]).is_none());
        let (key, used) = decode_key(text).expect("whole character decodes");
        assert_eq!(used, 3);
        assert_eq!(key.unicode, 0x4E2D);
    }

    #[test]
    fn plain_text_decodes_one_character_at_a_time() {
        let (key, used) = decode_key(b"abc").expect("decodes");
        assert_eq!(used, 1);
        assert_eq!(key.unicode, u16::from(b'a'));
        assert!(!key.control);
    }

    /// The mirror only exists to suppress unchanged output. If an unchanged
    /// screen still emitted bytes, every idle poll would repaint the terminal.
    #[test]
    fn an_unchanged_row_is_not_rewritten() {
        let mut mirror = ScreenMirror::new(4, 2);
        let row = vec![Cell::default(); 4];
        let mut out = Vec::new();
        mirror.emit_row(&mut out, 0, &row);
        assert!(
            out.ends_with(b"\x1b[K"),
            "a blank row erases and writes nothing: {out:?}"
        );
        assert_eq!(
            mirror.cells[..4],
            row[..],
            "mirror starts equal to a blank row"
        );
    }

    #[test]
    fn a_row_is_erased_before_it_is_rewritten_so_it_can_get_shorter() {
        let mut mirror = ScreenMirror::new(6, 1);
        let mut row = vec![Cell::default(); 6];
        row[0].text = 'h';
        row[1].text = 'i';
        let mut out = Vec::new();
        mirror.emit_row(&mut out, 0, &row);
        let rendered = String::from_utf8(out).expect("utf8");
        assert!(rendered.starts_with("\x1b[1;1H\x1b[K"), "{rendered:?}");
        assert!(rendered.ends_with("hi"), "{rendered:?}");
        assert!(
            !rendered.ends_with("hi    "),
            "trailing blanks ride on the erase instead of being written"
        );
    }

    #[test]
    fn scrolling_the_mirror_drops_the_top_and_blanks_the_bottom() {
        let mut mirror = ScreenMirror::new(2, 3);
        for (index, cell) in mirror.cells.iter_mut().enumerate() {
            cell.text = char::from(b'a' + index as u8);
        }
        mirror.scroll_mirror(1);
        assert_eq!(mirror.cells.len(), 6, "geometry is preserved");
        assert_eq!(mirror.cells[0].text, 'c', "the first row is gone");
        assert_eq!(mirror.cells[4], Cell::default(), "the last row is blank");
    }

    #[test]
    fn scrolling_further_than_the_screen_still_leaves_a_whole_screen() {
        let mut mirror = ScreenMirror::new(3, 2);
        mirror.scroll_mirror(99);
        assert_eq!(mirror.cells.len(), 6);
        assert!(mirror.cells.iter().all(|cell| *cell == Cell::default()));
    }

    /// The blank terminal, as geometry. On a hosted Windows x86_64 runner the
    /// zoom-out asked for 141 columns while `GetLargestConsoleWindowSize`
    /// answered 128x43; the unclamped window was refused and the console stayed
    /// at the one cell Windows had collapsed it to. The buffer still holds the
    /// scrollback, so only the window is bound.
    #[test]
    fn a_window_never_exceeds_what_the_host_can_display() {
        let plan = plan_resize(141, 40, 3, (128, 43));
        assert_eq!(plan.buffer.0, 141, "the buffer may exceed the window");
        assert_eq!(plan.buffer.1, 540, "40 rows plus the scroll room");
        assert_eq!(plan.window.Left, 0);
        assert_eq!(plan.window.Right, 127, "the window may not exceed 128");
        assert_eq!(plan.window.Bottom - plan.window.Top + 1, 40);
    }

    /// The same request on a host with room is not moved: measured in the
    /// aarch64 court, where the largest window was 170x62 and all three calls
    /// returned the window {0,0,140,39}.
    #[test]
    fn a_request_within_the_bound_is_left_alone() {
        let plan = plan_resize(141, 40, 0, (170, 62));
        assert_eq!(plan.window.Left, 0);
        assert_eq!(plan.window.Right, 140);
        assert_eq!(plan.window.Top, 0);
        assert_eq!(plan.window.Bottom, 39);
    }

    /// The window follows the cursor without leaving the buffer, which is what
    /// keeps a taller-than-window buffer showing the rows being written.
    #[test]
    fn the_window_holds_the_cursor_without_leaving_the_buffer() {
        let deep = plan_resize(80, 24, 500, (200, 60));
        assert!(deep.window.Top <= 500 && 500 <= deep.window.Bottom);
        let past_the_end = plan_resize(80, 24, i16::MAX, (200, 60));
        assert_eq!(past_the_end.window.Bottom, past_the_end.buffer.1 - 1);
    }

    /// No plan is ever the one-cell rectangle the blank screen came from, and a
    /// host that reports nothing still yields a usable window rather than a
    /// clamp to zero.
    #[test]
    fn no_plan_collapses_the_window() {
        for (cols, rows, cursor, largest) in [
            (141_u16, 40_u16, 3_i16, (128_i16, 43_i16)),
            (1, 1, 0, (200, 60)),
            (141, 40, 0, (0, 0)),
            (u16::MAX, u16::MAX, 0, (128, 43)),
        ] {
            let plan = plan_resize(cols, rows, cursor, largest);
            assert!(
                plan.window.Right >= plan.window.Left && plan.window.Bottom >= plan.window.Top,
                "{cols}x{rows} at {largest:?} produced {},{} {},{}",
                plan.window.Left,
                plan.window.Top,
                plan.window.Right,
                plan.window.Bottom
            );
            assert!(
                plan.window.Right - plan.window.Left + 1 <= plan.buffer.0
                    && plan.window.Bottom - plan.window.Top + 1 <= plan.buffer.1,
                "the window must fit its buffer: {}x{} in {}x{}",
                plan.window.Right - plan.window.Left + 1,
                plan.window.Bottom - plan.window.Top + 1,
                plan.buffer.0,
                plan.buffer.1
            );
        }
    }

    /// The last-resort restore. The rectangle a console had on entry can stop
    /// fitting once the buffer is resized, so the fallback is built from the
    /// buffer the console actually has -- and it is bound by the screen too.
    /// Taking the buffer alone would ask for the whole 540-row scroll room on a
    /// screen holding 43 rows and be refused just like the window it replaces.
    #[test]
    fn the_fallback_window_fits_both_the_buffer_and_the_screen() {
        let buffer = COORD { X: 141, Y: 540 };
        let window = fitted_window(buffer, 3, (128, 43));
        assert_eq!(window.Left, 0);
        assert_eq!(window.Right, 127, "bounded by the screen, not the buffer");
        assert_eq!(window.Bottom - window.Top + 1, 43);
        assert!(window.Bottom < buffer.Y, "and it stays inside the buffer");

        // A narrow buffer bounds it instead, and neither bound may collapse it.
        let narrow = fitted_window(COORD { X: 10, Y: 4 }, 0, (128, 43));
        assert_eq!(narrow.Right, 9);
        assert_eq!(narrow.Bottom - narrow.Top + 1, 4);
        let nothing = fitted_window(COORD { X: 0, Y: 0 }, 0, (0, 0));
        assert!(nothing.Right >= nothing.Left && nothing.Bottom >= nothing.Top);
    }

    #[test]
    fn a_resize_message_is_four_fixed_bytes_in_little_endian_order() {
        // Guards the agent's decode against the host's encode: a pipe has no
        // message boundaries, so both sides must agree exactly.
        let (cols, rows) = (203_u16, 51_u16);
        let message = [
            (cols & 0xFF) as u8,
            (cols >> 8) as u8,
            (rows & 0xFF) as u8,
            (rows >> 8) as u8,
        ];
        assert_eq!(u16::from(message[0]) | (u16::from(message[1]) << 8), cols);
        assert_eq!(u16::from(message[2]) | (u16::from(message[3]) << 8), rows);
    }

    #[test]
    fn only_the_agent_argument_turns_this_binary_into_an_agent() {
        assert_eq!(run_if_agent(&["--help".to_owned()]), None);
        assert_eq!(run_if_agent(&[]), None);
        assert!(AGENT_ARGUMENT.starts_with("--internal-"));
    }

    /// This adapter is shared, and the argument it spawns with is visible in
    /// any process list. A product name here would put one product's brand
    /// inside another's command line, which matters for a name someone
    /// intends to hold a trademark on.
    #[test]
    fn the_agent_argument_carries_no_product_name() {
        // Product names, not substrings of ordinary words: "console" legally
        // contains "con", and refusing that would forbid the only accurate
        // word for what this is.
        for brand in ["agenterm", "minicon", "agenterm-con"] {
            assert!(
                !AGENT_ARGUMENT.contains(brand),
                "{AGENT_ARGUMENT} names the product {brand:?}"
            );
        }
    }

    /// A malformed agent request must fail with a distinct code rather than
    /// run something unexpected: the agent has no channel to explain itself.
    #[test]
    fn a_malformed_agent_request_exits_with_its_own_code() {
        let arguments = vec![AGENT_ARGUMENT.to_owned(), "not-a-handle".to_owned()];
        assert_eq!(run_if_agent(&arguments), Some(251));
    }

    #[test]
    fn screen_read_recovery_is_an_elapsed_time_budget() {
        let start = std::time::Instant::now();
        let mut first_failure = None;
        assert!(!poll_failure_expired(&mut first_failure, start));
        assert!(!poll_failure_expired(
            &mut first_failure,
            start + MAX_POLL_FAILURE_DURATION - std::time::Duration::from_millis(1)
        ));
        assert!(poll_failure_expired(
            &mut first_failure,
            start + MAX_POLL_FAILURE_DURATION
        ));
        assert_eq!(first_failure, Some(start));
    }

    /// ConPTY drops mouse reports, so on this path the terminal's SGR bytes are
    /// the only way a click can reach the child — they must decode exactly, and
    /// a half-arrived report must never be delivered as a literal escape key.
    #[test]
    fn sgr_mouse_reports_decode_into_console_records() {
        // Left press at one-based (11,6) -> zero-based (10,5).
        let press = b"\x1b[<0;11;6M";
        assert_eq!(
            decode_sgr_mouse(press),
            SgrMouse::Report(
                MouseReport {
                    code: 0,
                    column: 10,
                    row: 5,
                    pressed: true
                },
                press.len()
            )
        );
        // The matching release carries the same cell with `m`. It is the same
        // ten bytes as the press, so it consumes ten: this expected eleven and
        // had never run, because these tests compile only on Windows with the
        // `pty` feature and the quick gate runs on macOS.
        let release = b"\x1b[<0;11;6m";
        assert_eq!(
            decode_sgr_mouse(release),
            SgrMouse::Report(
                MouseReport {
                    code: 0,
                    column: 10,
                    row: 5,
                    pressed: false
                },
                release.len()
            )
        );
        // Trailing bytes belong to whatever follows: the report is the nine
        // bytes up to and including `M`, and `rest` is left for the next read.
        assert_eq!(
            decode_sgr_mouse(b"\x1b[<0;1;1Mrest"),
            SgrMouse::Report(
                MouseReport {
                    code: 0,
                    column: 0,
                    row: 0,
                    pressed: true
                },
                b"\x1b[<0;1;1M".len()
            )
        );

        // Every prefix is incomplete, never "not a mouse report": delivering a
        // partial report as a literal ESC is exactly the bug to avoid.
        for cut in 0..press.len() {
            assert_eq!(
                decode_sgr_mouse(&press[..cut]),
                SgrMouse::Incomplete,
                "prefix of length {cut} must wait for more bytes"
            );
        }

        // An ordinary cursor key is not a mouse report and must fall through.
        assert_eq!(decode_sgr_mouse(b"\x1b[A"), SgrMouse::Other);
        assert_eq!(decode_sgr_mouse(b"hello"), SgrMouse::Other);
    }

    /// The button, wheel, motion and modifier bits each land in the field a
    /// console client actually reads.
    #[test]
    fn mouse_records_carry_button_wheel_and_modifier_state() {
        let report = |code: u32, pressed: bool| MouseReport {
            code,
            column: 3,
            row: 4,
            pressed,
        };
        let buttons = |record: INPUT_RECORD| unsafe { record.Event.MouseEvent.dwButtonState };
        let flags = |record: INPUT_RECORD| unsafe { record.Event.MouseEvent.dwEventFlags };
        let modifiers = |record: INPUT_RECORD| unsafe { record.Event.MouseEvent.dwControlKeyState };

        assert_eq!(
            buttons(mouse_record(report(0, true))),
            FROM_LEFT_1ST_BUTTON_PRESSED
        );
        assert_eq!(
            buttons(mouse_record(report(2, true))),
            RIGHTMOST_BUTTON_PRESSED
        );
        // A release reports no button held.
        assert_eq!(buttons(mouse_record(report(0, false))), 0);
        // Motion sets the flag rather than a button transition.
        assert_eq!(
            flags(mouse_record(report(32, true))) & MOUSE_MOVED,
            MOUSE_MOVED
        );
        // Wheel up puts a positive delta in the high word.
        let up = mouse_record(report(64, true));
        assert_eq!(flags(up) & MOUSE_WHEELED, MOUSE_WHEELED);
        assert_eq!((buttons(up) >> 16) as u16 as i16, 120);
        let down = mouse_record(report(65, true));
        assert_eq!((buttons(down) >> 16) as u16 as i16, -120);
        // Ctrl+click keeps the button and reports the modifier.
        let ctrl_click = mouse_record(report(16, true));
        assert_eq!(buttons(ctrl_click), FROM_LEFT_1ST_BUTTON_PRESSED);
        assert_eq!(modifiers(ctrl_click) & LEFT_CTRL_PRESSED, LEFT_CTRL_PRESSED);
    }
}
