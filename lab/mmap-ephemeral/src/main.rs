//! Lab: mmap mailbox API without listen sockets.
//!
//! Modes:
//! - `init <slot>`           create/truncate the shared slot file
//! - `worker <slot>`         one-shot: serve one request then exit
//! - `resident <slot>`       loop until `shutdown` flag is set
//! - `call <slot> <payload>` write one request; wait for response (caller must
//!                           already have a resident, or spawn a worker)
//! - `bench <slot> [n]`      compare ephemeral-spawn vs resident latencies

use std::env;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const MAGIC: u64 = 0x4d4d_4150_4550_4801; // "MMAPEPH\x01"
const SLOT_BYTES: usize = 64 * 1024;
const PAYLOAD_CAP: usize = SLOT_BYTES - HEADER_BYTES;
const HEADER_BYTES: usize = 64;

const STATE_IDLE: u32 = 0;
const STATE_REQ: u32 = 1;
const STATE_RESP: u32 = 2;

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const MAP_SHARED: i32 = 1;

#[repr(C)]
struct Header {
    magic: AtomicU64,
    seq: AtomicU64,
    state: AtomicU32,
    shutdown: AtomicU32,
    req_len: AtomicU32,
    resp_len: AtomicU32,
    _pad: [u8; 32],
}

struct SlotMap {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: SlotMap is only used on one thread in this lab binary.
unsafe impl Send for SlotMap {}

impl SlotMap {
    fn as_slice(&self) -> &[u8] {
        // SAFETY: ptr/len come from mmap of SLOT_BYTES; mapping stays alive for SlotMap.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn as_slice_mut(&mut self) -> &mut [u8] {
        // SAFETY: same as as_slice; exclusive &mut self.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for SlotMap {
    fn drop(&mut self) {
        // SAFETY: ptr was returned by mmap with this len.
        unsafe {
            let _ = munmap(self.ptr as *mut _, self.len);
        }
    }
}

#[link(name = "c")]
unsafe extern "C" {
    fn mmap(
        addr: *mut u8,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: isize,
    ) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

fn header(map: &SlotMap) -> &Header {
    assert!(map.len >= HEADER_BYTES);
    // SAFETY: slot is SLOT_BYTES; Header is #[repr(C)] and fits in HEADER_BYTES.
    unsafe { &*(map.ptr as *const Header) }
}

fn payload_mut(map: &mut SlotMap) -> &mut [u8] {
    &mut map.as_slice_mut()[HEADER_BYTES..HEADER_BYTES + PAYLOAD_CAP]
}

fn payload(map: &SlotMap) -> &[u8] {
    &map.as_slice()[HEADER_BYTES..HEADER_BYTES + PAYLOAD_CAP]
}

fn open_slot(path: &Path, create: bool) -> io::Result<SlotMap> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(create)
        .open(path)?;
    if create {
        file.set_len(SLOT_BYTES as u64)?;
    }
    let fd = file.as_raw_fd();
    // SAFETY: fd is open RW; length is SLOT_BYTES; MAP_SHARED for cross-process.
    let ptr = unsafe {
        mmap(
            ptr::null_mut(),
            SLOT_BYTES,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            fd,
            0,
        )
    };
    if ptr.is_null() || ptr == (!0usize as *mut u8) {
        return Err(io::Error::last_os_error());
    }
    let mut map = SlotMap {
        ptr,
        len: SLOT_BYTES,
    };
    if create {
        map.as_slice_mut().fill(0);
        let h = header(&map);
        h.magic.store(MAGIC, Ordering::Release);
        h.state.store(STATE_IDLE, Ordering::Release);
    } else {
        let magic = header(&map).magic.load(Ordering::Acquire);
        if magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bad slot magic: {magic:#x}"),
            ));
        }
    }
    Ok(map)
}

fn wait_state(h: &Header, want: u32, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    loop {
        if h.state.load(Ordering::Acquire) == want {
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timeout waiting for state {want}"),
            ));
        }
        thread::yield_now();
    }
}

fn serve_one(map: &mut SlotMap) -> io::Result<()> {
    wait_state(header(map), STATE_REQ, Duration::from_secs(5))?;
    let n = header(map).req_len.load(Ordering::Acquire) as usize;
    if n > PAYLOAD_CAP {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "req too large"));
    }
    // Toy work: XOR each byte with 0xA5 (proves worker touched the payload).
    for b in &mut payload_mut(map)[..n] {
        *b ^= 0xA5;
    }
    let h = header(map);
    h.resp_len.store(n as u32, Ordering::Release);
    h.seq.fetch_add(1, Ordering::AcqRel);
    h.state.store(STATE_RESP, Ordering::Release);
    Ok(())
}

fn call_once(map: &mut SlotMap, req: &[u8]) -> io::Result<Vec<u8>> {
    if req.len() > PAYLOAD_CAP {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "payload too large"));
    }
    {
        let h = header(map);
        if h.state.load(Ordering::Acquire) != STATE_IDLE {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "slot busy (state != idle)",
            ));
        }
    }
    payload_mut(map)[..req.len()].copy_from_slice(req);
    {
        let h = header(map);
        h.req_len.store(req.len() as u32, Ordering::Release);
        h.state.store(STATE_REQ, Ordering::Release);
    }
    wait_state(header(map), STATE_RESP, Duration::from_secs(10))?;
    let n = header(map).resp_len.load(Ordering::Acquire) as usize;
    let out = payload(map)[..n].to_vec();
    header(map).state.store(STATE_IDLE, Ordering::Release);
    Ok(out)
}

fn self_exe() -> io::Result<PathBuf> {
    env::current_exe()
}

fn spawn_worker(slot: &Path) -> io::Result<std::process::Child> {
    Command::new(self_exe()?)
        .arg("worker")
        .arg(slot)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
}

fn cmd_init(slot: &Path) -> io::Result<()> {
    let _ = open_slot(slot, true)?;
    println!("initialized {}", slot.display());
    Ok(())
}

fn cmd_worker(slot: &Path) -> io::Result<()> {
    let mut map = open_slot(slot, false)?;
    serve_one(&mut map)?;
    Ok(())
}

fn cmd_resident(slot: &Path) -> io::Result<()> {
    let mut map = open_slot(slot, false)?;
    loop {
        let h = header(&map);
        if h.shutdown.load(Ordering::Acquire) != 0 {
            return Ok(());
        }
        if h.state.load(Ordering::Acquire) == STATE_REQ {
            serve_one(&mut map)?;
            continue;
        }
        thread::yield_now();
    }
}

fn cmd_call(slot: &Path, payload_bytes: &[u8], ephemeral: bool) -> io::Result<()> {
    let mut map = open_slot(slot, false)?;
    let child = if ephemeral {
        Some(spawn_worker(slot)?)
    } else {
        None
    };
    let out = call_once(&mut map, payload_bytes)?;
    if let Some(mut c) = child {
        let status = c.wait()?;
        if !status.success() {
            return Err(io::Error::other(format!("worker exited {status}")));
        }
    }
    io::stdout().write_all(&out)?;
    Ok(())
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn measure_ephemeral(slot: &Path, n: usize, payload_bytes: &[u8]) -> io::Result<Vec<Duration>> {
    let mut map = open_slot(slot, false)?;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        header(&map).state.store(STATE_IDLE, Ordering::Release);
        let t0 = Instant::now();
        let mut child = spawn_worker(slot)?;
        let out = call_once(&mut map, payload_bytes)?;
        let status = child.wait()?;
        let dt = t0.elapsed();
        if !status.success() {
            return Err(io::Error::other(format!("worker exited {status}")));
        }
        if out.len() != payload_bytes.len() {
            return Err(io::Error::other("bad response length"));
        }
        samples.push(dt);
    }
    Ok(samples)
}

fn measure_resident(slot: &Path, n: usize, payload_bytes: &[u8]) -> io::Result<Vec<Duration>> {
    let mut resident = Command::new(self_exe()?)
        .arg("resident")
        .arg(slot)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;

    thread::sleep(Duration::from_millis(20));

    let mut map = open_slot(slot, false)?;
    header(&map).state.store(STATE_IDLE, Ordering::Release);
    header(&map).shutdown.store(0, Ordering::Release);

    let mut samples = Vec::with_capacity(n);
    let result = (|| -> io::Result<Vec<Duration>> {
        for _ in 0..n {
            let t0 = Instant::now();
            let out = call_once(&mut map, payload_bytes)?;
            let dt = t0.elapsed();
            if out.len() != payload_bytes.len() {
                return Err(io::Error::other("bad response length"));
            }
            samples.push(dt);
        }
        Ok(samples)
    })();

    header(&map).shutdown.store(1, Ordering::Release);
    let _ = resident.wait();
    result
}

fn print_stats(label: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let n = samples.len();
    let sum: Duration = samples.iter().copied().sum();
    let mean = if n == 0 {
        Duration::ZERO
    } else {
        sum / (n as u32)
    };
    println!(
        "{label}: n={n} min={:?} p50={:?} p95={:?} max={:?} mean={:?}",
        samples.first().copied().unwrap_or_default(),
        percentile(&samples, 0.50),
        percentile(&samples, 0.95),
        samples.last().copied().unwrap_or_default(),
        mean,
    );
}

fn cmd_bench(slot: &Path, n: usize) -> io::Result<()> {
    let _ = open_slot(slot, true)?;
    let payload_bytes = b"hello-mmap-ephemeral";
    let _ = measure_ephemeral(slot, 1, payload_bytes)?;
    let eph = measure_ephemeral(slot, n, payload_bytes)?;
    print_stats("ephemeral(spawn+mmap)", eph);
    let res = measure_resident(slot, n, payload_bytes)?;
    print_stats("resident(mmap poll)  ", res);
    Ok(())
}

fn usage() -> ! {
    eprintln!(
        "usage:
  mmap-lab init <slot>
  mmap-lab worker <slot>
  mmap-lab resident <slot>
  mmap-lab call [--ephemeral] <slot> <payload>
  mmap-lab bench <slot> [n]"
    );
    std::process::exit(2);
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let cmd = args.remove(0);
    let result = match cmd.as_str() {
        "init" => {
            let slot = PathBuf::from(args.first().map(String::as_str).unwrap_or_else(|| usage()));
            cmd_init(&slot)
        }
        "worker" => {
            let slot = PathBuf::from(args.first().map(String::as_str).unwrap_or_else(|| usage()));
            cmd_worker(&slot)
        }
        "resident" => {
            let slot = PathBuf::from(args.first().map(String::as_str).unwrap_or_else(|| usage()));
            cmd_resident(&slot)
        }
        "call" => {
            let ephemeral = args.first().is_some_and(|a| a == "--ephemeral");
            if ephemeral {
                args.remove(0);
            }
            if args.len() < 2 {
                usage();
            }
            let slot = PathBuf::from(&args[0]);
            let payload_bytes = args[1].as_bytes();
            cmd_call(&slot, payload_bytes, ephemeral)
        }
        "bench" => {
            if args.is_empty() {
                usage();
            }
            let slot = PathBuf::from(&args[0]);
            let n = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(200);
            cmd_bench(&slot, n)
        }
        _ => usage(),
    };
    if let Err(e) = result {
        eprintln!("mmap-lab: {e}");
        std::process::exit(1);
    }
}
