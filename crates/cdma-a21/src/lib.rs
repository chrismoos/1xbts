//! 1x BSC ↔ HRPD AN coordination protocol.
//!
//! Serves the cross-system coordination role of the A21 reference point
//! (A.S0017-D 1x↔HRPD interop, A.S0019-A hybrid AT operation): the 1x BSC and
//! HRPD AN exchange IMSI presence and cross-paging/suppression so a hybrid AT
//! is paged once across both systems. The message set and framing are a
//! 1XBTS-internal hand-rolled binary protocol between two 1XBTS components,
//! not the spec's A21 message encoding. The boundary is intentionally a real
//! network boundary — callers must not reach in-process across this crate.

pub mod client;
pub mod client_loop;
pub mod error;
pub mod hub;
pub mod identity_cache;
pub mod message;
pub mod server;
pub mod transport;

pub use client::A21Client;
pub use client_loop::A21ClientLoop;
pub use error::{A21Error, Result};
pub use hub::A21Hub;
pub use identity_cache::{CachedIdentity, HybridIdentityCache};
pub use message::{A21Message, PagingSource};
pub use server::{A21Connection, A21Handler, A21Server};
pub use transport::{MAX_FRAME_LEN, read_frame, write_frame};
