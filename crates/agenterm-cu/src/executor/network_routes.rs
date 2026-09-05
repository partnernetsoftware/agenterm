//! Bounded native route-table inventory.

use agenterm_platform::network_routes::{NetworkRoute, NetworkRouteErrorKind};

use super::*;

const RESPONSE_CEILING_BYTES: usize = 1024 * 1024;
const RESPONSE_HEADROOM_BYTES: usize = 16 * 1024;

pub(super) fn network_routes_payload(max: usize) -> Result<serde_json::Value, CuError> {
    let inventory = agenterm_platform::network_routes::enumerate(max)
        .map_err(|error| CuError::new(error_code(error.kind()), error.detail()))?;
    let visited = inventory.visited;
    let mut rows = Vec::with_capacity(inventory.routes.len());
    let mut encoded_rows = 0usize;
    let row_budget = RESPONSE_CEILING_BYTES - RESPONSE_HEADROOM_BYTES;
    for route in inventory.routes {
        let row = row_value(route);
        let row_bytes = serde_json::to_vec(&row)
            .map_err(|error| CuError::new("network_routes_encode_failed", error.to_string()))?
            .len();
        if encoded_rows.saturating_add(row_bytes) > row_budget {
            break;
        }
        encoded_rows += row_bytes;
        rows.push(row);
    }
    let returned = rows.len();
    let representable = visited.saturating_sub(inventory.read_errors);
    let truncated = inventory.truncated_scan || returned < representable;
    let payload = serde_json::json!({
        "routes": rows,
        "visited": visited,
        "returned": returned,
        "truncated": truncated,
        "truncated_scan": inventory.truncated_scan,
        "read_errors": inventory.read_errors,
        "provider": inventory.provider,
        "response_ceiling_bytes": RESPONSE_CEILING_BYTES,
    });
    if serde_json::to_vec(&payload)
        .map_err(|error| CuError::new("network_routes_encode_failed", error.to_string()))?
        .len()
        > RESPONSE_CEILING_BYTES
    {
        return Err(CuError::new(
            "network_routes_response_too_large",
            "route inventory exceeded its 1 MiB response ceiling",
        ));
    }
    Ok(payload)
}

fn error_code(kind: NetworkRouteErrorKind) -> &'static str {
    match kind {
        NetworkRouteErrorKind::InvalidLimit => "network_routes_invalid_limit",
        NetworkRouteErrorKind::Unavailable => "network_routes_unavailable",
        NetworkRouteErrorKind::MalformedSnapshot => "network_routes_malformed_snapshot",
        NetworkRouteErrorKind::ResourceLimit => "network_routes_resource_limit",
    }
}

fn row_value(route: NetworkRoute) -> serde_json::Value {
    let mut unavailable_fields = Vec::new();
    if route.interface.is_none() {
        unavailable_fields.push("interface");
    }
    if route.native_flags.is_none() {
        unavailable_fields.push("native_flags");
    }
    if route.table.is_none() {
        unavailable_fields.push("table");
    }
    if route.metric.is_none() {
        unavailable_fields.push("metric");
    }
    if route.protocol.is_none() {
        unavailable_fields.push("protocol");
    }
    if route.scope.is_none() {
        unavailable_fields.push("scope");
    }
    if route.route_type.is_none() {
        unavailable_fields.push("route_type");
    }
    serde_json::json!({
        "family": route.family.as_str(),
        "destination": format!("{}/{}", route.destination, route.prefix_length),
        "gateway": route.gateway.map(|address| address.to_string()),
        "interface": route.interface,
        "native_id": route.interface_native_id.to_string(),
        "native_id_kind": route.interface_native_id_kind.as_str(),
        "flags": route.flags,
        "native_flags": route.native_flags.map(|flags| flags.to_string()),
        "table": route.table,
        "metric": route.metric,
        "protocol": route.protocol,
        "scope": route.scope,
        "route_type": route.route_type,
        "unavailable_fields": unavailable_fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenterm_platform::network_routes::NetworkRouteFamily;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn row_normalizes_destination_and_names_unavailable_native_fields() {
        let row = row_value(NetworkRoute {
            family: NetworkRouteFamily::Ipv4,
            destination: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            prefix_length: 0,
            gateway: None,
            interface: Some("loop".into()),
            interface_native_id: 7,
            interface_native_id_kind:
                agenterm_platform::network_routes::NetworkRouteNativeIdKind::IfIndex,
            flags: vec!["up"],
            native_flags: Some(1),
            table: None,
            metric: None,
            protocol: None,
            scope: None,
            route_type: None,
        });
        assert_eq!(row["destination"], "0.0.0.0/0");
        assert!(row["gateway"].is_null());
        assert_eq!(row["native_flags"], "1");
        assert_eq!(row["native_id"], "7");
        assert_eq!(row["native_id_kind"], "ifindex");
        assert_eq!(
            row["unavailable_fields"],
            serde_json::json!(["table", "metric", "protocol", "scope", "route_type"])
        );
    }

    #[test]
    fn public_payload_is_bounded_and_counted() {
        let payload = network_routes_payload(1).expect("native route payload");
        assert!(payload["returned"].as_u64().unwrap() <= 1);
        assert_eq!(
            payload["routes"].as_array().unwrap().len() as u64,
            payload["returned"].as_u64().unwrap()
        );
        assert!(serde_json::to_vec(&payload).unwrap().len() <= RESPONSE_CEILING_BYTES);
    }

    #[test]
    fn native_failure_kinds_have_distinct_public_codes() {
        assert_eq!(
            error_code(NetworkRouteErrorKind::Unavailable),
            "network_routes_unavailable"
        );
        assert_eq!(
            error_code(NetworkRouteErrorKind::MalformedSnapshot),
            "network_routes_malformed_snapshot"
        );
        assert_eq!(
            error_code(NetworkRouteErrorKind::ResourceLimit),
            "network_routes_resource_limit"
        );
    }
}
