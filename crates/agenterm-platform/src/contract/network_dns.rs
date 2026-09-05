//! Product-neutral native DNS resolver inventory.

use std::net::IpAddr;

pub const NETWORK_DNS_MAX_ROWS: usize = 5_000;
pub const NETWORK_DNS_SCAN_CEILING: usize = 10_000;
pub const NETWORK_DNS_TEXT_CEILING: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NetworkDnsFamily {
    Ipv4,
    Ipv6,
}

impl NetworkDnsFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4 => "IPv4",
            Self::Ipv6 => "IPv6",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkDnsCoverage {
    SystemEffective,
    ResolverFile,
    StubOnly,
}

impl NetworkDnsCoverage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemEffective => "system-effective",
            Self::ResolverFile => "resolver-file",
            Self::StubOnly => "stub-only",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkDnsResolver {
    pub family: NetworkDnsFamily,
    pub address: IpAddr,
    pub port: u16,
    pub interface: Option<String>,
    pub interface_native_id: Option<u64>,
    pub scope_id: Option<u32>,
    pub service: Option<String>,
    pub resolver_index: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkDnsSearchDomain {
    pub domain: String,
    pub interface: Option<String>,
    pub interface_native_id: Option<u64>,
    pub service: Option<String>,
    pub resolver_index: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkDnsInventory {
    pub resolvers: Vec<NetworkDnsResolver>,
    pub search_domains: Vec<NetworkDnsSearchDomain>,
    pub visited: usize,
    pub read_errors: usize,
    pub truncated_scan: bool,
    pub truncated: bool,
    pub complete: bool,
    pub provider: &'static str,
    pub coverage: NetworkDnsCoverage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkDnsErrorKind {
    InvalidLimit,
    Unavailable,
    MalformedSnapshot,
    ResourceLimit,
    Timeout,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkDnsError {
    kind: NetworkDnsErrorKind,
    detail: String,
}

impl NetworkDnsError {
    pub(crate) fn new(kind: NetworkDnsErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> NetworkDnsErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for NetworkDnsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "network DNS {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for NetworkDnsError {}
