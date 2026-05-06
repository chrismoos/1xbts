use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub const BINDING_REQUEST: u16 = 0x0001;
pub const BINDING_SUCCESS_RESPONSE: u16 = 0x0101;
pub const MAPPED_ADDRESS: u16 = 0x0001;
pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;
pub const MAGIC_COOKIE: u32 = 0x2112_A442;

pub type TransactionId = [u8; 12];

pub fn binding_request(transaction_id: TransactionId) -> Vec<u8> {
    let mut packet = Vec::with_capacity(20);
    packet.extend_from_slice(&BINDING_REQUEST.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    packet.extend_from_slice(&transaction_id);
    packet
}

pub fn is_stun_message(packet: &[u8]) -> bool {
    packet.len() >= 20
        && packet[0] & 0b1100_0000 == 0
        && u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]) == MAGIC_COOKIE
}

pub fn parse_binding_response(
    packet: &[u8],
    expected_transaction_id: TransactionId,
) -> Result<SocketAddr, String> {
    if packet.len() < 20 {
        return Err("STUN response too short".to_string());
    }

    let message_type = u16::from_be_bytes([packet[0], packet[1]]);
    if message_type != BINDING_SUCCESS_RESPONSE {
        return Err(format!("unexpected STUN message type 0x{message_type:04x}"));
    }

    let length = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if packet.len() < 20 + length {
        return Err("truncated STUN response".to_string());
    }

    let cookie = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
    if cookie != MAGIC_COOKIE {
        return Err("invalid STUN magic cookie".to_string());
    }

    let transaction_id: TransactionId = packet[8..20]
        .try_into()
        .expect("slice length is checked above");
    if transaction_id != expected_transaction_id {
        return Err("STUN transaction ID mismatch".to_string());
    }

    let mut pos = 20;
    let end = 20 + length;
    let mut mapped = None;

    while pos + 4 <= end {
        let attr_type = u16::from_be_bytes([packet[pos], packet[pos + 1]]);
        let attr_len = u16::from_be_bytes([packet[pos + 2], packet[pos + 3]]) as usize;
        pos += 4;

        if pos + attr_len > end {
            return Err("truncated STUN attribute".to_string());
        }

        let value = &packet[pos..pos + attr_len];
        if attr_type == XOR_MAPPED_ADDRESS {
            return parse_address_attr(value, true, transaction_id);
        }
        if attr_type == MAPPED_ADDRESS && mapped.is_none() {
            mapped = Some(parse_address_attr(value, false, transaction_id)?);
        }

        pos += (attr_len + 3) & !3;
    }

    mapped.ok_or_else(|| "STUN response missing mapped address".to_string())
}

fn parse_address_attr(
    value: &[u8],
    xor: bool,
    transaction_id: TransactionId,
) -> Result<SocketAddr, String> {
    if value.len() < 4 || value[0] != 0 {
        return Err("invalid STUN mapped address attribute".to_string());
    }

    let family = value[1];
    let encoded_port = u16::from_be_bytes([value[2], value[3]]);
    let port = if xor {
        encoded_port ^ ((MAGIC_COOKIE >> 16) as u16)
    } else {
        encoded_port
    };

    match family {
        0x01 => {
            if value.len() < 8 {
                return Err("truncated STUN IPv4 mapped address".to_string());
            }
            let mut octets = [value[4], value[5], value[6], value[7]];
            if xor {
                let mask = MAGIC_COOKIE.to_be_bytes();
                for (byte, mask) in octets.iter_mut().zip(mask) {
                    *byte ^= mask;
                }
            }
            Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(octets)), port))
        }
        0x02 => {
            if value.len() < 20 {
                return Err("truncated STUN IPv6 mapped address".to_string());
            }
            let mut octets: [u8; 16] = value[4..20]
                .try_into()
                .expect("slice length is checked above");
            if xor {
                let mut mask = [0u8; 16];
                mask[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
                mask[4..].copy_from_slice(&transaction_id);
                for (byte, mask) in octets.iter_mut().zip(mask) {
                    *byte ^= mask;
                }
            }
            Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port))
        }
        other => Err(format!("unsupported STUN address family 0x{other:02x}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_binding_request() {
        let transaction_id = [0xab; 12];
        let request = binding_request(transaction_id);

        assert_eq!(&request[0..2], &BINDING_REQUEST.to_be_bytes());
        assert_eq!(&request[2..4], &0u16.to_be_bytes());
        assert_eq!(&request[4..8], &MAGIC_COOKIE.to_be_bytes());
        assert_eq!(&request[8..20], &transaction_id);
        assert!(is_stun_message(&request));
    }

    #[test]
    fn parses_xor_mapped_ipv4_response() {
        let transaction_id = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let mapped = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)), 43210);
        let mut attr = Vec::new();
        attr.extend_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
        attr.extend_from_slice(&8u16.to_be_bytes());
        attr.push(0);
        attr.push(0x01);
        attr.extend_from_slice(&(mapped.port() ^ ((MAGIC_COOKIE >> 16) as u16)).to_be_bytes());
        for (addr, mask) in match mapped.ip() {
            IpAddr::V4(addr) => addr.octets(),
            IpAddr::V6(_) => unreachable!(),
        }
        .iter()
        .zip(MAGIC_COOKIE.to_be_bytes())
        {
            attr.push(addr ^ mask);
        }

        let mut response = Vec::new();
        response.extend_from_slice(&BINDING_SUCCESS_RESPONSE.to_be_bytes());
        response.extend_from_slice(&(attr.len() as u16).to_be_bytes());
        response.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        response.extend_from_slice(&transaction_id);
        response.extend_from_slice(&attr);

        assert_eq!(
            parse_binding_response(&response, transaction_id).unwrap(),
            mapped
        );
    }

    #[test]
    fn rejects_wrong_transaction_id() {
        let mut response = binding_request([0x11; 12]);
        response[0..2].copy_from_slice(&BINDING_SUCCESS_RESPONSE.to_be_bytes());

        assert!(parse_binding_response(&response, [0x22; 12]).is_err());
    }
}
