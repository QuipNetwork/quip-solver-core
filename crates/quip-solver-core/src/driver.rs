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

/// Convert `--solve` coefficients to the milli integers `IsingGraph` holds.
///
/// Rounds to the nearest milli, ties away from zero, which is the rule
/// `quip_protocol::scoring::energy_milli` applies to every coefficient it
/// reads. `name` labels the field in the error.
fn to_milli(name: &str, values: &[f64]) -> Result<Vec<i32>, SolveError> {
    values
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let milli = (v * 1000.0).round();
            // A NaN fails both comparisons, and an infinity fails one.
            if milli >= f64::from(i32::MIN) && milli <= f64::from(i32::MAX) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "range-checked against i32 above, and round() left no fraction"
                )]
                Ok(milli as i32)
            } else {
                Err(SolveError::Malformed(format!(
                    "{name}[{i}] = {v} is outside the i32 milli range"
                )))
            }
        })
        .collect()
}

/// Solve one JSON problem and render its solutions as JSON.
///
/// # Errors
///
/// Returns [`SolveError::Malformed`] when `input` is not valid problem JSON,
/// does not describe a coherent problem, or holds a coefficient outside the
/// `i32` milli range, and [`SolveError::Sample`] when the sampler cannot
/// complete the job. A caller maps each to its own exit code;
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

    // The schema carries unit floats for every language. The graph holds the
    // wire's milli integers.
    let h_milli = to_milli("h", &p.h)?;
    let j_milli = to_milli("j", &p.j)?;

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

    let graph = IsingGraph::new(h_milli, j_milli, p.edges);
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
    use super::{solve, to_milli, SolveError};
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

    /// Records the milli coefficients it was handed, and returns no reads.
    #[derive(Default)]
    struct RecordingSampler(std::sync::Mutex<Option<(Vec<i32>, Vec<i32>)>>);

    impl Sampler for RecordingSampler {
        fn sample(
            &self,
            graph: &IsingGraph,
            _params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            *self.0.lock().unwrap() = Some((graph.h_milli.clone(), graph.j_milli.clone()));
            Ok(vec![])
        }
    }

    fn problem(h: &str, j: &str, edges: &str) -> Vec<u8> {
        format!(
            r#"{{"h":{h},"j":{j},"edges":{edges},"num_reads":1,"num_sweeps":1,
                "sweeps_per_beta":1,"beta_range":null,"seed":0}}"#
        )
        .into_bytes()
    }

    #[test]
    fn coefficients_round_to_the_nearest_milli() {
        let sampler = RecordingSampler::default();
        let _ = solve(
            &sampler,
            &problem(
                "[0.0004, 0.0005, -0.0005, 0.0015, 1.0, -2147483.648]",
                "[2147483.647]",
                "[[0, 1]]",
            ),
        )
        .expect("every value is inside the i32 milli range");
        assert_eq!(
            sampler.0.lock().unwrap().take(),
            Some((vec![0, 1, -1, 2, 1000, i32::MIN], vec![i32::MAX]))
        );
    }

    #[test]
    fn a_coefficient_outside_the_i32_milli_range_is_malformed() {
        for (h, j) in [
            ("[2147483.648, 0]", "[0]"),
            ("[0, 0]", "[-2147483.649]"),
            ("[1e300, 0]", "[0]"),
        ] {
            let sampler = RecordingSampler::default();
            let err = solve(&sampler, &problem(h, j, "[[0, 1]]"))
                .expect_err("out-of-range coefficient must fail");
            assert!(
                matches!(&err, SolveError::Malformed(why) if why.contains("i32 milli range")),
                "h={h} j={j}: {err:?}"
            );
            assert!(
                sampler.0.lock().unwrap().is_none(),
                "h={h} j={j} reached the sampler"
            );
        }
    }

    #[test]
    fn a_non_finite_coefficient_is_malformed() {
        // JSON cannot spell these, so call the conversion directly.
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                matches!(to_milli("h", &[v]), Err(SolveError::Malformed(_))),
                "{v}"
            );
        }
    }
}
