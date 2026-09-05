//! Selected-host bounded native route-table inventory.

pub use crate::contract::network_routes::{
    NETWORK_ROUTE_MAX_ROWS, NETWORK_ROUTE_SCAN_CEILING, NetworkRoute, NetworkRouteError,
    NetworkRouteErrorKind, NetworkRouteFamily, NetworkRouteInventory, NetworkRouteNativeIdKind,
};

pub fn enumerate(max_rows: usize) -> Result<NetworkRouteInventory, NetworkRouteError> {
    if !(1..=NETWORK_ROUTE_MAX_ROWS).contains(&max_rows) {
        return Err(NetworkRouteError::new(
            NetworkRouteErrorKind::InvalidLimit,
            format!("max_rows must be between 1 and {NETWORK_ROUTE_MAX_ROWS}, got {max_rows}"),
        ));
    }
    let inventory = crate::selected::network_routes::enumerate_native()?;
    Ok(finish_inventory(inventory, max_rows))
}

fn finish_inventory(
    mut inventory: NetworkRouteInventory,
    max_rows: usize,
) -> NetworkRouteInventory {
    for route in &mut inventory.routes {
        route.destination = network_address(route.destination, route.prefix_length);
        route.flags.sort_unstable();
    }
    inventory.routes.sort_by(compare_rows);
    inventory.routes.truncate(max_rows);
    inventory
}

fn network_address(address: std::net::IpAddr, prefix: u8) -> std::net::IpAddr {
    match address {
        std::net::IpAddr::V4(address) => {
            let mask = u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0);
            std::net::IpAddr::V4(std::net::Ipv4Addr::from(u32::from(address) & mask))
        }
        std::net::IpAddr::V6(address) => {
            let mask = u128::MAX.checked_shl(u32::from(128 - prefix)).unwrap_or(0);
            std::net::IpAddr::V6(std::net::Ipv6Addr::from(u128::from(address) & mask))
        }
    }
}

fn compare_rows(left: &NetworkRoute, right: &NetworkRoute) -> std::cmp::Ordering {
    left.family
        .cmp(&right.family)
        .then_with(|| ip_sort_key(left.destination).cmp(&ip_sort_key(right.destination)))
        .then_with(|| left.prefix_length.cmp(&right.prefix_length))
        .then_with(|| left.table.cmp(&right.table))
        .then_with(|| left.metric.cmp(&right.metric))
        .then_with(|| left.interface_native_id.cmp(&right.interface_native_id))
        .then_with(|| {
            left.gateway
                .map(ip_sort_key)
                .cmp(&right.gateway.map(ip_sort_key))
        })
        .then_with(|| left.native_flags.cmp(&right.native_flags))
        .then_with(|| {
            left.interface_native_id_kind
                .cmp(&right.interface_native_id_kind)
        })
        .then_with(|| left.interface.cmp(&right.interface))
        .then_with(|| left.flags.cmp(&right.flags))
        .then_with(|| left.protocol.cmp(&right.protocol))
        .then_with(|| left.scope.cmp(&right.scope))
        .then_with(|| left.route_type.cmp(&right.route_type))
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

    fn row(
        destination: &str,
        table: Option<u32>,
        metric: Option<u32>,
        native_id: u64,
    ) -> NetworkRoute {
        NetworkRoute {
            family: NetworkRouteFamily::Ipv4,
            destination: destination.parse().unwrap(),
            prefix_length: 24,
            gateway: None,
            interface: None,
            interface_native_id: native_id,
            interface_native_id_kind: NetworkRouteNativeIdKind::IfIndex,
            flags: Vec::new(),
            native_flags: Some(0),
            table,
            metric,
            protocol: None,
            scope: None,
            route_type: None,
        }
    }

    #[test]
    fn rejects_limits_before_native_enumeration() {
        assert_eq!(
            enumerate(0).unwrap_err().kind(),
            NetworkRouteErrorKind::InvalidLimit
        );
        assert_eq!(
            enumerate(NETWORK_ROUTE_MAX_ROWS + 1).unwrap_err().kind(),
            NetworkRouteErrorKind::InvalidLimit
        );
    }

    #[test]
    fn native_inventory_is_bounded_and_prefixes_match_families() {
        let inventory = enumerate(1).expect("native route inventory");
        assert!(inventory.routes.len() <= 1);
        assert!(inventory.visited <= NETWORK_ROUTE_SCAN_CEILING);
        for route in inventory.routes {
            assert!(matches!(
                (route.family, route.destination),
                (NetworkRouteFamily::Ipv4, std::net::IpAddr::V4(_))
                    | (NetworkRouteFamily::Ipv6, std::net::IpAddr::V6(_))
            ));
            assert!(
                route.prefix_length
                    <= if route.family == NetworkRouteFamily::Ipv4 {
                        32
                    } else {
                        128
                    }
            );
        }
    }

    #[test]
    fn destination_normalization_clears_host_bits() {
        assert_eq!(
            network_address("10.2.3.99".parse().unwrap(), 24).to_string(),
            "10.2.3.0"
        );
        assert_eq!(
            network_address("2001:db8::abcd".parse().unwrap(), 64).to_string(),
            "2001:db8::"
        );
    }

    #[test]
    fn finish_preserves_native_visit_count_and_uses_frozen_sort_key() {
        let inventory = NetworkRouteInventory {
            routes: vec![
                row("10.0.0.8", Some(2), Some(0), 1),
                row("10.0.0.8", Some(1), Some(9), 8),
                row("10.0.0.8", Some(1), Some(9), 7),
            ],
            visited: 11,
            read_errors: 8,
            truncated_scan: false,
            provider: "test",
        };
        let finished = finish_inventory(inventory, 2);
        assert_eq!(finished.visited, 11);
        assert_eq!(finished.routes.len(), 2);
        assert_eq!(finished.routes[0].table, Some(1));
        assert_eq!(finished.routes[0].interface_native_id, 7);
        assert_eq!(finished.routes[1].interface_native_id, 8);
    }

    #[test]
    fn remaining_public_fields_are_deterministic_tie_breakers() {
        let mut left = row("10.0.0.0", Some(1), Some(2), 3);
        left.interface = Some("z".into());
        left.flags = vec!["up", "gateway"];
        left.protocol = Some(9);
        let mut right = left.clone();
        right.interface = Some("a".into());
        right.flags = vec!["up"];
        right.protocol = Some(1);
        let inventory = NetworkRouteInventory {
            routes: vec![left, right],
            visited: 2,
            read_errors: 0,
            truncated_scan: false,
            provider: "test",
        };
        let finished = finish_inventory(inventory, 2);
        assert_eq!(finished.routes[0].interface.as_deref(), Some("a"));
        assert_eq!(finished.routes[1].flags, vec!["gateway", "up"]);
    }
}
