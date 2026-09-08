//! Platform-neutral application metadata facts.
//!
//! Facts distinguish a missing value from an unrequested, inapplicable,
//! unsupported, or temporarily unavailable observation.  Only `Present`
//! carries a value; every other status carries a reason.

use std::borrow::Cow;

/// Longest selector accepted by the native application-facts lookup.
pub const MAX_APP_FACTS_SELECTOR_BYTES: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "kebab-case")
)]
pub enum FactStatus {
    Present,
    Absent,
    NotRequested,
    NotApplicable,
    Unsupported,
    Unavailable,
}

/// One typed fact. Constructors preserve the value/reason invariant.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Fact<T> {
    pub status: FactStatus,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub value: Option<T>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub reason: Option<Cow<'static, str>>,
}

impl<T> Fact<T> {
    #[must_use]
    pub fn present(value: T) -> Self {
        Self {
            status: FactStatus::Present,
            value: Some(value),
            reason: None,
        }
    }

    #[must_use]
    pub fn absent(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::without_value(FactStatus::Absent, reason)
    }

    #[must_use]
    pub fn not_requested(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::without_value(FactStatus::NotRequested, reason)
    }

    #[must_use]
    pub fn not_applicable(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::without_value(FactStatus::NotApplicable, reason)
    }

    #[must_use]
    pub fn unsupported(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::without_value(FactStatus::Unsupported, reason)
    }

    #[must_use]
    pub fn unavailable(reason: impl Into<Cow<'static, str>>) -> Self {
        Self::without_value(FactStatus::Unavailable, reason)
    }

    fn without_value(status: FactStatus, reason: impl Into<Cow<'static, str>>) -> Self {
        let reason = reason.into();
        debug_assert!(!reason.is_empty());
        Self {
            status,
            value: None,
            reason: Some(reason),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AppFactsOptions {
    pub signing: bool,
    pub verify: bool,
    pub entitlements: bool,
}

/// Canonical application facts returned on every supported host.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct AppFacts {
    pub schema_version: u32,
    pub platform: String,
    pub selector: String,
    pub desktop_entry_id: Fact<String>,
    pub name: Fact<String>,
    pub bundle: Fact<String>,
    pub path: Fact<String>,
    pub version: Fact<String>,
    pub executable: Fact<String>,
    pub running: Fact<bool>,
    pub signature: Fact<String>,
    pub signature_verified: Fact<bool>,
    pub entitlements: Fact<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppFactsErrorKind {
    InvalidInput,
    NotFound,
    Ambiguous,
    ScanTruncated,
    IdentityDrift,
    Unsupported,
    Io,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppFactsError {
    kind: AppFactsErrorKind,
    code: Cow<'static, str>,
    message: String,
}

impl AppFactsError {
    #[must_use]
    pub fn new(
        kind: AppFactsErrorKind,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code: code.into(),
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> AppFactsErrorKind {
        self.kind
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for AppFactsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppFactsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_keep_value_and_reason_structurally_distinct() {
        let present = Fact::present("value".to_owned());
        assert_eq!(present.status, FactStatus::Present);
        assert_eq!(present.value.as_deref(), Some("value"));
        assert_eq!(present.reason, None);

        for fact in [
            Fact::<String>::absent("missing"),
            Fact::not_requested("not-requested"),
            Fact::not_applicable("not-applicable"),
            Fact::unsupported("unsupported"),
            Fact::unavailable("unavailable"),
        ] {
            assert!(fact.value.is_none());
            assert!(
                fact.reason
                    .as_deref()
                    .is_some_and(|reason| !reason.is_empty())
            );
        }
    }
}
