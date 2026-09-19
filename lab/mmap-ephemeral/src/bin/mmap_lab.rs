//! mmap-lab — CLI harness over the `shmbox` library (lab only).

use shmbox::{probe_shm, xor_a5, Slot, SlotLoc, WaitKind};
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn self_exe() -> io::Result<PathBuf> {
    env::current_exe()
}

fn spawn_worker(loc: &SlotLoc, wait: WaitKind) -> io::Result<std::process::Child> {
    Command::new(self_exe()?)
        .arg("--wait")
        .arg(wait.as_str())
        .arg("worker")
        .arg(loc.display())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
}

fn spawn_resident(loc: &SlotLoc, wait: WaitKind) -> io::Result<std::process::Child> {
    {
        let slot = Slot::open(loc)?;
        slot.reset_mailbox();
    }
    let mut child = Command::new(self_exe()?)
        .arg("--wait")
        .arg(wait.as_str())
        .arg("resident")
        .arg(loc.display())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    thread::sleep(Duration::from_millis(5));
    if let Some(status) = child.try_wait()? {
        return Err(io::Error::other(format!(
            "resident exited before ready: {status}"
        )));
    }
    Ok(child)
}

fn shutdown_resident(slot: &Slot, mut child: std::process::Child, wait: WaitKind) {
    slot.request_shutdown();
    slot.nudge_after_shutdown(wait);
    let _ = child.wait();
}

fn cmd_probe() -> io::Result<()> {
    match shmbox::probe_native() {
        Ok(name) => {
            println!("native wait: available ({name})");
            Ok(())
        }
        Err(e) => {
            eprintln!("native wait: UNAVAILABLE — {e}");
            Err(io::Error::other(e))
        }
    }
}

fn cmd_init(loc: &SlotLoc) -> io::Result<()> {
    let _ = Slot::create(loc)?;
    println!("initialized {}", loc.display());
    Ok(())
}

fn cmd_worker(loc: &SlotLoc, wait: WaitKind) -> io::Result<()> {
    let mut slot = Slot::open(loc)?;
    slot.serve_one(wait, xor_a5)
}

fn cmd_resident(loc: &SlotLoc, wait: WaitKind) -> io::Result<()> {
    let mut slot = Slot::open(loc)?;
    slot.run_resident(wait, xor_a5)
}

fn cmd_call(loc: &SlotLoc, payload: &[u8], ephemeral: bool, wait: WaitKind) -> io::Result<()> {
    let mut slot = Slot::open(loc)?;
    let child = if ephemeral {
        Some(spawn_worker(loc, wait)?)
    } else {
        None
    };
    let out = slot.call(payload, wait)?;
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

fn measure_ephemeral(
    loc: &SlotLoc,
    n: usize,
    payload: &[u8],
    wait: WaitKind,
) -> io::Result<Vec<Duration>> {
    let mut slot = Slot::open(loc)?;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        slot.reset_mailbox();
        let t0 = Instant::now();
        let mut child = spawn_worker(loc, wait)?;
        let out = slot.call(payload, wait)?;
        let status = child.wait()?;
        let dt = t0.elapsed();
        if !status.success() {
            return Err(io::Error::other(format!("worker exited {status}")));
        }
        if out.len() != payload.len() {
            return Err(io::Error::other("bad response length"));
        }
        samples.push(dt);
    }
    Ok(samples)
}

fn measure_resident(
    loc: &SlotLoc,
    n: usize,
    payload: &[u8],
    wait: WaitKind,
) -> io::Result<Vec<Duration>> {
    let mut resident = spawn_resident(loc, wait)?;
    let mut slot = Slot::open(loc)?;
    let mut samples = Vec::with_capacity(n);
    let result = (|| -> io::Result<Vec<Duration>> {
        for _ in 0..n {
            if let Some(status) = resident.try_wait()? {
                return Err(io::Error::other(format!(
                    "resident exited mid-bench: {status}"
                )));
            }
            let t0 = Instant::now();
            let out = slot.call(payload, wait)?;
            let dt = t0.elapsed();
            if out.len() != payload.len() {
                return Err(io::Error::other("bad response length"));
            }
            samples.push(dt);
        }
        Ok(samples)
    })();
    shutdown_resident(&slot, resident, wait);
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

fn cmd_bench(loc: &SlotLoc, n: usize, waits: &[WaitKind], skip_ephemeral: bool) -> io::Result<()> {
    let _ = Slot::create(loc)?;
    let payload = b"hello-mmap-ephemeral";
    println!(
        "bench: slot={} n={n} payload={}B warm=1 release strip waits={waits:?}",
        loc.display(),
        payload.len()
    );
    if !skip_ephemeral {
        for wait in waits {
            let label = format!("ephemeral(spawn+mmap,{wait:?})");
            let _ = measure_ephemeral(loc, 1, payload, *wait)?;
            print_stats(&label, measure_ephemeral(loc, n, payload, *wait)?);
        }
    }
    for wait in waits {
        let label = format!("resident(mmap,{wait:?})     ");
        let _ = measure_resident(loc, 1, payload, *wait)?;
        print_stats(&label, measure_resident(loc, n, payload, *wait)?);
    }
    Ok(())
}

fn cmd_rps(loc: &SlotLoc, n: usize, warm: usize, wait: WaitKind) -> io::Result<()> {
    let _ = Slot::create(loc)?;
    let payload = b"hello-mmap-ephemeral";
    let mut resident = spawn_resident(loc, wait)?;
    let mut slot = Slot::open(loc)?;
    for _ in 0..warm {
        let out = slot.call(payload, wait)?;
        if out.len() != payload.len() {
            return Err(io::Error::other("bad response length (warm)"));
        }
    }
    let t0 = Instant::now();
    for _ in 0..n {
        if let Some(status) = resident.try_wait()? {
            return Err(io::Error::other(format!(
                "resident exited mid-rps: {status}"
            )));
        }
        let out = slot.call(payload, wait)?;
        if out.len() != payload.len() {
            return Err(io::Error::other("bad response length"));
        }
    }
    let elapsed = t0.elapsed();
    let secs = elapsed.as_secs_f64().max(1e-12);
    let rps = (n as f64) / secs;
    println!(
        "mmap-rps(resident,{wait:?}): n={n} warm={warm} payload={}B elapsed={elapsed:?} rps={rps:.0}",
        payload.len()
    );
    shutdown_resident(&slot, resident, wait);
    Ok(())
}

fn usage() -> ! {
    eprintln!(
        "usage:
  mmap-lab probe-os-sync | probe-native
  mmap-lab probe-shm
  mmap-lab [--wait yield|native|os_sync] init <slot>
  mmap-lab [--wait yield|native|os_sync] worker <slot>
  mmap-lab [--wait yield|native|os_sync] resident <slot>
  mmap-lab [--wait yield|native|os_sync] call [--ephemeral] <slot> <payload>
  mmap-lab [--wait yield|native|os_sync|both] bench [--backend file|shm] [--skip-ephemeral] <slot> [n]
  mmap-lab [--wait yield|native|os_sync|both] rps <slot> [n] [warm]

Library: shmbox — Endpoint/Server/Client + WaitKind::Native
  (macOS os_sync / Linux futex / Windows WaitOnAddress).
Slot: path or shm:NAME. os_sync is an alias for native.
default --wait: yield (bench/rps default: both)"
    );
    std::process::exit(2);
}

fn parse_flags(
    args: &mut Vec<String>,
) -> (WaitKind, bool, bool, Option<&'static str>, bool) {
    let mut wait = WaitKind::Yield;
    let mut wait_both = false;
    let mut wait_flag_seen = false;
    let mut backend_override = None;
    let mut skip_ephemeral = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--wait" {
            wait_flag_seen = true;
            let v = args.get(i + 1).map(String::as_str).unwrap_or_else(|| usage());
            if v == "both" {
                wait_both = true;
            } else {
                wait = WaitKind::parse(v).unwrap_or_else(|| usage());
            }
            args.remove(i);
            args.remove(i);
            continue;
        }
        if let Some(v) = a.strip_prefix("--wait=") {
            wait_flag_seen = true;
            if v == "both" {
                wait_both = true;
            } else {
                wait = WaitKind::parse(v).unwrap_or_else(|| usage());
            }
            args.remove(i);
            continue;
        }
        if a == "--backend" {
            let v = args.get(i + 1).map(String::as_str).unwrap_or_else(|| usage());
            backend_override = match v {
                "file" => Some("file"),
                "shm" => Some("shm"),
                _ => usage(),
            };
            args.remove(i);
            args.remove(i);
            continue;
        }
        if a == "--skip-ephemeral" {
            skip_ephemeral = true;
            args.remove(i);
            continue;
        }
        i += 1;
    }
    (wait, wait_both, wait_flag_seen, backend_override, skip_ephemeral)
}

fn apply_backend(loc: SlotLoc, backend: Option<&str>) -> SlotLoc {
    match (backend, loc) {
        (Some("shm"), SlotLoc::File(p)) => SlotLoc::Shm(
            p.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("mmap-lab-bench")
                .to_string(),
        ),
        (_, loc) => loc,
    }
}

fn main() {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let (mut wait, mut wait_both, wait_flag_seen, backend_override, skip_ephemeral) =
        parse_flags(&mut args);
    if args.is_empty() {
        usage();
    }
    let cmd = args.remove(0);
    let (wait2, both2, seen2, backend2, skip2) = parse_flags(&mut args);
    if seen2 {
        wait = wait2;
        wait_both = both2;
    }
    let wait_flag_seen = wait_flag_seen || seen2;
    let backend_override = backend2.or(backend_override);
    let skip_ephemeral = skip_ephemeral || skip2;

    if (cmd == "bench" || cmd == "rps") && !wait_flag_seen {
        wait_both = true;
    }

    let result = match cmd.as_str() {
        "probe-os-sync" | "probe-native" => cmd_probe(),
        "probe-shm" => probe_shm(),
        "init" => {
            let loc = apply_backend(
                SlotLoc::parse(args.first().map(String::as_str).unwrap_or_else(|| usage())),
                backend_override,
            );
            cmd_init(&loc)
        }
        "worker" => {
            let loc = SlotLoc::parse(args.first().map(String::as_str).unwrap_or_else(|| usage()));
            cmd_worker(&loc, wait)
        }
        "resident" => {
            let loc = SlotLoc::parse(args.first().map(String::as_str).unwrap_or_else(|| usage()));
            cmd_resident(&loc, wait)
        }
        "call" => {
            let ephemeral = args.first().is_some_and(|a| a == "--ephemeral");
            if ephemeral {
                args.remove(0);
            }
            if args.len() < 2 {
                usage();
            }
            cmd_call(&SlotLoc::parse(&args[0]), args[1].as_bytes(), ephemeral, wait)
        }
        "bench" => {
            if args.is_empty() {
                usage();
            }
            let loc = apply_backend(SlotLoc::parse(&args[0]), backend_override);
            let n = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(200);
            let waits = if wait_both || !wait_flag_seen {
                vec![WaitKind::Yield, WaitKind::Native]
            } else {
                vec![wait]
            };
            cmd_bench(&loc, n, &waits, skip_ephemeral)
        }
        "rps" => {
            if args.is_empty() {
                usage();
            }
            let loc = SlotLoc::parse(&args[0]);
            let n = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
            let warm = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1_000);
            let waits = if wait_both || !wait_flag_seen {
                vec![WaitKind::Yield, WaitKind::Native]
            } else {
                vec![wait]
            };
            let mut ok = Ok(());
            for w in waits {
                if let Err(e) = cmd_rps(&loc, n, warm, w) {
                    ok = Err(e);
                    break;
                }
            }
            ok
        }
        _ => usage(),
    };
    if let Err(e) = result {
        eprintln!("mmap-lab: {e}");
        std::process::exit(1);
    }
}
