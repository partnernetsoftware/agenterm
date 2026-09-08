//! Process proxy environment facts for bounded runtime observation.

/// Canonical proxy variables reported by `capabilities`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyEnvFacts {
    pub http_proxy: Option<String>,
    pub https_proxy: Option<String>,
    pub no_proxy: Option<String>,
}

impl ProxyEnvFacts {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            http_proxy: None,
            https_proxy: None,
            no_proxy: None,
        }
    }
}
