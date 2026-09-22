//! Shared workbench loader for a pre-composed Chassis-L1/L2/L3 image.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use agenterm_chassis::bytecode::{L2Source, Program, assemble};
use agenterm_chassis::l2_dispatch::{Dispatcher, HostCallback};

const MAX_NATIVE_LOADER_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct ImageIdentity {
    #[serde(default)]
    native_cell: Option<String>,
    l1_sha256: std::collections::BTreeMap<String, String>,
}

#[derive(Debug)]
pub(crate) struct LoadedChassisImage {
    pub(crate) root: PathBuf,
    pub(crate) native_loader: PathBuf,
    pub(crate) l3_name: String,
    active_tab_program: Program,
    host_abi: String,
    declared_capabilities: Vec<String>,
    identity: ChassisImageIdentity,
}

/// What a process says about the image it loaded: a content address over the
/// exact bytes it verified and runs, never a host path. Two images that
/// differ in any L2/L3 byte, or in this cell's loader, have different ids.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ChassisImageIdentity {
    pub(crate) image_id: String,
    pub(crate) cell: String,
    pub(crate) l1_sha256: String,
}

impl LoadedChassisImage {
    pub(crate) fn identity(&self) -> &ChassisImageIdentity {
        &self.identity
    }
}

/// Where the image this process loaded is installed, for handing it to a
/// child authority on its command line. Never printed.
pub(crate) fn loaded_image_root() -> Option<&'static Path> {
    LOADED_IMAGE.get().map(|image| image.root.as_path())
}

/// The identity of the image this process loaded, if it loaded one.
pub(crate) fn loaded_image_identity() -> Option<&'static ChassisImageIdentity> {
    LOADED_IMAGE.get().map(LoadedChassisImage::identity)
}

const IMAGE_ID_DOMAIN: &[u8] = b"agenterm-chassis-image-id/v1\0";

/// Every file that defines what this host runs from `root`, as
/// `(relative path, bytes)` sorted by path: the manifest, this cell's loader,
/// and every file under `l2/` and `l3/`. A symbolic link anywhere in that set
/// is refused, so the bytes read are the bytes that exist.
fn image_files(root: &Path, cell: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    fn read_one(root: &Path, relative: &str) -> Result<(String, Vec<u8>), String> {
        let path = root.join(relative);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "chassis image file {relative} is unreadable: {}",
                error.kind()
            )
        })?;
        if !metadata.file_type().is_file() {
            return Err(format!(
                "chassis image entry {relative} is not a regular file"
            ));
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            format!(
                "chassis image file {relative} is unreadable: {}",
                error.kind()
            )
        })?;
        Ok((relative.to_owned(), bytes))
    }
    fn walk(root: &Path, relative: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
        let dir = root.join(relative);
        let metadata = std::fs::symlink_metadata(&dir).map_err(|error| {
            format!(
                "chassis image directory {relative} is unreadable: {}",
                error.kind()
            )
        })?;
        if !metadata.file_type().is_dir() {
            return Err(format!("chassis image entry {relative} is not a directory"));
        }
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(|error| {
            format!(
                "chassis image directory {relative} is unreadable: {}",
                error.kind()
            )
        })? {
            let entry = entry.map_err(|error| {
                format!(
                    "chassis image directory {relative} is unreadable: {}",
                    error.kind()
                )
            })?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| format!("chassis image directory {relative} has a non-UTF-8 name"))?;
            names.push(name);
        }
        names.sort();
        for name in names {
            let child = format!("{relative}/{name}");
            let kind = std::fs::symlink_metadata(root.join(&child))
                .map_err(|error| {
                    format!(
                        "chassis image entry {child} is unreadable: {}",
                        error.kind()
                    )
                })?
                .file_type();
            if kind.is_dir() {
                walk(root, &child, out)?;
            } else {
                out.push(read_one(root, &child)?);
            }
        }
        Ok(())
    }
    let mut files = vec![
        read_one(root, "manifest.json")?,
        read_one(root, &format!("l1/{cell}/loader"))?,
    ];
    walk(root, "l2", &mut files)?;
    walk(root, "l3", &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Domain-separated, length-prefixed digest of `files`: no two different
/// `(path, bytes)` sets can encode to the same stream.
fn image_id_of(files: &[(String, Vec<u8>)]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(IMAGE_ID_DOMAIN);
    hasher.update((files.len() as u64).to_le_bytes());
    for (path, bytes) in files {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

static LOADED_IMAGE: OnceLock<LoadedChassisImage> = OnceLock::new();

pub(crate) fn load_selected_image(
    root: Option<&Path>,
) -> Result<Option<&'static LoadedChassisImage>, String> {
    let Some(root) = root else {
        return Ok(None);
    };
    if let Some(loaded) = LOADED_IMAGE.get() {
        if loaded.root == root {
            return Ok(Some(loaded));
        }
        return Err("a different chassis image is already loaded".to_owned());
    }
    let loaded = load_image(root)?;
    LOADED_IMAGE
        .set(loaded)
        .map_err(|_| "chassis image initialization raced".to_owned())?;
    Ok(LOADED_IMAGE.get())
}

/// Load and verify `root`. Every refusal names what failed inside the image
/// and never the host path it was installed at: a diagnostic travels to logs
/// and other processes, the location does not need to.
pub(crate) fn load_image(root: &Path) -> Result<LoadedChassisImage, String> {
    load_image_at(root, &|| {}).map_err(|message| redact_image_root(&message, root))
}

fn redact_image_root(message: &str, root: &Path) -> String {
    let mut redacted = message.to_owned();
    let mut spellings = vec![root.display().to_string()];
    if let Ok(canonical) = root.canonicalize() {
        spellings.push(canonical.display().to_string());
    }
    for spelling in spellings {
        if !spelling.is_empty() {
            redacted = redacted.replace(&spelling, "<image>");
        }
    }
    redacted
}

/// `between` runs after the bytes are first read and before they are checked
/// and parsed: production passes nothing, a test changes the image there.
fn load_image_at(root: &Path, between: &dyn Fn()) -> Result<LoadedChassisImage, String> {
    if !root.is_dir() {
        return Err(
            "chassis image is not an installed directory; extract a Candidate .tgz before launch"
                .to_owned(),
        );
    }
    let native_cell = agenterm_platform::chassis_loader::native_cell()
        .ok_or_else(|| "this OS/ISA has no Chassis-L1 loader cell".to_owned())?;
    // The id names the bytes read here. The same set is read again after
    // every check and parse below; if the directory changed in between, the
    // id would not describe what runs, so the load is refused.
    let before = image_id_of(&image_files(root, native_cell)?);
    between();
    agenterm_chassis::check_product_image(root)
        .map_err(|error| format!("chassis image check failed: {error}"))?;
    let native_loader = root.join("l1").join(native_cell).join("loader");
    if !native_loader.is_file() {
        return Err(format!(
            "chassis image lacks native loader cell {native_cell}"
        ));
    }
    validate_native_loader(root, native_cell, &native_loader)?;
    let app = agenterm_chassis::load_app(&root.join("l3/app.json"))
        .map_err(|error| format!("cannot load chassis L3 manifest: {error}"))?;
    let host_abi = std::fs::read_to_string(root.join("l2/host-abi.json"))
        .map_err(|error| format!("cannot read chassis L2 Host ABI: {error}"))?;
    Dispatcher::from_host_abi_json(&host_abi, &app.capabilities, ValidationOnlyHost)
        .map_err(|error| format!("cannot validate chassis L2 Host ABI: {error}"))?;
    let source: L2Source = serde_json::from_slice(
        &std::fs::read(root.join("l2/programs/active-tab.json"))
            .map_err(|error| format!("cannot read chassis L2 active-tab program: {error}"))?,
    )
    .map_err(|error| format!("cannot parse chassis L2 active-tab program: {error}"))?;
    let active_tab_program = assemble(&source, Some(&app.capabilities))
        .map_err(|error| format!("cannot assemble chassis L2 active-tab program: {error}"))?;
    let after = image_id_of(&image_files(root, native_cell)?);
    if after != before {
        return Err("chassis image changed while it was being verified and loaded".to_owned());
    }
    let l1_sha256 = sha256_hex(
        &std::fs::read(&native_loader)
            .map_err(|error| format!("cannot read native chassis loader: {}", error.kind()))?,
    );
    Ok(LoadedChassisImage {
        root: root.to_path_buf(),
        native_loader,
        l3_name: app.name,
        active_tab_program,
        host_abi,
        declared_capabilities: app.capabilities,
        identity: ChassisImageIdentity {
            image_id: after,
            cell: native_cell.to_owned(),
            l1_sha256,
        },
    })
}

/// Run the checked image's first-window L2 artifact against the live product host.
///
/// Callers invoke this only after their real IPC server and first PTY exist.
pub(crate) fn eval_active_tab<H: HostCallback>(
    image: &LoadedChassisImage,
    host: H,
) -> Result<(i64, H), String> {
    let mut dispatcher =
        Dispatcher::from_host_abi_json(&image.host_abi, &image.declared_capabilities, host)
            .map_err(|error| error.to_string())?;
    let value = agenterm_chassis::vm::run(
        &image.active_tab_program,
        &mut dispatcher,
        agenterm_chassis::vm::DEFAULT_MAX_STEPS,
    )
    .map_err(|error| error.to_string())?;
    Ok((value, dispatcher.into_host()))
}

struct ValidationOnlyHost;

impl HostCallback for ValidationOnlyHost {
    fn call(&mut self, _capability: &str, _parameters: &Value) -> Result<Value, String> {
        Err("validation-only host must not be called".to_owned())
    }
}

fn validate_native_loader(root: &Path, cell: &str, loader: &Path) -> Result<(), String> {
    let identity: ImageIdentity = serde_json::from_slice(
        &std::fs::read(root.join("manifest.json"))
            .map_err(|error| format!("cannot read chassis product manifest: {error}"))?,
    )
    .map_err(|error| format!("cannot parse chassis product identity: {error}"))?;
    let expected_sha = identity
        .l1_sha256
        .get(cell)
        .ok_or_else(|| format!("chassis manifest lacks SHA-256 for native loader cell {cell}"))?;
    if let Some(declared_cell) = identity.native_cell.as_deref()
        && declared_cell != cell
    {
        return Err(format!(
            "chassis manifest native cell {declared_cell} does not match this host {cell}"
        ));
    }
    if expected_sha.len() != 64
        || !expected_sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "chassis manifest has invalid SHA-256 for native loader cell {cell}"
        ));
    }

    let metadata = std::fs::symlink_metadata(loader)
        .map_err(|error| format!("cannot inspect native chassis loader: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("native chassis loader must not be a symbolic link".to_owned());
    }
    if metadata.len() == 0 || metadata.len() > MAX_NATIVE_LOADER_BYTES {
        return Err(format!(
            "native chassis loader size must be 1..{MAX_NATIVE_LOADER_BYTES} bytes"
        ));
    }

    let bytes = std::fs::read(loader)
        .map_err(|error| format!("cannot read native chassis loader: {error}"))?;
    agenterm_platform::chassis_loader::validate_executable(loader, &bytes)
        .map_err(|error| format!("native chassis loader for cell {cell}: {error}"))?;
    let actual_sha = sha256_hex(&bytes);
    if &actual_sha != expected_sha {
        return Err(format!(
            "native chassis loader SHA-256 mismatch for cell {cell}"
        ));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenterm_chassis::CELLS;
    use std::fs;

    fn write_image(root: &Path, l3_note: Option<&str>) {
        let mut hashes = serde_json::Map::new();
        for cell in CELLS {
            let dir = root.join("l1").join(cell);
            fs::create_dir_all(&dir).expect("cell");
            let bytes = executable_bytes(cell);
            let loader = dir.join("loader");
            fs::write(&loader, &bytes).expect("loader");
            make_executable(&loader);
            hashes.insert(
                cell.to_owned(),
                serde_json::Value::String(sha256_hex(&bytes)),
            );
        }
        fs::create_dir_all(root.join("l2/programs")).expect("l2");
        fs::write(
            root.join("l2/host-abi.json"),
            include_str!("../../crates/agenterm-chassis/l2/host-abi.json"),
        )
        .expect("abi");
        fs::write(
            root.join("l2/programs/active-tab.json"),
            include_str!("../../crates/agenterm-chassis/l2/programs/active-tab.json"),
        )
        .expect("program");
        fs::create_dir_all(root.join("l3")).expect("l3");
        let note = l3_note.unwrap_or("");
        fs::write(
            root.join("l3/app.json"),
            format!(
                r#"{{"schema":1,"name":"workbench","capabilities":["tabs.active"],"note":"{note}"}}"#
            ),
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
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&manifest).expect("manifest json"),
        )
        .expect("manifest");
    }

    fn executable_bytes(cell: &str) -> Vec<u8> {
        let mut bytes = if Some(cell) == agenterm_platform::chassis_loader::native_cell() {
            agenterm_platform::chassis_loader::native_executable_header().to_vec()
        } else {
            b"non-native-loader".to_vec()
        };
        bytes.extend_from_slice(format!("thin-loader-{cell}").as_bytes());
        bytes
    }

    fn make_executable(path: &Path) {
        agenterm_platform::chassis_loader::make_executable(path).expect("executable");
    }

    #[test]
    fn loads_native_cell_from_composed_image() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_image(tmp.path(), None);
        let image = load_image(tmp.path()).expect("load");
        assert!(image.native_loader.is_file());
        assert_eq!(image.l3_name, "workbench");
    }

    #[test]
    fn fails_closed_when_l3_names_native_library() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_image(tmp.path(), Some("libc.so.6"));
        let error = load_image(tmp.path()).expect_err("forbidden L3");
        assert!(error.contains("libc.so.6"), "{error}");
    }

    /// The refusal that names an L3 file used to carry the install path; the
    /// loader now reports it relative to the image.
    #[test]
    fn a_refusal_never_names_the_install_path() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_image(tmp.path(), Some("libc.so.6"));
        let error = load_image(tmp.path()).expect_err("forbidden L3");
        let root = tmp.path().display().to_string();
        let canonical = tmp
            .path()
            .canonicalize()
            .expect("canonical")
            .display()
            .to_string();
        assert!(
            !error.contains(&root) && !error.contains(&canonical),
            "{error}"
        );
        assert!(error.contains("<image>"), "{error}");
        let missing = load_image(&tmp.path().join("absent")).expect_err("no directory");
        assert!(!missing.contains(&root), "{missing}");
    }

    #[test]
    fn the_image_id_is_stable_and_names_this_cell() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_image(tmp.path(), None);
        let first = load_image(tmp.path()).expect("load");
        let second = load_image(tmp.path()).expect("load again");
        assert_eq!(first.identity(), second.identity());
        assert_eq!(first.identity().image_id.len(), 64);
        let cell = agenterm_platform::chassis_loader::native_cell().expect("cell");
        assert_eq!(first.identity().cell, cell);
        assert_eq!(
            first.identity().l1_sha256,
            sha256_hex(&executable_bytes(cell))
        );
    }

    /// Any byte of L2 or L3 is part of the id, including files the manifest
    /// does not describe by content.
    #[test]
    fn changing_one_l2_or_l3_byte_changes_the_id() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_image(tmp.path(), None);
        let base = load_image(tmp.path()).expect("load").identity().clone();

        let program = tmp.path().join("l2/programs/active-tab.json");
        let original = fs::read_to_string(&program).expect("program");
        fs::write(
            &program,
            original.replace("\"active-tab\"", "\"active-tab \""),
        )
        .expect("edit");
        let l2 = load_image(tmp.path())
            .expect("load l2 edit")
            .identity()
            .clone();
        assert_ne!(base.image_id, l2.image_id);
        assert_eq!(base.l1_sha256, l2.l1_sha256, "L1 bytes did not change");
        fs::write(&program, &original).expect("restore");

        write_image(tmp.path(), Some("changed"));
        let l3 = load_image(tmp.path())
            .expect("load l3 edit")
            .identity()
            .clone();
        assert_ne!(base.image_id, l3.image_id);

        write_image(tmp.path(), None);
        fs::write(tmp.path().join("l3/extra.txt"), b"x").expect("extra");
        let extra = load_image(tmp.path())
            .expect("load extra")
            .identity()
            .clone();
        assert_ne!(
            base.image_id, extra.image_id,
            "an added file is part of the id"
        );
    }

    /// Length prefixes keep a path/content boundary from sliding.
    #[test]
    fn the_encoding_has_no_concatenation_ambiguity() {
        let a = image_id_of(&[("ab".to_owned(), b"c".to_vec())]);
        let b = image_id_of(&[("a".to_owned(), b"bc".to_vec())]);
        let c = image_id_of(&[
            ("a".to_owned(), b"".to_vec()),
            ("b".to_owned(), b"c".to_vec()),
        ]);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    /// An image edited while it is being verified is refused rather than
    /// reported under an id that no longer describes what runs.
    #[test]
    fn an_image_changed_during_load_is_refused() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_image(tmp.path(), None);
        let program = tmp.path().join("l2/programs/active-tab.json");
        let error = load_image_at(tmp.path(), &|| {
            let text = fs::read_to_string(&program).expect("program");
            fs::write(&program, format!("{text} ")).expect("edit");
        })
        .expect_err("changed mid-load");
        assert!(
            error.contains("changed while it was being verified"),
            "{error}"
        );
        assert!(load_image(tmp.path()).is_ok(), "the settled image loads");
    }
}
