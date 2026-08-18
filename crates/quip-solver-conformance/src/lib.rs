//! What a Quip solver is tested against.
//!
//! Two things live here: the golden vectors that pin cross-language parity,
//! and the scripted session driver that runs a solver binary through the
//! protocol. A solver repository takes this crate as a single dev-dependency
//! and needs no vendored fixtures and no mock coordinator of its own.

/// Scripted session driver: runs a solver binary through the protocol over a
/// Unix domain socket and reports what it observed.
pub mod driver;

mod vectors;

pub use vectors::{
    adapt_cases, adapt_params_cases, AdaptParamsCase, EnergyToDifficultyCase, GOLDEN_ADAPT,
    GOLDEN_VECTORS,
};
