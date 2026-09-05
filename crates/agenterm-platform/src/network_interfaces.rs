//! Selected-host bounded network-interface inventory.

pub use crate::contract::network_interfaces::{
    NETWORK_INTERFACE_MAX_ROWS, NETWORK_INTERFACE_SCAN_CEILING, NetworkAddressFamily,
    NetworkInterfaceAddress, NetworkInterfaceError, NetworkInterfaceErrorKind,
    NetworkInterfaceInventory, NetworkInterfaceNativeIdKind,
};

/// Enumerate native interface addresses, sort them deterministically, and
/// retain at most `max_rows` entries. An empty host inventory is successful.
pub fn enumerate(max_rows: usize) -> Result<NetworkInterfaceInventory, NetworkInterfaceError> {
    if !(1..=NETWORK_INTERFACE_MAX_ROWS).contains(&max_rows) {
        return Err(NetworkInterfaceError::new(
            NetworkInterfaceErrorKind::InvalidLimit,
            format!("max_rows must be between 1 and {NETWORK_INTERFACE_MAX_ROWS}, got {max_rows}"),
        ));
    }
    let mut inventory = crate::selected::network_interfaces::enumerate_native()?;
    inventory.interfaces.sort_by(compare_rows);
    inventory.visited = inventory.interfaces.len();
    inventory.interfaces.truncate(max_rows);
    Ok(inventory)
}

fn compare_rows(
    left: &NetworkInterfaceAddress,
    right: &NetworkInterfaceAddress,
) -> std::cmp::Ordering {
    left.name
        .cmp(&right.name)
        .then_with(|| left.family.cmp(&right.family))
        .then_with(|| ip_sort_key(left.address).cmp(&ip_sort_key(right.address)))
        .then_with(|| left.scope_id.cmp(&right.scope_id))
        .then_with(|| left.native_id.cmp(&right.native_id))
}

fn ip_sort_key(address: std::net::IpAddr) -> [u8; 16] {
    match address {
        std::net::IpAddr::V4(address) => {
            let mut bytes = [0; 16];
            bytes[..4].copy_from_slice(&address.octets());
            bytes
        }
        std::net::IpAddr::V6(address) => address.octets(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, address: &str, native_id: u64) -> NetworkInterfaceAddress {
        let address = address.parse::<std::net::IpAddr>().unwrap();
        NetworkInterfaceAddress {
            name: name.to_owned(),
            family: if address.is_ipv4() {
                NetworkAddressFamily::Ipv4
            } else {
                NetworkAddressFamily::Ipv6
            },
            address,
            netmask: None,
            cidr: None,
            mac: None,
            internal: false,
            scope_id: None,
            native_id,
            native_id_kind: NetworkInterfaceNativeIdKind::IfIndex,
        }
    }

    #[test]
    fn rejects_limits_before_native_enumeration() {
        assert_eq!(
            enumerate(0).unwrap_err().kind(),
            NetworkInterfaceErrorKind::InvalidLimit
        );
        assert_eq!(
            enumerate(NETWORK_INTERFACE_MAX_ROWS + 1)
                .unwrap_err()
                .kind(),
            NetworkInterfaceErrorKind::InvalidLimit
        );
    }

    #[test]
    fn native_inventory_is_bounded_and_has_consistent_address_families() {
        let inventory = enumerate(1).expect("native interface inventory");
        assert!(inventory.interfaces.len() <= 1);
        assert!(inventory.visited <= NETWORK_INTERFACE_SCAN_CEILING);
        for row in inventory.interfaces {
            assert_ne!(row.native_id, 0);
            assert!(matches!(
                (row.family, row.address),
                (NetworkAddressFamily::Ipv4, std::net::IpAddr::V4(_))
                    | (NetworkAddressFamily::Ipv6, std::net::IpAddr::V6(_))
            ));
            assert!(row.cidr.is_none_or(|cidr| match row.family {
                NetworkAddressFamily::Ipv4 => cidr <= 32,
                NetworkAddressFamily::Ipv6 => cidr <= 128,
            }));
        }
    }

    #[test]
    fn row_order_is_name_family_address_scope_then_native_id() {
        let mut rows = [
            row("z", "127.0.0.1", 1),
            row("a", "::1", 1),
            row("a", "10.0.0.2", 3),
            row("a", "10.0.0.1", 2),
        ];
        rows.sort_by(compare_rows);
        assert_eq!(
            rows.iter().map(|row| row.address).collect::<Vec<_>>(),
            vec![
                "10.0.0.1".parse::<std::net::IpAddr>().unwrap(),
                "10.0.0.2".parse::<std::net::IpAddr>().unwrap(),
                "::1".parse::<std::net::IpAddr>().unwrap(),
                "127.0.0.1".parse::<std::net::IpAddr>().unwrap(),
            ]
        );
    }
}
