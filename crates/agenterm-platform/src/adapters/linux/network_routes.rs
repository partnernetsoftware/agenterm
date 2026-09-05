//! Linux `NETLINK_ROUTE` / `RTM_GETROUTE` inventory.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use crate::contract::network_routes::{
    NETWORK_ROUTE_SCAN_CEILING, NetworkRoute, NetworkRouteError, NetworkRouteErrorKind,
    NetworkRouteFamily, NetworkRouteInventory, NetworkRouteNativeIdKind,
};

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;
const NLM_F_REQUEST: u16 = 1;
const NLM_F_DUMP_INTR: u16 = 0x10;
const NLM_F_DUMP: u16 = 0x300;
const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PRIORITY: u16 = 6;
const RTA_MULTIPATH: u16 = 9;
const RTA_TABLE: u16 = 15;
const RECEIVE_BYTES: usize = 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy)]
struct Header {
    length: u32,
    message_type: u16,
    flags: u16,
    sequence: u32,
    port_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RouteMessage {
    family: u8,
    destination_length: u8,
    source_length: u8,
    tos: u8,
    table: u8,
    protocol: u8,
    scope: u8,
    route_type: u8,
    flags: u32,
}

#[repr(C)]
struct Request {
    header: Header,
    route: RouteMessage,
}

pub(crate) fn enumerate_native() -> Result<NetworkRouteInventory, NetworkRouteError> {
    // SAFETY: socket returns a new descriptor, immediately transferred to OwnedFd.
    let raw = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if raw < 0 {
        return Err(unavailable(std::io::Error::last_os_error().to_string()));
    }
    // SAFETY: `raw` is uniquely owned after a successful socket call.
    let socket = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: zero is the required initialization for sockaddr_nl padding.
    let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    address.nl_family = libc::AF_NETLINK as u16;
    // SAFETY: address is a valid initialized sockaddr_nl.
    if unsafe {
        libc::bind(
            socket.as_raw_fd(),
            (&raw const address).cast::<libc::sockaddr>(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    } != 0
    {
        return Err(unavailable(std::io::Error::last_os_error().to_string()));
    }

    let request = Request {
        header: Header {
            length: std::mem::size_of::<Request>() as u32,
            message_type: RTM_GETROUTE,
            flags: NLM_F_REQUEST | NLM_F_DUMP,
            sequence: 1,
            port_id: 0,
        },
        route: RouteMessage {
            family: libc::AF_UNSPEC as u8,
            destination_length: 0,
            source_length: 0,
            tos: 0,
            table: 0,
            protocol: 0,
            scope: 0,
            route_type: 0,
            flags: 0,
        },
    };
    // SAFETY: request and kernel destination are initialized for sendto.
    let sent = unsafe {
        libc::sendto(
            socket.as_raw_fd(),
            (&raw const request).cast(),
            std::mem::size_of::<Request>(),
            0,
            (&raw const address).cast::<libc::sockaddr>(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if sent != std::mem::size_of::<Request>() as isize {
        return Err(unavailable(std::io::Error::last_os_error().to_string()));
    }

    let mut inventory = NetworkRouteInventory {
        routes: Vec::new(),
        visited: 0,
        read_errors: 0,
        truncated_scan: false,
        provider: "NETLINK_ROUTE/RTM_GETROUTE",
    };
    let mut buffer = vec![0u8; RECEIVE_BYTES];
    loop {
        let mut sender: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        let mut sender_length = std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t;
        // SAFETY: buffer and sender are writable for their supplied lengths.
        let received = unsafe {
            libc::recvfrom(
                socket.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                libc::MSG_TRUNC,
                (&raw mut sender).cast::<libc::sockaddr>(),
                &mut sender_length,
            )
        };
        if received < 0 {
            return Err(unavailable(std::io::Error::last_os_error().to_string()));
        }
        if received == 0 {
            return Err(malformed("route dump ended before NLMSG_DONE"));
        }
        if received as usize > buffer.len() {
            return Err(resource_limit(format!(
                "native route datagram {} exceeds {RECEIVE_BYTES}-byte ceiling",
                received
            )));
        }
        if sender_length as usize != std::mem::size_of::<libc::sockaddr_nl>()
            || sender.nl_family != libc::AF_NETLINK as u16
            || sender.nl_pid != 0
        {
            continue;
        }
        if parse_datagram(&buffer[..received as usize], &mut inventory)? {
            return Ok(inventory);
        }
        if inventory.truncated_scan {
            return Ok(inventory);
        }
    }
}

fn parse_datagram(
    bytes: &[u8],
    inventory: &mut NetworkRouteInventory,
) -> Result<bool, NetworkRouteError> {
    let mut offset = 0usize;
    while offset + std::mem::size_of::<Header>() <= bytes.len() {
        let header: Header =
            read_unaligned(&bytes[offset..]).ok_or_else(|| malformed("short netlink header"))?;
        let length = header.length as usize;
        if length < std::mem::size_of::<Header>() || offset + length > bytes.len() {
            return Err(malformed("invalid netlink message length"));
        }
        if header.sequence != 1 {
            let next = offset.saturating_add(align4(length));
            if next <= offset || next > bytes.len() {
                return Err(malformed("netlink message is missing alignment padding"));
            }
            offset = next;
            continue;
        }
        let body = &bytes[offset + std::mem::size_of::<Header>()..offset + length];
        match header.message_type {
            NLMSG_DONE => {
                if header.flags & NLM_F_DUMP_INTR != 0 {
                    return Err(unavailable("RTM_GETROUTE dump was interrupted"));
                }
                return Ok(true);
            }
            NLMSG_ERROR => {
                let code = read_i32(body).ok_or_else(|| malformed("short netlink error"))?;
                if code != 0 {
                    return Err(unavailable(format!(
                        "RTM_GETROUTE failed with errno {}",
                        code.unsigned_abs()
                    )));
                }
            }
            RTM_NEWROUTE => {
                if inventory.visited == NETWORK_ROUTE_SCAN_CEILING {
                    inventory.truncated_scan = true;
                    return Ok(false);
                }
                inventory.visited += 1;
                match parse_route(body) {
                    Some(route) => inventory.routes.push(route),
                    None => inventory.read_errors += 1,
                }
            }
            _ => {}
        }
        let next = offset.saturating_add(align4(length));
        if next <= offset {
            return Err(malformed("netlink message offset overflow"));
        }
        if next > bytes.len() {
            return Err(malformed("netlink message is missing alignment padding"));
        }
        offset = next;
    }
    if offset != bytes.len() && bytes[offset..].iter().any(|byte| *byte != 0) {
        return Err(malformed("trailing partial netlink message"));
    }
    Ok(false)
}

fn parse_route(body: &[u8]) -> Option<NetworkRoute> {
    let native: RouteMessage = read_unaligned(body)?;
    let family = match i32::from(native.family) {
        libc::AF_INET => NetworkRouteFamily::Ipv4,
        libc::AF_INET6 => NetworkRouteFamily::Ipv6,
        _ => return None,
    };
    let max_prefix = if family == NetworkRouteFamily::Ipv4 {
        32
    } else {
        128
    };
    if native.destination_length > max_prefix {
        return None;
    }
    let mut destination = None;
    let mut gateway = None;
    let mut interface_index = None;
    let mut metric = None;
    let mut table = (native.table != 0).then_some(u32::from(native.table));
    let mut attributes = &body[std::mem::size_of::<RouteMessage>()..];
    while attributes.len() >= 4 {
        let length = usize::from(u16::from_ne_bytes([attributes[0], attributes[1]]));
        let kind = u16::from_ne_bytes([attributes[2], attributes[3]]) & 0x3fff;
        if length < 4 || length > attributes.len() {
            return None;
        }
        let value = &attributes[4..length];
        match kind {
            RTA_DST => destination = parse_ip(family, value),
            RTA_GATEWAY => gateway = parse_ip(family, value),
            RTA_OIF => interface_index = read_u32(value),
            RTA_PRIORITY => metric = read_u32(value),
            RTA_TABLE => table = read_u32(value),
            RTA_MULTIPATH => return None,
            _ => {}
        }
        let consumed = align4(length);
        if consumed > attributes.len() {
            return None;
        }
        attributes = &attributes[consumed..];
    }
    let destination = destination.unwrap_or_else(|| unspecified(family));
    let interface_native_id = u64::from(interface_index.filter(|index| *index != 0)?);
    let interface = interface_name(interface_native_id as u32);
    Some(NetworkRoute {
        family,
        destination,
        prefix_length: native.destination_length,
        gateway,
        interface,
        interface_native_id,
        interface_native_id_kind: NetworkRouteNativeIdKind::IfIndex,
        flags: linux_flags(native.flags),
        native_flags: Some(u64::from(native.flags)),
        table,
        metric,
        protocol: Some(u32::from(native.protocol)),
        scope: Some(u32::from(native.scope)),
        route_type: Some(u32::from(native.route_type)),
    })
}

fn parse_ip(family: NetworkRouteFamily, bytes: &[u8]) -> Option<IpAddr> {
    match family {
        NetworkRouteFamily::Ipv4 if bytes.len() >= 4 => Some(IpAddr::V4(Ipv4Addr::new(
            bytes[0], bytes[1], bytes[2], bytes[3],
        ))),
        NetworkRouteFamily::Ipv6 if bytes.len() >= 16 => Some(IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(&bytes[..16]).ok()?,
        ))),
        _ => None,
    }
}

fn unspecified(family: NetworkRouteFamily) -> IpAddr {
    match family {
        NetworkRouteFamily::Ipv4 => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        NetworkRouteFamily::Ipv6 => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    }
}

fn interface_name(index: u32) -> Option<String> {
    if index == 0 {
        return None;
    }
    let mut name = [0 as libc::c_char; libc::IFNAMSIZ];
    // SAFETY: name is a writable IFNAMSIZ buffer.
    let result = unsafe { libc::if_indextoname(index, name.as_mut_ptr()) };
    if result.is_null() {
        return None;
    }
    // SAFETY: successful if_indextoname wrote a NUL-terminated name.
    Some(
        unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn linux_flags(flags: u32) -> Vec<&'static str> {
    [
        (0x100, "notify"),
        (0x200, "cloned"),
        (0x400, "equalize"),
        (0x800, "prefix"),
    ]
    .into_iter()
    .filter_map(|(bit, name)| (flags & bit != 0).then_some(name))
    .collect()
}

fn align4(value: usize) -> usize {
    value.saturating_add(3) & !3
}
fn read_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?))
}
fn read_i32(bytes: &[u8]) -> Option<i32> {
    Some(i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?))
}
fn read_unaligned<T: Copy>(bytes: &[u8]) -> Option<T> {
    (bytes.len() >= std::mem::size_of::<T>()).then(|| {
        // SAFETY: the preceding length check proves a readable T-sized region;
        // unaligned handles byte-buffer alignment and T is Copy.
        unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<T>()) }
    })
}
fn unavailable(detail: impl Into<String>) -> NetworkRouteError {
    NetworkRouteError::new(NetworkRouteErrorKind::Unavailable, detail)
}
fn malformed(detail: impl Into<String>) -> NetworkRouteError {
    NetworkRouteError::new(NetworkRouteErrorKind::MalformedSnapshot, detail)
}
fn resource_limit(detail: impl Into<String>) -> NetworkRouteError {
    NetworkRouteError::new(NetworkRouteErrorKind::ResourceLimit, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synthetic_ipv4_route_without_guessing_gateway() {
        let route = RouteMessage {
            family: libc::AF_INET as u8,
            destination_length: 24,
            source_length: 0,
            tos: 0,
            table: 254,
            protocol: 2,
            scope: 253,
            route_type: 1,
            flags: 0,
        };
        let mut bytes = vec![0u8; std::mem::size_of::<RouteMessage>()];
        // SAFETY: both regions are valid for exactly RouteMessage bytes and do not overlap.
        unsafe {
            std::ptr::copy_nonoverlapping(
                (&raw const route).cast::<u8>(),
                bytes.as_mut_ptr(),
                bytes.len(),
            )
        };
        bytes.extend_from_slice(&[8, 0, RTA_DST as u8, 0, 10, 2, 3, 0]);
        bytes.extend_from_slice(&[8, 0, RTA_PRIORITY as u8, 0, 7, 0, 0, 0]);
        bytes.extend_from_slice(&[8, 0, RTA_OIF as u8, 0, 9, 0, 0, 0]);
        let parsed = parse_route(&bytes).unwrap();
        assert_eq!(parsed.destination.to_string(), "10.2.3.0");
        assert_eq!(parsed.prefix_length, 24);
        assert_eq!(parsed.gateway, None);
        assert_eq!(parsed.metric, Some(7));
        assert_eq!(parsed.table, Some(254));
        assert_eq!(parsed.interface_native_id, 9);
    }

    #[test]
    fn multipath_route_is_rejected_for_read_error_accounting() {
        let route = RouteMessage {
            family: libc::AF_INET as u8,
            destination_length: 0,
            source_length: 0,
            tos: 0,
            table: 254,
            protocol: 2,
            scope: 0,
            route_type: 1,
            flags: 0,
        };
        let mut bytes = vec![0u8; std::mem::size_of::<RouteMessage>()];
        unsafe {
            std::ptr::copy_nonoverlapping(
                (&raw const route).cast::<u8>(),
                bytes.as_mut_ptr(),
                bytes.len(),
            )
        };
        bytes.extend_from_slice(&[8, 0, RTA_OIF as u8, 0, 7, 0, 0, 0]);
        bytes.extend_from_slice(&[4, 0, RTA_MULTIPATH as u8, 0]);
        let header = Header {
            length: (std::mem::size_of::<Header>() + bytes.len()) as u32,
            message_type: RTM_NEWROUTE,
            flags: 0,
            sequence: 1,
            port_id: 0,
        };
        let mut datagram = vec![0u8; std::mem::size_of::<Header>()];
        unsafe {
            std::ptr::copy_nonoverlapping(
                (&raw const header).cast::<u8>(),
                datagram.as_mut_ptr(),
                datagram.len(),
            )
        };
        datagram.extend_from_slice(&bytes);
        let mut inventory = NetworkRouteInventory {
            routes: Vec::new(),
            visited: 0,
            read_errors: 0,
            truncated_scan: false,
            provider: "test",
        };
        assert!(!parse_datagram(&datagram, &mut inventory).unwrap());
        assert_eq!(inventory.visited, 1);
        assert_eq!(inventory.read_errors, 1);
        assert!(inventory.routes.is_empty());
    }

    #[test]
    fn missing_alignment_padding_is_typed_malformed_not_a_slice_panic() {
        let header = Header {
            length: 17,
            message_type: 99,
            flags: 0,
            sequence: 1,
            port_id: 0,
        };
        let mut bytes = vec![0u8; 17];
        unsafe {
            std::ptr::copy_nonoverlapping(
                (&raw const header).cast::<u8>(),
                bytes.as_mut_ptr(),
                std::mem::size_of::<Header>(),
            )
        };
        let mut inventory = NetworkRouteInventory {
            routes: Vec::new(),
            visited: 0,
            read_errors: 0,
            truncated_scan: false,
            provider: "test",
        };
        assert_eq!(
            parse_datagram(&bytes, &mut inventory).unwrap_err().kind(),
            NetworkRouteErrorKind::MalformedSnapshot
        );
    }

    #[test]
    fn interrupted_dump_is_never_published_as_complete() {
        let header = Header {
            length: std::mem::size_of::<Header>() as u32,
            message_type: NLMSG_DONE,
            flags: NLM_F_DUMP_INTR,
            sequence: 1,
            port_id: 0,
        };
        let mut bytes = vec![0u8; std::mem::size_of::<Header>()];
        unsafe {
            std::ptr::copy_nonoverlapping(
                (&raw const header).cast::<u8>(),
                bytes.as_mut_ptr(),
                bytes.len(),
            )
        };
        let mut inventory = NetworkRouteInventory {
            routes: Vec::new(),
            visited: 0,
            read_errors: 0,
            truncated_scan: false,
            provider: "test",
        };
        assert_eq!(
            parse_datagram(&bytes, &mut inventory).unwrap_err().kind(),
            NetworkRouteErrorKind::Unavailable
        );
    }
}
