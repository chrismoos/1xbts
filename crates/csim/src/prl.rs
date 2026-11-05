//! PRL image checks (C.S0016 §3.5.5).

const CRC_POLY: u16 = 0x1021;
const PRL_HEADER_MIN: usize = 4;

/// PR_LIST_CRC of C.S0016 §3.5.5.1: CRC-16, polynomial 1021, initial
/// value FFFF, final inversion, over every byte before the CRC.
pub fn prl_crc(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &octet in data {
        crc ^= u16::from(octet) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ CRC_POLY
            } else {
                crc << 1
            };
        }
    }
    !crc
}

/// Problems with a PRL or EPRL image: it must fit the EF, its
/// PR_LIST_SIZE must equal its length and its CRC must verify.
pub fn check_prl_bytes(key: &str, data: &[u8], ef_size: usize) -> Vec<String> {
    if data.len() < PRL_HEADER_MIN {
        return vec![format!("{key}: {} bytes is not a PRL", data.len())];
    }
    let mut problems = Vec::new();
    if data.len() > ef_size {
        problems.push(format!(
            "{key}: {} bytes does not fit EF ({ef_size} bytes)",
            data.len()
        ));
    }
    let size = usize::from(u16::from_be_bytes([data[0], data[1]]));
    let n = data.len();
    if size != n {
        problems.push(format!(
            "{key}: PR_LIST_SIZE {size} but the image is {n} bytes"
        ));
    } else if prl_crc(&data[..n - 2]) != u16::from_be_bytes([data[n - 2], data[n - 1]]) {
        problems.push(format!("{key}: PR_LIST_CRC does not verify"));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const STUB: &str = "001200018100400130b052b664000000dccc";
    const FIXTURE: &[u8] = include_bytes!("../../cdma-otasp/tests/fixtures/verizon_51611.prl");

    #[test]
    fn crc_matches_fixtures() {
        let stub = crate::parse_hex(STUB).unwrap();
        for data in [&stub[..], FIXTURE] {
            let n = data.len();
            assert_eq!(
                prl_crc(&data[..n - 2]),
                u16::from_be_bytes([data[n - 2], data[n - 1]])
            );
        }
    }

    #[test]
    fn checks() {
        let stub = crate::parse_hex(STUB).unwrap();
        assert!(check_prl_bytes("eprl", FIXTURE, 12288).is_empty());
        assert!(check_prl_bytes("prl", &stub, 256).is_empty());
        let mut bad = stub.clone();
        bad[5] ^= 1;
        assert!(check_prl_bytes("prl", &bad, 256)[0].contains("CRC"));
        assert!(check_prl_bytes("eprl", FIXTURE, 256)[0].contains("fit"));
        assert!(check_prl_bytes("prl", &stub[..stub.len() - 1], 256)[0].contains("PR_LIST_SIZE"));
    }
}
