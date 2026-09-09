//! HTML-to-WML transcoding proxy for the WAP gateway.
//!
//! Kannel terminates WSP/WTP and compiles WML to WBXML, but it does not convert
//! HTML, so a WAP browser pointed at an ordinary site gets content it cannot
//! render. This crate sits between the two as a forward proxy: wapbox fetches
//! through it, it fetches the real page, and it returns a WML deck.
//!
//! The fetch and the HTML→deck pass are shared with the HDTP gateway and live
//! in [`gw_transcode`]. Only [`wml`] is specific to WAP.

pub mod wml;

/// Content a WAP handset renders on its own. A WAP site already serves WML, and
/// transcoding it would escape the markup and show the source as text, so these
/// go back untouched with the type the origin gave them.
pub fn is_handset_native(content_type: &str) -> bool {
    let main = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        main.as_str(),
        "text/vnd.wap.wml"
            | "application/vnd.wap.wmlc"
            | "text/vnd.wap.wmlscript"
            | "application/vnd.wap.wmlscriptc"
            | "image/vnd.wap.wbmp"
    )
}
