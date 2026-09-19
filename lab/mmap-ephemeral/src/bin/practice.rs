//! Practical court: real cross-process mailbox API.
//!
//! Three experiments on the public `Server` / `Client` + `shmbox:file:` address:
//!
//! 1. **ephemeral** — each call spawns a short-lived worker (bind → one reply → exit)
//! 2. **resident** — one long-lived worker, many calls
//! 3. **crash** — `kill -9` mid-flight, then reclaim and serve again
//!
//! Tiny wire verbs (no serde): `XOR <bytes>`, `STAT` → `pid=… gen=…`.

use shmbox::{Client, Endpoint, Error, Server, WaitKind, xor_a5};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const PAYLOAD: &[u8] = b"hello-mmap-ephemeral";

fn self_exe() -> io::Result<PathBuf> {
    env::current_exe()
}

fn slot_path(tag: &str) -> PathBuf {
    env::temp_dir().join(format!("shmbox-practice-{tag}-{}.slot", std::process::id()))
}

fn addr_of(path: &Path) -> String {
    format!("shmbox:file:{}", path.display())
}

fn ep_of(path: &Path) -> Endpoint {
    Endpoint::parse(&addr_of(path))
        .expect("address")
        .with_wait(WaitKind::Native)
}

fn handle_request(req: &[u8], generation: u32) -> Result<Vec<u8>, Error> {
    if req == b"STAT" {
        let body = format!("pid={} gen={generation}", std::process::id());
        return Ok(body.into_bytes());
    }
    if let Some(rest) = req.strip_prefix(b"XOR ") {
        let mut out = rest.to_vec();
        xor_a5(&mut out);
        return Ok(out);
    }
    if let Some(rest) = req.strip_prefix(b"ECHO ") {
        return Ok(rest.to_vec());
    }
    Ok(b"ERR".to_vec())
}

fn serve_one_api(server: &mut Server) -> Result<(), Error> {
    let req = server.accept()?;
    let resp = handle_request(&req, server.generation())?;
    server.reply(&resp)
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn fmt_us(d: Duration) -> String {
    format!("{:.3}µs", d.as_secs_f64() * 1_000_000.0)
}

fn wait_connect(ep: &Endpoint, deadline: Duration) -> Result<Client, Error> {
    let start = Instant::now();
    loop {
        match Client::connect(ep) {
            Ok(c) => return Ok(c),
            Err(Error::NoOwner) | Err(Error::Dead) => {
                if start.elapsed() >= deadline {
                    return Err(Error::NoOwner);
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(Error::Io(e)) if e.kind() == io::ErrorKind::InvalidData => {
                if start.elapsed() >= deadline {
                    return Err(Error::Io(e));
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(e) => return Err(e),
        }
    }
}

fn spawn_worker(path: &Path, mode: &str) -> io::Result<std::process::Child> {
    Command::new(self_exe()?)
        .arg("__worker")
        .arg(mode)
        .arg(addr_of(path))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
}

fn worker_main(mode: &str, addr: &str) -> Result<(), Error> {
    let ep = Endpoint::parse(addr)?.with_wait(WaitKind::Native);
    let mut server = Server::bind(&ep)?;
    match mode {
        "once" => serve_one_api(&mut server),
        "loop" => loop {
            let req = server.accept()?;
            if req == b"QUIT" {
                server.reply(b"bye")?;
                return Ok(());
            }
            let resp = handle_request(&req, server.generation())?;
            server.reply(&resp)?;
        },
        "hang" => {
            let _ = server.accept()?;
            loop {
                thread::sleep(Duration::from_secs(60));
            }
        }
        _ => Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "worker mode",
        ))),
    }
}

fn xor_req() -> Vec<u8> {
    let mut v = Vec::from(&b"XOR "[..]);
    v.extend_from_slice(PAYLOAD);
    v
}

fn expect_xor() -> Vec<u8> {
    let mut v = PAYLOAD.to_vec();
    xor_a5(&mut v);
    v
}

fn run_ephemeral(n: usize) -> Result<(), Error> {
    println!("=== ephemeral (spawn per call, n={n}) ===");
    let path = slot_path("eph");
    let _ = fs::remove_file(&path);
    let ep = ep_of(&path);
    let mut samples = Vec::with_capacity(n);
    let want = expect_xor();
    let req = xor_req();

    for i in 0..n {
        let _ = fs::remove_file(&path);
        let t0 = Instant::now();
        let mut child = spawn_worker(&path, "once").map_err(Error::from)?;
        let mut client = wait_connect(&ep, Duration::from_secs(2))?;
        let out = client.call_timeout(&req, Duration::from_secs(2))?;
        samples.push(t0.elapsed());
        let status = child.wait().map_err(Error::from)?;
        if !status.success() {
            return Err(Error::Io(io::Error::other(format!(
                "worker {i} exited {status}"
            ))));
        }
        if out != want {
            return Err(Error::Io(io::Error::other("bad xor reply")));
        }
    }
    samples.sort();
    println!(
        "  wall (spawn+call+join)  p50={}  p95={}  max={}",
        fmt_us(percentile(&samples, 0.50)),
        fmt_us(percentile(&samples, 0.95)),
        fmt_us(*samples.last().unwrap())
    );
    let _ = fs::remove_file(path);
    Ok(())
}

fn run_resident(n: usize) -> Result<(), Error> {
    println!("=== resident (one worker, n={n} XOR + 1 STAT) ===");
    let path = slot_path("res");
    let _ = fs::remove_file(&path);
    let ep = ep_of(&path);
    let mut child = spawn_worker(&path, "loop").map_err(Error::from)?;
    let mut client = wait_connect(&ep, Duration::from_secs(2))?;

    for _ in 0..20 {
        let _ = client.call_timeout(&xor_req(), Duration::from_secs(1))?;
    }

    let mut samples = Vec::with_capacity(n);
    let want = expect_xor();
    let req = xor_req();
    for _ in 0..n {
        let t0 = Instant::now();
        let out = client.call_timeout(&req, Duration::from_secs(1))?;
        samples.push(t0.elapsed());
        if out != want {
            let _ = child.kill();
            return Err(Error::Io(io::Error::other("bad xor reply")));
        }
    }
    samples.sort();

    let stat = client.call_timeout(b"STAT", Duration::from_secs(1))?;
    let stat_s = String::from_utf8_lossy(&stat);
    let child_pid = child.id();
    if !stat_s.contains(&format!("pid={child_pid}")) {
        let _ = child.kill();
        return Err(Error::Io(io::Error::other(format!(
            "STAT mismatch: {stat_s} vs child {child_pid}"
        ))));
    }
    println!("  STAT ok ({stat_s})");
    println!(
        "  call p50={}  p95={}  max={}",
        fmt_us(percentile(&samples, 0.50)),
        fmt_us(percentile(&samples, 0.95)),
        fmt_us(*samples.last().unwrap())
    );

    let _ = client.call_timeout(b"QUIT", Duration::from_secs(1));
    let _ = child.wait();
    let _ = fs::remove_file(path);
    Ok(())
}

fn run_crash() -> Result<(), Error> {
    println!("=== crash (kill -9 mid-REQ, then reclaim) ===");
    let path = slot_path("crash");
    let _ = fs::remove_file(&path);
    let ep = ep_of(&path);
    let mut child = spawn_worker(&path, "hang").map_err(Error::from)?;
    let mut client = wait_connect(&ep, Duration::from_secs(2))?;
    client.set_call_timeout(Duration::from_millis(800))?;
    client.ask(&xor_req())?;
    thread::sleep(Duration::from_millis(50));
    let killed = child.kill().is_ok();
    let _ = child.wait();
    if !killed {
        return Err(Error::Io(io::Error::other("kill failed")));
    }
    let err = client.await_reply();
    match &err {
        Err(Error::Timeout) | Err(Error::Dead) | Err(Error::Busy) => {
            println!("  client after kill: {err:?} (expected fail)");
        }
        Ok(_) => {
            return Err(Error::Io(io::Error::other("unexpected success after kill")));
        }
        Err(e) => println!("  client after kill: {e} (acceptable)"),
    }
    drop(client);

    let mut server = Server::bind(&ep)?;
    let h = thread::spawn(move || {
        let req = server.accept().unwrap();
        let resp = handle_request(&req, server.generation()).unwrap();
        server.reply(&resp).unwrap();
    });
    thread::sleep(Duration::from_millis(20));
    let mut client = Client::connect(&ep)?;
    let out = client.call_timeout(&xor_req(), Duration::from_secs(2))?;
    if out != expect_xor() {
        return Err(Error::Io(io::Error::other("reclaim xor failed")));
    }
    h.join().map_err(|_| Error::Io(io::Error::other("join")))?;
    println!("  reclaim + XOR ok");
    let _ = fs::remove_file(path);
    Ok(())
}

fn print_usage() -> ! {
    eprintln!(
        "usage:\n  shmbox-practice              # run all three courts\n  shmbox-practice __worker <once|loop|hang> <shmbox:file:…>"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("__worker") {
        if args.len() != 3 {
            print_usage();
        }
        let mode = args[1].clone();
        let addr = args[2].clone();
        if let Err(e) = worker_main(&mode, &addr) {
            eprintln!("shmbox-practice worker: {e}");
            std::process::exit(1);
        }
        return;
    }
    if !args.is_empty() {
        print_usage();
    }

    println!("shmbox practice court (release, WaitKind::Native)");
    println!("exe={}", self_exe().unwrap_or_default().display());

    let mut failed = false;
    for (name, f) in [
        (
            "ephemeral",
            Box::new(|| run_ephemeral(40)) as Box<dyn Fn() -> Result<(), Error>>,
        ),
        ("resident", Box::new(|| run_resident(500))),
        ("crash", Box::new(run_crash)),
    ] {
        let t0 = Instant::now();
        match f() {
            Ok(()) => println!("  PASS {name} in {:.2}s\n", t0.elapsed().as_secs_f64()),
            Err(e) => {
                eprintln!("  FAIL {name}: {e}\n");
                failed = true;
            }
        }
    }
    let _ = io::stdout().flush();
    if failed {
        std::process::exit(1);
    }
    println!("all practice courts PASS");
}
