//! Regression: the crash reported on 2026-08-23 at 15:16.
//!
//! `vnc-rs` 0.5.3 transmutes the RFB SecurityResult word into a two-variant
//! `AuthResult` (`client/auth.rs:102`). A word that is neither 0 nor 1 is
//! undefined behaviour and aborts the process *without unwinding*, so no
//! `catch_unwind` in the Tauri or tokio layers can contain it. The preflight
//! has to reject such a server before `vnc-rs` touches the socket.

use std::io::{Read, Write};
use std::net::TcpListener;

use agenterm_vnc::ConnectOptions;

/// Offer VncAuth, then answer the challenge with a poisoned result word.
fn serve_poisoned_security_result() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(b"RFB 003.008\n");
            let mut version = [0u8; 12];
            if stream.read_exact(&mut version).is_err() {
                continue;
            }
            // One security type: VncAuth, the path that reaches the transmute.
            let _ = stream.write_all(&[1, 2]);
            let mut chosen = [0u8; 1];
            if stream.read_exact(&mut chosen).is_err() {
                continue;
            }
            let _ = stream.write_all(&[0u8; 16]);
            let mut response = [0u8; 16];
            let _ = stream.read_exact(&mut response);
            // The value from the crash report: 0x1000000.
            let _ = stream.write_all(&0x0100_0000u32.to_be_bytes());
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    });
    port
}

#[test]
fn a_poisoned_security_result_cannot_abort_the_process() {
    let port = serve_poisoned_security_result();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    // Reaching an assertion at all is the point: before the preflight, this
    // call aborted the whole test binary instead of returning.
    let result = runtime.block_on(agenterm_vnc::connect(ConnectOptions::new(
        "127.0.0.1",
        port,
        Some("secret".into()),
    )));
    assert!(result.is_err(), "a poisoned server must not connect");
}
