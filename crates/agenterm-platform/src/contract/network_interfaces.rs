//! Product-neutral network-interface address inventory.

use std::net::IpAddr;

/// Maximum address rows accepted by one public inventory request.
pub const NETWORK_INTERFACE_MAX_ROWS: usize = 5_000;
/// Maximum native adapter/address records examined by one snapshot.
pub const NETWORK_INTERFACE_SCAN_CEILING: usize = 10_000;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NetworkAddressFamily {
    Ipv4,
    Ipv6,
}

impl NetworkAddressFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4 => "IPv4",
            Self::Ipv6 => "IPv6",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum NetworkInterfaceNativeIdKind {
    IfIndex,
    AdapterLuid,
}

impl NetworkInterfaceNativeIdKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IfIndex => "ifindex",
            Self::AdapterLuid => "adapter-luid",
        }
    }
}

/// One IP address assigned to one native interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkInterfaceAddress {
    pub name: String,
    pub family: NetworkAddressFamily,
    pub address: IpAddr,
    pub netmask: Option<IpAddr>,
    pub cidr: Option<u8>,
    pub mac: Option<Vec<u8>>,
    pub internal: bool,
    pub scope_id: Option<u32>,
    pub native_id: u64,
    pub native_id_kind: NetworkInterfaceNativeIdKind,
}

/// Bounded, stably sorted native address inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkInterfaceInventory {
    pub interfaces: Vec<NetworkInterfaceAddress>,
    /// IP address rows found before the caller's row limit was applied.
    pub visited: usize,
    /// Native records that could not be represented without guessing.
    pub read_errors: usize,
    /// True when the native record ceiling stopped enumeration.
    pub truncated_scan: bool,
    pub provider: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkInterfaceErrorKind {
    InvalidLimit,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkInterfaceError {
    kind: NetworkInterfaceErrorKind,
    detail: String,
}

impl NetworkInterfaceError {
    pub(crate) fn new(kind: NetworkInterfaceErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }

    pub const fn kind(&self) -> NetworkInterfaceErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for NetworkInterfaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "network interfaces {:?}: {}",
            self.kind, self.detail
        )
    }
}

impl std::error::Error for NetworkInterfaceError {}
