use log::debug;

pub(crate) fn normalize_air_voice_bits(bits: &[u8], rate_bps: u32) -> Vec<u8> {
    let has_mux_header = matches!(rate_bps, 9_600 | 14_400 | 7_200 | 3_600 | 1_800);
    let header_included =
        primary_voice_bits(rate_bps).is_some_and(|primary_bits| bits.len() == primary_bits + 1);
    if has_mux_header && header_included && bits.first() == Some(&0) {
        bits[1..].to_vec()
    } else {
        bits.to_vec()
    }
}

fn primary_voice_bits(rate_bps: u32) -> Option<usize> {
    match rate_bps {
        9_600 => Some(171),
        4_800 => Some(80),
        2_400 | 2_700 => Some(40),
        1_200 | 1_500 => Some(16),
        14_400 => Some(266),
        7_200 => Some(124),
        3_600 => Some(54),
        1_800 => Some(20),
        _ => None,
    }
}

pub(crate) fn pack_voice_bits_for_bearer(bits: &[u8], rate_bps: u32) -> Option<Vec<u8>> {
    let normalized = normalize_air_voice_bits(bits, rate_bps);
    let bit_count = primary_voice_bits(rate_bps)?;
    Some(cdma_voice::pack_voice_bits(&normalized, bit_count))
}

pub(crate) fn mux_voice_bits_for_air(payload: &[u8], rate_bps: u32) -> Option<(Vec<u8>, u32)> {
    let bit_count = primary_voice_bits(rate_bps)?;
    let bits = cdma_voice::unpack_voice_bits(payload, bit_count);
    let has_mux_header = matches!(rate_bps, 9_600 | 14_400 | 7_200 | 3_600 | 1_800);
    let mux_bits = if has_mux_header {
        let mut framed = Vec::with_capacity(bits.len() + 1);
        framed.push(0);
        framed.extend_from_slice(&bits);
        framed
    } else {
        bits
    };

    debug!(
        "BSC: prepared voice bearer frame for air rate_bps={} bits={}",
        rate_bps,
        mux_bits.len()
    );

    Some((mux_bits, rate_bps))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_full_rate_mux_header_before_gateway_media() {
        let mut bits = vec![1; 172];
        bits[0] = 0;
        let normalized = normalize_air_voice_bits(&bits, 9600);
        assert_eq!(normalized.len(), 171);
        assert_eq!(normalized[0], 1);
    }

    #[test]
    fn leaves_subrate_gateway_media_unchanged() {
        let bits = vec![1, 0, 1, 1];
        assert_eq!(normalize_air_voice_bits(&bits, 4800), bits);
    }

    #[test]
    fn preserves_leading_zero_in_already_extracted_primary_bits() {
        for (rate_bps, bit_count) in [
            (9_600, 171usize),
            (14_400, 266),
            (7_200, 124),
            (3_600, 54),
            (1_800, 20),
        ] {
            let mut primary = vec![1; bit_count];
            primary[0] = 0;
            assert_eq!(normalize_air_voice_bits(&primary, rate_bps), primary);
        }
    }

    #[test]
    fn prepends_full_rate_mux_header_for_air_media() {
        let payload = cdma_voice::pack_voice_bits(&[1, 0, 1], 171);
        let (bits, rate_bps) = mux_voice_bits_for_air(&payload, 9600).unwrap();
        assert_eq!(rate_bps, 9600);
        assert_eq!(&bits[..4], &[0, 1, 0, 1]);
        assert_eq!(bits.len(), 172);
    }

    #[test]
    fn leaves_subrate_air_media_unchanged() {
        let payload = cdma_voice::pack_voice_bits(&[1, 0, 1], 16);
        let (bits, rate_bps) = mux_voice_bits_for_air(&payload, 1200).unwrap();
        assert_eq!(rate_bps, 1200);
        assert_eq!(&bits[..3], &[1, 0, 1]);
        assert_eq!(bits.len(), 16);
    }

    #[test]
    fn adds_and_removes_rate_set_two_mux_header() {
        for (rate_bps, bit_count) in [(14_400, 266usize), (7_200, 124), (3_600, 54), (1_800, 20)] {
            let mut primary = vec![0; bit_count];
            primary[..3].copy_from_slice(&[1, 0, 1]);
            let payload = cdma_voice::pack_voice_bits(&primary, bit_count);
            let (air, rate) = mux_voice_bits_for_air(&payload, rate_bps).unwrap();
            assert_eq!(rate, rate_bps);
            assert_eq!(air.len(), bit_count + 1);
            assert_eq!(normalize_air_voice_bits(&air, rate_bps), primary);
            assert_eq!(pack_voice_bits_for_bearer(&air, rate_bps).unwrap(), payload);
        }
    }
}
