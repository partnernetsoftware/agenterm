//! Product host routing policy shared by product modules.
//!
//! Native platform identity comes from agenterm-platform; this table turns
//! that fact into the product-level host labels used by paths, fonts, IPC and
//! Script runtime policy.

pub(crate) fn is_windows_host() -> bool {
    matches!(
        agenterm_platform::platform_kind(),
        agenterm_platform::PlatformKind::Windows
    )
}

pub(crate) fn is_macos_host() -> bool {
    matches!(
        agenterm_platform::platform_kind(),
        agenterm_platform::PlatformKind::Macos
    )
}

pub(crate) fn is_unix_host() -> bool {
    matches!(
        agenterm_platform::platform_kind(),
        agenterm_platform::PlatformKind::Linux | agenterm_platform::PlatformKind::Macos
    )
}

pub(crate) fn headless_composer_height() -> i32 {
    if is_windows_host() { 104 } else { 64 }
}

#[cfg(test)]
mod tests {
    use super::{is_macos_host, is_unix_host, is_windows_host};

    #[test]
    fn host_predicates_match_runtime_kind() {
        let kind = agenterm_platform::platform_kind();
        assert_eq!(
            is_windows_host(),
            matches!(kind, agenterm_platform::PlatformKind::Windows)
        );
        assert_eq!(
            is_macos_host(),
            matches!(kind, agenterm_platform::PlatformKind::Macos)
        );
        assert_eq!(
            is_unix_host(),
            matches!(
                kind,
                agenterm_platform::PlatformKind::Linux | agenterm_platform::PlatformKind::Macos
            )
        );
    }
}
