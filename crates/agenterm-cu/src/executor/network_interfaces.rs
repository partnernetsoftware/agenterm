//! Bounded native network-interface inventory.

use agenterm_platform::network_interfaces::{NetworkInterfaceAddress, NetworkInterfaceErrorKind};

use super::*;

const RESPONSE_CEILING_BYTES: usize = 1024 * 1024;
const RESPONSE_HEADROOM_BYTES: usize = 4096;

pub(super) fn network_interfaces_payload(max: usize) -> Result<serde_json::Value, CuError> {
    let inventory = agenterm_platform::network_interfaces::enumerate(max).map_err(|error| {
        let code = match error.kind() {
            NetworkInterfaceErrorKind::InvalidLimit => "network_interfaces_invalid_limit",
            NetworkInterfaceErrorKind::Unavailable => "network_interfaces_unavailable",
        };
        CuError::new(code, error.detail())
    })?;

    let visited = inventory.visited;
    let mut rows = Vec::with_capacity(inventory.interfaces.len());
    let mut encoded_rows = 0usize;
    let row_budget = RESPONSE_CEILING_BYTES - RESPONSE_HEADROOM_BYTES;
    for interface in inventory.interfaces {
        let row = row_value(interface);
        let row_bytes = serde_json::to_vec(&row)
            .map_err(|error| CuError::new("network_interfaces_encode_failed", error.to_string()))?
            .len();
        if encoded_rows.saturating_add(row_bytes) > row_budget {
            break;
        }
        encoded_rows += row_bytes;
        rows.push(row);
    }
    let returned = rows.len();
    let truncated = inventory.truncated_scan || returned < visited;
    Ok(serde_json::json!({
        "interfaces": rows,
        "visited": visited,
        "returned": returned,
        "truncated": truncated,
        "truncated_scan": inventory.truncated_scan,
        "read_errors": inventory.read_errors,
        "provider": inventory.provider,
        "response_ceiling_bytes": RESPONSE_CEILING_BYTES,
    }))
}

fn row_value(interface: NetworkInterfaceAddress) -> serde_json::Value {
    let mut unavailable_fields = Vec::new();
    if interface.netmask.is_none() {
        unavailable_fields.push("netmask");
    }
    if interface.cidr.is_none() {
        unavailable_fields.push("cidr");
    }
    if interface.mac.is_none() {
        unavailable_fields.push("mac");
    }
    let mac = interface.mac.as_deref().map(format_mac);
    serde_json::json!({
        "name": interface.name,
        "family": interface.family.as_str(),
        "address": interface.address.to_string(),
        "netmask": interface.netmask.map(|address| address.to_string()),
        "cidr": interface.cidr,
        "mac": mac,
        "internal": interface.internal,
        "scopeid": interface.scope_id,
        "native_id": interface.native_id.to_string(),
        "native_id_kind": interface.native_id_kind.as_str(),
        "unavailable_fields": unavailable_fields,
    })
}

fn format_mac(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenterm_platform::network_interfaces::{
        NetworkAddressFamily, NetworkInterfaceNativeIdKind,
    };
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn row_keeps_native_identity_and_names_unavailable_fields() {
        let row = row_value(NetworkInterfaceAddress {
            name: "loop".into(),
            family: NetworkAddressFamily::Ipv4,
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            netmask: None,
            cidr: None,
            mac: Some(vec![0, 1, 2, 0xfe, 0xff]),
            internal: true,
            scope_id: None,
            native_id: 7,
            native_id_kind: NetworkInterfaceNativeIdKind::IfIndex,
        });
        assert_eq!(row["native_id"], "7");
        assert_eq!(row["native_id_kind"], "ifindex");
        assert_eq!(row["mac"], "00:01:02:fe:ff");
        assert_eq!(
            row["unavailable_fields"],
            serde_json::json!(["netmask", "cidr"])
        );
    }

    #[test]
    fn public_payload_is_bounded_and_counted() {
        let payload = network_interfaces_payload(1).expect("native interface payload");
        assert!(payload["returned"].as_u64().unwrap() <= 1);
        assert_eq!(
            payload["interfaces"].as_array().unwrap().len() as u64,
            payload["returned"].as_u64().unwrap()
        );
        assert!(serde_json::to_vec(&payload).unwrap().len() <= RESPONSE_CEILING_BYTES);
    }
}
