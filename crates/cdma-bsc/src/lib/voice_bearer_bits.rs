use log::debug;

pub(crate) fn normalize_air_voice_bits(bits: &[u8], rate_bps: u32) -> Vec<u8> {
    if rate_bps == 9600 && bits.len() == 172 && bits.first() == Some(&0) {
        bits[1..].to_vec()
    } else {
        bits.to_vec()
    }
}

pub(crate) fn mux_voice_bits_for_air(bits: &[u8], rate_bps: u32) -> Option<(Vec<u8>, u32)> {
    let mux_bits = if rate_bps == 9600 {
        let mut framed = Vec::with_capacity(bits.len() + 1);
        framed.push(0);
        framed.extend_from_slice(bits);
        framed
    } else {
        bits.to_vec()
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
    fn prepends_full_rate_mux_header_for_air_media() {
        let (bits, rate_bps) = mux_voice_bits_for_air(&[1, 0, 1], 9600).unwrap();
        assert_eq!(rate_bps, 9600);
        assert_eq!(bits, vec![0, 1, 0, 1]);
    }

    #[test]
    fn leaves_subrate_air_media_unchanged() {
        let (bits, rate_bps) = mux_voice_bits_for_air(&[1, 0, 1], 1200).unwrap();
        assert_eq!(rate_bps, 1200);
        assert_eq!(bits, vec![1, 0, 1]);
    }
}
