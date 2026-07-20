//! Bridges `cdma_common::hrpd::messages::HrpdOverheadMessage` to this crate's
//! `OverheadMessage` trait.

use super::overhead::OverheadMessage;
use cdma_common::hrpd::messages::HrpdOverheadMessage;

impl OverheadMessage for HrpdOverheadMessage {
    fn encode(&self) -> Vec<u8> {
        HrpdOverheadMessage::encode(self)
    }
}
