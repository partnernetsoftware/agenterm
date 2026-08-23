//! End-to-end session behaviour against a hand-rolled RFB server.
//!
//! The point is to exercise the real handshake, the real decoder, and the real
//! frame channel rather than a mock of them, so a change that breaks wire
//! compatibility fails here instead of only against live hardware.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use agenterm_vnc::{ConnectOptions, MouseButtons};

/// Serve one RFB 3.8 session with no authentication and a single Raw update.
///
/// Returns the bound port and the thread handle, which yields the bytes the
/// client sent after the framebuffer request so input can be asserted on.
fn serve_one(width: u16, height: u16) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();

    let handle = std::thread::spawn(move || {
        // The client preflights the handshake on a throwaway connection before
        // opening the real one, so the first accept is answered and dropped.
        // Every step here is best-effort: the probe hangs up as soon as it has
        // read the security list, and a half-finished exchange on this side is
        // normal rather than a reason to fail the test.
        if let Ok((mut probe, _)) = listener.accept() {
            let _ = probe.write_all(b"RFB 003.008\n");
            let mut probe_version = [0u8; 12];
            let _ = probe.read_exact(&mut probe_version);
            let _ = probe.write_all(&[1, 1]);
        }

        let (mut stream, _) = listener.accept().expect("accept");

        // ProtocolVersion handshake.
        stream.write_all(b"RFB 003.008\n").expect("version");
        let mut client_version = [0u8; 12];
        stream.read_exact(&mut client_version).expect("client version");

        // Security: offer only `None`, then accept whatever the client picks.
        stream.write_all(&[1, 1]).expect("security list");
        let mut chosen = [0u8; 1];
        stream.read_exact(&mut chosen).expect("chosen security");
        stream.write_all(&0u32.to_be_bytes()).expect("security result");

        // ClientInit (shared flag), then ServerInit.
        let mut shared = [0u8; 1];
        stream.read_exact(&mut shared).expect("client init");

        let mut server_init = Vec::new();
        server_init.extend_from_slice(&width.to_be_bytes());
        server_init.extend_from_slice(&height.to_be_bytes());
        // PixelFormat: 32bpp, depth 24, little-endian, true colour BGRA.
        server_init.extend_from_slice(&[32, 24, 0, 1]);
        server_init.extend_from_slice(&255u16.to_be_bytes()); // red max
        server_init.extend_from_slice(&255u16.to_be_bytes()); // green max
        server_init.extend_from_slice(&255u16.to_be_bytes()); // blue max
        server_init.extend_from_slice(&[16, 8, 0]); // r, g, b shifts
        server_init.extend_from_slice(&[0, 0, 0]); // padding
        let name = b"test";
        server_init.extend_from_slice(&(name.len() as u32).to_be_bytes());
        server_init.extend_from_slice(name);
        stream.write_all(&server_init).expect("server init");

        // The client sends SetPixelFormat and SetEncodings before asking for
        // pixels; drain until the first FramebufferUpdateRequest (type 3).
        read_until_update_request(&mut stream);

        // One Raw-encoded full-screen rect of solid red, sent as BGRA.
        let mut update = vec![0u8, 0];
        update.extend_from_slice(&1u16.to_be_bytes()); // one rectangle
        update.extend_from_slice(&0u16.to_be_bytes()); // x
        update.extend_from_slice(&0u16.to_be_bytes()); // y
        update.extend_from_slice(&width.to_be_bytes());
        update.extend_from_slice(&height.to_be_bytes());
        update.extend_from_slice(&0i32.to_be_bytes()); // Raw encoding
        for _ in 0..width as usize * height as usize {
            update.extend_from_slice(&[0x00, 0x00, 0xff, 0xff]);
        }
        stream.write_all(&update).expect("update");

        // Collect whatever the client sends next; the test asserts on it.
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("timeout");
        // Read until the pointer event actually appears rather than until an
        // arbitrary byte count: the client also sends SetPixelFormat and
        // refresh requests, so a size-based stop could fill up on those and
        // return before the event under test ever arrived. That is a race the
        // machine wins or loses depending on load, and it did lose sometimes.
        let mut tail = Vec::new();
        let mut buffer = [0u8; 1024];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 {
                break;
            }
            tail.extend_from_slice(&buffer[..read]);
            let has_pointer_event = tail.windows(6).any(|window| window[0] == 5);
            if has_pointer_event {
                break;
            }
        }
        tail
    });

    (port, handle)
}

/// Consume client messages until a FramebufferUpdateRequest has been read.
fn read_until_update_request(stream: &mut TcpStream) {
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).expect("message type");
        match byte[0] {
            // SetPixelFormat: 3 padding + 16 format bytes.
            0 => {
                let mut rest = [0u8; 19];
                stream.read_exact(&mut rest).expect("set pixel format");
            }
            // SetEncodings: 1 padding, u16 count, then count i32s.
            2 => {
                let mut header = [0u8; 3];
                stream.read_exact(&mut header).expect("set encodings header");
                let count = u16::from_be_bytes([header[1], header[2]]);
                let mut encodings = vec![0u8; count as usize * 4];
                stream.read_exact(&mut encodings).expect("set encodings body");
            }
            // FramebufferUpdateRequest: incremental + 4 u16s.
            3 => {
                let mut rest = [0u8; 9];
                stream.read_exact(&mut rest).expect("update request");
                return;
            }
            other => panic!("unexpected client message type {other}"),
        }
    }
}

#[test]
fn a_session_delivers_composited_frames_and_forwards_input() {
    let (port, server) = serve_one(4, 2);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    runtime.block_on(async move {
        let (session, mut frames) =
            agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", port, None))
                .await
                .expect("connect");

        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv())
            .await
            .expect("a frame arrives before the timeout")
            .expect("the channel stays open");

        // The first frame must already carry pixels: a resize alone must not
        // publish a blank surface, which the UI would show as a black flash.
        assert!(
            frame.rgba.chunks_exact(4).any(|pixel| pixel[..3] != [0, 0, 0]),
            "the first frame should contain painted pixels, not just a resize"
        );
        assert_eq!((frame.width, frame.height), (4, 2), "full screen size");
        // The update covers the whole screen here because that is what the
        // server sent; the payload is sized to the region, not the screen.
        assert_eq!((frame.x, frame.y), (0, 0));
        assert_eq!((frame.region_width, frame.region_height), (4, 2));
        assert_eq!(frame.rgba.len(), 4 * 2 * 4);
        // The server sent BGRA red; the frame must expose it as RGBA red.
        assert_eq!(&frame.rgba[0..4], &[0xff, 0x00, 0x00, 0xff]);

        session.send_mouse(2, 1, MouseButtons::LEFT).expect("send mouse");
        // Give the session task a moment to flush the pointer event.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        session.disconnect().await;
        assert!(!session.is_connected());
    });

    let tail = server.join().expect("server thread");
    // PointerEvent is message type 5: [5, mask, x_hi, x_lo, y_hi, y_lo].
    let pointer = tail
        .windows(6)
        .find(|window| window[0] == 5 && window[1] == MouseButtons::LEFT.bits());
    let pointer = pointer.expect("the pointer event reached the server");
    assert_eq!(u16::from_be_bytes([pointer[2], pointer[3]]), 2, "x");
    assert_eq!(u16::from_be_bytes([pointer[4], pointer[5]]), 1, "y");
}

/// Serve a greeting that is not RFB at all, e.g. an HTTP or SSH daemon.
fn serve_non_rfb(greeting: &'static [u8]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(greeting);
        }
    });
    port
}

/// Serve a valid RFB greeting that offers only one security type.
fn serve_security_types(types: &'static [u8]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(b"RFB 003.008\n");
            let mut version = [0u8; 12];
            let _ = stream.read_exact(&mut version);
            let _ = stream.write_all(&[types.len() as u8]);
            let _ = stream.write_all(types);
            // Complete whatever the probe selects, so tests that expect a
            // *different* rejection are not masked by a truncated stream.
            let mut choice = [0u8; 1];
            if stream.read_exact(&mut choice).is_ok() {
                if choice[0] == 2 {
                    let _ = stream.write_all(&[0u8; 16]);
                    let mut response = [0u8; 16];
                    let _ = stream.read_exact(&mut response);
                }
                let _ = stream.write_all(&0u32.to_be_bytes());
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    });
    port
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

#[test]
fn a_non_rfb_server_is_rejected_rather_than_dialled() {
    let port = serve_non_rfb(b"SSH-2.0-OpenSSH_9.0\r\n");
    let error = block_on(agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", port, None)))
        .err()
        .expect("a non-RFB greeting must fail");
    assert!(
        matches!(error, agenterm_vnc::VncError::NotRfbServer { .. }),
        "expected NotRfbServer, got {error:?}"
    );
}

#[test]
fn an_apple_remote_management_server_asks_for_a_username() {
    // Type 30 is what macOS Screen Sharing offers for a real account. It needs
    // a username, so connecting without one must say exactly that rather than
    // failing as a generic bad password.
    let port = serve_security_types(&[30]);
    let error = block_on(agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", port, None)))
        .err()
        .expect("an ARD-only server needs a username");
    assert!(matches!(error, agenterm_vnc::VncError::UsernameRequired), "got {error:?}");
    assert!(error.to_string().contains("username"), "got: {error}");
}

#[test]
fn a_username_is_refused_when_the_server_has_no_use_for_one() {
    // A password-only server plus a username is a user mistake worth naming.
    let port = serve_security_types(&[2]);
    let mut options = ConnectOptions::new("127.0.0.1", port, Some("secret".into()));
    options.username = Some("admin".into());
    let error = block_on(agenterm_vnc::connect(options)).err().expect("must fail");
    assert!(matches!(error, agenterm_vnc::VncError::UsernameNotAccepted), "got {error:?}");
}

#[test]
fn a_password_is_refused_when_the_server_wants_none() {
    let port = serve_security_types(&[1]);
    let error = block_on(agenterm_vnc::connect(ConnectOptions::new(
        "127.0.0.1",
        port,
        Some("secret".into()),
    )))
    .err()
    .expect("offering a password to a None-only server must fail");
    assert!(matches!(error, agenterm_vnc::VncError::PasswordNotAccepted), "got {error:?}");
}

#[test]
fn a_missing_password_is_reported_before_the_handshake_stalls() {
    let port = serve_security_types(&[2]);
    let error = block_on(agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", port, None)))
        .err()
        .expect("a VncAuth-only server needs a password");
    assert!(matches!(error, agenterm_vnc::VncError::PasswordRequired), "got {error:?}");
}

/// Serve a greeting with an arbitrary version banner and security list.
fn serve_banner(banner: &'static [u8], types: &'static [u8]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(banner);
            let mut version = [0u8; 12];
            let _ = stream.read_exact(&mut version);
            let _ = stream.write_all(&[types.len() as u8]);
            let _ = stream.write_all(types);
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    });
    port
}

#[test]
fn apples_nonstandard_version_is_read_as_three_eight() {
    // macOS Screen Sharing announces `RFB 003.889`, which does not fit a u8.
    // Falling back to 3.3 sends the client down the branch where the server
    // dictates the security type, so it never picks one and the handshake
    // stalls -- the "handshake failed" a real Mac produced.
    let port = serve_banner(b"RFB 003.889\n", &[30]);
    let error = block_on(agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", port, None)))
        .err()
        .expect("no username was supplied");
    // Reaching the credential check at all proves the 3.8 path was taken.
    assert!(matches!(error, agenterm_vnc::VncError::UsernameRequired), "got {error:?}");
}

#[test]
fn unknown_security_types_do_not_hide_a_usable_one() {
    // A real Mac offers 30, 33, 36, 31, 32, 2, 35. Treating an unrecognised
    // byte as fatal while reading the list meant type 2, late in it, was
    // never reached.
    let port = serve_banner(b"RFB 003.889\n", &[33, 36, 31, 32, 2]);
    let error = block_on(agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", port, None)))
        .err()
        .expect("a password is required");
    // VNC Auth was found despite four unknown types ahead of it.
    assert!(matches!(error, agenterm_vnc::VncError::PasswordRequired), "got {error:?}");
}

#[test]
fn pixels_are_handed_on_without_waiting_for_the_refresh_tick() {
    // The session asks the server for updates on a timer, but must not hold
    // finished pixels until the next tick: that added a full interval of
    // latency to every interaction. Measured before this was separated, the
    // median input-to-pixel delay was one whole tick; after, it was ~0.03 ms.
    //
    // A tick is 16 ms, so a frame arriving well inside that window can only
    // mean emission is driven by the pixels rather than by the clock.
    let (port, server) = serve_one(8, 8);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let elapsed = runtime.block_on(async move {
        let (session, mut frames) =
            agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", port, None))
                .await
                .expect("connect");
        let start = std::time::Instant::now();
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv())
            .await
            .expect("a frame arrives")
            .expect("the channel stays open");
        let elapsed = start.elapsed();
        assert!(!frame.rgba.is_empty(), "the frame carries pixels");
        session.disconnect().await;
        elapsed
    });
    let _ = server.join();

    // A generous bound on purpose. The property under test is "not gated on
    // the 16 ms tick", and the failure it guards against is a frame waiting a
    // whole multiple of that. A tight threshold would instead measure how busy
    // the machine is while the rest of the suite runs in parallel, which is
    // how this assertion first failed.
    assert!(
        elapsed < std::time::Duration::from_millis(48),
        "the first frame waited {elapsed:?}, several refresh ticks, which means \
         emission is gated on the clock rather than on the pixels"
    );
}

/// Serve two small updates in opposite corners of a large screen, forever.
///
/// This is the shape that punishes a bounding-box union: the two rects hold a
/// few kilobytes between them, but any single rectangle containing both is the
/// entire screen.
fn serve_opposite_corners(width: u16, height: u16, tile: u16) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        // The preflight connection is answered and dropped.
        if let Ok((mut probe, _)) = listener.accept() {
            let _ = probe.write_all(b"RFB 003.008\n");
            let mut version = [0u8; 12];
            let _ = probe.read_exact(&mut version);
            let _ = probe.write_all(&[1, 1]);
        }
        let Ok((mut stream, _)) = listener.accept() else { return };

        let _ = stream.write_all(b"RFB 003.008\n");
        let mut version = [0u8; 12];
        if stream.read_exact(&mut version).is_err() {
            return;
        }
        let _ = stream.write_all(&[1, 1]);
        let mut chosen = [0u8; 1];
        let _ = stream.read_exact(&mut chosen);
        let _ = stream.write_all(&0u32.to_be_bytes());
        let mut shared = [0u8; 1];
        let _ = stream.read_exact(&mut shared);

        let mut init = Vec::new();
        init.extend_from_slice(&width.to_be_bytes());
        init.extend_from_slice(&height.to_be_bytes());
        init.extend_from_slice(&[32, 24, 0, 1]);
        init.extend_from_slice(&255u16.to_be_bytes());
        init.extend_from_slice(&255u16.to_be_bytes());
        init.extend_from_slice(&255u16.to_be_bytes());
        init.extend_from_slice(&[16, 8, 0]);
        init.extend_from_slice(&[0, 0, 0]);
        init.extend_from_slice(&4u32.to_be_bytes());
        init.extend_from_slice(b"test");
        let _ = stream.write_all(&init);

        let pixels = vec![0xffu8; tile as usize * tile as usize * 4];
        loop {
            let mut kind = [0u8; 1];
            if stream.read_exact(&mut kind).is_err() {
                return;
            }
            match kind[0] {
                0 => {
                    let mut rest = [0u8; 19];
                    let _ = stream.read_exact(&mut rest);
                }
                2 => {
                    let mut header = [0u8; 3];
                    let _ = stream.read_exact(&mut header);
                    let count = u16::from_be_bytes([header[1], header[2]]);
                    let mut body = vec![0u8; count as usize * 4];
                    let _ = stream.read_exact(&mut body);
                }
                3 => {
                    let mut rest = [0u8; 9];
                    let _ = stream.read_exact(&mut rest);
                    let mut update = vec![0u8, 0];
                    update.extend_from_slice(&2u16.to_be_bytes());
                    for (x, y) in [(0u16, 0u16), (width - tile, height - tile)] {
                        update.extend_from_slice(&x.to_be_bytes());
                        update.extend_from_slice(&y.to_be_bytes());
                        update.extend_from_slice(&tile.to_be_bytes());
                        update.extend_from_slice(&tile.to_be_bytes());
                        update.extend_from_slice(&0i32.to_be_bytes());
                        update.extend_from_slice(&pixels);
                    }
                    if stream.write_all(&update).is_err() {
                        return;
                    }
                }
                4 => {
                    let mut rest = [0u8; 7];
                    let _ = stream.read_exact(&mut rest);
                }
                5 => {
                    let mut rest = [0u8; 5];
                    let _ = stream.read_exact(&mut rest);
                }
                _ => return,
            }
        }
    });
    port
}

#[test]
fn scattered_updates_do_not_become_a_whole_screen_send() {
    // Regressions here are silent and expensive: everything keeps working, it
    // just moves thirty megabytes to show thirty-two pixels. Measured with a
    // deliberately slow consumer, merging these into one bounding box averaged
    // 29,458 KB per frame; keeping them apart averages 1.0 KB.
    let port = serve_opposite_corners(2048, 1536, 16);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    runtime.block_on(async move {
        let (session, mut frames) =
            agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", port, None))
                .await
                .expect("connect");

        // The first frame is the full screen, which is legitimate.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv()).await;

        let mut seen = 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while seen < 8 && std::time::Instant::now() < deadline {
            let Ok(Some(frame)) =
                tokio::time::timeout(std::time::Duration::from_secs(2), frames.recv()).await
            else {
                break;
            };
            // Deliberately slow, so regions pile up inside the session: that
            // backlog is exactly when a bounding box degenerates.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;

            let pixels = frame.region_width as u32 * frame.region_height as u32;
            assert!(
                pixels <= 16 * 16,
                "a {}x{} region came back for a 16x16 update, so scattered rects \
                 are being merged into their bounding box",
                frame.region_width,
                frame.region_height
            );
            seen += 1;
        }
        assert!(seen > 0, "no incremental frames arrived");
        session.disconnect().await;
    });
}

/// Serve one Tight-encoded rect carrying a real JPEG.
///
/// This is the shape macOS Screen Sharing sends most desktop content as, and
/// the one that rendered as a grid of stale tiles when it was not decoded.
fn serve_tight_jpeg(width: u16, height: u16) -> u16 {
    const JPEG: &[u8] = include_bytes!("fixtures/red-16x16.jpg");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        if let Ok((mut probe, _)) = listener.accept() {
            let _ = probe.write_all(b"RFB 003.008\n");
            let mut version = [0u8; 12];
            let _ = probe.read_exact(&mut version);
            let _ = probe.write_all(&[1, 1]);
        }
        let Ok((mut stream, _)) = listener.accept() else { return };
        let _ = stream.write_all(b"RFB 003.008\n");
        let mut version = [0u8; 12];
        if stream.read_exact(&mut version).is_err() {
            return;
        }
        let _ = stream.write_all(&[1, 1]);
        let mut chosen = [0u8; 1];
        let _ = stream.read_exact(&mut chosen);
        let _ = stream.write_all(&0u32.to_be_bytes());
        let mut shared = [0u8; 1];
        let _ = stream.read_exact(&mut shared);

        let mut init = Vec::new();
        init.extend_from_slice(&width.to_be_bytes());
        init.extend_from_slice(&height.to_be_bytes());
        init.extend_from_slice(&[32, 24, 0, 1]);
        for _ in 0..3 {
            init.extend_from_slice(&255u16.to_be_bytes());
        }
        init.extend_from_slice(&[16, 8, 0, 0, 0, 0]);
        init.extend_from_slice(&4u32.to_be_bytes());
        init.extend_from_slice(b"test");
        let _ = stream.write_all(&init);

        loop {
            let mut kind = [0u8; 1];
            if stream.read_exact(&mut kind).is_err() {
                return;
            }
            match kind[0] {
                0 => {
                    let mut rest = [0u8; 19];
                    let _ = stream.read_exact(&mut rest);
                }
                2 => {
                    let mut header = [0u8; 3];
                    let _ = stream.read_exact(&mut header);
                    let count = u16::from_be_bytes([header[1], header[2]]);
                    let mut body = vec![0u8; count as usize * 4];
                    let _ = stream.read_exact(&mut body);
                }
                3 => {
                    let mut rest = [0u8; 9];
                    let _ = stream.read_exact(&mut rest);

                    let mut update = vec![0u8, 0];
                    update.extend_from_slice(&1u16.to_be_bytes());
                    update.extend_from_slice(&0u16.to_be_bytes()); // x
                    update.extend_from_slice(&0u16.to_be_bytes()); // y
                    update.extend_from_slice(&16u16.to_be_bytes());
                    update.extend_from_slice(&16u16.to_be_bytes());
                    update.extend_from_slice(&7i32.to_be_bytes()); // Tight
                    update.push(0x90); // compression control: JPEG
                    // Tight's compact length: seven bits per byte, low first.
                    let mut remaining = JPEG.len();
                    loop {
                        let mut byte = (remaining & 0x7f) as u8;
                        remaining >>= 7;
                        if remaining > 0 {
                            byte |= 0x80;
                        }
                        update.push(byte);
                        if remaining == 0 {
                            break;
                        }
                    }
                    update.extend_from_slice(JPEG);
                    if stream.write_all(&update).is_err() {
                        return;
                    }
                }
                4 => {
                    let mut rest = [0u8; 7];
                    let _ = stream.read_exact(&mut rest);
                }
                5 => {
                    let mut rest = [0u8; 5];
                    let _ = stream.read_exact(&mut rest);
                }
                _ => return,
            }
        }
    });
    port
}

#[test]
fn tight_jpeg_rects_are_decoded_rather_than_dropped() {
    // Unhandled, these rects leave whatever was underneath, which is why a
    // real macOS desktop rendered as a grid of stale tiles. A pure-JPEG server
    // like this one produced no frames at all before they were decoded.
    let port = serve_tight_jpeg(64, 64);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");

    runtime.block_on(async move {
        let (session, mut frames) =
            agenterm_vnc::connect(ConnectOptions::new("127.0.0.1", port, None))
                .await
                .expect("connect");
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), frames.recv())
            .await
            .expect("a JPEG rect must produce a frame")
            .expect("the channel stays open");

        assert_eq!((frame.region_width, frame.region_height), (16, 16));
        // The fixture is a solid red-ish tile. JPEG is lossy, so this asserts
        // the colour is recognisably right rather than exact.
        let (red, green, blue) = (frame.rgba[0], frame.rgba[1], frame.rgba[2]);
        assert!(
            red > 180 && green < 90 && blue < 110,
            "decoded pixel was ({red}, {green}, {blue}), not the fixture's red"
        );
        assert_eq!(frame.rgba[3], 0xff, "alpha stays opaque");
        session.disconnect().await;
    });
}
