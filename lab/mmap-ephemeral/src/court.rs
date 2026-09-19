//! One court over the same mailbox.
//!
//! IPC = file slot (size, mode, magic, owner).
//! RPC = `call` / `serve`, and split flight `ask`/`await_reply` · `accept`/`reply`.
//! Address = `shmbox:file:…` / `shmbox:shm:…`.
//!
//! ```sh
//! cargo test --release --lib court -- --test-threads=1
//! ```

use crate::address::Address;
use crate::channel::{Client, Endpoint, Server};
use crate::error::Error;
use crate::rpc::xor_a5;
use crate::slot::{SLOT_BYTES, Slot};
use crate::wait::WaitKind;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn slot_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("shmbox-court-{name}-{}", std::process::id()));
    let _ = fs::remove_file(&path);
    path
}

fn yield_ep(path: &Path) -> Endpoint {
    Endpoint::file(path).with_wait(WaitKind::Yield)
}

fn native_ep(path: &Path) -> Endpoint {
    Endpoint::file(path).with_wait(WaitKind::Native)
}

#[test]
fn ipc_short_file_rejected() {
    let path = slot_path("short");
    let mut f = File::create(&path).unwrap();
    f.write_all(b"nope").unwrap();
    match Slot::open(&yield_ep(&path).loc) {
        Err(err) => assert!(err.to_string().contains("short slot"), "{err}"),
        Ok(_) => panic!("expected short slot"),
    }
    let _ = fs::remove_file(path);
}

#[test]
fn ipc_bad_magic_rejected() {
    let path = slot_path("magic");
    let f = File::create(&path).unwrap();
    f.set_len(SLOT_BYTES as u64).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(fs::Permissions::from_mode(0o600))
            .unwrap();
    }
    drop(f);
    match Slot::open(&yield_ep(&path).loc) {
        Err(err) => assert!(err.to_string().contains("bad slot magic"), "{err}"),
        Ok(_) => panic!("expected bad magic"),
    }
    let _ = fs::remove_file(path);
}

#[test]
#[cfg(unix)]
fn ipc_group_writable_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let path = slot_path("mode");
    let ep = yield_ep(&path);
    let server = Server::bind(&ep).unwrap();
    drop(server);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    match Slot::open(&ep.loc) {
        Err(err) => assert!(err.to_string().contains("slot mode"), "{err}"),
        Ok(_) => panic!("expected slot mode"),
    }
    let _ = fs::remove_file(path);
}

#[test]
#[cfg(unix)]
fn rpc_roundtrip_xor_and_mode() {
    use std::os::unix::fs::PermissionsExt;
    let path = slot_path("xor");
    let ep = yield_ep(&path);
    let mut server = Server::bind(&ep).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let h = thread::spawn(move || server.serve_one(xor_a5).unwrap());
    thread::sleep(Duration::from_millis(20));
    let mut client = Client::connect(&ep).unwrap();
    let out = client.call(b"hello-mmap-ephemeral").unwrap();
    assert_eq!(out.len(), 20);
    assert_eq!(out[0], b'h' ^ 0xA5);
    h.join().unwrap();
    let _ = fs::remove_file(path);
}

#[test]
fn rpc_timeout_then_next_call_works() {
    let path = slot_path("timeout");
    let ep = yield_ep(&path);
    let mut server = Server::bind(&ep).unwrap();
    let h = thread::spawn(move || {
        server
            .serve_one(|buf| {
                thread::sleep(Duration::from_millis(400));
                xor_a5(buf);
            })
            .unwrap();
        server.serve_one(xor_a5).unwrap();
    });
    thread::sleep(Duration::from_millis(20));
    let mut client = Client::connect(&ep).unwrap();
    let err = client
        .call_timeout(b"hello-mmap-ephemeral", Duration::from_millis(50))
        .unwrap_err();
    assert!(matches!(err, Error::Timeout), "{err}");
    let out = client
        .call_timeout(b"hello-mmap-ephemeral", Duration::from_secs(2))
        .unwrap();
    assert_eq!(out[0], b'h' ^ 0xA5);
    h.join().unwrap();
    let _ = fs::remove_file(path);
}

#[test]
fn rpc_second_client_is_busy() {
    let path = slot_path("busy");
    let ep = yield_ep(&path);
    let mut server = Server::bind(&ep).unwrap();
    let h = thread::spawn(move || {
        server
            .serve_one(|buf| {
                thread::sleep(Duration::from_millis(300));
                xor_a5(buf);
            })
            .unwrap();
    });
    thread::sleep(Duration::from_millis(20));
    let mut first = Client::connect(&ep).unwrap();
    let mut second = Client::connect(&ep).unwrap();
    let slow = thread::spawn(move || {
        first
            .call_timeout(b"hello-mmap-ephemeral", Duration::from_secs(2))
            .unwrap()
    });
    thread::sleep(Duration::from_millis(30));
    let err = second
        .call_timeout(b"z", Duration::from_millis(50))
        .unwrap_err();
    assert!(matches!(err, Error::Busy), "{err}");
    let out = slow.join().unwrap();
    assert_eq!(out[0], b'h' ^ 0xA5);
    h.join().unwrap();
    let _ = fs::remove_file(path);
}

#[test]
fn rpc_rejected_is_a_reply() {
    let path = slot_path("rej");
    let ep = yield_ep(&path);
    let mut server = Server::bind(&ep).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let h = thread::spawn(move || {
        server
            .serve_reply(|_| Err(std::io::Error::other("no")))
            .unwrap();
        let _ = rx.recv();
    });
    thread::sleep(Duration::from_millis(20));
    let mut client = Client::connect(&ep).unwrap();
    let err = client.call(b"x").unwrap_err();
    assert!(matches!(err, Error::Rejected), "{err}");
    let _ = tx.send(());
    h.join().unwrap();
    let _ = fs::remove_file(path);
}

#[test]
fn rpc_live_owner_blocks_bind_dead_owner_reclaimed() {
    let path = slot_path("own");
    let ep = yield_ep(&path);
    let server = Server::bind(&ep).unwrap();
    server.slot().force_owner_pid(1);
    match Server::bind(&ep) {
        Err(err) => assert!(matches!(err, Error::AlreadyBound), "{err}"),
        Ok(_) => panic!("expected already bound"),
    }
    server.slot().force_owner_pid(u32::MAX - 7);
    let again = Server::bind(&ep).unwrap();
    assert_ne!(again.generation(), server.generation());
    drop(again);
    drop(server);
    let _ = fs::remove_file(path);
}

#[test]
fn rpc_connect_dead_owner_is_dead() {
    let path = slot_path("conn-dead");
    let ep = yield_ep(&path);
    let server = Server::bind(&ep).unwrap();
    server.slot().force_owner_pid(u32::MAX - 7);
    match Client::connect(&ep) {
        Err(err) => assert!(matches!(err, Error::Dead), "{err}"),
        Ok(_) => panic!("expected dead"),
    }
    drop(server);
    let _ = fs::remove_file(path);
}

#[test]
fn rpc_attach_refuses_foreign_owner() {
    let path = slot_path("attach");
    let ep = yield_ep(&path);
    let server = Server::bind(&ep).unwrap();
    server.slot().request_shutdown();
    server.slot().force_owner_pid(1);
    match Server::attach(&ep) {
        Err(err) => assert!(matches!(err, Error::AlreadyBound), "{err}"),
        Ok(_) => panic!("expected already bound"),
    }
    assert!(server.slot().is_shutdown());
    drop(server);
    let _ = fs::remove_file(path);
}

#[test]
fn rpc_drop_releases_owner_and_old_client_is_dead() {
    let path = slot_path("drop");
    let ep = yield_ep(&path);
    let server = Server::bind(&ep).unwrap();
    let mut client = Client::connect(&ep).unwrap();
    drop(server);
    let err = client
        .call_timeout(b"x", Duration::from_millis(200))
        .unwrap_err();
    assert!(matches!(err, Error::Dead), "{err}");
    let slot = Slot::open(&ep.loc).unwrap();
    assert_eq!(slot.owner_pid(), 0);
    drop(slot);
    let mut server = Server::bind(&ep).unwrap();
    let h = thread::spawn(move || server.serve_one(xor_a5).unwrap());
    thread::sleep(Duration::from_millis(20));
    let mut client = Client::connect(&ep).unwrap();
    let out = client.call(b"ab").unwrap();
    assert_eq!(out[0], b'a' ^ 0xA5);
    h.join().unwrap();
    let _ = fs::remove_file(path);
}

#[test]
fn ipc_self_birth_is_stable() {
    let birth = Slot::current_birth();
    assert!(birth.is_some() && birth != Some(0), "{birth:?}");
    assert_eq!(birth, Slot::current_birth());
}

#[test]
fn rpc_recycled_pid_does_not_block_bind() {
    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = u32::try_from(child.id()).unwrap();
    let path = slot_path("recycle");
    let ep = yield_ep(&path);
    let server = Server::bind(&ep).unwrap();
    server.slot().force_owner_pid(pid);
    match Server::bind(&ep) {
        Err(err) => assert!(matches!(err, Error::AlreadyBound), "{err}"),
        Ok(_) => panic!("expected live pid to block"),
    }
    server.slot().force_owner_birth(1);
    let again = Server::bind(&ep);
    let _ = child.kill();
    let _ = child.wait();
    let again = again.expect("recycled pid must not block");
    assert_ne!(again.generation(), server.generation());
    drop(again);
    drop(server);
    let _ = fs::remove_file(path);
}

#[test]
fn rpc_dead_client_flight_reclaimed() {
    let path = slot_path("dead-flight");
    let ep = yield_ep(&path);
    let mut server = Server::bind(&ep).unwrap();
    let h = thread::spawn(move || server.serve_one(xor_a5).unwrap());
    thread::sleep(Duration::from_millis(20));
    let peek = Slot::open(&ep.loc).unwrap();
    peek.force_dead_client_flight();
    drop(peek);
    let mut client = Client::connect(&ep).unwrap();
    let out = client.call_timeout(b"ab", Duration::from_secs(2)).unwrap();
    assert_eq!(out[0], b'a' ^ 0xA5);
    h.join().unwrap();
    let _ = fs::remove_file(path);
}

#[test]
fn rpc_bind_recreates_bad_magic() {
    let path = slot_path("recreate");
    let ep = yield_ep(&path);
    let f = File::create(&path).unwrap();
    f.set_len(SLOT_BYTES as u64).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(fs::Permissions::from_mode(0o600))
            .unwrap();
    }
    drop(f);
    let mut server = Server::bind(&ep).unwrap();
    let h = thread::spawn(move || server.serve_one(xor_a5).unwrap());
    thread::sleep(Duration::from_millis(20));
    let mut client = Client::connect(&ep).unwrap();
    let out = client.call(b"ab").unwrap();
    assert_eq!(out[0], b'a' ^ 0xA5);
    h.join().unwrap();
    let _ = fs::remove_file(path);
}

#[test]
#[cfg(unix)]
fn rpc_bind_tightens_group_writable() {
    use std::os::unix::fs::PermissionsExt;
    let path = slot_path("tighten");
    let ep = yield_ep(&path);
    let server = Server::bind(&ep).unwrap();
    drop(server);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    let server = Server::bind(&ep).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    drop(server);
    let _ = fs::remove_file(path);
}

/// Spawned by [`cross_process_native_call`]. No-op when run inside the suite.
#[test]
fn child_serve_one() {
    let Ok(path) = std::env::var("SHMBOX_COURT_SLOT") else {
        return;
    };
    let ep = native_ep(Path::new(&path));
    let mut server = Server::bind(&ep).expect("child bind");
    server.serve_one(xor_a5).expect("child serve");
}

#[test]
fn cross_process_native_call() {
    if std::env::var_os("SHMBOX_COURT_SLOT").is_some() {
        return;
    }
    let path = slot_path("xproc");
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .arg("--exact")
        .arg("court::child_serve_one")
        .env("SHMBOX_COURT_SLOT", &path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let ep = native_ep(&path);
    let mut client = None;
    for _ in 0..100 {
        if let Ok(c) = Client::connect(&ep) {
            client = Some(c);
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            let err = child.wait_with_output().unwrap();
            panic!(
                "child exited early {status}: {}",
                String::from_utf8_lossy(&err.stderr)
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
    let Some(mut client) = client else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("connect");
    };
    let out = client
        .call_timeout(b"hello-mmap-ephemeral", Duration::from_secs(2))
        .unwrap();
    assert_eq!(out[0], b'h' ^ 0xA5);
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = fs::remove_file(path);
}

fn mbox_addr(path: &Path) -> String {
    format!("shmbox:file:{}", path.display())
}

#[test]
fn address_forms() {
    let a = Address::parse("shmbox:file:/tmp/shmbox-court.slot").unwrap();
    assert_eq!(a, Address::File(PathBuf::from("/tmp/shmbox-court.slot")));
    let a = Address::parse("shmbox:/tmp/shmbox-court.slot").unwrap();
    assert_eq!(a, Address::File(PathBuf::from("/tmp/shmbox-court.slot")));
    let a = Address::parse("shmbox:shm:labname").unwrap();
    assert_eq!(a, Address::Shm("labname".into()));
    for bad in [
        "ipc:///tmp/x",
        "shm://x",
        "tcp://1",
        "shmbox:",
        "shmbox:shm:a/b",
    ] {
        let err = Address::parse(bad).unwrap_err();
        assert!(err.to_string().contains("bad address"), "{bad}: {err}");
    }
}

#[test]
fn split_flight_roundtrip() {
    let path = slot_path("split-rt");
    let ep = Endpoint::parse(&mbox_addr(&path))
        .unwrap()
        .with_wait(WaitKind::Yield);
    let mut server = Server::bind(&ep).unwrap();
    let h = thread::spawn(move || {
        let mut buf = server.accept().unwrap();
        xor_a5(&mut buf);
        server.reply(&buf).unwrap();
    });
    let mut client = Client::connect(&ep).unwrap();
    client.ask(b"ab").unwrap();
    let out = client.await_reply().unwrap();
    assert_eq!(out[0], b'a' ^ 0xA5);
    h.join().unwrap();
    let _ = fs::remove_file(path);
}

#[test]
fn split_flight_wrong_phase_is_state() {
    let path = slot_path("split-state");
    let ep = Endpoint::parse(&mbox_addr(&path))
        .unwrap()
        .with_wait(WaitKind::Yield);
    let mut server = Server::bind(&ep).unwrap();
    let err = server.reply(b"x").unwrap_err();
    assert!(matches!(err, Error::State), "{err}");
    let mut client = Client::connect(&ep).unwrap();
    client.ask(b"ab").unwrap();
    let err = client.ask(b"c").unwrap_err();
    assert!(matches!(err, Error::State), "{err}");
    drop(client);
    drop(server);
    let _ = fs::remove_file(path);
}

#[test]
fn try_accept_empty_is_busy() {
    let path = slot_path("try-accept");
    let ep = Endpoint::parse(&mbox_addr(&path))
        .unwrap()
        .with_wait(WaitKind::Yield);
    let mut server = Server::bind(&ep).unwrap();
    let err = server.try_accept().unwrap_err();
    assert!(matches!(err, Error::Busy), "{err}");
    let _ = fs::remove_file(path);
}

#[test]
fn close_rejects_later_ask() {
    let path = slot_path("close");
    let ep = Endpoint::parse(&mbox_addr(&path))
        .unwrap()
        .with_wait(WaitKind::Yield);
    let server = Server::bind(&ep).unwrap();
    let mut client = Client::connect(&ep).unwrap();
    client.close().unwrap();
    let err = client.ask(b"ab").unwrap_err();
    assert!(matches!(err, Error::Closed), "{err}");
    let err = client.close().unwrap_err();
    assert!(matches!(err, Error::Closed), "{err}");
    drop(server);
    let _ = fs::remove_file(path);
}
