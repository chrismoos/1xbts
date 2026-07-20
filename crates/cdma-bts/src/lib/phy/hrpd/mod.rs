//! HRPD (1xEV-DO Rev 0) physical-layer primitives.
//!
//! Spec references: C.S0024-200 (HRPD PHY) and C.S0024-300 (HRPD MAC). This
//! module hosts the forward-link slot layout, channel encoders, and Walsh
//! covers used by the HRPD forward modulator. It is kept separate from the
//! 1x-only PHY helpers in sibling modules so the two stacks stay independently
//! testable.

pub mod crc;
pub mod interleaver;
pub mod rates;
pub mod scrambler;
pub mod slot;
pub mod turbo;
pub mod turbo_decoder;
