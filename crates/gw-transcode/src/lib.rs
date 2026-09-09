//! Shared web-fetch and transcoding pass for the legacy markup gateways.
//!
//! An HDTP or WAP handset renders a card deck, not HTML, so both gateways need
//! the same two steps: fetch a URL, then reduce the document to something a
//! phone screen can show. Only the final serialization differs — HDML for
//! UP.Browser, WML for a WAP browser — so that step stays in each gateway and
//! everything before it lives here.
//!
//! * [`proxy`] — the outbound HTTP(S) fetch.
//! * [`doc`] — the markup-neutral deck a backend renders.
//! * [`transcode`] — the HTML→deck pass.

pub mod doc;
pub mod proxy;
pub mod transcode;
