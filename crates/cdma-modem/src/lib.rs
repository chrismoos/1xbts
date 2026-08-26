//! Interworking Function (IWF) modem emulation for the CDMA async data service
//! (SO 12). The IWF is the network element that terminates the mobile's async
//! data call and interworks it to the network, i.e. the emulated modem bank.
//!
//! Implements the network side of TIA/EIA/IS-707-A.3: the AT command engine,
//! modem result codes, the TIA-617 in-band control-channel codec, and the
//! command/online state machine an emulated modem presents to the mobile over
//! the TCP "modem server" connection (well-known port 380).
//!
//! The backend is transport-agnostic: it consumes the octet stream from the
//! mobile and produces octets to send back plus higher-level events (dial,
//! hang up, return-online) for the integration layer to act on.

pub mod at;
pub mod modem;
pub mod result;
pub mod server;
pub mod tia617;

pub use modem::{ModemEvent, ModemIwf, ModemState, Reply};
pub use result::ResultCode;
pub use server::{ModemServer, ServerEvent};
