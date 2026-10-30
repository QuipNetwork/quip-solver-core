//! What a Quip solver is tested against.
//!
//! Three things live here: the golden vectors that pin cross-language parity,
//! the scripted session driver that runs a solver binary through the
//! protocol, and the `--solve` gate that runs it in one-shot mode. A solver
//! repository takes this crate as a single dev-dependency and needs no
//! vendored fixtures and no mock coordinator of its own.

/// Scripted session driver: runs a solver binary through the protocol over a
/// Unix domain socket and reports what it observed.
pub mod driver;

/// The `--solve` gate: runs a solver binary in one-shot mode against SPEC
/// section 2 and reports each case.
pub mod solve;

mod vectors;

pub use vectors::{
    adapt_cases, adapt_params_cases, golden_wire, AdaptParamsCase, EnergyToDifficultyCase,
    GoldenWire, FAMILY_CORE, FAMILY_LEGACY, FAMILY_SCHEMA, GOLDEN_ADAPT, GOLDEN_LEASE,
    GOLDEN_TARGET, GOLDEN_VECTORS, GOLDEN_WIRE,
};
