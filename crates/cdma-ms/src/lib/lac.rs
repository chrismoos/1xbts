//! Common and dedicated ARQ procedures (C.S0004-E §2.1.1.2, §2.1.2.1).

/// Retransmissions of an assured r-dsch PDU before the mobile gives up
/// (N1m, C.S0004-E Annex A).
pub const N1M_MAX_TRANSMISSIONS: u32 = 13;
/// Minimum spacing between transmissions of the same r-dsch PDU (T1m).
pub const T1M_RETRANSMIT_MS: u64 = 400;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MsgSeqCounters {
    ack: u8,
    noack: u8,
}

impl MsgSeqCounters {
    pub fn next(&mut self, ack_req: bool) -> u8 {
        let slot = if ack_req {
            &mut self.ack
        } else {
            &mut self.noack
        };
        let seq = *slot;
        *slot = (*slot + 1) & 0x07;
        seq
    }
}

/// MSG_SEQ_RCVD: received-PDU indicators for duplicate detection. A regular
/// PDU with sequence `i` clears the indicator for `(i + 4) mod 8`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReceivedSeqs {
    rcvd: [bool; 8],
}

impl ReceivedSeqs {
    pub fn mark(&mut self, seq: u8) -> bool {
        let seq = (seq & 0x07) as usize;
        let duplicate = self.rcvd[seq];
        self.rcvd[seq] = true;
        self.rcvd[(seq + 4) % 8] = false;
        duplicate
    }

    pub fn clear(&mut self) {
        self.rcvd = [false; 8];
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRdsch {
    pub name: &'static str,
    pub ack_seq: u8,
    pub msg_seq: u8,
    pub pdu_bits: Vec<u8>,
    pub transmissions: u32,
    /// System time chip of the last transmission's first frame.
    pub last_tx_chip: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_numbers_wrap_per_ack_class() {
        let mut c = MsgSeqCounters::default();
        let seqs: Vec<u8> = (0..9).map(|_| c.next(true)).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3, 4, 5, 6, 7, 0]);
        assert_eq!(c.next(false), 0);
    }

    #[test]
    fn duplicates_are_detected_until_the_window_moves_on() {
        let mut r = ReceivedSeqs::default();
        assert!(!r.mark(1));
        assert!(r.mark(1));
        assert!(!r.mark(5));
        assert!(!r.mark(1));
    }
}
