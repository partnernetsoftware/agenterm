use std::{
    borrow::Cow,
    collections::BTreeSet,
    io::Read as _,
    path::{Component, Path, PathBuf},
};

use agenterm_platform::filesystem_open::{ExistingEntryType, open_existing_path};
use serde_json::json;
use sha2::{Digest, Sha256};

#[cfg(test)]
use super::PROTOCOL_VERSION;
use super::{ACU_EXTENSION_ID, ACU_NATIVE_HOST_NAME};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtensionAsset {
    pub relative_path: &'static str,
    pub bytes: &'static [u8],
}

const ASSETS: &[ExtensionAsset] = &[
    ExtensionAsset {
        relative_path: "manifest.json",
        bytes: include_bytes!("../../assets/browser-bridge/manifest.json"),
    },
    ExtensionAsset {
        relative_path: "background.js",
        bytes: include_bytes!("../../assets/browser-bridge/background.js"),
    },
];

const BUILD_ID_PLACEHOLDER: &[u8] = b"__ACU_BUILD_ID__";

pub fn extension_assets() -> &'static [ExtensionAsset] {
    ASSETS
}

/// Stable identity of the raw, reviewable extension bundle. The JavaScript
/// source contains one fixed-width placeholder, so the digest has no
/// self-reference; publication substitutes the hexadecimal digest exactly
/// once without changing the source asset embedded in this binary.
pub fn extension_build_id() -> String {
    let mut digest = Sha256::new();
    for asset in ASSETS {
        let path = asset.relative_path.as_bytes();
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path);
        digest.update((asset.bytes.len() as u64).to_le_bytes());
        digest.update(asset.bytes);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn materialized_extension_asset(asset: &ExtensionAsset) -> Cow<'static, [u8]> {
    if asset.relative_path != "background.js" {
        return Cow::Borrowed(asset.bytes);
    }
    let occurrences = asset
        .bytes
        .windows(BUILD_ID_PLACEHOLDER.len())
        .filter(|window| *window == BUILD_ID_PLACEHOLDER)
        .count();
    assert_eq!(
        occurrences, 1,
        "background build-id placeholder must be unique"
    );
    let start = asset
        .bytes
        .windows(BUILD_ID_PLACEHOLDER.len())
        .position(|window| window == BUILD_ID_PLACEHOLDER)
        .expect("checked unique build-id placeholder");
    let build_id = extension_build_id();
    let mut rendered = Vec::with_capacity(asset.bytes.len() - BUILD_ID_PLACEHOLDER.len() + 64);
    rendered.extend_from_slice(&asset.bytes[..start]);
    rendered.extend_from_slice(build_id.as_bytes());
    rendered.extend_from_slice(&asset.bytes[start + BUILD_ID_PLACEHOLDER.len()..]);
    Cow::Owned(rendered)
}

/// Proves that one already-published directory is exactly the bundle embedded
/// in this binary. Extra files, links, short reads and any byte drift are all
/// rejected; a caller may therefore use the returned id as the reload target.
pub fn verify_materialized_extension(root: &Path) -> Result<String, MaterializationError> {
    let _directory = open_existing_path(root, ExistingEntryType::Directory)
        .map_err(|_| MaterializationError::PublishedBundleInvalid)?;
    let expected = ASSETS
        .iter()
        .map(|asset| asset.relative_path.to_owned())
        .collect::<BTreeSet<_>>();
    let observed = published_names(root)?;
    if observed != expected {
        return Err(MaterializationError::PublishedBundleInvalid);
    }
    for asset in ASSETS {
        let expected = materialized_extension_asset(asset);
        let mut file = open_existing_path(&root.join(asset.relative_path), ExistingEntryType::File)
            .map_err(|_| MaterializationError::PublishedBundleInvalid)?;
        let metadata = file
            .metadata()
            .map_err(|_| MaterializationError::PublishedBundleInvalid)?;
        if metadata.len() != expected.len() as u64 {
            return Err(MaterializationError::PublishedBundleInvalid);
        }
        let mut bytes = Vec::with_capacity(expected.len());
        (&mut file)
            .take((expected.len() + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| MaterializationError::PublishedBundleInvalid)?;
        if bytes.as_slice() != expected.as_ref() {
            return Err(MaterializationError::PublishedBundleInvalid);
        }
    }
    if published_names(root)? != expected {
        return Err(MaterializationError::PublishedBundleInvalid);
    }
    Ok(extension_build_id())
}

fn published_names(root: &Path) -> Result<BTreeSet<String>, MaterializationError> {
    let mut names = BTreeSet::new();
    for entry in
        std::fs::read_dir(root).map_err(|_| MaterializationError::PublishedBundleInvalid)?
    {
        let entry = entry.map_err(|_| MaterializationError::PublishedBundleInvalid)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| MaterializationError::PublishedBundleInvalid)?;
        names.insert(name);
    }
    Ok(names)
}

/// A side-by-side staging plan. The caller writes every asset into `staging`,
/// validates it, then performs one platform-owned replace of `destination`.
/// This helper intentionally performs no partial filesystem mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtensionMaterializationPlan {
    pub destination: PathBuf,
    pub staging: PathBuf,
    pub assets: &'static [ExtensionAsset],
}

impl ExtensionMaterializationPlan {
    pub fn new(destination: &Path, random_suffix: &str) -> Result<Self, MaterializationError> {
        if !destination.is_absolute() || destination.file_name().is_none() {
            return Err(MaterializationError::DestinationNotAbsolute);
        }
        if random_suffix.len() < 32
            || random_suffix.len() > 128
            || !random_suffix.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(MaterializationError::RandomSuffixInvalid);
        }
        let file_name = destination.file_name().expect("checked").to_string_lossy();
        let staging = destination.with_file_name(format!(".{file_name}.stage-{random_suffix}"));
        if staging == destination {
            return Err(MaterializationError::StagingAliasesDestination);
        }
        for asset in ASSETS {
            let path = Path::new(asset.relative_path);
            if path.is_absolute()
                || path
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err(MaterializationError::AssetPathInvalid);
            }
        }
        Ok(Self {
            destination: destination.to_owned(),
            staging,
            assets: ASSETS,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterializationError {
    DestinationNotAbsolute,
    RandomSuffixInvalid,
    StagingAliasesDestination,
    AssetPathInvalid,
    ExecutablePathInvalid,
    PublishedBundleInvalid,
}

/// Produces the per-user Chromium native-host manifest for the same
/// `agenterm-cu` executable. Installation and atomic publication stay with the
/// platform-specific caller.
pub fn native_host_manifest(executable: &Path) -> Result<Vec<u8>, MaterializationError> {
    if !executable.is_absolute() || executable.to_str().is_none() {
        return Err(MaterializationError::ExecutablePathInvalid);
    }
    serde_json::to_vec_pretty(&json!({
        "name": ACU_NATIVE_HOST_NAME,
        "description": "AgenTerm ACU browser bridge",
        "path": executable,
        "type": "stdio",
        "allowed_origins": [format!("chrome-extension://{ACU_EXTENSION_ID}/")]
    }))
    .map_err(|_| MaterializationError::ExecutablePathInvalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn embedded_bundle_is_small_unique_and_uses_only_relative_leaf_paths() {
        assert_eq!(ASSETS.len(), 2);
        assert!(
            ASSETS
                .iter()
                .all(|asset| !asset.bytes.is_empty() && asset.bytes.len() < 256 * 1024)
        );
        assert_ne!(ASSETS[0].relative_path, ASSETS[1].relative_path);
        assert!(ASSETS.iter().all(|asset| {
            Path::new(asset.relative_path)
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        }));
        assert_eq!(extension_build_id().len(), 64);
        let source = std::str::from_utf8(ASSETS[1].bytes).unwrap();
        assert_eq!(source.matches("__ACU_BUILD_ID__").count(), 1);
        let rendered = materialized_extension_asset(&ASSETS[1]);
        assert!(
            !rendered
                .windows(BUILD_ID_PLACEHOLDER.len())
                .any(|row| row == BUILD_ID_PLACEHOLDER)
        );
        assert!(
            std::str::from_utf8(&rendered)
                .unwrap()
                .contains(&extension_build_id())
        );
    }

    #[test]
    fn staging_plan_requires_absolute_destination_and_random_hex() {
        let destination = if cfg!(windows) {
            Path::new(r"C:\acu\browser-bridge")
        } else {
            Path::new("/acu/browser-bridge")
        };
        let plan =
            ExtensionMaterializationPlan::new(destination, "0123456789abcdef0123456789abcdef")
                .unwrap();
        assert_eq!(plan.destination, destination);
        assert_ne!(plan.staging, plan.destination);
        assert!(matches!(
            ExtensionMaterializationPlan::new(
                Path::new("relative"),
                "0123456789abcdef0123456789abcdef"
            ),
            Err(MaterializationError::DestinationNotAbsolute)
        ));
        assert!(matches!(
            ExtensionMaterializationPlan::new(destination, "predictable"),
            Err(MaterializationError::RandomSuffixInvalid)
        ));
    }

    #[test]
    fn native_manifest_is_fixed_acu_identity_and_same_binary_path() {
        let executable = if cfg!(windows) {
            Path::new(r"C:\Program Files\AgenTerm\agenterm-cu.exe")
        } else {
            Path::new("/opt/agenterm/agenterm-cu")
        };
        let value: Value =
            serde_json::from_slice(&native_host_manifest(executable).unwrap()).unwrap();
        assert_eq!(value["name"], ACU_NATIVE_HOST_NAME);
        assert_eq!(value["path"], executable.to_str().unwrap());
        assert_eq!(
            value["allowed_origins"][0],
            format!("chrome-extension://{ACU_EXTENSION_ID}/")
        );
        let joined = String::from_utf8(native_host_manifest(executable).unwrap()).unwrap();
        assert!(!joined.to_ascii_lowercase().contains("moltbaby"));
        assert!(!joined.to_ascii_lowercase().contains("mcu"));
    }

    #[test]
    fn published_bundle_identity_rejects_byte_drift_and_extra_files() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-cu-extension-identity-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        for asset in ASSETS {
            std::fs::write(
                root.join(asset.relative_path),
                materialized_extension_asset(asset),
            )
            .unwrap();
        }
        assert_eq!(
            verify_materialized_extension(&root).unwrap(),
            extension_build_id()
        );
        std::fs::write(root.join("extra"), b"unexpected").unwrap();
        assert_eq!(
            verify_materialized_extension(&root),
            Err(MaterializationError::PublishedBundleInvalid)
        );
        std::fs::remove_file(root.join("extra")).unwrap();
        std::fs::write(root.join("manifest.json"), b"{}").unwrap();
        assert_eq!(
            verify_materialized_extension(&root),
            Err(MaterializationError::PublishedBundleInvalid)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extension_manifest_matches_native_identity_and_permissions() {
        let manifest: Value = serde_json::from_slice(ASSETS[0].bytes).unwrap();
        assert_eq!(manifest["manifest_version"], 3);
        assert_eq!(
            manifest["permissions"],
            serde_json::json!(["nativeMessaging", "tabs", "debugger", "storage"])
        );
        assert_eq!(manifest["version"], super::super::BRIDGE_EXTENSION_VERSION);
        assert_eq!(manifest["background"]["service_worker"], "background.js");
        let source = std::str::from_utf8(ASSETS[1].bytes).unwrap();
        assert!(source.contains(ACU_NATIVE_HOST_NAME));
        assert!(source.contains(&format!("const PROTOCOL = {PROTOCOL_VERSION};")));
        for command in [
            "status",
            "tabs",
            "windows",
            "window-open",
            "window-state",
            "nav",
            "debug-read",
            "debug-invoke",
            "debug-type",
            "debug-files",
            "reload",
            "extension-reload",
        ] {
            assert!(source.contains(command));
        }
        assert!(source.contains("\"reload\", \"extension-reload\"].includes(request.command)"));
        let actionable_filter = source
            .find("if (request.actionable && !actionable) continue;")
            .expect("debug-read provider-side actionable filter");
        let result_ceiling = source
            .find("if (result.length >= resultBudget)")
            .expect("debug-read result ceiling");
        assert!(actionable_filter < result_ceiling);
        for fact in [
            "focusable",
            "editable",
            "actionable",
            "disabled",
            "focused",
            "request_actionable",
        ] {
            assert!(source.contains(fact));
        }
        for forbidden in [
            "DOM.getFlattenedDocument",
            "node.attributes",
            "node.nodeValue",
        ] {
            assert!(!source.contains(forbidden));
        }
        assert!(source.contains("Accessibility.getFullAXTree"));
        let open_start = source.find("async function openWindow").unwrap();
        let open_end = source[open_start..]
            .find("async function waitWindowState")
            .map(|offset| open_start + offset)
            .unwrap();
        let open_source = &source[open_start..open_end];
        assert!(open_source.contains("chrome.windows.create"));
        assert!(open_source.contains("state,"));
        assert!(!open_source.contains("chrome.windows.update(createdId"));

        let nav_start = source.find("async function navigateTab").unwrap();
        let nav_end = source[nav_start..]
            .find("async function openWindow")
            .map(|offset| nav_start + offset)
            .unwrap();
        let nav_source = &source[nav_start..nav_end];
        assert!(source.contains("Object.keys(args).sort().join(\",\") !== \"tab_id,url\""));
        assert!(source.contains("request.command === \"nav\" && !validNavArgs(request.args)"));
        assert!(nav_source.contains("if (!validNavArgs(args))"));
        assert!(source.contains("parsed.username.length === 0"));
        assert!(source.contains("parsed.password.length === 0"));
        assert!(nav_source.contains("chrome.debugger.attach(target, \"1.3\")"));
        assert!(nav_source.contains("chrome.debugger.sendCommand("));
        assert!(nav_source.contains("\"Page.navigate\""));
        assert!(nav_source.contains("Page.frameNavigated"));
        assert!(nav_source.contains("params.frame.parentId === undefined"));
        assert!(nav_source.contains("Page.navigatedWithinDocument"));
        assert!(nav_source.contains("params.frameId === rootFrameId && params.url === args.url"));
        assert!(nav_source.contains("Page.javascriptDialogOpening"));
        assert!(nav_source.contains("dialogSignal.then"));
        assert!(nav_source.contains("NAV_EVENT_MAX"));
        assert!(nav_source.contains("browser_bridge_nav_dialog_blocked"));
        assert!(
            nav_source.find("if (dialogBlocked)").unwrap()
                < nav_source.find("commitProven = true").unwrap()
        );
        assert!(nav_source.contains("navigationDeadline = Date.now() + NAV_COMMIT_TIMEOUT_MS"));
        assert!(nav_source.contains("beforeDeadline("));
        assert!(nav_source.contains("navigationPromise.catch(() => {})"));
        assert!(nav_source.contains("browser_bridge_nav_commit_timeout"));
        assert!(nav_source.contains("browser_bridge_nav_actuation_failed"));
        assert!(source.contains(
            "NAV_ERROR_CODES.has(raw)\n    ? raw : \"browser_bridge_nav_actuation_failed\""
        ));
        assert!(nav_source.contains("navigation.errorText"));
        assert!(nav_source.contains("failureNavigation ="));
        assert!(nav_source.contains("navigation: failureNavigation"));
        assert!(nav_source.contains("afterTab.windowId !== beforeTab.windowId"));
        assert!(nav_source.contains("afterActive !== beforeActive"));
        assert!(nav_source.contains("afterFocused !== beforeFocused"));
        assert!(nav_source.contains("method === \"Page.loadEventFired\" && commitProven"));
        assert!(nav_source.contains("![\"loading\", \"complete\"].includes(afterTab.status)"));
        assert!(nav_source.contains("load_state: afterTab.status"));
        assert!(nav_source.contains("activation_requested: false"));
        assert!(nav_source.contains("raw === \"browser_bridge_nav_failed\""));
        assert!(nav_source.contains("performedFailure ? \"performed\""));
        assert!(nav_source.contains("navEffect = \"unknown\""));
        assert!(nav_source.contains("navEffect = effectStarted ? \"unknown\" : \"not-performed\""));
        assert!(nav_source.contains("browser_bridge_nav_detach_failed"));
        assert!(nav_source.contains("chrome.debugger.detach(target)"));
        assert_eq!(nav_source.matches("effectStarted = true").count(), 1);
        assert!(
            nav_source.find("effectStarted = true").unwrap()
                < nav_source.find("\"Page.navigate\"").unwrap()
        );
        assert!(source.contains("errorResult.detach = error && error.detach ||"));
        assert!(!nav_source.contains("chrome.tabs.update"));
        assert!(!nav_source.contains("chrome.tabs.remove"));
        assert!(!source.contains("Page.handleJavaScriptDialog"));
    }
}
