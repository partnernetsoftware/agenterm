//! Current-user native-messaging registration primitives.
//!
//! Browser product selection remains with the caller: this module accepts one
//! explicit Chromium-family registry root and never probes or guesses roots.

use std::fmt;
use std::path::{Path, PathBuf};

mod selected;

const MAX_HOST_NAME_BYTES: usize = 255;
const MAX_REGISTRY_ROOT_UTF16: usize = 512;
const MAX_MANIFEST_PATH_UTF16: usize = 32_766;

/// One caller-selected Chromium-family product key below `HKEY_CURRENT_USER`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChromiumRegistryTarget {
    product_key: String,
}

impl ChromiumRegistryTarget {
    pub fn new(product_key: impl Into<String>) -> Result<Self, NativeMessagingRegistryError> {
        let product_key = product_key.into();
        validate_product_key(&product_key)?;
        Ok(Self { product_key })
    }

    #[must_use]
    pub fn product_key(&self) -> &str {
        &self.product_key
    }

    pub(crate) fn host_key(&self, host_name: &str) -> String {
        format!("{}\\NativeMessagingHosts\\{host_name}", self.product_key)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NativeMessagingRegistryErrorKind {
    Unsupported,
    InvalidTarget,
    InvalidHostName,
    InvalidManifestPath,
    ManifestNotFound,
    ManifestNotFile,
    ManifestIsSymlink,
    RegistryOpen,
    RegistryRead,
    RegistryValueInvalid,
    RegistryWrite,
    RegistryVerify,
}

impl NativeMessagingRegistryErrorKind {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unsupported => "native_messaging_registry_unsupported",
            Self::InvalidTarget => "native_messaging_registry_target_invalid",
            Self::InvalidHostName => "native_messaging_host_name_invalid",
            Self::InvalidManifestPath => "native_messaging_manifest_path_invalid",
            Self::ManifestNotFound => "native_messaging_manifest_not_found",
            Self::ManifestNotFile => "native_messaging_manifest_not_file",
            Self::ManifestIsSymlink => "native_messaging_manifest_symlink",
            Self::RegistryOpen => "native_messaging_registry_open_failed",
            Self::RegistryRead => "native_messaging_registry_read_failed",
            Self::RegistryValueInvalid => "native_messaging_registry_value_invalid",
            Self::RegistryWrite => "native_messaging_registry_write_failed",
            Self::RegistryVerify => "native_messaging_registry_verify_failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeMessagingRegistryError {
    pub kind: NativeMessagingRegistryErrorKind,
    pub os_code: Option<u32>,
    message: String,
}

impl NativeMessagingRegistryError {
    pub(crate) fn new(
        kind: NativeMessagingRegistryErrorKind,
        os_code: Option<u32>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            os_code,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.kind.code()
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for NativeMessagingRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for NativeMessagingRegistryError {}

/// Verified result for one exact caller-selected registry target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeMessagingRegistryReceipt {
    pub target: ChromiumRegistryTarget,
    pub before: Option<PathBuf>,
    pub after: PathBuf,
    /// True only when a different existing value was replaced.
    pub replaced: bool,
}

/// Register one host manifest below the current user's selected browser root.
///
/// On Windows this writes the default `REG_SZ` value below
/// `NativeMessagingHosts/<host-name>` and reads it back before returning. Other
/// hosts return [`NativeMessagingRegistryErrorKind::Unsupported`].
pub fn register_current_user_host(
    target: &ChromiumRegistryTarget,
    host_name: &str,
    manifest_path: &Path,
) -> Result<NativeMessagingRegistryReceipt, NativeMessagingRegistryError> {
    validate_host_name(host_name)?;
    validate_manifest(manifest_path)?;
    selected::register(target, &target.host_key(host_name), manifest_path)
}

fn validate_product_key(product_key: &str) -> Result<(), NativeMessagingRegistryError> {
    let invalid = product_key.is_empty()
        || product_key.encode_utf16().count() > MAX_REGISTRY_ROOT_UTF16
        || product_key.contains('/')
        || product_key.chars().any(char::is_control)
        || !product_key.starts_with("Software\\")
        || product_key.ends_with('\\')
        || product_key.split('\\').count() < 2
        || product_key
            .split('\\')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        || product_key
            .split('\\')
            .any(|segment| segment.eq_ignore_ascii_case("NativeMessagingHosts"));
    if invalid {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::InvalidTarget,
            None,
            "target must be an explicit Software\\<product...> current-user product key",
        ));
    }
    Ok(())
}

fn validate_host_name(host_name: &str) -> Result<(), NativeMessagingRegistryError> {
    let mut bytes = host_name.bytes();
    let first = bytes.next();
    let invalid = host_name.len() > MAX_HOST_NAME_BYTES
        || !matches!(first, Some(b'a'..=b'z' | b'0'..=b'9' | b'_'))
        || bytes.any(|byte| !matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.'))
        || host_name.ends_with('.')
        || host_name.contains("..");
    if invalid {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::InvalidHostName,
            None,
            "host name must use Chromium's lowercase dotted identifier grammar",
        ));
    }
    Ok(())
}

fn validate_manifest(path: &Path) -> Result<(), NativeMessagingRegistryError> {
    if !path.is_absolute()
        || path.as_os_str().is_empty()
        || path.as_os_str().to_string_lossy().contains('\0')
    {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::InvalidManifestPath,
            None,
            "manifest path must be absolute",
        ));
    }
    if path.as_os_str().to_string_lossy().encode_utf16().count() > MAX_MANIFEST_PATH_UTF16 {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::InvalidManifestPath,
            None,
            "manifest path exceeds the bounded native path limit",
        ));
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        NativeMessagingRegistryError::new(
            if error.kind() == std::io::ErrorKind::NotFound {
                NativeMessagingRegistryErrorKind::ManifestNotFound
            } else {
                NativeMessagingRegistryErrorKind::InvalidManifestPath
            },
            error
                .raw_os_error()
                .and_then(|code| u32::try_from(code).ok()),
            "manifest metadata could not be read",
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::ManifestIsSymlink,
            None,
            "manifest must not be a symbolic link",
        ));
    }
    if !metadata.is_file() {
        return Err(NativeMessagingRegistryError::new(
            NativeMessagingRegistryErrorKind::ManifestNotFile,
            None,
            "manifest must be a regular file",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_requires_explicit_product_root() {
        assert!(ChromiumRegistryTarget::new("Software\\Google\\Chrome").is_ok());
        assert!(ChromiumRegistryTarget::new("Software\\Chromium").is_ok());
        for invalid in [
            "Google\\Chrome",
            "Software",
            "Software\\Google\\Chrome\\",
            "Software\\Google\\..\\Chrome",
            "Software\\Google\\Chrome/Dev",
            "Software\\Google\\Chrome\\NativeMessagingHosts",
        ] {
            assert_eq!(
                ChromiumRegistryTarget::new(invalid).unwrap_err().kind,
                NativeMessagingRegistryErrorKind::InvalidTarget
            );
        }
    }

    #[test]
    fn host_name_uses_closed_chromium_grammar() {
        for valid in ["com.example.bridge", "org_1.bridge", "_local.bridge"] {
            assert!(validate_host_name(valid).is_ok());
        }
        for invalid in [
            "",
            "Com.example",
            ".com.example",
            "com..example",
            "com-example",
        ] {
            assert_eq!(
                validate_host_name(invalid).unwrap_err().kind,
                NativeMessagingRegistryErrorKind::InvalidHostName
            );
        }
    }

    #[test]
    fn host_key_is_exact_and_current_user_relative() {
        let target = ChromiumRegistryTarget::new("Software\\Vendor\\Browser").unwrap();
        assert_eq!(
            target.host_key("com.example.bridge"),
            "Software\\Vendor\\Browser\\NativeMessagingHosts\\com.example.bridge"
        );
    }

    #[test]
    fn manifest_rejects_relative_and_non_file_paths() {
        assert_eq!(
            validate_manifest(Path::new("relative.json"))
                .unwrap_err()
                .kind,
            NativeMessagingRegistryErrorKind::InvalidManifestPath
        );
        let directory = std::env::temp_dir();
        assert_eq!(
            validate_manifest(&directory).unwrap_err().kind,
            NativeMessagingRegistryErrorKind::ManifestNotFile
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_registration_is_typed_unsupported() {
        let root = std::env::temp_dir().join(format!(
            "agenterm-platform-native-message-unsupported-{}",
            std::process::id()
        ));
        std::fs::write(&root, b"{}").unwrap();
        let target = ChromiumRegistryTarget::new("Software\\Vendor\\Browser").unwrap();
        let result = register_current_user_host(&target, "com.example.bridge", &root);
        std::fs::remove_file(root).unwrap();
        assert_eq!(
            result.unwrap_err().kind,
            NativeMessagingRegistryErrorKind::Unsupported
        );
    }

    #[cfg(unix)]
    #[test]
    fn manifest_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let unique = format!(
            "agenterm-platform-native-message-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir(&root).unwrap();
        let manifest = root.join("manifest.json");
        let link = root.join("link.json");
        std::fs::write(&manifest, b"{}").unwrap();
        symlink(&manifest, &link).unwrap();
        assert_eq!(
            validate_manifest(&link).unwrap_err().kind,
            NativeMessagingRegistryErrorKind::ManifestIsSymlink
        );
        std::fs::remove_file(link).unwrap();
        std::fs::remove_file(manifest).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
