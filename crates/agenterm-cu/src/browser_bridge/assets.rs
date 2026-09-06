use std::path::{Component, Path, PathBuf};

use serde_json::json;

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
        for command in [
            "status",
            "tabs",
            "windows",
            "window-open",
            "window-state",
            "debug-read",
            "reload",
        ] {
            assert!(source.contains(command));
        }
        for forbidden in ["debug-type", "debug-invoke", "debug-files"] {
            assert!(!source.contains(forbidden));
        }
        for forbidden in [
            "DOM.getFlattenedDocument",
            "node.attributes",
            "node.nodeValue",
        ] {
            assert!(!source.contains(forbidden));
        }
        assert!(source.contains("Accessibility.getFullAXTree"));
    }
}
