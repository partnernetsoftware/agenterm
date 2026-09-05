use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use agenterm_platform::{
    entropy::secure_random_array,
    file_identity::file_identity,
    filesystem::{host_directories, user_home_directory},
    filesystem_open::{ExistingEntryType, open_existing_path},
    filesystem_publish::{publish_directory, write_file_atomic},
};
use serde::Serialize;

#[cfg(any(test, not(windows)))]
use super::ACU_NATIVE_HOST_NAME;
use super::{ExtensionMaterializationPlan, extension_assets, native_host_manifest};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserBridgeInstallPaths {
    pub extension: PathBuf,
    pub native_manifests: Vec<PathBuf>,
}

impl BrowserBridgeInstallPaths {
    pub fn for_current_user() -> Result<Self, BrowserBridgeInstallError> {
        let directories =
            host_directories().map_err(|_| error("browser_bridge_home_unavailable"))?;
        let extension = directories
            .local_data
            .join("agenterm")
            .join("cu")
            .join("browser-bridge")
            .join("extension");
        let home = user_home_directory().map_err(|_| error("browser_bridge_home_unavailable"))?;
        Ok(Self {
            extension,
            native_manifests: manifest_destinations(&home, &directories.config)?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserBridgeInstall {
    pub extension: PathBuf,
    pub native_manifests: Vec<PathBuf>,
    pub replaced_extension: bool,
    /// The complete reviewed MV3 bundle was published at `extension`.
    pub bundle_materialized: bool,
    /// Every path in `native_manifests` was atomically written.
    pub native_manifests_written: bool,
    /// Setup cannot activate an unpacked extension inside Chromium.
    pub extension_loaded: bool,
    pub manual_activation_required: bool,
}

pub fn install_for_current_user(
    executable: &Path,
) -> Result<BrowserBridgeInstall, BrowserBridgeInstallError> {
    validate_current_executable(executable)?;
    install_at(executable, BrowserBridgeInstallPaths::for_current_user()?)
}

fn validate_current_executable(executable: &Path) -> Result<(), BrowserBridgeInstallError> {
    if !executable.is_absolute() {
        return Err(error("browser_bridge_executable_invalid"));
    }
    let candidate = open_existing_path(executable, ExistingEntryType::File)
        .map_err(|_| error("browser_bridge_executable_invalid"))?;
    let current_path = std::env::current_exe()
        .map_err(|_| error("browser_bridge_current_executable_unavailable"))?;
    let current = open_existing_path(&current_path, ExistingEntryType::File)
        .map_err(|_| error("browser_bridge_current_executable_unavailable"))?;
    let candidate_identity = file_identity(&candidate)
        .map_err(|_| error("browser_bridge_executable_identity_unavailable"))?;
    let current_identity = file_identity(&current)
        .map_err(|_| error("browser_bridge_current_executable_unavailable"))?;
    if !candidate_identity.same_object(current_identity) {
        return Err(error("browser_bridge_executable_identity_mismatch"));
    }
    Ok(())
}

fn install_at(
    executable: &Path,
    paths: BrowserBridgeInstallPaths,
) -> Result<BrowserBridgeInstall, BrowserBridgeInstallError> {
    let suffix = secure_random_array::<32>()
        .map_err(|_| error("browser_bridge_entropy_unavailable"))?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let plan = ExtensionMaterializationPlan::new(&paths.extension, &suffix)
        .map_err(|_| error("browser_bridge_install_plan_invalid"))?;
    let parent = plan
        .destination
        .parent()
        .ok_or_else(|| error("browser_bridge_install_plan_invalid"))?;
    fs::create_dir_all(parent).map_err(|_| error("browser_bridge_install_prepare_failed"))?;
    fs::create_dir(&plan.staging).map_err(|_| error("browser_bridge_install_prepare_failed"))?;
    let prepared = prepare_extension(&plan.staging);
    if let Err(error) = prepared {
        let _ = fs::remove_dir_all(&plan.staging);
        return Err(error);
    }
    let outcome = publish_directory(&plan.staging, &plan.destination)
        .map_err(|_| error("browser_bridge_extension_publish_failed"))?;
    let manifest = native_host_manifest(executable)
        .map_err(|_| error("browser_bridge_native_manifest_invalid"))?;
    let mut published = Vec::new();
    for destination in &paths.native_manifests {
        let parent = destination
            .parent()
            .ok_or_else(|| error("browser_bridge_install_plan_invalid"))?;
        fs::create_dir_all(parent).map_err(|_| error("browser_bridge_install_prepare_failed"))?;
        write_file_atomic(destination, |file| file.write_all(&manifest))
            .map_err(|_| error("browser_bridge_native_manifest_publish_failed"))?;
        published.push(destination.clone());
    }
    Ok(BrowserBridgeInstall {
        extension: paths.extension,
        native_manifests: published,
        replaced_extension: outcome.replaced_existing(),
        bundle_materialized: true,
        native_manifests_written: true,
        extension_loaded: false,
        manual_activation_required: true,
    })
}

fn prepare_extension(staging: &Path) -> Result<(), BrowserBridgeInstallError> {
    for asset in extension_assets() {
        let destination = staging.join(asset.relative_path);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|_| error("browser_bridge_install_prepare_failed"))?;
        file.write_all(asset.bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| error("browser_bridge_install_prepare_failed"))?;
        if fs::read(&destination).ok().as_deref() != Some(asset.bytes) {
            return Err(error("browser_bridge_install_verify_failed"));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn manifest_destinations(
    home: &Path,
    _config: &Path,
) -> Result<Vec<PathBuf>, BrowserBridgeInstallError> {
    let support = home.join("Library").join("Application Support");
    existing_manifest_destinations([
        support.join("Google/Chrome"),
        support.join("Chromium"),
        support.join("BraveSoftware/Brave-Browser"),
        support.join("Microsoft Edge"),
    ])
}

#[cfg(target_os = "linux")]
fn manifest_destinations(
    _home: &Path,
    config: &Path,
) -> Result<Vec<PathBuf>, BrowserBridgeInstallError> {
    existing_manifest_destinations([
        config.join("google-chrome"),
        config.join("chromium"),
        config.join("BraveSoftware/Brave-Browser"),
        config.join("microsoft-edge"),
    ])
}

#[cfg(not(windows))]
fn existing_manifest_destinations<const N: usize>(
    roots: [PathBuf; N],
) -> Result<Vec<PathBuf>, BrowserBridgeInstallError> {
    let destinations: Vec<_> = roots
        .into_iter()
        .filter(|root| {
            fs::symlink_metadata(root)
                .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        })
        .map(|root| {
            root.join("NativeMessagingHosts")
                .join(format!("{ACU_NATIVE_HOST_NAME}.json"))
        })
        .collect();
    if destinations.is_empty() {
        Err(error("browser_bridge_no_supported_browser_profile"))
    } else {
        Ok(destinations)
    }
}

#[cfg(windows)]
fn manifest_destinations(
    _home: &Path,
    _config: &Path,
) -> Result<Vec<PathBuf>, BrowserBridgeInstallError> {
    // Chromium requires HKCU registration on Windows. That native registry
    // mechanism does not yet exist in agenterm-platform, so this installer
    // refuses instead of invoking reg.exe or leaking raw Win32 into product code.
    Err(error("browser_bridge_windows_registry_unavailable"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserBridgeInstallError {
    pub code: &'static str,
}
fn error(code: &'static str) -> BrowserBridgeInstallError {
    BrowserBridgeInstallError { code }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_are_current_user_and_fixed_acu_identity() {
        #[cfg(not(windows))]
        {
            let root = std::env::temp_dir()
                .join(format!("agenterm-cu-browser-roots-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let existing = root.join("existing");
            fs::create_dir_all(&existing).unwrap();
            let paths = existing_manifest_destinations([
                root.join("absent"),
                existing,
                root.join("also-absent"),
            ])
            .unwrap();
            assert_eq!(paths.len(), 1);
            let expected_name = format!("{ACU_NATIVE_HOST_NAME}.json");
            assert!(paths.iter().all(|path| path.is_absolute()
                && path.file_name().and_then(|name| name.to_str())
                    == Some(expected_name.as_str())));
            assert!(
                paths
                    .iter()
                    .all(|path| !path.to_string_lossy().to_ascii_lowercase().contains("mcu"))
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn no_existing_browser_root_is_typed_and_creates_nothing() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-browser-no-roots-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            existing_manifest_destinations([root.join("one"), root.join("two")])
                .unwrap_err()
                .code,
            "browser_bridge_no_supported_browser_profile"
        );
        assert!(!root.exists());
    }

    #[test]
    fn prepared_bundle_is_byte_exact() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-browser-installer-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        prepare_extension(&root).unwrap();
        for asset in extension_assets() {
            assert_eq!(
                fs::read(root.join(asset.relative_path)).unwrap(),
                asset.bytes
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn install_atomically_replaces_extension_and_writes_same_binary_manifests() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-browser-install-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let executable = root.join(if cfg!(windows) {
            "agenterm-cu.exe"
        } else {
            "agenterm-cu"
        });
        fs::write(&executable, b"fixture").unwrap();
        let paths = BrowserBridgeInstallPaths {
            extension: root.join("extension"),
            native_manifests: vec![root.join("native-host.json")],
        };
        let first = install_at(&executable, paths.clone()).unwrap();
        assert!(!first.replaced_extension);
        assert!(first.bundle_materialized);
        assert!(first.native_manifests_written);
        assert!(!first.extension_loaded);
        assert!(first.manual_activation_required);
        fs::write(paths.extension.join("stale"), b"must disappear").unwrap();
        let second = install_at(&executable, paths.clone()).unwrap();
        assert!(second.replaced_extension);
        assert!(!paths.extension.join("stale").exists());
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&paths.native_manifests[0]).unwrap()).unwrap();
        assert_eq!(manifest["name"], ACU_NATIVE_HOST_NAME);
        assert_eq!(manifest["path"], executable.to_str().unwrap());
        for asset in extension_assets() {
            assert_eq!(
                fs::read(paths.extension.join(asset.relative_path)).unwrap(),
                asset.bytes
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_installer_accepts_only_the_exact_running_regular_executable() {
        let current = std::env::current_exe().unwrap();
        validate_current_executable(&current).unwrap();
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-browser-executable-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        assert_eq!(
            validate_current_executable(&root).unwrap_err().code,
            "browser_bridge_executable_invalid"
        );
        let replacement = root.join("replacement");
        fs::write(&replacement, b"not-the-running-binary").unwrap();
        assert_eq!(
            validate_current_executable(&replacement).unwrap_err().code,
            "browser_bridge_executable_identity_mismatch"
        );
        assert_eq!(
            validate_current_executable(&root.join("missing"))
                .unwrap_err()
                .code,
            "browser_bridge_executable_invalid"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.join("link");
            symlink(&current, &link).unwrap();
            assert_eq!(
                validate_current_executable(&link).unwrap_err().code,
                "browser_bridge_executable_invalid"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
