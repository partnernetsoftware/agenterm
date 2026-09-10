use std::{
    collections::BTreeSet,
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
use serde::{Deserialize, Serialize};

use super::{
    ACU_NATIVE_HOST_NAME, ExtensionMaterializationPlan, extension_assets, native_host_manifest,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChromiumFamily {
    Chrome,
    Chromium,
    Brave,
    BraveOrigin,
    Edge,
}

/// Closed public selector for Native Messaging registration targets. An empty
/// selector list keeps the direct native command's all-discovered behavior;
/// compatibility callers use an explicit set and therefore cannot widen it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BrowserSetupBrowser {
    Chrome,
    Chromium,
    BraveBrowser,
    BraveOrigin,
    Edge,
}

impl BrowserSetupBrowser {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::BraveBrowser => "brave-browser",
            Self::BraveOrigin => "brave-origin",
            Self::Edge => "edge",
        }
    }

    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "chrome" => Ok(Self::Chrome),
            "chromium" => Ok(Self::Chromium),
            "brave-browser" => Ok(Self::BraveBrowser),
            "brave-origin" => Ok(Self::BraveOrigin),
            "edge" => Ok(Self::Edge),
            _ => Err("browser must be chrome, chromium, brave-browser, brave-origin or edge"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BrowserSetupDiscovery {
    pub browser: BrowserSetupBrowser,
    pub user_data_root: PathBuf,
    pub requested: bool,
    pub present: bool,
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
    /// Empty means the direct native all-discovered mode.
    pub requested_browsers: Vec<BrowserSetupBrowser>,
    pub discovered_roots: Vec<BrowserSetupDiscovery>,
    pub targets: Vec<BrowserRegistrationTarget>,
    pub skipped_registrations: Vec<BrowserRegistrationReceipt>,
}

impl BrowserBridgeInstallPaths {
    pub fn for_current_user() -> Result<Self, BrowserBridgeInstallError> {
        Self::for_current_user_selected(&[])
    }

    pub fn for_current_user_selected(
        requested: &[BrowserSetupBrowser],
    ) -> Result<Self, BrowserBridgeInstallError> {
        let directories =
            host_directories().map_err(|_| error("browser_bridge_home_unavailable"))?;
        let bridge_root = directories
            .local_data
            .join("agenterm")
            .join("cu")
            .join("browser-bridge");
        let home = user_home_directory().map_err(|_| error("browser_bridge_home_unavailable"))?;
        selected_paths(
            HOST_KIND,
            &home,
            &directories.config,
            &directories.local_data,
            bridge_root,
            requested,
        )
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
    SkippedRootMissing,
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
    pub effect: BrowserSetupEffect,
    /// Empty means the direct native all-discovered mode.
    pub requested_browsers: Vec<BrowserSetupBrowser>,
    pub discovered_roots: Vec<BrowserSetupDiscovery>,
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
    pub complete: bool,
    /// A caller may deliberately rerun the same selector set after repairing a
    /// partial registration. This does not authorize an automatic retry.
    pub idempotent_rerun: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BrowserSetupEffect {
    NotPerformed,
    Performed,
    PerformedPartial,
    Unknown,
}

impl BrowserBridgeInstall {
    fn empty(paths: &BrowserBridgeInstallPaths) -> Self {
        Self {
            effect: BrowserSetupEffect::NotPerformed,
            requested_browsers: paths.requested_browsers.clone(),
            discovered_roots: paths.discovered_roots.clone(),
            extension: paths.extension.clone(),
            native_manifest_file: paths.native_manifest_file.clone(),
            replaced_extension: false,
            bundle_materialized: false,
            native_manifest_file_written: false,
            registrations: paths.skipped_registrations.clone(),
            extension_loaded: false,
            manual_activation_required: true,
            complete: false,
            idempotent_rerun: true,
        }
    }
}

pub fn install_for_current_user(
    executable: &Path,
) -> Result<BrowserBridgeInstall, BrowserBridgeInstallError> {
    validate_current_executable(executable)?;
    install_at(executable, BrowserBridgeInstallPaths::for_current_user()?)
}

pub fn install_for_current_user_selected(
    executable: &Path,
    requested: &[BrowserSetupBrowser],
) -> Result<BrowserBridgeInstall, BrowserBridgeInstallError> {
    validate_current_executable(executable)?;
    install_at(
        executable,
        BrowserBridgeInstallPaths::for_current_user_selected(requested)?,
    )
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
    let mut receipt = BrowserBridgeInstall::empty(&paths);
    if paths.targets.is_empty() {
        return Err(error("browser_bridge_no_supported_browser_profile").with_receipt(receipt));
    }
    let manifest = native_host_manifest(executable)
        .map_err(|_| error("browser_bridge_native_manifest_invalid"))?;
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
    let outcome = publish_directory(&plan.staging, &plan.destination).map_err(|failure| {
        if matches!(
            failure.kind(),
            agenterm_platform::filesystem_publish::DirectoryPublishErrorKind::Rollback
        ) {
            receipt.effect = BrowserSetupEffect::Unknown;
        }
        error("browser_bridge_extension_publish_failed").with_receipt(receipt.clone())
    })?;
    receipt.replaced_extension = outcome.replaced_existing();
    receipt.bundle_materialized = true;
    receipt.effect = BrowserSetupEffect::Performed;

    let manifest_parent = paths.native_manifest_file.parent().ok_or_else(|| {
        let mut failed = receipt.clone();
        failed.effect = BrowserSetupEffect::PerformedPartial;
        error("browser_bridge_install_plan_invalid").with_receipt(failed)
    })?;
    fs::create_dir_all(manifest_parent).map_err(|_| {
        let mut failed = receipt.clone();
        failed.effect = BrowserSetupEffect::PerformedPartial;
        error("browser_bridge_install_prepare_failed").with_receipt(failed)
    })?;
    match write_file_atomic(&paths.native_manifest_file, |file| {
        file.write_all(&manifest)
    }) {
        Ok(()) => receipt.native_manifest_file_written = true,
        Err(failure) => {
            receipt.native_manifest_file_written = failure.published();
            receipt.effect = if failure.published() {
                BrowserSetupEffect::Unknown
            } else {
                BrowserSetupEffect::PerformedPartial
            };
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
    receipt.registrations.sort_by_key(|row| row.browser);
    if receipt.registrations.iter().any(|row| {
        matches!(
            &row.outcome,
            BrowserRegistrationOutcome::Failed { code }
                if code.ends_with("_durability_uncertain")
        )
    }) {
        receipt.effect = BrowserSetupEffect::Unknown;
    }
    if any_failed {
        if receipt.effect != BrowserSetupEffect::Unknown {
            receipt.effect = BrowserSetupEffect::PerformedPartial;
        }
        Err(error("browser_bridge_registration_partial").with_receipt(receipt))
    } else {
        receipt.complete = true;
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

#[derive(Clone, Copy, Eq, PartialEq)]
#[allow(dead_code)]
enum HostKind {
    Macos,
    Linux,
    Windows,
}

#[cfg(target_os = "macos")]
const HOST_KIND: HostKind = HostKind::Macos;
#[cfg(target_os = "linux")]
const HOST_KIND: HostKind = HostKind::Linux;
#[cfg(windows)]
const HOST_KIND: HostKind = HostKind::Windows;

fn selected_paths(
    host: HostKind,
    home: &Path,
    config: &Path,
    local_data: &Path,
    bridge_root: PathBuf,
    requested: &[BrowserSetupBrowser],
) -> Result<BrowserBridgeInstallPaths, BrowserBridgeInstallError> {
    let unique = requested.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != requested.len() {
        return Err(error("browser_bridge_setup_selector_invalid"));
    }
    if host == HostKind::Windows && unique.contains(&BrowserSetupBrowser::BraveOrigin) {
        return Err(error("browser_bridge_setup_selector_not_applicable"));
    }
    let explicit = !requested.is_empty();
    let mut targets = Vec::new();
    let mut skipped_registrations = Vec::new();
    let mut discovered_roots = Vec::new();
    for (browser, candidate) in setup_candidates_for(host, home, config, local_data) {
        let selected = !explicit || unique.contains(&browser);
        let present =
            open_existing_path(&candidate.user_data_root, ExistingEntryType::Directory).is_ok();
        discovered_roots.push(BrowserSetupDiscovery {
            browser,
            user_data_root: candidate.user_data_root.clone(),
            requested: selected,
            present,
        });
        if selected && present {
            targets.push(candidate);
        } else if explicit && selected {
            skipped_registrations.push(BrowserRegistrationReceipt {
                browser: candidate.browser,
                user_data_root: candidate.user_data_root,
                registration: candidate.registration,
                outcome: BrowserRegistrationOutcome::SkippedRootMissing,
            });
        }
    }
    Ok(BrowserBridgeInstallPaths {
        extension: bridge_root.join("extension"),
        native_manifest_file: bridge_root.join("native-host.json"),
        requested_browsers: requested.to_vec(),
        discovered_roots,
        targets,
        skipped_registrations,
    })
}

#[cfg(test)]
fn candidates_for(
    host: HostKind,
    _home: &Path,
    _config: &Path,
    _local_data: &Path,
) -> Vec<BrowserRegistrationTarget> {
    setup_candidates_for(host, _home, _config, _local_data)
        .into_iter()
        .map(|(_, target)| target)
        .collect()
}

fn setup_candidates_for(
    host: HostKind,
    _home: &Path,
    _config: &Path,
    _local_data: &Path,
) -> Vec<(BrowserSetupBrowser, BrowserRegistrationTarget)> {
    let families = match host {
        HostKind::Macos => {
            let support = _home.join("Library").join("Application Support");
            vec![
                (
                    BrowserSetupBrowser::Chrome,
                    ChromiumFamily::Chrome,
                    support.join("Google/Chrome"),
                    "Software\\Google\\Chrome",
                ),
                (
                    BrowserSetupBrowser::Chromium,
                    ChromiumFamily::Chromium,
                    support.join("Chromium"),
                    "Software\\Chromium",
                ),
                (
                    BrowserSetupBrowser::BraveBrowser,
                    ChromiumFamily::Brave,
                    support.join("BraveSoftware/Brave-Browser"),
                    "Software\\BraveSoftware\\Brave-Browser",
                ),
                (
                    BrowserSetupBrowser::BraveOrigin,
                    ChromiumFamily::BraveOrigin,
                    support.join("BraveSoftware/Brave-Origin"),
                    "Software\\BraveSoftware\\Brave-Origin",
                ),
                (
                    BrowserSetupBrowser::Edge,
                    ChromiumFamily::Edge,
                    support.join("Microsoft Edge"),
                    "Software\\Microsoft\\Edge",
                ),
            ]
        }
        HostKind::Linux => vec![
            (
                BrowserSetupBrowser::Chrome,
                ChromiumFamily::Chrome,
                _config.join("google-chrome"),
                "Software\\Google\\Chrome",
            ),
            (
                BrowserSetupBrowser::Chromium,
                ChromiumFamily::Chromium,
                _config.join("chromium"),
                "Software\\Chromium",
            ),
            (
                BrowserSetupBrowser::BraveBrowser,
                ChromiumFamily::Brave,
                _config.join("BraveSoftware/Brave-Browser"),
                "Software\\BraveSoftware\\Brave-Browser",
            ),
            (
                BrowserSetupBrowser::BraveOrigin,
                ChromiumFamily::BraveOrigin,
                _config.join("BraveSoftware/Brave-Origin"),
                "Software\\BraveSoftware\\Brave-Origin",
            ),
            (
                BrowserSetupBrowser::Edge,
                ChromiumFamily::Edge,
                _config.join("microsoft-edge"),
                "Software\\Microsoft\\Edge",
            ),
        ],
        HostKind::Windows => vec![
            (
                BrowserSetupBrowser::Chrome,
                ChromiumFamily::Chrome,
                _local_data.join("Google/Chrome/User Data"),
                "Software\\Google\\Chrome",
            ),
            (
                BrowserSetupBrowser::Chromium,
                ChromiumFamily::Chromium,
                _local_data.join("Chromium/User Data"),
                "Software\\Chromium",
            ),
            (
                BrowserSetupBrowser::BraveBrowser,
                ChromiumFamily::Brave,
                _local_data.join("BraveSoftware/Brave-Browser/User Data"),
                "Software\\BraveSoftware\\Brave-Browser",
            ),
            (
                BrowserSetupBrowser::Edge,
                ChromiumFamily::Edge,
                _local_data.join("Microsoft/Edge/User Data"),
                "Software\\Microsoft\\Edge",
            ),
        ],
    };
    families
        .into_iter()
        .map(|(selector, browser, root, product_key)| {
            (
                selector,
                BrowserRegistrationTarget {
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
                },
            )
        })
        .collect()
}

fn host_uses_registry(host: HostKind) -> bool {
    match host {
        HostKind::Windows => true,
        HostKind::Macos => false,
        HostKind::Linux => false,
    }
}

#[cfg(test)]
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
    pub receipt: Option<Box<BrowserBridgeInstall>>,
}

impl BrowserBridgeInstallError {
    fn with_receipt(mut self, receipt: BrowserBridgeInstall) -> Self {
        self.receipt = Some(Box::new(receipt));
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

    #[test]
    fn selected_setup_keeps_exact_scope_and_reports_a_missing_requested_root() {
        let root = fixture("selected-roots");
        let support = root.join("Library/Application Support");
        let chrome = support.join("Google/Chrome");
        let brave = support.join("BraveSoftware/Brave-Browser");
        let chromium = support.join("Chromium");
        fs::create_dir_all(&chrome).unwrap();
        fs::create_dir_all(&brave).unwrap();
        fs::create_dir_all(&chromium).unwrap();
        let requested = [
            BrowserSetupBrowser::BraveBrowser,
            BrowserSetupBrowser::BraveOrigin,
            BrowserSetupBrowser::Chrome,
        ];
        let paths = selected_paths(
            HostKind::Macos,
            &root,
            &root.join("config"),
            &root.join("local"),
            root.join("bridge"),
            &requested,
        )
        .unwrap();
        assert_eq!(paths.targets.len(), 2);
        assert_eq!(paths.skipped_registrations.len(), 1);
        assert!(matches!(
            paths.skipped_registrations[0].outcome,
            BrowserRegistrationOutcome::SkippedRootMissing
        ));
        assert!(
            paths
                .discovered_roots
                .iter()
                .any(|row| row.browser == BrowserSetupBrowser::Chromium
                    && row.present
                    && !row.requested)
        );

        let executable = root.join("agenterm-cu");
        fs::write(&executable, b"fixture").unwrap();
        let receipt = install_at(&executable, paths).unwrap();
        assert_eq!(receipt.effect, BrowserSetupEffect::Performed);
        assert!(receipt.complete && receipt.idempotent_rerun);
        assert_eq!(receipt.registrations.len(), 3);
        assert!(receipt.registrations.iter().any(|row| {
            row.browser == ChromiumFamily::BraveOrigin
                && matches!(row.outcome, BrowserRegistrationOutcome::SkippedRootMissing)
        }));
        assert!(!chromium.join("NativeMessagingHosts").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn brave_origin_is_not_applicable_to_windows_registration() {
        let root = fixture("origin-windows");
        let failure = selected_paths(
            HostKind::Windows,
            &root,
            &root.join("config"),
            &root.join("local"),
            root.join("bridge"),
            &[BrowserSetupBrowser::BraveOrigin],
        )
        .unwrap_err();
        assert_eq!(failure.code, "browser_bridge_setup_selector_not_applicable");
        assert!(!root.join("bridge").exists());
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
            requested_browsers: Vec::new(),
            discovered_roots: Vec::new(),
            targets: Vec::new(),
            skipped_registrations: Vec::new(),
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
            requested_browsers: Vec::new(),
            discovered_roots: Vec::new(),
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
            skipped_registrations: Vec::new(),
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
