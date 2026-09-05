//! macOS `sysctl` `PF_ROUTE` / `NET_RT_DUMP2` inventory.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::contract::network_routes::{
    NETWORK_ROUTE_SCAN_CEILING, NetworkRoute, NetworkRouteError, NetworkRouteErrorKind,
    NetworkRouteFamily, NetworkRouteInventory, NetworkRouteNativeIdKind,
};

const NET_RT_DUMP2: libc::c_int = 7;
const ROUTE_BUFFER_CEILING: usize = 16 * 1024 * 1024;
const RTAX_MAX: usize = 8;

pub(crate) fn enumerate_native() -> Result<NetworkRouteInventory, NetworkRouteError> {
    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, NET_RT_DUMP2, 0];
    let mut size = 0usize;
    // SAFETY: mib and size are valid for the documented sysctl sizing call.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(unavailable(std::io::Error::last_os_error().to_string()));
    }
    if size > ROUTE_BUFFER_CEILING {
        return Err(resource_limit(format!(
            "native route dump {size} exceeds {ROUTE_BUFFER_CEILING}-byte ceiling"
        )));
    }
    let mut buffer = vec![0u8; size];
    // The table can grow between sizing and retrieval. Retrying once is bounded
    // and any second growth fails closed rather than allocating without limit.
    for attempt in 0..2 {
        let mut actual = buffer.len();
        // SAFETY: buffer is writable for `actual` bytes; mib is initialized.
        let status = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                buffer.as_mut_ptr().cast(),
                &mut actual,
                std::ptr::null_mut(),
                0,
            )
        };
        if status == 0 {
            buffer.truncate(actual);
            return parse_dump(&buffer);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOMEM) && actual > ROUTE_BUFFER_CEILING {
            return Err(resource_limit(format!(
                "native route dump {actual} exceeds {ROUTE_BUFFER_CEILING}-byte ceiling"
            )));
        }
        if attempt == 0
            && error.raw_os_error() == Some(libc::ENOMEM)
            && actual <= ROUTE_BUFFER_CEILING
            && actual > buffer.len()
        {
            buffer.resize(actual, 0);
            continue;
        }
        return Err(unavailable(error.to_string()));
    }
    Err(unavailable("native route dump changed repeatedly"))
}

fn parse_dump(bytes: &[u8]) -> Result<NetworkRouteInventory, NetworkRouteError> {
    let mut inventory = NetworkRouteInventory {
        routes: Vec::new(),
        visited: 0,
        read_errors: 0,
        truncated_scan: false,
        provider: "sysctl/PF_ROUTE/NET_RT_DUMP2",
    };
    let mut offset = 0usize;
    while offset < bytes.len() {
        if inventory.visited == NETWORK_ROUTE_SCAN_CEILING {
            inventory.truncated_scan = true;
            break;
        }
        inventory.visited += 1;
        let header: libc::rt_msghdr2 =
            read_unaligned(&bytes[offset..]).ok_or_else(|| malformed("short rt_msghdr2"))?;
        let length = usize::from(header.rtm_msglen);
        if length < std::mem::size_of::<libc::rt_msghdr2>() || offset + length > bytes.len() {
            return Err(malformed("invalid rt_msghdr2 length"));
        }
        match parse_route(
            &header,
            &bytes[offset + std::mem::size_of::<libc::rt_msghdr2>()..offset + length],
        ) {
            Some(route) => inventory.routes.push(route),
            None => inventory.read_errors += 1,
        }
        offset += length;
    }
    Ok(inventory)
}

fn parse_route(header: &libc::rt_msghdr2, mut bytes: &[u8]) -> Option<NetworkRoute> {
    let mut addresses: [Option<&[u8]>; RTAX_MAX] = [None; RTAX_MAX];
    for (index, slot) in addresses.iter_mut().enumerate() {
        if header.rtm_addrs & (1 << index) == 0 {
            continue;
        }
        let length = usize::from(*bytes.first()?);
        let consumed = sockaddr_rounded_length(length);
        if consumed > bytes.len() {
            return None;
        }
        *slot = Some(&bytes[..length.min(bytes.len())]);
        bytes = &bytes[consumed..];
    }
    let destination_socket = addresses[0]?;
    let (family, destination) = sockaddr_ip(destination_socket, None)?;
    let gateway = addresses[1]
        .and_then(|socket| sockaddr_ip(socket, Some(family)))
        .map(|(_, ip)| ip);
    let prefix_length = if header.rtm_flags & libc::RTF_HOST != 0 {
        if family == NetworkRouteFamily::Ipv4 {
            32
        } else {
            128
        }
    } else {
        addresses[2].and_then(|mask| mask_prefix(mask, family))?
    };
    let interface_native_id = u64::from(header.rtm_index);
    if interface_native_id == 0 {
        return None;
    }
    Some(NetworkRoute {
        family,
        destination,
        prefix_length,
        gateway,
        interface: interface_name(interface_native_id as u32),
        interface_native_id,
        interface_native_id_kind: NetworkRouteNativeIdKind::IfIndex,
        flags: route_flags(header.rtm_flags),
        native_flags: Some(header.rtm_flags as u32 as u64),
        table: None,
        metric: None,
        protocol: None,
        scope: None,
        route_type: None,
    })
}

fn sockaddr_ip(
    bytes: &[u8],
    expected: Option<NetworkRouteFamily>,
) -> Option<(NetworkRouteFamily, IpAddr)> {
    let family = match *bytes.get(1)? as i32 {
        libc::AF_INET => NetworkRouteFamily::Ipv4,
        libc::AF_INET6 => NetworkRouteFamily::Ipv6,
        0 => expected?,
        _ => return None,
    };
    match family {
        NetworkRouteFamily::Ipv4 => {
            let mut octets = [0u8; 4];
            copy_available(&mut octets, bytes.get(4..).unwrap_or_default());
            Some((family, IpAddr::V4(Ipv4Addr::from(octets))))
        }
        NetworkRouteFamily::Ipv6 => {
            let mut octets = [0u8; 16];
            copy_available(&mut octets, bytes.get(8..).unwrap_or_default());
            Some((family, IpAddr::V6(Ipv6Addr::from(octets))))
        }
    }
}

fn mask_prefix(bytes: &[u8], family: NetworkRouteFamily) -> Option<u8> {
    let count = if family == NetworkRouteFamily::Ipv4 {
        4
    } else {
        16
    };
    let offset = if family == NetworkRouteFamily::Ipv4 {
        4
    } else {
        8
    };
    let mut mask = vec![0u8; count];
    copy_available(&mut mask, bytes.get(offset..).unwrap_or_default());
    let mut prefix = 0u8;
    let mut zero_seen = false;
    for byte in mask {
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

fn copy_available(destination: &mut [u8], source: &[u8]) {
    let count = destination.len().min(source.len());
    destination[..count].copy_from_slice(&source[..count]);
}

fn sockaddr_rounded_length(length: usize) -> usize {
    let word = std::mem::size_of::<libc::c_long>();
    if length == 0 {
        word
    } else {
        length.saturating_add(word - 1) & !(word - 1)
    }
}

fn interface_name(index: u32) -> Option<String> {
    let mut name = [0 as libc::c_char; libc::IFNAMSIZ];
    // SAFETY: name is a writable IFNAMSIZ buffer.
    let result = unsafe { libc::if_indextoname(index, name.as_mut_ptr()) };
    if result.is_null() {
        return None;
    }
    // SAFETY: successful if_indextoname wrote a NUL-terminated string.
    Some(
        unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn route_flags(flags: libc::c_int) -> Vec<&'static str> {
    [
        (libc::RTF_UP, "up"),
        (libc::RTF_GATEWAY, "gateway"),
        (libc::RTF_HOST, "host"),
        (libc::RTF_REJECT, "reject"),
        (libc::RTF_STATIC, "static"),
        (libc::RTF_BLACKHOLE, "blackhole"),
        (libc::RTF_CLONING, "cloning"),
        (libc::RTF_WASCLONED, "was-cloned"),
        (libc::RTF_IFSCOPE, "interface-scope"),
    ]
    .into_iter()
    .filter_map(|(bit, name)| (flags & bit != 0).then_some(name))
    .collect()
}

fn read_unaligned<T: Copy>(bytes: &[u8]) -> Option<T> {
    (bytes.len() >= std::mem::size_of::<T>()).then(|| {
        // SAFETY: the length check proves a readable T-sized byte region.
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
    fn compact_masks_are_zero_extended_and_must_be_contiguous() {
        assert_eq!(
            mask_prefix(&[7, 0, 0, 0, 255, 255, 240], NetworkRouteFamily::Ipv4),
            Some(20)
        );
        assert_eq!(
            mask_prefix(&[8, 0, 0, 0, 255, 0, 255, 0], NetworkRouteFamily::Ipv4),
            None
        );
    }

    #[test]
    fn corrupt_message_framing_is_typed_malformed() {
        assert_eq!(
            parse_dump(&[1, 2, 3]).unwrap_err().kind(),
            NetworkRouteErrorKind::MalformedSnapshot
        );
    }

    #[test]
    fn hopcount_is_not_reinterpreted_as_portable_metric() {
        // SAFETY: zero initialization is valid for the C header and the fields
        // needed by this synthetic route are assigned below.
        let mut header: libc::rt_msghdr2 = unsafe { std::mem::zeroed() };
        header.rtm_flags = libc::RTF_HOST;
        header.rtm_addrs = 1;
        header.rtm_index = 1;
        header.rtm_inits = 0x2;
        header.rtm_rmx.rmx_hopcount = 42;
        let destination = [
            16,
            libc::AF_INET as u8,
            0,
            0,
            127,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        let route = parse_route(&header, &destination).unwrap();
        assert_eq!(route.metric, None);
        assert_eq!(route.interface_native_id, 1);
    }
}
