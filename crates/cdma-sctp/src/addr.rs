//! Packed `sockaddr` arrays as used by `sctp_bindx`, `sctp_connectx` and
//! `SCTP_GET_PEER_ADDRS` (RFC 6458 §9.1, §9.9, §8.3).
//!
//! Each address is a native `sockaddr_in` or `sockaddr_in6` placed back to back
//! with no padding, so the array is parsed by reading each entry's family.

use std::mem::size_of;
use std::net::SocketAddr;

use socket2::SockAddr;

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    V4,
    V6,
}

impl Family {
    pub(crate) fn for_addrs(addrs: &[SocketAddr]) -> Result<Self, Error> {
        if addrs.is_empty() {
            return Err(Error::NoAddresses);
        }
        Ok(if addrs.iter().any(SocketAddr::is_ipv6) {
            Self::V6
        } else {
            Self::V4
        })
    }

    pub(crate) fn domain(self) -> libc::c_int {
        match self {
            Self::V4 => libc::AF_INET,
            Self::V6 => libc::AF_INET6,
        }
    }
}

pub(crate) fn pack(addrs: &[SocketAddr]) -> Vec<u8> {
    let mut out = Vec::new();
    for addr in addrs {
        let sock_addr = SockAddr::from(*addr);
        // SAFETY: `as_ptr` points to a `sockaddr_storage` that is valid for
        // `len()` bytes, as guaranteed by socket2 for every `SockAddr`.
        let bytes = unsafe {
            std::slice::from_raw_parts(sock_addr.as_ptr().cast::<u8>(), sock_addr.len() as usize)
        };
        out.extend_from_slice(bytes);
    }
    out
}

/// Stops early at a truncated or unknown entry.
pub(crate) fn unpack(buf: &[u8], count: usize) -> Vec<SocketAddr> {
    let mut addrs = Vec::with_capacity(count);
    let mut offset = 0;
    while addrs.len() < count {
        let Some((addr, len)) = decode_one(&buf[offset..]) else {
            break;
        };
        addrs.push(addr);
        offset += len;
    }
    addrs
}

fn decode_one(buf: &[u8]) -> Option<(SocketAddr, usize)> {
    if buf.len() < size_of::<libc::sockaddr>() {
        return None;
    }
    // SAFETY: `buf` holds at least one `sockaddr`, read unaligned because the
    // packed array gives no alignment guarantee.
    let header = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<libc::sockaddr>()) };
    let len = match libc::c_int::from(header.sa_family) {
        libc::AF_INET => size_of::<libc::sockaddr_in>(),
        libc::AF_INET6 => size_of::<libc::sockaddr_in6>(),
        _ => return None,
    };
    if buf.len() < len {
        return None;
    }
    let addr = sockaddr_to_socket_addr(&buf[..len])?;
    Some((addr, len))
}

pub(crate) fn sockaddr_to_socket_addr(bytes: &[u8]) -> Option<SocketAddr> {
    if bytes.len() > size_of::<libc::sockaddr_storage>() {
        return None;
    }
    // SAFETY: the closure copies at most `sockaddr_storage` bytes into the
    // zeroed storage socket2 provides and reports that length back.
    let ((), sock_addr) = unsafe {
        SockAddr::try_init(|storage, len| {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), storage.cast::<u8>(), bytes.len());
            *len = bytes.len() as libc::socklen_t;
            Ok(())
        })
    }
    .ok()?;
    // An AF_INET6 socket reports IPv4 peers as v4-mapped addresses.
    sock_addr
        .as_socket()
        .map(|addr| SocketAddr::new(addr.ip().to_canonical(), addr.port()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_family_list_round_trips_through_packed_array() {
        let addrs: Vec<SocketAddr> = vec![
            "127.0.0.1:14001".parse().unwrap(),
            "[::1]:14001".parse().unwrap(),
            "10.1.2.3:2905".parse().unwrap(),
        ];
        let packed = pack(&addrs);
        assert_eq!(
            packed.len(),
            2 * size_of::<libc::sockaddr_in>() + size_of::<libc::sockaddr_in6>()
        );
        assert_eq!(unpack(&packed, addrs.len()), addrs);
    }

    #[test]
    fn family_is_v6_when_any_address_is_v6() {
        let v4: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let v6: SocketAddr = "[::1]:1".parse().unwrap();
        assert_eq!(Family::for_addrs(&[v4]).unwrap(), Family::V4);
        assert_eq!(Family::for_addrs(&[v4, v6]).unwrap(), Family::V6);
        assert!(matches!(Family::for_addrs(&[]), Err(Error::NoAddresses)));
    }

    #[test]
    fn truncated_array_stops_without_panicking() {
        let addrs: Vec<SocketAddr> = vec!["127.0.0.1:1".parse().unwrap()];
        let packed = pack(&addrs);
        assert!(unpack(&packed[..packed.len() - 1], 1).is_empty());
    }
}
