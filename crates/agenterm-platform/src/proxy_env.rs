//! Selected-host proxy environment facts for the current process.

pub use crate::contract::proxy_env::ProxyEnvFacts;

/// Read the three canonical proxy variables from this process environment.
///
/// Names are matched exactly: `http_proxy`, `HTTPS_PROXY`, and `no_proxy`.
/// Empty values are reported as absent so callers can distinguish unset from
/// explicitly cleared.
pub fn current_process_facts() -> ProxyEnvFacts {
    ProxyEnvFacts {
        http_proxy: read_exact("http_proxy"),
        https_proxy: read_exact("HTTPS_PROXY"),
        no_proxy: read_exact("no_proxy"),
    }
}

fn read_exact(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_values_are_absent() {
        let facts = current_process_facts();
        for value in [facts.http_proxy, facts.https_proxy, facts.no_proxy] {
            assert!(value.is_none() || !value.unwrap().is_empty());
        }
    }

    #[test]
    fn empty_string_values_are_absent() {
        assert_eq!(read_exact("AGENTERM_PROXY_ENV_EMPTY_TEST_VAR"), None);
    }
}
