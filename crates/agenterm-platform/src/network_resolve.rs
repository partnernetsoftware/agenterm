//! Bounded system DNS resolution without product-level probe policy.

use std::{
    collections::BTreeSet,
    fmt,
    net::{SocketAddr, ToSocketAddrs},
};

pub const MAX_RESOLVED_ADDRESSES: usize = 256;

#[derive(Debug)]
pub enum NetworkResolveError {
    InvalidHost,
    Resolve(std::io::Error),
    Empty,
    TooMany,
}

impl fmt::Display for NetworkResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHost => f.write_str("host must be non-empty UTF-8 without NUL"),
            Self::Resolve(error) => write!(f, "system resolver failed: {error}"),
            Self::Empty => f.write_str("system resolver returned no addresses"),
            Self::TooMany => write!(
                f,
                "system resolver returned more than {MAX_RESOLVED_ADDRESSES} unique addresses"
            ),
        }
    }
}

pub fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, NetworkResolveError> {
    if host.is_empty() || host.as_bytes().contains(&0) {
        return Err(NetworkResolveError::InvalidHost);
    }
    let mut unique = BTreeSet::new();
    for address in (host, port)
        .to_socket_addrs()
        .map_err(NetworkResolveError::Resolve)?
    {
        unique.insert(address);
        if unique.len() > MAX_RESOLVED_ADDRESSES {
            return Err(NetworkResolveError::TooMany);
        }
    }
    if unique.is_empty() {
        return Err(NetworkResolveError::Empty);
    }
    Ok(unique.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_loopback_is_deterministic() {
        assert_eq!(
            resolve("127.0.0.1", 443).unwrap(),
            ["127.0.0.1:443".parse().unwrap()]
        );
        assert!(matches!(
            resolve("", 443),
            Err(NetworkResolveError::InvalidHost)
        ));
    }
}
