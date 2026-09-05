//! Product-neutral native route-table inventory.

use std::net::IpAddr;

pub const NETWORK_ROUTE_MAX_ROWS: usize = 5_000;
pub const NETWORK_ROUTE_SCAN_CEILING: usize = 10_000;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NetworkRouteFamily {
    Ipv4,
    Ipv6,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NetworkRouteNativeIdKind {
    IfIndex,
    AdapterLuid,
}

impl NetworkRouteNativeIdKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IfIndex => "ifindex",
            Self::AdapterLuid => "adapter-luid",
        }
    }
}

impl NetworkRouteFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4 => "IPv4",
            Self::Ipv6 => "IPv6",
        }
    }
}

/// One kernel route. Numeric native metadata is retained instead of being
/// projected onto another operating system's vocabulary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkRoute {
    pub family: NetworkRouteFamily,
    pub destination: IpAddr,
    pub prefix_length: u8,
    pub gateway: Option<IpAddr>,
    pub interface: Option<String>,
    pub interface_native_id: u64,
    pub interface_native_id_kind: NetworkRouteNativeIdKind,
    pub flags: Vec<&'static str>,
    pub native_flags: Option<u64>,
    pub table: Option<u32>,
    pub metric: Option<u32>,
    pub protocol: Option<u32>,
    pub scope: Option<u32>,
    pub route_type: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkRouteInventory {
    pub routes: Vec<NetworkRoute>,
    pub visited: usize,
    pub read_errors: usize,
    pub truncated_scan: bool,
    pub provider: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkRouteErrorKind {
    InvalidLimit,
    Unavailable,
    MalformedSnapshot,
    ResourceLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkRouteError {
    kind: NetworkRouteErrorKind,
    detail: String,
}

impl NetworkRouteError {
    pub(crate) fn new(kind: NetworkRouteErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> NetworkRouteErrorKind {
        self.kind
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for NetworkRouteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "network routes {:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for NetworkRouteError {}
