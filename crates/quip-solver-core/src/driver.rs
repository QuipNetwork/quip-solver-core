//! Driver mode: solve one problem and exit.
//!
//! A one-shot caller hands a problem in and reads solutions out. There is no
//! session, no coordinator, and no credits. The JSON shape matches what
//! `quip-miner-exec` already sends to an external solver, so the two are duals
//! and the schema stays single.

use crate::error::SampleError;
use crate::ising::{IsingGraph, SampleParams, SamplerResult};
use crate::Sampler;
use serde::{Deserialize, Serialize};

/// One problem, as read from stdin.
#[derive(Debug, Deserialize)]
pub struct ProblemJson {
    /// Linear biases, one per variable.
    pub h: Vec<f64>,
    /// Couplings aligned with `edges`.
    pub j: Vec<f64>,
    /// Undirected edge list.
    pub edges: Vec<(usize, usize)>,
    /// Independent reads to produce.
    pub num_reads: usize,
    /// Annealing sweeps per read.
    pub num_sweeps: usize,
    /// Sweeps spent at each beta rung.
    pub sweeps_per_beta: usize,
    /// Optional explicit `(hot_beta, cold_beta)`.
    pub beta_range: Option<(f64, f64)>,
    /// PRNG seed.
    pub seed: u64,
}

/// One completed read, as written to stdout.
#[derive(Debug, Serialize)]
pub struct SolutionJson {
    /// Spin configuration, values in `{-1, +1}`.
    pub spins: Vec<i8>,
    /// Consensus energy in milli units.
    pub energy_milli: i64,
}

/// Why [`solve`] could not produce solutions.
#[derive(Debug)]
pub enum SolveError {
    /// The input was not valid problem JSON. A caller error, not a device
    /// condition: retrying the same bytes fails again no matter the device
    /// state.
    Malformed(String),
    /// The sampler could not complete the job.
    Sample(SampleError),
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "malformed problem JSON: {detail}"),
            Self::Sample(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SolveError {}

impl From<SampleError> for SolveError {
    fn from(e: SampleError) -> Self {
        Self::Sample(e)
    }
}

/// Solve one JSON problem and render its solutions as JSON.
///
/// # Errors
///
/// Returns [`SolveError::Malformed`] when `input` is not valid problem JSON,
/// and [`SolveError::Sample`] when the sampler cannot complete the job. A
/// caller maps each to its own exit code; only the latter is a device
/// condition.
///
/// # Panics
///
/// Never. Serializing `Vec<SolutionJson>` cannot fail.
pub fn solve<S: Sampler>(sampler: &S, input: &[u8]) -> Result<Vec<u8>, SolveError> {
    let p: ProblemJson =
        serde_json::from_slice(input).map_err(|e| SolveError::Malformed(e.to_string()))?;
    let graph = IsingGraph::new(p.h, p.j, p.edges);
    let params = SampleParams {
        num_reads: p.num_reads,
        num_sweeps: p.num_sweeps,
        sweeps_per_beta: p.sweeps_per_beta,
        beta_range: p.beta_range,
        seed: p.seed,
    };
    let out: Vec<SolutionJson> = sampler
        .sample(&graph, &params)?
        .into_iter()
        .map(|r: SamplerResult| SolutionJson {
            spins: r.spins,
            energy_milli: r.energy_milli,
        })
        .collect();
    #[expect(
        clippy::expect_used,
        reason = "serializing Vec<SolutionJson> has no failure mode"
    )]
    Ok(serde_json::to_vec(&out).expect("serialize solutions"))
}

#[cfg(test)]
mod tests {
    use super::{solve, SolveError};
    use crate::error::SampleError;
    use crate::ising::{IsingGraph, SampleParams, SamplerResult};
    use crate::Sampler;

    struct StubSampler;

    impl Sampler for StubSampler {
        fn sample(
            &self,
            _graph: &IsingGraph,
            _params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            Ok(vec![])
        }
    }

    #[test]
    fn a_malformed_input_is_a_caller_error_not_a_device_fault() {
        let err = solve(&StubSampler, b"not json").expect_err("malformed input must fail");
        assert!(
            matches!(err, SolveError::Malformed(_)),
            "a malformed problem document is a caller error, not {err:?}"
        );
    }
}
