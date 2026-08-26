//! Openwave UP.Link gateway.
//!
//! This crate implements HDTP (Handheld Device Transport Protocol), the
//! datagram protocol an Openwave UP.Browser handset uses to reach a UP.Link
//! proxy, and a proxy backend that terminates handset sessions and fetches the
//! web on their behalf, transcoding HTML to HDML. The protocol references are
//! the HDTP 1.1 and HDML 2.0 specifications vendored under `docs/`.
//!
//! Layering:
//! * [`pdu`] / [`header`] / [`cipher`] — HDTP wire format.
//! * [`session`] — the session-creation handshake and session table.
//! * [`hdml`] / [`transcode`] — the HDML document model and the HTML→HDML pass.
//! * [`proxy`] — the outbound HTTP fetch.
//! * [`server`] — the UDP service loop tying request to response.

pub mod cipher;
pub mod hdml;
pub mod hdmlc;
pub mod header;
pub mod keyexch;
pub mod pdu;
pub mod proxy;
pub mod rc5;
pub mod server;
pub mod session;
pub mod transcode;
