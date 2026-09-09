//! Selected-host application-facts facade.

pub use crate::contract::app_facts::{
    AppFacts, AppFactsError, AppFactsErrorKind, AppFactsOptions, Fact, FactStatus,
    MAX_APP_FACTS_SELECTOR_BYTES,
};

/// Resolve one exact application and return requested metadata facts.
///
/// Linux uses bounded XDG desktop-entry discovery and direct procfs reads.
/// macOS uses bounded bundle discovery plus CoreFoundation and Security facts.
/// Windows uses bounded Uninstall registration and native executable facts
/// when the `app-facts` feature is enabled. Other hosts remain typed
/// unsupported until their native adapter lands.
pub fn query(selector: &str, options: AppFactsOptions) -> Result<AppFacts, AppFactsError> {
    if selector.is_empty()
        || selector.len() > MAX_APP_FACTS_SELECTOR_BYTES
        || selector.as_bytes().contains(&0)
    {
        return Err(AppFactsError::new(
            AppFactsErrorKind::InvalidInput,
            "app_facts_invalid_selector",
            format!(
                "application selector must contain 1..={MAX_APP_FACTS_SELECTOR_BYTES} non-NUL UTF-8 bytes"
            ),
        ));
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        all(target_os = "windows", feature = "app-facts")
    ))]
    {
        crate::selected::app_facts::query(selector, options)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        all(target_os = "windows", feature = "app-facts")
    )))]
    {
        let _ = options;
        Err(AppFactsError::new(
            AppFactsErrorKind::Unsupported,
            "app_facts_platform_unsupported",
            "application facts are unavailable because this host has no selected native adapter",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_selector_is_rejected_before_platform_dispatch() {
        let error = query("", AppFactsOptions::default()).unwrap_err();
        assert_eq!(error.kind(), AppFactsErrorKind::InvalidInput);
        assert_eq!(error.code(), "app_facts_invalid_selector");
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        all(target_os = "windows", feature = "app-facts")
    )))]
    #[test]
    fn unimplemented_hosts_fail_typed() {
        let error = query("example", AppFactsOptions::default()).unwrap_err();
        assert_eq!(error.kind(), AppFactsErrorKind::Unsupported);
    }
}
