//! The server's chassis image contract, end to end on a headless authority.
//!
//! `agenterm server --chassis-image IMAGE` verifies the image itself and
//! reports it by content id through `protocol-info --running`. A CLI that
//! declares `--chassis-image` runs its command only against a server
//! reporting the same id; a CLI that declares none is an ordinary client.
//! No test here opens a window: the authority is started `--empty`.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use agenterm_chassis::CELLS;
use agenterm_platform::chassis_loader;
use sha2::{Digest, Sha256};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A composed image with a synthetic loader per cell (the native cell's
/// carries the host's executable header), the repository's L2 and a minimal
/// L3. `l2_note` appends bytes to an L2 file that no check reads, so two
/// images differ only there.
fn write_image(root: &Path, l2_note: &str) {
    let mut hashes = serde_json::Map::new();
    for cell in CELLS {
        let dir = root.join("l1").join(cell);
        fs::create_dir_all(&dir).expect("cell");
        let mut bytes = if Some(cell) == chassis_loader::native_cell() {
            chassis_loader::native_executable_header().to_vec()
        } else {
            b"non-native-loader".to_vec()
        };
        bytes.extend_from_slice(format!("thin-loader-{cell}").as_bytes());
        let loader = dir.join("loader");
        fs::write(&loader, &bytes).expect("loader");
        chassis_loader::make_executable(&loader).expect("executable");
        hashes.insert(
            cell.to_owned(),
            serde_json::Value::String(sha256_hex(&bytes)),
        );
    }
    let repo_l2 = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/agenterm-chassis/l2");
    fs::create_dir_all(root.join("l2/programs")).expect("l2");
    for file in ["host-abi.json", "programs/active-tab.json"] {
        fs::copy(repo_l2.join(file), root.join("l2").join(file)).expect("copy l2");
    }
    fs::write(root.join("l2/README.md"), format!("test image{l2_note}\n")).expect("readme");
    fs::create_dir_all(root.join("l3")).expect("l3");
    fs::write(
        root.join("l3/app.json"),
        r#"{"schema":1,"name":"workbench","capabilities":["tabs.active"]}"#,
    )
    .expect("app");
    let manifest = serde_json::json!({
        "schema": 1,
        "compile": false,
        "invokes_cargo": false,
        "cells": CELLS,
        "native_cell": null,
        "l1_sha256": hashes,
    });
    fs::write(root.join("manifest.json"), manifest.to_string()).expect("manifest");
}

struct Authority {
    root: tempfile::TempDir,
    address: String,
    child: Option<Child>,
}

impl Authority {
    fn start(image: Option<&Path>) -> Result<Self, String> {
        let root = tempfile::tempdir().expect("isolation root");
        fs::create_dir_all(root.path().join("instances")).expect("instances");
        let address = TcpListener::bind("127.0.0.1:0")
            .expect("reserve loopback")
            .local_addr()
            .expect("address")
            .to_string();
        let mut authority = Self {
            root,
            address,
            child: None,
        };
        let mut command = authority.command();
        command.args(["server", "--address", &authority.address, "--empty"]);
        if let Some(image) = image {
            command.arg("--chassis-image").arg(image);
        }
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start server");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = child.try_wait().expect("poll server") {
                let output = child.wait_with_output().expect("server output");
                return Err(format!(
                    "server exited {status}: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            if authority
                .cli(&["protocol-info", "--running"])
                .status
                .success()
            {
                break;
            }
            assert!(Instant::now() < deadline, "server never answered");
            std::thread::sleep(Duration::from_millis(50));
        }
        authority.child = Some(child);
        Ok(authority)
    }

    fn command(&self) -> Command {
        // The instance registry refuses a path through a symbolic link, and
        // macOS's temporary directory is one (`/var` -> `/private/var`).
        let root = self.root.path().canonicalize().expect("canonical root");
        let mut command = Command::new(env!("CARGO_BIN_EXE_agenterm"));
        command
            .env("AGENTERM_WORKSPACE_PATH", root.join("workspace.json"))
            .env("AGENTERM_SETTINGS_PATH", root.join("settings.json"))
            .env("AGENTERM_INSTANCE_DIR", root.join("instances"));
        command
    }

    fn cli(&self, arguments: &[&str]) -> Output {
        self.command()
            .args(["cli", "--address", &self.address])
            .args(arguments)
            .output()
            .expect("run cli")
    }

    fn declared(&self, image: &Path, arguments: &[&str]) -> Output {
        self.command()
            .args(["cli", "--address", &self.address, "--chassis-image"])
            .arg(image)
            .args(arguments)
            .output()
            .expect("run declared cli")
    }

    fn reported_image_id(&self) -> Option<String> {
        let output = self.cli(&["protocol-info", "--running"]);
        assert!(output.status.success(), "protocol-info --running");
        let info: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
        info["chassis_image"]["image_id"]
            .as_str()
            .map(str::to_owned)
    }
}

impl Drop for Authority {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn images() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("images");
    let a = dir.path().join("image-a");
    let b = dir.path().join("image-b");
    write_image(&a, "");
    write_image(&b, " B");
    (dir, a, b)
}

#[test]
fn a_server_reports_the_image_it_verified_by_content_id() {
    let (_dir, a, b) = images();
    let with_a = Authority::start(Some(&a)).expect("server with A");
    let id_a = with_a.reported_image_id().expect("an id");
    assert_eq!(id_a.len(), 64);
    let with_b = Authority::start(Some(&b)).expect("server with B");
    let id_b = with_b.reported_image_id().expect("an id");
    assert_ne!(id_a, id_b, "one L2 byte apart is a different image");
    let plain = Authority::start(None).expect("plain server");
    assert_eq!(plain.reported_image_id(), None);
}

#[test]
fn a_declared_cli_runs_only_against_the_same_image() {
    let (_dir, a, b) = images();
    let with_a = Authority::start(Some(&a)).expect("server with A");
    assert!(with_a.declared(&a, &["list-windows"]).status.success());

    let refused = with_a.declared(&b, &["list-windows"]);
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("chassis_image_mismatch"), "{stderr}");
    assert!(!stderr.contains(&a.display().to_string()), "{stderr}");
    assert!(!stderr.contains(&b.display().to_string()), "{stderr}");

    let plain = Authority::start(None).expect("plain server");
    let refused = plain.declared(&a, &["list-windows"]);
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("the server runs no image"), "{stderr}");
}

/// An undeclared CLI is an ordinary client: it is served by an image-backed
/// server and can read that server's id, but it is never a verified client.
#[test]
fn an_undeclared_cli_is_an_ordinary_client() {
    let (_dir, a, _b) = images();
    let with_a = Authority::start(Some(&a)).expect("server with A");
    assert!(with_a.cli(&["list-windows"]).status.success());
    assert!(with_a.reported_image_id().is_some());
}

#[test]
fn a_server_refuses_to_start_on_a_bad_image_without_naming_its_path() {
    let (_dir, a, _b) = images();
    fs::remove_file(a.join("l3/app.json")).expect("break image");
    let error = match Authority::start(Some(&a)) {
        Ok(_) => panic!("a broken image must refuse the start"),
        Err(error) => error,
    };
    assert!(error.contains("chassis_image_refused"), "{error}");
    assert!(!error.contains(&a.display().to_string()), "{error}");
}

/// A symbolic link inside L2 is refused: the id must name bytes that exist in
/// the image, not bytes reached through it.
#[cfg(unix)]
#[test]
fn a_server_refuses_an_image_with_a_symbolic_link() {
    let (_dir, a, _b) = images();
    std::os::unix::fs::symlink(a.join("manifest.json"), a.join("l2/programs/link.json"))
        .expect("symlink");
    let error = match Authority::start(Some(&a)) {
        Ok(_) => panic!("a symbolic link must refuse the start"),
        Err(error) => error,
    };
    assert!(error.contains("not a regular file"), "{error}");
}

/// Every GUI (re)attach -- first connect, reconnect after the server went
/// away, switching instance -- must pass the image check. The check lives in
/// `frontend_server::connect_verified_frontend_gui_client` and the wrapped
/// first connect; a direct `UiClientModel::connect` anywhere else would
/// accept a server running a different image, or none.
#[test]
fn no_gui_path_attaches_without_the_image_check() {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("src dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    let mut offenders = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).expect("read source");
        let relative = file.strip_prefix(&src).expect("under src");
        if relative == Path::new("frontend_server.rs") {
            // Only the live port's `attach` may call it; `verified_attach`
            // reaches it after a read-only image check.
            let mut current_fn = "";
            for line in text.lines() {
                let trimmed = line.trim_start();
                if let Some(rest) = trimmed
                    .strip_prefix("pub(crate) fn ")
                    .or_else(|| trimmed.strip_prefix("fn "))
                {
                    current_fn = rest.split(['(', '<']).next().unwrap_or("");
                }
                if line.contains("UiClientModel::connect(") && current_fn != "attach" {
                    offenders.push(format!("frontend_server.rs in {current_fn}"));
                }
            }
        } else if text.contains("UiClientModel::connect(") {
            offenders.push(relative.display().to_string());
        }
    }
    assert!(
        offenders.is_empty(),
        "GUI attach without the image check: {offenders:?}"
    );
}
