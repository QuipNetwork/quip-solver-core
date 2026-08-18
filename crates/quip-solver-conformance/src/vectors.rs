//! Golden vectors, embedded from this crate's own manifest directory.
//!
//! Resolving against `CARGO_MANIFEST_DIR` here, rather than in each consumer,
//! is what lets a solver repository delete its vendored copy. The bytes travel
//! with the version pin.

use serde::Deserialize;

/// Raw `golden_adapt.json`: adaptive-parameter parity with the Python source.
pub const GOLDEN_ADAPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/golden_adapt.json"
));

/// Raw `golden_vectors.json`: consensus parity for `ChaCha8`, nonce derivation,
/// energy, diversity, the Ising draw, and truncation.
pub const GOLDEN_VECTORS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/golden_vectors.json"
));

/// One `energy_to_difficulty` case: a difficulty target and the graph it is
/// measured against. Field names are the verbatim JSON keys.
#[derive(Debug, Clone, Deserialize)]
pub struct EnergyToDifficultyCase {
    /// Acceptance ceiling in milli-energy.
    pub target_milli: i64,
    /// Variables in the graph.
    pub num_nodes: usize,
    /// Edges in the graph.
    pub num_edges: usize,
    /// The h-field values the topology advertises, in milli units.
    pub allowed_h_milli: Vec<i32>,
    /// Normalized difficulty the Python source produces for this case. This is
    /// the expected output.
    pub difficulty: f64,
    /// Ground-state-energy estimate at `c = 0.75`, carried for diagnostics when
    /// a mismatch needs explaining.
    pub gse_c75: f64,
}

/// One `adapt_params` case. Field names are the verbatim JSON keys; `num_reads`
/// and `num_sweeps` are the expected outputs, the rest are inputs.
#[derive(Debug, Clone, Deserialize)]
pub struct AdaptParamsCase {
    /// Acceptance ceiling in milli-energy.
    pub target_milli: i64,
    /// Variables in the graph.
    pub num_nodes: usize,
    /// Edges in the graph.
    pub num_edges: usize,
    /// The h-field values the topology advertises, in milli units.
    pub allowed_h_milli: Vec<i32>,
    /// Solutions the coordinator asks for.
    pub min_solutions: u32,
    /// Which `AdaptBounds` preset the case is measured against. Every current
    /// case says `"cpu_sa"`.
    pub bounds: String,
    /// Reads the Python source produces. Expected output.
    pub num_reads: usize,
    /// Sweeps the Python source produces. Expected output.
    pub num_sweeps: usize,
}

#[derive(Deserialize)]
struct AdaptFile {
    energy_to_difficulty: Vec<EnergyToDifficultyCase>,
    adapt_params_cpu_sa: Vec<AdaptParamsCase>,
}

/// Every `energy_to_difficulty` case.
///
/// # Panics
///
/// Panics when the embedded JSON does not match the case schema. That is a
/// build-time fixture error, not a runtime condition.
#[must_use]
pub fn adapt_cases() -> Vec<EnergyToDifficultyCase> {
    parse_adapt().energy_to_difficulty
}

/// Every `adapt_params` case for the CPU simulated-annealing bounds.
///
/// # Panics
///
/// Panics when the embedded JSON does not match the case schema.
#[must_use]
pub fn adapt_params_cases() -> Vec<AdaptParamsCase> {
    parse_adapt().adapt_params_cpu_sa
}

fn parse_adapt() -> AdaptFile {
    #[expect(
        clippy::expect_used,
        reason = "the fixture is embedded at build time; a schema mismatch is a build error"
    )]
    serde_json::from_str(GOLDEN_ADAPT).expect("golden_adapt.json matches the case schema")
}
