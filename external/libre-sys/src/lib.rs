#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

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
