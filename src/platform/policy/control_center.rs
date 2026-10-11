//! Product Control Center policy: how each host obtains screenshots.
//!
//! The host mechanism lives in minicon-platform; this table is the
//! AgenTerm-level product decision shared by frontend hosts.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScreenshotStrategy {
    DirectNativeWindow,
    RendererRequest,
    Unsupported,
}

pub(crate) fn screenshot_strategy() -> ScreenshotStrategy {
    match minicon_platform::platform_kind() {
        minicon_platform::PlatformKind::Windows => ScreenshotStrategy::DirectNativeWindow,
        minicon_platform::PlatformKind::Linux | minicon_platform::PlatformKind::Macos => {
            ScreenshotStrategy::RendererRequest
        }
        _ => ScreenshotStrategy::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_matches_runtime_kind() {
        assert_eq!(
            screenshot_strategy(),
            match minicon_platform::platform_kind() {
                minicon_platform::PlatformKind::Windows => ScreenshotStrategy::DirectNativeWindow,
                minicon_platform::PlatformKind::Linux | minicon_platform::PlatformKind::Macos => {
                    ScreenshotStrategy::RendererRequest
                }
                _ => ScreenshotStrategy::Unsupported,
            }
        );
    }
}
