//! Windows `GetIpForwardTable2` route inventory.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceLuidToAlias, FreeMibTable, GetIpForwardTable2, MIB_IPFORWARD_ROW2,
    MIB_IPFORWARD_TABLE2,
};
use windows_sys::Win32::NetworkManagement::Ndis::IF_MAX_STRING_SIZE;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_INET};

use crate::contract::network_routes::{
    NETWORK_ROUTE_SCAN_CEILING, NetworkRoute, NetworkRouteError, NetworkRouteErrorKind,
    NetworkRouteFamily, NetworkRouteInventory, NetworkRouteNativeIdKind,
};

struct ForwardTable(*mut MIB_IPFORWARD_TABLE2);

impl Drop for ForwardTable {
    fn drop(&mut self) {
        // SAFETY: GetIpForwardTable2 allocated this table and ownership has not
        // been transferred. FreeMibTable accepts null, though success is non-null.
        unsafe { FreeMibTable(self.0.cast()) };
    }
}

pub(crate) fn enumerate_native() -> Result<NetworkRouteInventory, NetworkRouteError> {
    let mut pointer = std::ptr::null_mut();
    // SAFETY: pointer is a valid out parameter and successful ownership is
    // immediately transferred to ForwardTable.
    let status = unsafe { GetIpForwardTable2(AF_UNSPEC, &mut pointer) };
    if status != ERROR_SUCCESS {
        return Err(unavailable(format!(
            "GetIpForwardTable2 failed with Windows status {status}"
        )));
    }
    if pointer.is_null() {
        return Err(unavailable("GetIpForwardTable2 returned a null table"));
    }
    let table = ForwardTable(pointer);
    // SAFETY: successful GetIpForwardTable2 returned a live table.
    let count = unsafe { (*table.0).NumEntries as usize };
    let scanned = count.min(NETWORK_ROUTE_SCAN_CEILING);
    // SAFETY: the variable-sized Table member contains NumEntries contiguous rows.
    let rows = unsafe { std::slice::from_raw_parts((*table.0).Table.as_ptr(), scanned) };
    let mut routes = Vec::with_capacity(scanned);
    let mut read_errors = 0usize;
    for row in rows {
        match parse_row(row) {
            Some(route) => routes.push(route),
            None => read_errors += 1,
        }
    }
    Ok(NetworkRouteInventory {
        routes,
        visited: scanned,
        read_errors,
        truncated_scan: count > NETWORK_ROUTE_SCAN_CEILING,
        provider: "GetIpForwardTable2",
    })
}

fn parse_row(row: &MIB_IPFORWARD_ROW2) -> Option<NetworkRoute> {
    let (family, destination) = socket_address(&row.DestinationPrefix.Prefix)?;
    let max_prefix = if family == NetworkRouteFamily::Ipv4 {
        32
    } else {
        128
    };
    if row.DestinationPrefix.PrefixLength > max_prefix {
        return None;
    }
    let gateway = socket_address(&row.NextHop)
        .filter(|(gateway_family, _)| *gateway_family == family)
        .map(|(_, address)| address)
        .filter(|address| !address.is_unspecified());
    // SAFETY: NET_LUID_LH exposes its lossless u64 representation.
    let interface_native_id = unsafe { row.InterfaceLuid.Value };
    if interface_native_id == 0 {
        return None;
    }
    Some(NetworkRoute {
        family,
        destination,
        prefix_length: row.DestinationPrefix.PrefixLength,
        gateway,
        interface: interface_alias(row),
        interface_native_id,
        interface_native_id_kind: NetworkRouteNativeIdKind::AdapterLuid,
        flags: windows_flags(row),
        native_flags: None,
        table: None,
        metric: Some(row.Metric),
        protocol: Some(row.Protocol as u32),
        scope: None,
        route_type: None,
    })
}

fn socket_address(socket: &SOCKADDR_INET) -> Option<(NetworkRouteFamily, IpAddr)> {
    // SAFETY: the union family tag selects the active socket layout.
    unsafe {
        match socket.si_family {
            AF_INET => {
                let bytes = socket.Ipv4.sin_addr.S_un.S_un_b;
                Some((
                    NetworkRouteFamily::Ipv4,
                    IpAddr::V4(Ipv4Addr::new(
                        bytes.s_b1, bytes.s_b2, bytes.s_b3, bytes.s_b4,
                    )),
                ))
            }
            AF_INET6 => Some((
                NetworkRouteFamily::Ipv6,
                IpAddr::V6(Ipv6Addr::from(socket.Ipv6.sin6_addr.u.Byte)),
            )),
            _ => None,
        }
    }
}

fn interface_alias(row: &MIB_IPFORWARD_ROW2) -> Option<String> {
    let mut buffer = vec![0u16; IF_MAX_STRING_SIZE as usize + 1];
    // SAFETY: buffer is writable for the supplied number of UTF-16 units and
    // the row's LUID remains live for the duration of the call.
    let status = unsafe {
        ConvertInterfaceLuidToAlias(&row.InterfaceLuid, buffer.as_mut_ptr(), buffer.len())
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let length = buffer.iter().position(|unit| *unit == 0)?;
    Some(String::from_utf16_lossy(&buffer[..length]))
}

fn windows_flags(row: &MIB_IPFORWARD_ROW2) -> Vec<&'static str> {
    [
        (row.Loopback, "loopback"),
        (row.AutoconfigureAddress, "autoconfigure-address"),
        (row.Publish, "publish"),
        (row.Immortal, "immortal"),
    ]
    .into_iter()
    .filter_map(|(set, name)| set.then_some(name))
    .collect()
}

fn unavailable(detail: impl Into<String>) -> NetworkRouteError {
    NetworkRouteError::new(NetworkRouteErrorKind::Unavailable, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_next_hop_is_an_explicit_on_link_route() {
        let mut row = MIB_IPFORWARD_ROW2::default();
        row.DestinationPrefix.PrefixLength = 0;
        row.DestinationPrefix.Prefix.Ipv4.sin_family = AF_INET;
        row.NextHop.Ipv4.sin_family = AF_INET;
        row.InterfaceLuid.Value = 7;
        let parsed = parse_row(&row).unwrap();
        assert_eq!(parsed.destination, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(parsed.gateway, None);
        assert_eq!(parsed.interface_native_id, 7);
        assert_eq!(
            parsed.interface_native_id_kind,
            NetworkRouteNativeIdKind::AdapterLuid
        );
    }
}
