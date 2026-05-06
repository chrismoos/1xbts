/// Internal PHY helpers for spreading and short-code generation.
pub(crate) mod lfsr;

/// Channel coding primitives.
pub mod coding;
/// Short-code PN generation and spreading.
pub mod spread;
/// Walsh covering code generation and decoding.
pub mod walsh;
