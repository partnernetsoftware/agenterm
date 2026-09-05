use std::path::Path;

use super::{ChromiumRegistryTarget, NativeMessagingRegistryError, NativeMessagingRegistryReceipt};

#[cfg(all(windows, feature = "native-messaging-registry"))]
#[path = "../adapters/windows/native_messaging.rs"]
mod platform;

#[cfg(all(windows, feature = "native-messaging-registry"))]
pub(super) fn register(
    target: &ChromiumRegistryTarget,
    host_key: &str,
    manifest_path: &Path,
) -> Result<NativeMessagingRegistryReceipt, NativeMessagingRegistryError> {
    platform::register(target, host_key, manifest_path)
}

#[cfg(not(all(windows, feature = "native-messaging-registry")))]
pub(super) fn register(
    _target: &ChromiumRegistryTarget,
    _host_key: &str,
    _manifest_path: &Path,
) -> Result<NativeMessagingRegistryReceipt, NativeMessagingRegistryError> {
    Err(NativeMessagingRegistryError::new(
        super::NativeMessagingRegistryErrorKind::Unsupported,
        None,
        "current-user native-messaging registration is unavailable on this build",
    ))
}
