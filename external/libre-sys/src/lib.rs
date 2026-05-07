#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
// Suppress lints on bindgen-generated code that hasn't caught up with Rust 1.95+.
// See https://rust-lang.github.io/rust-bindgen/tutorial-4.html
#![allow(unnecessary_transmutes)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::ptr_offset_with_cast)]

//!
//! Low-level bindings to Baresip's `libre`/`re` C library.
//!
//! `libre-sys` discovers the native library with `pkg-config` package `libre`.
//! Set `LIBRE_INCLUDE_DIR` and optionally `LIBRE_LIB_DIR`/`LIBRE_LIB_NAME` to
//! point at a custom install. Enable the `required` feature in production builds
//! that should fail immediately when the native library is missing.
//!

pub const LIBRE_AVAILABLE: bool = cfg!(libre_available);

include!(concat!(env!("OUT_DIR"), "/bindgen.rs"));
