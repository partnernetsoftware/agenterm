//! `GetAdaptersAddresses` network-interface inventory for Windows.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, ERROR_SUCCESS};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_PREFIX, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
    GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IF_TYPE_SOFTWARE_LOOPBACK,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
};

use crate::contract::network_interfaces::{
    NETWORK_INTERFACE_SCAN_CEILING, NetworkAddressFamily, NetworkInterfaceAddress,
    NetworkInterfaceError, NetworkInterfaceErrorKind, NetworkInterfaceInventory,
    NetworkInterfaceNativeIdKind,
};

const INITIAL_BUFFER_BYTES: u32 = 15_000;
const MAX_BUFFER_BYTES: u32 = 4 * 1024 * 1024;
const MAX_NAME_UNITS: usize = 1_024;

pub(crate) fn enumerate_native() -> Result<NetworkInterfaceInventory, NetworkInterfaceError> {
    let mut buffer_bytes = INITIAL_BUFFER_BYTES;
    for _ in 0..3 {
        let words = (buffer_bytes as usize).div_ceil(std::mem::size_of::<u64>());
        let mut storage = vec![0u64; words];
        let adapters = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        let mut required = buffer_bytes;
        // SAFETY: `storage` is aligned, writable for `buffer_bytes`, and remains
        // alive while all returned pointers are traversed.
        let status = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC),
                GAA_FLAG_SKIP_ANYCAST
                    | GAA_FLAG_SKIP_MULTICAST
                    | GAA_FLAG_SKIP_DNS_SERVER
                    | GAA_FLAG_INCLUDE_PREFIX,
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
                return Err(NetworkInterfaceError::new(
                    NetworkInterfaceErrorKind::Unavailable,
                    format!("native inventory buffer request {required} exceeds limit"),
                ));
            }
            buffer_bytes = required;
            continue;
        }
        if status != ERROR_SUCCESS {
            return Err(NetworkInterfaceError::new(
                NetworkInterfaceErrorKind::Unavailable,
                format!("GetAdaptersAddresses failed with Windows status {status}"),
            ));
        }
        return Ok(parse_adapters(adapters));
    }
    Err(NetworkInterfaceError::new(
        NetworkInterfaceErrorKind::Unavailable,
        "native inventory buffer changed repeatedly",
    ))
}

fn parse_adapters(head: *const IP_ADAPTER_ADDRESSES_LH) -> NetworkInterfaceInventory {
    let mut interfaces = Vec::new();
    let mut read_errors = 0usize;
    let mut scanned = 0usize;
    let mut truncated_scan = false;
    let mut adapter = head;
    while !adapter.is_null() {
        if scanned == NETWORK_INTERFACE_SCAN_CEILING {
            truncated_scan = true;
            break;
        }
        scanned += 1;
        // SAFETY: adapter pointers refer into the successful native buffer and
        // are only traversed while that buffer remains alive.
        let current = unsafe { &*adapter };
        let Some(name) = wide_string(current.FriendlyName) else {
            read_errors += 1;
            adapter = current.Next;
            continue;
        };
        // SAFETY: NET_LUID_LH exposes its lossless u64 representation.
        let luid = unsafe { current.Luid.Value };
        if luid == 0 {
            read_errors += 1;
            adapter = current.Next;
            continue;
        }
        let physical_length = current.PhysicalAddressLength as usize;
        let mac = if physical_length > current.PhysicalAddress.len() {
            read_errors += 1;
            None
        } else {
            (physical_length != 0).then(|| current.PhysicalAddress[..physical_length].to_vec())
        };
        let internal = current.IfType == IF_TYPE_SOFTWARE_LOOPBACK;
        let mut unicast = current.FirstUnicastAddress;
        while !unicast.is_null() {
            if scanned == NETWORK_INTERFACE_SCAN_CEILING {
                truncated_scan = true;
                break;
            }
            scanned += 1;
            // SAFETY: the unicast node belongs to the current native adapter.
            let address = unsafe { &*unicast };
            if let Some((family, ip, scope_id)) =
                socket_address(address.Address.lpSockaddr, address.Address.iSockaddrLength)
            {
                let cidr = Some(address.OnLinkPrefixLength).filter(|prefix| match family {
                    NetworkAddressFamily::Ipv4 => *prefix <= 32,
                    NetworkAddressFamily::Ipv6 => *prefix <= 128,
                });
                interfaces.push(NetworkInterfaceAddress {
                    name: name.clone(),
                    family,
                    address: ip,
                    netmask: cidr.map(|prefix| netmask(family, prefix)),
                    cidr,
                    mac: mac.clone(),
                    internal: internal || ip.is_loopback(),
                    scope_id,
                    native_id: luid,
                    native_id_kind: NetworkInterfaceNativeIdKind::AdapterLuid,
                });
            } else {
                read_errors += 1;
            }
            unicast = address.Next;
        }
        if truncated_scan {
            break;
        }
        adapter = current.Next;
    }
    NetworkInterfaceInventory {
        interfaces,
        visited: 0,
        read_errors,
        truncated_scan,
        provider: "GetAdaptersAddresses",
    }
}

fn empty_inventory() -> NetworkInterfaceInventory {
    NetworkInterfaceInventory {
        interfaces: Vec::new(),
        visited: 0,
        read_errors: 0,
        truncated_scan: false,
        provider: "GetAdaptersAddresses",
    }
}

fn wide_string(pointer: *const u16) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let mut length = 0usize;
    // SAFETY: FriendlyName is a native NUL-terminated string. The explicit
    // ceiling prevents an unbounded scan if native data is malformed.
    unsafe {
        while length < MAX_NAME_UNITS && *pointer.add(length) != 0 {
            length += 1;
        }
        if length == MAX_NAME_UNITS {
            return None;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(
            pointer, length,
        )))
    }
}

fn socket_address(
    pointer: *const windows_sys::Win32::Networking::WinSock::SOCKADDR,
    length: i32,
) -> Option<(NetworkAddressFamily, IpAddr, Option<u32>)> {
    if pointer.is_null() || length < i32::try_from(std::mem::size_of::<u16>()).ok()? {
        return None;
    }
    // SAFETY: casts are guarded by the native family tag and minimum struct size.
    unsafe {
        match (*pointer).sa_family {
            AF_INET if usize::try_from(length).ok()? >= std::mem::size_of::<SOCKADDR_IN>() => {
                let socket = &*pointer.cast::<SOCKADDR_IN>();
                let bytes = socket.sin_addr.S_un.S_un_b;
                Some((
                    NetworkAddressFamily::Ipv4,
                    IpAddr::V4(Ipv4Addr::new(
                        bytes.s_b1, bytes.s_b2, bytes.s_b3, bytes.s_b4,
                    )),
                    None,
                ))
            }
            AF_INET6 if usize::try_from(length).ok()? >= std::mem::size_of::<SOCKADDR_IN6>() => {
                let socket = &*pointer.cast::<SOCKADDR_IN6>();
                let ip = Ipv6Addr::from(socket.sin6_addr.u.Byte);
                let scope_id = socket.Anonymous.sin6_scope_id;
                Some((
                    NetworkAddressFamily::Ipv6,
                    IpAddr::V6(ip),
                    (scope_id != 0).then_some(scope_id),
                ))
            }
            _ => None,
        }
    }
}

fn netmask(family: NetworkAddressFamily, prefix: u8) -> IpAddr {
    match family {
        NetworkAddressFamily::Ipv4 => {
            let bits = u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0);
            IpAddr::V4(Ipv4Addr::from(bits.to_be_bytes()))
        }
        NetworkAddressFamily::Ipv6 => {
            let bits = u128::MAX.checked_shl(u32::from(128 - prefix)).unwrap_or(0);
            IpAddr::V6(Ipv6Addr::from(bits.to_be_bytes()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netmask_handles_zero_and_full_prefixes() {
        assert_eq!(
            netmask(NetworkAddressFamily::Ipv4, 0),
            "0.0.0.0".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            netmask(NetworkAddressFamily::Ipv4, 32),
            "255.255.255.255".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            netmask(NetworkAddressFamily::Ipv6, 64),
            "ffff:ffff:ffff:ffff::".parse::<IpAddr>().unwrap()
        );
    }
}
