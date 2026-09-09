use std::path::{Component, Path, PathBuf};

use serde_json::json;

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

pub fn extension_assets() -> &'static [ExtensionAsset] {
    ASSETS
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
        ] {
            assert!(source.contains(command));
        }
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
