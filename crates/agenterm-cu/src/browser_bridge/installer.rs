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
    native_messaging::{ChromiumRegistryTarget, register_current_user_host},
};
use serde::Serialize;

use super::{
    ACU_NATIVE_HOST_NAME, ExtensionMaterializationPlan, extension_assets, native_host_manifest,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChromiumFamily {
    Chrome,
    Chromium,
    Brave,
    Edge,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum BrowserRegistrationPlan {
    ManifestFile { destination: PathBuf },
    CurrentUserRegistry { product_key: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserRegistrationTarget {
    pub browser: ChromiumFamily,
    /// The existing user-data root that authorized selecting this target.
    pub user_data_root: PathBuf,
    pub registration: BrowserRegistrationPlan,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserBridgeInstallPaths {
    pub extension: PathBuf,
    /// One stable ACU-owned manifest is published before registrations.
    pub native_manifest_file: PathBuf,
    pub targets: Vec<BrowserRegistrationTarget>,
}

impl BrowserBridgeInstallPaths {
    pub fn for_current_user() -> Result<Self, BrowserBridgeInstallError> {
        let directories =
            host_directories().map_err(|_| error("browser_bridge_home_unavailable"))?;
        let bridge_root = directories
            .local_data
            .join("agenterm")
            .join("cu")
            .join("browser-bridge");
        let home = user_home_directory().map_err(|_| error("browser_bridge_home_unavailable"))?;
        let targets = existing_targets(target_candidates(
            &home,
            &directories.config,
            &directories.local_data,
        ));
        if targets.is_empty() {
            return Err(error("browser_bridge_no_supported_browser_profile"));
        }
        Ok(Self {
            extension: bridge_root.join("extension"),
            native_manifest_file: bridge_root.join("native-host.json"),
            targets,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum BrowserRegistrationOutcome {
    ManifestWritten {
        replaced: bool,
    },
    RegistryWritten {
        before: Option<PathBuf>,
        after: PathBuf,
        replaced: bool,
    },
    Failed {
        code: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserRegistrationReceipt {
    pub browser: ChromiumFamily,
    pub user_data_root: PathBuf,
    pub registration: BrowserRegistrationPlan,
    pub outcome: BrowserRegistrationOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserBridgeInstall {
    pub extension: PathBuf,
    pub native_manifest_file: PathBuf,
    pub replaced_extension: bool,
    pub bundle_materialized: bool,
    pub native_manifest_file_written: bool,
    /// Independent per-browser results in deterministic Chrome/Chromium/Brave/Edge order.
    /// These registrations are deliberately not described as one atomic mutation.
    pub registrations: Vec<BrowserRegistrationReceipt>,
    /// Setup cannot activate an unpacked extension inside Chromium.
    pub extension_loaded: bool,
    pub manual_activation_required: bool,
}

impl BrowserBridgeInstall {
    fn empty(paths: &BrowserBridgeInstallPaths) -> Self {
        Self {
            extension: paths.extension.clone(),
            native_manifest_file: paths.native_manifest_file.clone(),
            replaced_extension: false,
            bundle_materialized: false,
            native_manifest_file_written: false,
            registrations: Vec::new(),
            extension_loaded: false,
            manual_activation_required: true,
        }
    }
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
    if paths.targets.is_empty() {
        return Err(error("browser_bridge_no_supported_browser_profile"));
    }
    let manifest = native_host_manifest(executable)
        .map_err(|_| error("browser_bridge_native_manifest_invalid"))?;
    let mut receipt = BrowserBridgeInstall::empty(&paths);
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
    if let Err(failure) = prepare_extension(&plan.staging) {
        let _ = fs::remove_dir_all(&plan.staging);
        return Err(failure.with_receipt(receipt));
    }
    let outcome = publish_directory(&plan.staging, &plan.destination).map_err(|_| {
        error("browser_bridge_extension_publish_failed").with_receipt(receipt.clone())
    })?;
    receipt.replaced_extension = outcome.replaced_existing();
    receipt.bundle_materialized = true;

    let manifest_parent = paths.native_manifest_file.parent().ok_or_else(|| {
        error("browser_bridge_install_plan_invalid").with_receipt(receipt.clone())
    })?;
    fs::create_dir_all(manifest_parent).map_err(|_| {
        error("browser_bridge_install_prepare_failed").with_receipt(receipt.clone())
    })?;
    match write_file_atomic(&paths.native_manifest_file, |file| {
        file.write_all(&manifest)
    }) {
        Ok(()) => receipt.native_manifest_file_written = true,
        Err(failure) => {
            receipt.native_manifest_file_written = failure.published();
            return Err(
                error("browser_bridge_native_manifest_publish_failed").with_receipt(receipt)
            );
        }
    }

    let mut any_failed = false;
    for target in paths.targets {
        let outcome = register_target(&target, &paths.native_manifest_file, &manifest);
        any_failed |= matches!(outcome, BrowserRegistrationOutcome::Failed { .. });
        receipt.registrations.push(BrowserRegistrationReceipt {
            browser: target.browser,
            user_data_root: target.user_data_root,
            registration: target.registration,
            outcome,
        });
    }
    if any_failed {
        Err(error("browser_bridge_registration_partial").with_receipt(receipt))
    } else {
        Ok(receipt)
    }
}

fn register_target(
    target: &BrowserRegistrationTarget,
    stable_manifest: &Path,
    manifest: &[u8],
) -> BrowserRegistrationOutcome {
    match &target.registration {
        BrowserRegistrationPlan::ManifestFile { destination } => {
            let Some(parent) = destination.parent() else {
                return BrowserRegistrationOutcome::Failed {
                    code: "browser_bridge_native_manifest_registration_failed".into(),
                };
            };
            if fs::create_dir_all(parent).is_err() {
                return BrowserRegistrationOutcome::Failed {
                    code: "browser_bridge_native_manifest_registration_failed".into(),
                };
            }
            if open_existing_path(parent, ExistingEntryType::Directory).is_err() {
                return BrowserRegistrationOutcome::Failed {
                    code: "browser_bridge_native_manifest_registration_destination_invalid".into(),
                };
            }
            let replaced = match existing_regular_file(destination) {
                Ok(replaced) => replaced,
                Err(()) => {
                    return BrowserRegistrationOutcome::Failed {
                        code: "browser_bridge_native_manifest_registration_destination_invalid"
                            .into(),
                    };
                }
            };
            match write_file_atomic(destination, |file| file.write_all(manifest)) {
                Ok(()) => BrowserRegistrationOutcome::ManifestWritten { replaced },
                Err(failure) => BrowserRegistrationOutcome::Failed {
                    code: if failure.published() {
                        "browser_bridge_native_manifest_registration_durability_uncertain"
                    } else {
                        "browser_bridge_native_manifest_registration_failed"
                    }
                    .into(),
                },
            }
        }
        BrowserRegistrationPlan::CurrentUserRegistry { product_key } => {
            let target = match ChromiumRegistryTarget::new(product_key.clone()) {
                Ok(target) => target,
                Err(failure) => {
                    return BrowserRegistrationOutcome::Failed {
                        code: failure.code().into(),
                    };
                }
            };
            match register_current_user_host(&target, ACU_NATIVE_HOST_NAME, stable_manifest) {
                Ok(platform) => BrowserRegistrationOutcome::RegistryWritten {
                    before: platform.before,
                    after: platform.after,
                    replaced: platform.replaced,
                },
                Err(failure) => BrowserRegistrationOutcome::Failed {
                    code: failure.code().into(),
                },
            }
        }
    }
}

fn existing_regular_file(path: &Path) -> Result<bool, ()> {
    // This preflight provides the receipt's `replaced` truth. Publication remains
    // authoritative: `write_file_atomic` rechecks the destination and refuses to
    // replace a link or non-regular entry if it changes after this inspection.
    match fs::symlink_metadata(path) {
        Ok(_) => open_existing_path(path, ExistingEntryType::File)
            .map(|_| true)
            .map_err(|_| ()),
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(()),
    }
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

#[derive(Clone, Copy)]
enum HostKind {
    #[cfg(any(test, target_os = "macos"))]
    Macos,
    #[cfg(any(test, target_os = "linux"))]
    Linux,
    #[cfg(any(test, windows))]
    Windows,
}

#[cfg(target_os = "macos")]
const HOST_KIND: HostKind = HostKind::Macos;
#[cfg(target_os = "linux")]
const HOST_KIND: HostKind = HostKind::Linux;
#[cfg(windows)]
const HOST_KIND: HostKind = HostKind::Windows;

fn target_candidates(
    home: &Path,
    config: &Path,
    local_data: &Path,
) -> Vec<BrowserRegistrationTarget> {
    candidates_for(HOST_KIND, home, config, local_data)
}

fn candidates_for(
    host: HostKind,
    _home: &Path,
    _config: &Path,
    _local_data: &Path,
) -> Vec<BrowserRegistrationTarget> {
    let families = match host {
        #[cfg(any(test, target_os = "macos"))]
        HostKind::Macos => {
            let support = _home.join("Library").join("Application Support");
            vec![
                (
                    ChromiumFamily::Chrome,
                    support.join("Google/Chrome"),
                    "Software\\Google\\Chrome",
                ),
                (
                    ChromiumFamily::Chromium,
                    support.join("Chromium"),
                    "Software\\Chromium",
                ),
                (
                    ChromiumFamily::Brave,
                    support.join("BraveSoftware/Brave-Browser"),
                    "Software\\BraveSoftware\\Brave-Browser",
                ),
                (
                    ChromiumFamily::Edge,
                    support.join("Microsoft Edge"),
                    "Software\\Microsoft\\Edge",
                ),
            ]
        }
        #[cfg(any(test, target_os = "linux"))]
        HostKind::Linux => vec![
            (
                ChromiumFamily::Chrome,
                _config.join("google-chrome"),
                "Software\\Google\\Chrome",
            ),
            (
                ChromiumFamily::Chromium,
                _config.join("chromium"),
                "Software\\Chromium",
            ),
            (
                ChromiumFamily::Brave,
                _config.join("BraveSoftware/Brave-Browser"),
                "Software\\BraveSoftware\\Brave-Browser",
            ),
            (
                ChromiumFamily::Edge,
                _config.join("microsoft-edge"),
                "Software\\Microsoft\\Edge",
            ),
        ],
        #[cfg(any(test, windows))]
        HostKind::Windows => vec![
            (
                ChromiumFamily::Chrome,
                _local_data.join("Google/Chrome/User Data"),
                "Software\\Google\\Chrome",
            ),
            (
                ChromiumFamily::Chromium,
                _local_data.join("Chromium/User Data"),
                "Software\\Chromium",
            ),
            (
                ChromiumFamily::Brave,
                _local_data.join("BraveSoftware/Brave-Browser/User Data"),
                "Software\\BraveSoftware\\Brave-Browser",
            ),
            (
                ChromiumFamily::Edge,
                _local_data.join("Microsoft/Edge/User Data"),
                "Software\\Microsoft\\Edge",
            ),
        ],
    };
    families
        .into_iter()
        .map(|(browser, root, product_key)| BrowserRegistrationTarget {
            browser,
            user_data_root: root.clone(),
            registration: if host_uses_registry(host) {
                BrowserRegistrationPlan::CurrentUserRegistry {
                    product_key: product_key.into(),
                }
            } else {
                BrowserRegistrationPlan::ManifestFile {
                    destination: root
                        .join("NativeMessagingHosts")
                        .join(format!("{ACU_NATIVE_HOST_NAME}.json")),
                }
            },
        })
        .collect()
}

fn host_uses_registry(host: HostKind) -> bool {
    match host {
        #[cfg(any(test, windows))]
        HostKind::Windows => true,
        #[cfg(any(test, target_os = "macos"))]
        HostKind::Macos => false,
        #[cfg(any(test, target_os = "linux"))]
        HostKind::Linux => false,
    }
}

fn existing_targets(candidates: Vec<BrowserRegistrationTarget>) -> Vec<BrowserRegistrationTarget> {
    candidates
        .into_iter()
        .filter(|candidate| {
            open_existing_path(&candidate.user_data_root, ExistingEntryType::Directory).is_ok()
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserBridgeInstallError {
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<BrowserBridgeInstall>,
}

impl BrowserBridgeInstallError {
    fn with_receipt(mut self, receipt: BrowserBridgeInstall) -> Self {
        self.receipt = Some(receipt);
        self
    }
}

fn error(code: &'static str) -> BrowserBridgeInstallError {
    BrowserBridgeInstallError {
        code,
        receipt: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-browser-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        fs::canonicalize(root).unwrap()
    }

    #[test]
    fn discovery_selects_only_existing_roots_and_maps_windows_hkcu_targets() {
        let root = fixture("roots");
        let local = root.join("local");
        let chrome = local.join("Google/Chrome/User Data");
        let edge = local.join("Microsoft/Edge/User Data");
        fs::create_dir_all(&chrome).unwrap();
        fs::create_dir_all(&edge).unwrap();
        let selected = existing_targets(candidates_for(
            HostKind::Windows,
            &root,
            &root.join("config"),
            &local,
        ));
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].browser, ChromiumFamily::Chrome);
        assert_eq!(selected[1].browser, ChromiumFamily::Edge);
        assert!(matches!(
            &selected[0].registration,
            BrowserRegistrationPlan::CurrentUserRegistry { product_key }
                if product_key == "Software\\Google\\Chrome"
        ));
        assert!(!local.join("Chromium/User Data").exists());
        assert!(!local.join("BraveSoftware/Brave-Browser/User Data").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_rejects_a_browser_root_beneath_an_intermediate_symlink() {
        use std::os::unix::fs::symlink;

        let root = fixture("root-intermediate-link");
        let real_config = root.join("real-config");
        fs::create_dir_all(real_config.join("google-chrome")).unwrap();
        let linked_config = root.join("linked-config");
        symlink(&real_config, &linked_config).unwrap();

        let candidates = candidates_for(HostKind::Linux, &root, &linked_config, &root);
        assert!(existing_targets(candidates).is_empty());
        assert!(real_config.join("google-chrome").is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registry_receipt_serializes_verified_before_after_without_global_atomic_claim() {
        let receipt = BrowserRegistrationReceipt {
            browser: ChromiumFamily::Brave,
            user_data_root: PathBuf::from("browser-root"),
            registration: BrowserRegistrationPlan::CurrentUserRegistry {
                product_key: "Software\\BraveSoftware\\Brave-Browser".into(),
            },
            outcome: BrowserRegistrationOutcome::RegistryWritten {
                before: Some(PathBuf::from("old-manifest.json")),
                after: PathBuf::from("native-host.json"),
                replaced: true,
            },
        };
        let value = serde_json::to_value(receipt).unwrap();
        assert_eq!(value["outcome"]["outcome"], "registry-written");
        assert_eq!(value["outcome"]["before"], "old-manifest.json");
        assert_eq!(value["outcome"]["after"], "native-host.json");
        assert_eq!(value["outcome"]["replaced"], true);
        assert!(value.get("atomic").is_none());
    }

    #[test]
    fn no_existing_browser_root_is_typed_before_materialization() {
        let root = fixture("no-roots");
        let candidates = candidates_for(HostKind::Linux, &root, &root.join("config"), &root);
        assert!(existing_targets(candidates).is_empty());
        let paths = BrowserBridgeInstallPaths {
            extension: root.join("extension"),
            native_manifest_file: root.join("native-host.json"),
            targets: Vec::new(),
        };
        assert_eq!(
            install_at(&root.join("unused"), paths).unwrap_err().code,
            "browser_bridge_no_supported_browser_profile"
        );
        assert!(!root.join("extension").exists());
        assert!(!root.join("native-host.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn install_receipt_separates_shared_publication_from_each_registration() {
        let root = fixture("install");
        let executable = root.join("agenterm-cu");
        fs::write(&executable, b"fixture").unwrap();
        let first_destination = root.join("browser-one/NativeMessagingHosts/host.json");
        let blocked_parent = root.join("blocked");
        fs::write(&blocked_parent, b"not-a-directory").unwrap();
        let paths = BrowserBridgeInstallPaths {
            extension: root.join("extension"),
            native_manifest_file: root.join("native-host.json"),
            targets: vec![
                BrowserRegistrationTarget {
                    browser: ChromiumFamily::Chrome,
                    user_data_root: root.join("browser-one"),
                    registration: BrowserRegistrationPlan::ManifestFile {
                        destination: first_destination.clone(),
                    },
                },
                BrowserRegistrationTarget {
                    browser: ChromiumFamily::Edge,
                    user_data_root: blocked_parent.clone(),
                    registration: BrowserRegistrationPlan::ManifestFile {
                        destination: blocked_parent.join("host.json"),
                    },
                },
            ],
        };
        let failure = install_at(&executable, paths).unwrap_err();
        assert_eq!(failure.code, "browser_bridge_registration_partial");
        let receipt = failure.receipt.unwrap();
        assert!(receipt.bundle_materialized);
        assert!(receipt.native_manifest_file_written);
        assert!(!receipt.extension_loaded);
        assert!(receipt.manual_activation_required);
        assert_eq!(receipt.registrations.len(), 2);
        assert!(matches!(
            receipt.registrations[0].outcome,
            BrowserRegistrationOutcome::ManifestWritten { .. }
        ));
        assert!(matches!(
            receipt.registrations[1].outcome,
            BrowserRegistrationOutcome::Failed { .. }
        ));
        assert!(first_destination.is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn manifest_registration_rejects_and_preserves_an_existing_symlink() {
        use std::os::unix::fs::symlink;

        let root = fixture("manifest-link");
        let sentinel = root.join("sentinel.json");
        fs::write(&sentinel, b"original").unwrap();
        let destination = root.join("host.json");
        symlink("sentinel.json", &destination).unwrap();
        let target = BrowserRegistrationTarget {
            browser: ChromiumFamily::Chromium,
            user_data_root: root.clone(),
            registration: BrowserRegistrationPlan::ManifestFile {
                destination: destination.clone(),
            },
        };

        let outcome = register_target(&target, &root.join("stable.json"), b"replacement");
        assert!(matches!(
            outcome,
            BrowserRegistrationOutcome::Failed { ref code }
                if code == "browser_bridge_native_manifest_registration_destination_invalid"
        ));
        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(&destination).unwrap(),
            PathBuf::from("sentinel.json")
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"original");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn manifest_registration_rejects_a_symlink_parent_without_writing_through_it() {
        use std::os::unix::fs::symlink;

        let root = fixture("manifest-parent-link");
        let browser_root = root.join("browser");
        let external = root.join("external");
        fs::create_dir(&browser_root).unwrap();
        fs::create_dir(&external).unwrap();
        let registration_parent = browser_root.join("NativeMessagingHosts");
        symlink(&external, &registration_parent).unwrap();
        let destination = registration_parent.join("host.json");
        let target = BrowserRegistrationTarget {
            browser: ChromiumFamily::Chrome,
            user_data_root: browser_root,
            registration: BrowserRegistrationPlan::ManifestFile { destination },
        };

        let outcome = register_target(&target, &root.join("stable.json"), b"replacement");
        assert!(matches!(
            outcome,
            BrowserRegistrationOutcome::Failed { ref code }
                if code == "browser_bridge_native_manifest_registration_destination_invalid"
        ));
        assert!(
            fs::symlink_metadata(&registration_parent)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!external.join("host.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_installer_accepts_only_the_exact_running_regular_executable() {
        let current = std::env::current_exe().unwrap();
        validate_current_executable(&current).unwrap();
        let root = fixture("executable");
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

    #[test]
    fn prepared_bundle_is_byte_exact() {
        let root = fixture("bundle");
        prepare_extension(&root).unwrap();
        for asset in extension_assets() {
            assert_eq!(
                fs::read(root.join(asset.relative_path)).unwrap(),
                asset.bytes
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
