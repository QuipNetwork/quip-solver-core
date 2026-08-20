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
///
/// `deny_unknown_fields` because every field here is required and none carries a
/// serde default: a caller who misspells one gets a missing-field error rather
/// than a silent default, and this closes the remaining hole where a *sixth*,
/// unrecognized key (a typo alongside the real one, or a field from a newer
/// schema) was accepted and dropped.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
/// Returns [`SolveError::Malformed`] when `input` is not valid problem JSON or
/// does not describe a coherent problem, and [`SolveError::Sample`] when the
/// sampler cannot complete the job. A caller maps each to its own exit code;
/// only the latter is a device condition.
///
/// # Panics
///
/// Never. Serializing `Vec<SolutionJson>` cannot fail.
pub fn solve<S: Sampler>(sampler: &S, input: &[u8]) -> Result<Vec<u8>, SolveError> {
    let p: ProblemJson =
        serde_json::from_slice(input).map_err(|e| SolveError::Malformed(e.to_string()))?;

    // Driver mode is a second front door onto the same samplers the session
    // loop feeds, so it owes them the same guarantees. Deserializing succeeded,
    // which only means the JSON had the right *shape*; nothing above has
    // checked that `j` pairs with `edges` or that an endpoint names a real
    // node, and a sampler behind the C ABI reads out of bounds when they do not
    // (see `job::validate_shape`).
    crate::job::validate_shape(p.h.len(), p.j.len(), &p.edges).map_err(SolveError::Malformed)?;

    // A read count of zero asks for no solutions and gets an empty array back,
    // which is indistinguishable from a sampler that failed to find any.
    if p.num_reads == 0 {
        return Err(SolveError::Malformed("num_reads must be at least 1".into()));
    }
    // The session path applies this cap in `prepare_job`; a backend with a
    // device-memory bound is entitled to it here too, before it allocates.
    let max_reads = usize::try_from(sampler.max_reads()).unwrap_or(usize::MAX);
    if p.num_reads > max_reads {
        return Err(SolveError::Malformed(format!(
            "num_reads {} exceeds this backend's maximum of {max_reads}",
            p.num_reads
        )));
    }
    // Mirrors the `--sweeps-per-beta` floor in `CommonArgs`: the same field,
    // reached through the other door. Backends divide the sweep budget by it.
    if p.sweeps_per_beta == 0 {
        return Err(SolveError::Malformed(
            "sweeps_per_beta must be at least 1".into(),
        ));
    }

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
