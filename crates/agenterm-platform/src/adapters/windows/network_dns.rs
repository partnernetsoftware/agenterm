//! Windows adapter-scoped DNS inventory from `GetAdaptersAddresses`.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, ERROR_SUCCESS},
    NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GAA_FLAG_SKIP_UNICAST,
        GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH, IP_ADAPTER_DNS_SUFFIX,
    },
    Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6},
};

use crate::contract::network_dns::{
    NETWORK_DNS_SCAN_CEILING, NETWORK_DNS_TEXT_CEILING, NetworkDnsCoverage, NetworkDnsError,
    NetworkDnsErrorKind, NetworkDnsFamily, NetworkDnsInventory, NetworkDnsResolver,
    NetworkDnsSearchDomain,
};

const INITIAL_BUFFER_BYTES: u32 = 15_000;
const MAX_BUFFER_BYTES: u32 = 4 * 1024 * 1024;

pub(crate) fn enumerate_native() -> Result<NetworkDnsInventory, NetworkDnsError> {
    let mut buffer_bytes = INITIAL_BUFFER_BYTES;
    for _ in 0..3 {
        let words = (buffer_bytes as usize).div_ceil(std::mem::size_of::<u64>());
        let mut storage = vec![0u64; words];
        let adapters = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let mut required = buffer_bytes;
        // SAFETY: `storage` is aligned, writable for `buffer_bytes`, and
        // remains alive while the returned linked structures are traversed.
        let status = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC),
                GAA_FLAG_SKIP_UNICAST | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST,
                ptr::null_mut(),
                adapters,
                &mut required,
            )
        };
        if status == ERROR_NO_DATA {
            return Ok(empty_inventory());
        }
        if status == ERROR_BUFFER_OVERFLOW {
            if required == 0 || required > MAX_BUFFER_BYTES {
                return Err(unavailable(format!(
                    "native DNS buffer request {required} exceeds limit"
                )));
            }
            buffer_bytes = required;
            continue;
        }
        if status != ERROR_SUCCESS {
            return Err(unavailable(format!(
                "GetAdaptersAddresses failed with Windows status {status}"
            )));
        }
        return Ok(parse_adapters(adapters));
    }
    Err(unavailable(
        "native DNS inventory buffer changed repeatedly",
    ))
}

fn parse_adapters(head: *const IP_ADAPTER_ADDRESSES_LH) -> NetworkDnsInventory {
    let mut inventory = empty_inventory();
    let mut adapter = head;
    while !adapter.is_null() && !inventory.truncated_scan {
        // SAFETY: adapter pointers refer into the successful native buffer.
        let current = unsafe { &*adapter };
        let interface = wide_pointer(current.FriendlyName);
        // SAFETY: NET_LUID_LH exposes its lossless u64 representation.
        let luid = unsafe { current.Luid.Value };
        if interface.is_none() || luid == 0 {
            inventory.read_errors += 1;
        }
        let mut server = current.FirstDnsServerAddress;
        while !server.is_null() {
            if !reserve_row(&mut inventory) {
                break;
            }
            // SAFETY: DNS server nodes belong to the current adapter buffer.
            let current_server = unsafe { &*server };
            match socket_address(
                current_server.Address.lpSockaddr,
                current_server.Address.iSockaddrLength,
            ) {
                Some((family, address, port, scope_id)) => {
                    inventory.resolvers.push(NetworkDnsResolver {
                        family,
                        address,
                        port,
                        interface: interface.clone(),
                        interface_native_id: (luid != 0).then_some(luid),
                        scope_id,
                        service: interface.clone(),
                        resolver_index: None,
                    });
                }
                None => inventory.read_errors += 1,
            }
            server = current_server.Next;
        }
        add_domain(
            wide_pointer(current.DnsSuffix),
            &interface,
            luid,
            &mut inventory,
        );
        let mut suffix = current.FirstDnsSuffix;
        while !suffix.is_null() && !inventory.truncated_scan {
            // SAFETY: suffix nodes belong to the current adapter buffer.
            let current_suffix: &IP_ADAPTER_DNS_SUFFIX = unsafe { &*suffix };
            add_domain(
                wide_array(&current_suffix.String),
                &interface,
                luid,
                &mut inventory,
            );
            suffix = current_suffix.Next;
        }
        adapter = current.Next;
    }
    inventory
}

fn add_domain(
    domain: Option<String>,
    interface: &Option<String>,
    luid: u64,
    inventory: &mut NetworkDnsInventory,
) {
    let Some(domain) = domain.filter(|value| !value.is_empty()) else {
        return;
    };
    if !reserve_row(inventory) {
        return;
    }
    if !valid_domain(&domain) {
        inventory.read_errors += 1;
        return;
    }
    inventory.search_domains.push(NetworkDnsSearchDomain {
        domain: domain.trim_end_matches('.').to_owned(),
        interface: interface.clone(),
        interface_native_id: (luid != 0).then_some(luid),
        service: interface.clone(),
        resolver_index: None,
    });
}

fn reserve_row(inventory: &mut NetworkDnsInventory) -> bool {
    if inventory.visited == NETWORK_DNS_SCAN_CEILING {
        inventory.truncated_scan = true;
        return false;
    }
    inventory.visited += 1;
    true
}

fn socket_address(
    pointer: *const SOCKADDR,
    length: i32,
) -> Option<(NetworkDnsFamily, IpAddr, u16, Option<u32>)> {
    if pointer.is_null() || length < i32::try_from(std::mem::size_of::<u16>()).ok()? {
        return None;
    }
    // SAFETY: casts are guarded by the native family tag and struct length.
    unsafe {
        match (*pointer).sa_family {
            AF_INET if usize::try_from(length).ok()? >= std::mem::size_of::<SOCKADDR_IN>() => {
                let socket = &*pointer.cast::<SOCKADDR_IN>();
                let bytes = socket.sin_addr.S_un.S_un_b;
                Some((
                    NetworkDnsFamily::Ipv4,
                    IpAddr::V4(Ipv4Addr::new(
                        bytes.s_b1, bytes.s_b2, bytes.s_b3, bytes.s_b4,
                    )),
                    dns_port(socket.sin_port),
                    None,
                ))
            }
            AF_INET6 if usize::try_from(length).ok()? >= std::mem::size_of::<SOCKADDR_IN6>() => {
                let socket = &*pointer.cast::<SOCKADDR_IN6>();
                Some((
                    NetworkDnsFamily::Ipv6,
                    IpAddr::V6(Ipv6Addr::from(socket.sin6_addr.u.Byte)),
                    dns_port(socket.sin6_port),
                    (socket.Anonymous.sin6_scope_id != 0).then_some(socket.Anonymous.sin6_scope_id),
                ))
            }
            _ => None,
        }
    }
}

fn dns_port(network_order: u16) -> u16 {
    match u16::from_be(network_order) {
        0 => 53,
        port => port,
    }
}

fn wide_pointer(pointer: *const u16) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let mut length = 0usize;
    // SAFETY: native strings are NUL-terminated; the explicit ceiling keeps a
    // malformed provider pointer from causing an unbounded scan.
    unsafe {
        while length < NETWORK_DNS_TEXT_CEILING && *pointer.add(length) != 0 {
            length += 1;
        }
        if length == NETWORK_DNS_TEXT_CEILING {
            return None;
        }
        String::from_utf16(std::slice::from_raw_parts(pointer, length)).ok()
    }
}

fn wide_array(value: &[u16]) -> Option<String> {
    let length = value
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(value.len());
    String::from_utf16(&value[..length]).ok()
}

fn valid_domain(value: &str) -> bool {
    let value = value.trim_end_matches('.');
    !value.is_empty()
        && value.len() <= NETWORK_DNS_TEXT_CEILING.min(253)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn empty_inventory() -> NetworkDnsInventory {
    NetworkDnsInventory {
        resolvers: Vec::new(),
        search_domains: Vec::new(),
        visited: 0,
        read_errors: 0,
        truncated_scan: false,
        truncated: false,
        complete: false,
        provider: "GetAdaptersAddresses",
        coverage: NetworkDnsCoverage::SystemEffective,
    }
}

fn unavailable(detail: impl Into<String>) -> NetworkDnsError {
    NetworkDnsError::new(NetworkDnsErrorKind::Unavailable, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_validation_is_bounded() {
        assert!(valid_domain("example.test."));
        assert!(!valid_domain("bad/value"));
        assert!(!valid_domain(""));
    }
}
