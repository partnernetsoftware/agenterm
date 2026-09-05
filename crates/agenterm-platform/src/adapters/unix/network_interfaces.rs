//! `getifaddrs` network-interface inventory for Linux and macOS.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ptr;

use crate::contract::network_interfaces::{
    NETWORK_INTERFACE_SCAN_CEILING, NetworkAddressFamily, NetworkInterfaceAddress,
    NetworkInterfaceError, NetworkInterfaceErrorKind, NetworkInterfaceInventory,
    NetworkInterfaceNativeIdKind,
};

struct IfAddrs(*mut libc::ifaddrs);

impl Drop for IfAddrs {
    fn drop(&mut self) {
        // SAFETY: getifaddrs initialized this list and this owner frees it once.
        unsafe { libc::freeifaddrs(self.0) };
    }
}

pub(crate) fn enumerate_native() -> Result<NetworkInterfaceInventory, NetworkInterfaceError> {
    let mut head = ptr::null_mut();
    // SAFETY: `head` is a valid out pointer and successful ownership is
    // transferred to `IfAddrs` immediately.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(NetworkInterfaceError::new(
            NetworkInterfaceErrorKind::Unavailable,
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let list = IfAddrs(head);
    let mut macs = HashMap::<String, Vec<u8>>::new();
    let mut interfaces = Vec::new();
    let mut current = list.0;
    let mut scanned = 0usize;
    let mut truncated_scan = false;
    let mut read_errors = 0usize;
    while !current.is_null() {
        if scanned == NETWORK_INTERFACE_SCAN_CEILING {
            truncated_scan = true;
            break;
        }
        scanned += 1;
        // SAFETY: each node belongs to the live `IfAddrs` list.
        let entry = unsafe { &*current };
        let Some(name) = interface_name(entry.ifa_name) else {
            read_errors += 1;
            current = entry.ifa_next;
            continue;
        };
        if let Some(mac) = link_address(entry.ifa_addr) {
            macs.entry(name).or_insert(mac);
            current = entry.ifa_next;
            continue;
        }
        let Some((family, address, scope_id)) = ip_address(entry.ifa_addr) else {
            current = entry.ifa_next;
            continue;
        };
        // SAFETY: the NUL-terminated name comes from the same live native list.
        let native_id = unsafe { libc::if_nametoindex(entry.ifa_name) };
        if native_id == 0 {
            read_errors += 1;
            current = entry.ifa_next;
            continue;
        }
        let netmask = ip_address(entry.ifa_netmask)
            .and_then(|(mask_family, mask, _)| (mask_family == family).then_some(mask));
        let cidr = netmask.and_then(prefix_length);
        interfaces.push(NetworkInterfaceAddress {
            name: name.clone(),
            family,
            address,
            netmask,
            cidr,
            // Filled after the single bounded traversal because a native link
            // record is not guaranteed to precede the interface's IP rows.
            mac: None,
            internal: entry.ifa_flags & libc::IFF_LOOPBACK as u32 != 0 || address.is_loopback(),
            scope_id,
            native_id: u64::from(native_id),
            native_id_kind: NetworkInterfaceNativeIdKind::IfIndex,
        });
        current = entry.ifa_next;
    }
    for row in &mut interfaces {
        row.mac = macs.get(&row.name).cloned();
    }

    Ok(NetworkInterfaceInventory {
        interfaces,
        visited: 0,
        read_errors,
        truncated_scan,
        provider: "getifaddrs",
    })
}

fn interface_name(pointer: *const libc::c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the pointer belongs to the live getifaddrs list. Interface names
    // are bounded by IFNAMSIZ; rejecting an unterminated field avoids an
    // unbounded native scan. Non-UTF-8 is projected lossily because the public
    // product schema is Unicode text rather than raw path identity.
    let length = unsafe { libc::strnlen(pointer, libc::IFNAMSIZ) };
    if length == libc::IFNAMSIZ {
        return None;
    }
    // SAFETY: `strnlen` proved that `length` bytes are readable before NUL.
    Some(
        String::from_utf8_lossy(unsafe {
            std::slice::from_raw_parts(pointer.cast::<u8>(), length)
        })
        .into_owned(),
    )
}

fn ip_address(
    pointer: *const libc::sockaddr,
) -> Option<(NetworkAddressFamily, IpAddr, Option<u32>)> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: family-specific casts follow the `sa_family` tag supplied by the kernel.
    unsafe {
        match i32::from((*pointer).sa_family) {
            libc::AF_INET => {
                let socket = &*pointer.cast::<libc::sockaddr_in>();
                let address = Ipv4Addr::from(socket.sin_addr.s_addr.to_ne_bytes());
                Some((NetworkAddressFamily::Ipv4, IpAddr::V4(address), None))
            }
            libc::AF_INET6 => {
                let socket = &*pointer.cast::<libc::sockaddr_in6>();
                let address = Ipv6Addr::from(socket.sin6_addr.s6_addr);
                let scope_id = (socket.sin6_scope_id != 0).then_some(socket.sin6_scope_id);
                Some((NetworkAddressFamily::Ipv6, IpAddr::V6(address), scope_id))
            }
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
fn link_address(pointer: *const libc::sockaddr) -> Option<Vec<u8>> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the AF_PACKET tag establishes the sockaddr_ll layout.
    unsafe {
        if i32::from((*pointer).sa_family) != libc::AF_PACKET {
            return None;
        }
        let socket = &*pointer.cast::<libc::sockaddr_ll>();
        let length = usize::from(socket.sll_halen).min(socket.sll_addr.len());
        (length != 0).then(|| socket.sll_addr[..length].to_vec())
    }
}

#[cfg(target_os = "macos")]
fn link_address(pointer: *const libc::sockaddr) -> Option<Vec<u8>> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the AF_LINK tag establishes sockaddr_dl. `sdl_nlen + sdl_alen`
    // is bounded by `sdl_len` before the variable tail is read.
    unsafe {
        if i32::from((*pointer).sa_family) != libc::AF_LINK {
            return None;
        }
        let socket = &*pointer.cast::<libc::sockaddr_dl>();
        let name_length = usize::from(socket.sdl_nlen);
        let address_length = usize::from(socket.sdl_alen);
        let fixed_offset = std::mem::offset_of!(libc::sockaddr_dl, sdl_data);
        let total = fixed_offset
            .checked_add(name_length)?
            .checked_add(address_length)?;
        if address_length == 0 || total > usize::from(socket.sdl_len) {
            return None;
        }
        let start = socket.sdl_data.as_ptr().cast::<u8>().add(name_length);
        Some(std::slice::from_raw_parts(start, address_length).to_vec())
    }
}

fn prefix_length(mask: IpAddr) -> Option<u8> {
    let bytes = match mask {
        IpAddr::V4(address) => address.octets().to_vec(),
        IpAddr::V6(address) => address.octets().to_vec(),
    };
    let mut prefix = 0u8;
    let mut zero_seen = false;
    for byte in bytes {
        for bit in (0..8).rev() {
            let set = byte & (1 << bit) != 0;
            if set && zero_seen {
                return None;
            }
            if set {
                prefix += 1;
            } else {
                zero_seen = true;
            }
        }
    }
    Some(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_length_rejects_non_contiguous_masks() {
        assert_eq!(prefix_length("255.255.240.0".parse().unwrap()), Some(20));
        assert_eq!(prefix_length("255.0.255.0".parse().unwrap()), None);
        assert_eq!(
            prefix_length("ffff:ffff:ffff:ffff::".parse().unwrap()),
            Some(64)
        );
    }
}
