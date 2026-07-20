/// Internal PHY helpers for spreading and short-code generation.
pub(crate) mod lfsr;

/// Channel coding primitives.
pub mod coding;
/// HRPD (1xEV-DO) physical-layer primitives.
pub mod hrpd;
/// Short-code PN generation and spreading.
pub mod spread;
/// Walsh covering code generation and decoding.
pub mod walsh;
