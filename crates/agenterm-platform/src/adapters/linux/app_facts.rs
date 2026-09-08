//! Linux application facts from bounded XDG desktop-entry discovery.
//!
//! This adapter reads desktop entries and procfs directly. It never invokes a
//! shell or a package manager, and it refuses incomplete selector scans.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, Metadata};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crate::contract::app_facts::{
    AppFacts, AppFactsError, AppFactsErrorKind, AppFactsOptions, Fact,
};

const MAX_DESKTOP_ENTRIES: usize = 4_096;
const MAX_DIRECTORY_DEPTH: usize = 8;
const MAX_ENTRY_BYTES: u64 = 128 * 1024;
const MAX_PROCESS_ENTRIES: usize = 32_768;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
        }
    }
}

#[derive(Debug)]
struct ParsedEntry {
    name: String,
    exec: Option<String>,
}

#[derive(Debug)]
struct Candidate {
    id: String,
    canonical_path: PathBuf,
    identity: FileIdentity,
    entry: ParsedEntry,
}

#[derive(Debug)]
struct ExecutableBinding {
    path: PathBuf,
    identity: FileIdentity,
}

fn error(kind: AppFactsErrorKind, code: &'static str, message: impl Into<String>) -> AppFactsError {
    AppFactsError::new(kind, code, message)
}

fn application_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let home_data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    if let Some(home_data) = home_data {
        directories.push(home_data.join("applications"));
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    directories.extend(system.split(':').filter_map(|entry| {
        let path = PathBuf::from(entry);
        path.is_absolute().then(|| path.join("applications"))
    }));
    let mut seen = BTreeSet::new();
    directories.retain(|path| seen.insert(path.clone()));
    directories
}

fn directory_entries(
    path: &Path,
    visited: &mut usize,
) -> Result<Vec<std::fs::DirEntry>, AppFactsError> {
    let reader = std::fs::read_dir(path).map_err(|cause| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_scan_failed",
            format!(
                "could not read XDG application directory {}: {cause}",
                path.display()
            ),
        )
    })?;
    let mut entries = Vec::new();
    for entry in reader {
        *visited = visited.saturating_add(1);
        if *visited > MAX_DESKTOP_ENTRIES {
            return Err(error(
                AppFactsErrorKind::ScanTruncated,
                "app_facts_scan_truncated",
                format!("XDG application scan exceeded {MAX_DESKTOP_ENTRIES} entries"),
            ));
        }
        entries.push(entry.map_err(|cause| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_scan_failed",
                format!(
                    "could not read an entry beneath {}: {cause}",
                    path.display()
                ),
            )
        })?);
    }
    entries.sort_by_key(std::fs::DirEntry::file_name);
    Ok(entries)
}

fn desktop_id(relative: &Path) -> Option<String> {
    let mut components = Vec::new();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return None;
        };
        components.push(component.to_str()?);
    }
    (!components.is_empty()).then(|| components.join("-"))
}

fn read_entry(path: &Path) -> Result<Option<(ParsedEntry, FileIdentity, PathBuf)>, AppFactsError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => {
            return Err(error(
                AppFactsErrorKind::Io,
                "app_facts_entry_read_failed",
                format!("could not open desktop entry {}: {cause}", path.display()),
            ));
        }
    };
    let before = file.metadata().map_err(|cause| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_entry_read_failed",
            format!(
                "could not inspect desktop entry {}: {cause}",
                path.display()
            ),
        )
    })?;
    if !before.is_file() || before.len() > MAX_ENTRY_BYTES {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|cause| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_entry_read_failed",
                format!("could not read desktop entry {}: {cause}", path.display()),
            )
        })?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Ok(None);
    }
    let after = file.metadata().map_err(|cause| {
        error(
            AppFactsErrorKind::IdentityDrift,
            "app_facts_identity_drift",
            format!(
                "desktop entry {} changed while it was read: {cause}",
                path.display()
            ),
        )
    })?;
    let identity = FileIdentity::from_metadata(&before);
    if identity != FileIdentity::from_metadata(&after) {
        return Err(error(
            AppFactsErrorKind::IdentityDrift,
            "app_facts_identity_drift",
            format!("desktop entry {} changed while it was read", path.display()),
        ));
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    let mut in_desktop_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut kind = None;
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Name" if name.is_none() => name = Some(value.to_owned()),
            "Exec" if exec.is_none() => exec = Some(value.to_owned()),
            "Type" if kind.is_none() => kind = Some(value.to_owned()),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            // `Version` is the desktop-entry file-format version. It is
            // deliberately not captured as the application's version.
            _ => {}
        }
    }
    if hidden || kind.as_deref() != Some("Application") {
        return Ok(None);
    }
    let Some(name) = name.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let canonical_path = std::fs::canonicalize(path).map_err(|cause| {
        error(
            AppFactsErrorKind::IdentityDrift,
            "app_facts_identity_drift",
            format!(
                "desktop entry {} disappeared while it was resolved: {cause}",
                path.display()
            ),
        )
    })?;
    let path_identity = std::fs::metadata(&canonical_path)
        .ok()
        .map(|metadata| FileIdentity::from_metadata(&metadata));
    if path_identity != Some(identity) {
        return Err(error(
            AppFactsErrorKind::IdentityDrift,
            "app_facts_identity_drift",
            format!(
                "desktop entry {} changed while it was resolved",
                path.display()
            ),
        ));
    }
    Ok(Some((ParsedEntry { name, exec }, identity, canonical_path)))
}

fn walk_directory(
    root: &Path,
    directory: &Path,
    depth: usize,
    visited: &mut usize,
    by_id: &mut BTreeMap<String, Option<Candidate>>,
) -> Result<(), AppFactsError> {
    if depth > MAX_DIRECTORY_DEPTH {
        return Err(error(
            AppFactsErrorKind::ScanTruncated,
            "app_facts_scan_truncated",
            format!("XDG application scan exceeded directory depth {MAX_DIRECTORY_DEPTH}"),
        ));
    }
    for entry in directory_entries(directory, visited)? {
        let file_type = entry.file_type().map_err(|cause| {
            error(
                AppFactsErrorKind::Io,
                "app_facts_scan_failed",
                format!("could not inspect {}: {cause}", entry.path().display()),
            )
        })?;
        if file_type.is_dir() && !file_type.is_symlink() {
            walk_directory(root, &entry.path(), depth + 1, visited, by_id)?;
            continue;
        }
        if entry.path().extension().and_then(|value| value.to_str()) != Some("desktop") {
            continue;
        }
        let entry_path = entry.path();
        let Ok(relative) = entry_path.strip_prefix(root) else {
            continue;
        };
        let Some(id) = desktop_id(relative) else {
            continue;
        };
        if by_id.contains_key(&id) {
            continue;
        }
        // The first occurrence blocks every lower-precedence occurrence even
        // when it is hidden or malformed.
        let candidate =
            read_entry(&entry.path())?.map(|(entry, identity, canonical_path)| Candidate {
                id: id.clone(),
                canonical_path,
                identity,
                entry,
            });
        by_id.insert(id, candidate);
    }
    Ok(())
}

fn scan(directories: &[PathBuf]) -> Result<Vec<Candidate>, AppFactsError> {
    let mut visited = 0;
    let mut readable_roots = 0;
    let mut by_id = BTreeMap::new();
    for root in directories {
        match std::fs::symlink_metadata(root) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                readable_roots += 1;
                walk_directory(root, root, 0, &mut visited, &mut by_id)?;
            }
            Ok(_) => continue,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(cause) => {
                return Err(error(
                    AppFactsErrorKind::Io,
                    "app_facts_scan_failed",
                    format!(
                        "could not inspect XDG application directory {}: {cause}",
                        root.display()
                    ),
                ));
            }
        }
    }
    if readable_roots == 0 {
        return Err(error(
            AppFactsErrorKind::Io,
            "app_facts_scan_failed",
            "no XDG application directory could be read",
        ));
    }
    Ok(by_id.into_values().flatten().collect())
}

fn resolve(selector: &str, candidates: Vec<Candidate>) -> Result<Candidate, AppFactsError> {
    let path_selector = Path::new(selector).is_absolute()
        || selector.contains('/')
        || selector.starts_with("./")
        || selector.starts_with("../");
    if path_selector {
        let selected_path = std::fs::canonicalize(selector).map_err(|_| {
            error(
                AppFactsErrorKind::NotFound,
                "app_facts_not_found",
                "application selector path did not resolve to an XDG desktop entry",
            )
        })?;
        return candidates
            .into_iter()
            .find(|candidate| candidate.canonical_path == selected_path)
            .ok_or_else(|| {
                error(
                    AppFactsErrorKind::NotFound,
                    "app_facts_not_found",
                    "application selector path did not resolve to an active XDG desktop entry",
                )
            });
    }
    if let Some(candidate) = candidates.iter().find(|candidate| candidate.id == selector) {
        let id = candidate.id.clone();
        return candidates
            .into_iter()
            .find(|candidate| candidate.id == id)
            .ok_or_else(|| unreachable!("candidate came from the same vector"));
    }
    let mut named = candidates
        .into_iter()
        .filter(|candidate| candidate.entry.name == selector);
    let Some(candidate) = named.next() else {
        return Err(error(
            AppFactsErrorKind::NotFound,
            "app_facts_not_found",
            "application selector matched no desktop entry id, normalized path, or exact Name",
        ));
    };
    if named.next().is_some() {
        return Err(error(
            AppFactsErrorKind::Ambiguous,
            "app_facts_ambiguous",
            "application Name selector matched more than one active desktop entry",
        ));
    }
    Ok(candidate)
}

fn has_exec_field_code(exec: &str) -> bool {
    let bytes = exec.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] == b'%' {
            if bytes[index + 1] == b'%' {
                index += 2;
                continue;
            }
            return true;
        }
        index += 1;
    }
    false
}

fn first_exec_token(exec: &str) -> Option<String> {
    let exec = exec.trim_start();
    if exec.is_empty() {
        return None;
    }
    if let Some(rest) = exec.strip_prefix('"') {
        let mut token = String::new();
        let mut escaped = false;
        for character in rest.chars() {
            if escaped {
                token.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                return Some(token);
            } else {
                token.push(character);
            }
        }
        return None;
    }
    Some(exec.split_whitespace().next()?.replace("\\ ", " "))
}

fn is_wrapper(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    matches!(
        name,
        "env"
            | "sh"
            | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "python"
            | "python3"
            | "perl"
            | "ruby"
            | "node"
            | "java"
            | "flatpak"
            | "snap"
            | "gtk-launch"
            | "systemd-run"
    )
}

fn bind_executable(exec: Option<&str>) -> Result<ExecutableBinding, &'static str> {
    let exec = exec.ok_or("desktop-entry-exec-absent")?;
    if has_exec_field_code(exec) {
        return Err("desktop-entry-exec-contains-field-code");
    }
    let token = first_exec_token(exec).ok_or("desktop-entry-exec-malformed")?;
    let path = PathBuf::from(token);
    if !path.is_absolute() {
        return Err("desktop-entry-exec-requires-path-search");
    }
    if is_wrapper(&path) {
        return Err("desktop-entry-exec-is-wrapper");
    }
    let path = std::fs::canonicalize(path).map_err(|_| "desktop-entry-executable-unavailable")?;
    let metadata = std::fs::metadata(&path).map_err(|_| "desktop-entry-executable-unavailable")?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err("desktop-entry-executable-unavailable");
    }
    Ok(ExecutableBinding {
        path,
        identity: FileIdentity::from_metadata(&metadata),
    })
}

fn running_fact(binding: &ExecutableBinding) -> Result<Fact<bool>, AppFactsError> {
    if std::fs::read_link("/proc/self/exe")
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .as_ref()
        == Some(&binding.path)
    {
        return Ok(Fact::present(true));
    }
    let processes = match std::fs::read_dir("/proc") {
        Ok(processes) => processes,
        Err(_) => return Ok(Fact::unavailable("linux-procfs-unavailable")),
    };
    let mut visited = 0;
    let mut unreadable = 0;
    for process in processes {
        let Ok(process) = process else {
            unreadable += 1;
            continue;
        };
        if !process
            .file_name()
            .as_encoded_bytes()
            .iter()
            .all(u8::is_ascii_digit)
        {
            continue;
        }
        visited += 1;
        if visited > MAX_PROCESS_ENTRIES {
            return Err(error(
                AppFactsErrorKind::ScanTruncated,
                "app_facts_scan_truncated",
                format!("Linux process scan exceeded {MAX_PROCESS_ENTRIES} entries"),
            ));
        }
        match std::fs::read_link(process.path().join("exe")) {
            Ok(path) if path == binding.path => return Ok(Fact::present(true)),
            Ok(_) => {}
            Err(_) => unreadable += 1,
        }
    }
    if unreadable == 0 {
        Ok(Fact::present(false))
    } else {
        Ok(Fact::unavailable("linux-process-inventory-incomplete"))
    }
}

fn revalidate(path: &Path, identity: FileIdentity, what: &str) -> Result<(), AppFactsError> {
    let current = std::fs::metadata(path).map(|metadata| FileIdentity::from_metadata(&metadata));
    if matches!(current, Ok(current) if current == identity) {
        Ok(())
    } else {
        Err(error(
            AppFactsErrorKind::IdentityDrift,
            "app_facts_identity_drift",
            format!("{what} identity changed during application-facts observation"),
        ))
    }
}

fn query_in(
    selector: &str,
    options: AppFactsOptions,
    directories: &[PathBuf],
) -> Result<AppFacts, AppFactsError> {
    let candidate = resolve(selector, scan(directories)?)?;
    let executable = bind_executable(candidate.entry.exec.as_deref());
    let (executable_fact, running) = match &executable {
        Ok(binding) => {
            let path = binding.path.to_str().ok_or_else(|| {
                error(
                    AppFactsErrorKind::Io,
                    "app_facts_non_utf8_path",
                    "application executable path is not valid UTF-8",
                )
            })?;
            (Fact::present(path.to_owned()), running_fact(binding)?)
        }
        Err(reason) => (Fact::unavailable(*reason), Fact::unavailable(*reason)),
    };
    revalidate(
        &candidate.canonical_path,
        candidate.identity,
        "desktop entry",
    )?;
    if let Ok(binding) = &executable {
        revalidate(&binding.path, binding.identity, "application executable")?;
    }
    let path = candidate.canonical_path.to_str().ok_or_else(|| {
        error(
            AppFactsErrorKind::Io,
            "app_facts_non_utf8_path",
            "desktop entry path is not valid UTF-8",
        )
    })?;
    let signing_requested = options.signing || options.verify;
    Ok(AppFacts {
        schema_version: 1,
        platform: "linux".to_owned(),
        selector: selector.to_owned(),
        desktop_entry_id: Fact::present(candidate.id),
        name: Fact::present(candidate.entry.name),
        bundle: Fact::not_applicable("linux-desktop-entry-is-not-an-application-bundle"),
        path: Fact::present(path.to_owned()),
        version: Fact::absent(
            "desktop-entry-does-not-declare-application-version;Version-is-file-format-version",
        ),
        executable: executable_fact,
        running,
        signature: if signing_requested {
            Fact::unsupported("linux-application-signature-provider-unavailable")
        } else {
            Fact::not_requested("signing-was-not-requested")
        },
        signature_verified: if options.verify {
            Fact::unsupported("linux-application-signature-verification-unavailable")
        } else {
            Fact::not_requested("signature-verification-was-not-requested")
        },
        entitlements: if options.entitlements {
            Fact::not_applicable("linux-applications-do-not-have-apple-entitlements")
        } else {
            Fact::not_requested("entitlements-were-not-requested")
        },
    })
}

pub(crate) fn query(selector: &str, options: AppFactsOptions) -> Result<AppFacts, AppFactsError> {
    query_in(selector, options, &application_directories())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    fn fixture_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agenterm-app-facts-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_entry(root: &Path, relative: &str, body: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn resolves_path_id_and_unique_name_with_xdg_precedence() {
        let high = fixture_directory();
        let low = fixture_directory();
        let executable = std::env::current_exe().unwrap();
        let high_path = write_entry(
            &high,
            "nested/example.desktop",
            &format!(
                "[Desktop Entry]\nType=Application\nName=Chosen App\nVersion=999\nExec={}\n",
                executable.display()
            ),
        );
        write_entry(
            &low,
            "nested/example.desktop",
            "[Desktop Entry]\nType=Application\nName=Shadowed App\nExec=/bin/false\n",
        );
        let roots = [high.clone(), low.clone()];
        for selector in [
            std::fs::canonicalize(&high_path)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
            "nested-example.desktop".to_owned(),
            "Chosen App".to_owned(),
        ] {
            let facts = query_in(&selector, AppFactsOptions::default(), &roots).unwrap();
            assert_eq!(
                facts.desktop_entry_id.value.as_deref(),
                Some("nested-example.desktop")
            );
            assert_eq!(facts.name.value.as_deref(), Some("Chosen App"));
            assert_eq!(
                facts.version.status,
                crate::contract::app_facts::FactStatus::Absent
            );
            assert!(!facts.version.reason.as_deref().unwrap().starts_with("999"));
        }
        std::fs::remove_dir_all(high).ok();
        std::fs::remove_dir_all(low).ok();
    }

    #[test]
    fn refuses_ambiguous_and_missing_names() {
        let root = fixture_directory();
        for id in ["one.desktop", "two.desktop"] {
            write_entry(
                &root,
                id,
                "[Desktop Entry]\nType=Application\nName=Same Name\nExec=/bin/false\n",
            );
        }
        let ambiguous = query_in(
            "Same Name",
            AppFactsOptions::default(),
            std::slice::from_ref(&root),
        )
        .unwrap_err();
        assert_eq!(ambiguous.kind(), AppFactsErrorKind::Ambiguous);
        let missing = query_in(
            "Missing",
            AppFactsOptions::default(),
            std::slice::from_ref(&root),
        )
        .unwrap_err();
        assert_eq!(missing.kind(), AppFactsErrorKind::NotFound);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn field_codes_path_lookup_and_wrappers_do_not_claim_an_executable() {
        for (exec, reason) in [
            ("/bin/echo %U", "desktop-entry-exec-contains-field-code"),
            ("echo value", "desktop-entry-exec-requires-path-search"),
            ("/usr/bin/env /bin/echo", "desktop-entry-exec-is-wrapper"),
        ] {
            assert_eq!(bind_executable(Some(exec)).unwrap_err(), reason);
        }
    }

    #[test]
    fn requested_linux_security_facts_are_explicit() {
        let root = fixture_directory();
        write_entry(
            &root,
            "facts.desktop",
            "[Desktop Entry]\nType=Application\nName=Facts\nExec=/bin/false\n",
        );
        let facts = query_in(
            "facts.desktop",
            AppFactsOptions {
                signing: true,
                verify: true,
                entitlements: true,
            },
            std::slice::from_ref(&root),
        )
        .unwrap();
        use crate::contract::app_facts::FactStatus;
        assert_eq!(facts.signature.status, FactStatus::Unsupported);
        assert_eq!(facts.signature_verified.status, FactStatus::Unsupported);
        assert_eq!(facts.entitlements.status, FactStatus::NotApplicable);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scan_ceiling_and_changed_identity_fail_typed() {
        let root = fixture_directory();
        let path = write_entry(
            &root,
            "one.desktop",
            "[Desktop Entry]\nType=Application\nName=One\nExec=/bin/false\n",
        );
        let mut visited = MAX_DESKTOP_ENTRIES;
        let truncated = directory_entries(&root, &mut visited).unwrap_err();
        assert_eq!(truncated.kind(), AppFactsErrorKind::ScanTruncated);

        let identity = FileIdentity::from_metadata(&std::fs::metadata(&path).unwrap());
        let replacement = root.join("replacement.desktop");
        std::fs::write(
            &replacement,
            "[Desktop Entry]\nType=Application\nName=Replacement\nExec=/bin/true\n",
        )
        .unwrap();
        std::fs::rename(replacement, &path).unwrap();
        let drift = revalidate(&path, identity, "desktop entry").unwrap_err();
        assert_eq!(drift.kind(), AppFactsErrorKind::IdentityDrift);
        std::fs::remove_dir_all(root).ok();
    }
}
