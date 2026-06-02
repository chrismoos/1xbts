//! Change SPC parameter block — C.S0016-D §4.5.4.2. Same 24-bit BCD wire
//! format as Verify SPC; distinct type so the application layer can't mix
//! them up.

use crate::Error;
use crate::param::verify_spc::{decode_spc, encode_spc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSpc {
    pub spc: String,
}

impl ChangeSpc {
    pub fn new(spc: impl Into<String>) -> Self {
        Self { spc: spc.into() }
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        encode_spc(&self.spc)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self {
            spc: decode_spc(bytes)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_spc_round_trip() {
        let v = ChangeSpc::new("246810");
        let bytes = v.encode().unwrap();
        assert_eq!(bytes, vec![0x24, 0x68, 0x10]);
        assert_eq!(ChangeSpc::decode(&bytes).unwrap(), v);
    }
}
