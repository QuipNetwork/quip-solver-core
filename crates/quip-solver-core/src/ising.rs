//! Base Ising problem type and per-job sampling knobs, shared by all backends.
//!
//! The base [`IsingGraph`] holds the wire-parsed `(h, j, edges)`. Backends
//! derive their own representation from it: the CPU miner builds an adjacency
//! list, the GPU miners build [`crate::csr::CsrGraph`].

use crate::coefficient::Coefficient;
use std::sync::Arc;

/// Sampling algorithm selected by the binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    /// Metropolis-Hastings simulated annealing (neal default).
    Sa,
    /// Single-site heat-bath Gibbs along the same beta ladder.
    Gibbs,
}

/// Shape of the beta ladder between the hot and cold ends.
///
/// Every binary builds its ladder host-side, so the kind only takes effect in
/// a binary that reads it. A binary that does lists
/// [`LINEAR_SCHEDULE_FEATURE`](crate::LINEAR_SCHEDULE_FEATURE); `--solve`
/// refuses [`Linear`](Self::Linear) for one that does not, so no kernel
/// silently stays geometric. Session jobs carry no schedule kind and are always
/// [`Geometric`](Self::Geometric).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BetaSchedule {
    /// Log-spaced rungs, [`geometric_beta_schedule`](crate::beta::geometric_beta_schedule).
    /// The mining default.
    #[default]
    Geometric,
    /// Evenly spaced rungs, [`linear_beta_schedule`](crate::beta::linear_beta_schedule).
    Linear,
}

impl BetaSchedule {
    /// Build the ladder of this kind: `num_betas` rungs from `hot` to `cold`.
    #[must_use]
    pub fn build(self, hot: f64, cold: f64, num_betas: usize) -> Vec<f64> {
        match self {
            Self::Geometric => crate::beta::geometric_beta_schedule(hot, cold, num_betas),
            Self::Linear => crate::beta::linear_beta_schedule(hot, cold, num_betas),
        }
    }

    /// The `--solve` JSON name: `"geometric"` or `"linear"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Geometric => "geometric",
            Self::Linear => "linear",
        }
    }
}

/// Per-job sampling knobs.
#[derive(Clone, Debug)]
pub struct SampleParams {
    /// Independent reads (samples) to produce.
    pub num_reads: usize,
    /// Annealing sweeps per read.
    pub num_sweeps: usize,
    /// Sweeps spent at each beta rung.
    pub sweeps_per_beta: usize,
    /// Optional `(hot_beta, cold_beta)`. `None` → auto from biases.
    pub beta_range: Option<(f64, f64)>,
    /// Shape of the ladder between the ends of `beta_range`.
    pub schedule: BetaSchedule,
    /// PRNG seed for this job.
    pub seed: u64,
}

impl Default for SampleParams {
    fn default() -> Self {
        Self {
            num_reads: 1,
            num_sweeps: 64,
            sweeps_per_beta: 1,
            beta_range: None,
            schedule: BetaSchedule::Geometric,
            seed: 0,
        }
    }
}

/// Warm-start states and anneal start point for one seeded job, decoded from
/// `IsingProblem` fields 9 to 12.
///
/// The session builds one only for a job that carries at least one state, and
/// hands it only to a [`Sampler`](crate::Sampler) whose
/// [`accepts_warm_start`](crate::Sampler::accepts_warm_start) is true. Every
/// state has already been checked to cover the job's nodes, and the list is
/// already cut to `num_reads`.
///
/// Seed one read per state and start the remaining reads cold. A backend that
/// takes one state per job, as the QPU does, uses the first state.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct WarmStart {
    /// Start states, best first, each with one `{-1,+1}` entry per variable.
    /// Never empty.
    pub spins: Vec<Vec<i8>>,
    /// Seeded SA: the inverse temperature the anneal starts from. `None` = the
    /// backend picks.
    pub start_beta: Option<f64>,
    /// Seeded QPU: the anneal fraction `s` in `(0, 1)` that the reverse anneal
    /// backs off to. `None` = the backend picks.
    pub reversal_s: Option<f64>,
    /// Seeded QPU: the pause at the reversal point, in microseconds. `None` =
    /// the backend picks.
    pub reversal_pause_us: Option<u32>,
}

/// One completed read: spins in {-1,+1} and consensus milli-energy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplerResult {
    /// Spin configuration, one entry per variable, values in `{-1, +1}`.
    pub spins: Vec<i8>,
    /// Consensus energy of `spins`, in milli units.
    pub energy_milli: i64,
}

/// Wire-parsed Ising problem in the sampler's coefficient representation.
#[derive(Clone, Debug)]
pub struct IsingGraph<C: Coefficient = f64> {
    /// Linear biases, one per variable, in the sampler's coefficient type.
    pub h: Vec<C>,
    /// Couplings aligned with `edges`, in the sampler's coefficient type.
    pub j: Vec<C>,
    /// Undirected edge list `(u, v)` in received order.
    ///
    /// Shared, so jobs on one topology reuse a single allocation.
    pub edges: Arc<[(usize, usize)]>,
}

impl IsingGraph {
    /// Store flat `h` / `j` / edge lists as the base problem.
    #[must_use]
    pub fn new(h: Vec<f64>, j: Vec<f64>, edges: Vec<(usize, usize)>) -> Self {
        Self {
            h,
            j,
            edges: edges.into(),
        }
    }
}

impl<C: Coefficient> IsingGraph<C> {
    /// Number of variables, equal to the bias count.
    #[must_use]
    pub fn num_nodes(&self) -> usize {
        self.h.len()
    }
}

/// Read one pinned graph from the shared conformance fixtures.
#[cfg(test)]
pub(crate) fn golden_graph<C: Coefficient>(section: &str, index: usize) -> IsingGraph<C> {
    let golden: serde_json::Value =
        serde_json::from_str(quip_solver_conformance::GOLDEN_VECTORS).expect("golden JSON");
    let case = golden
        .pointer(&format!("/{section}/{index}"))
        .expect("golden case");
    let coefficients = |key: &str| {
        case.get(key)
            .and_then(serde_json::Value::as_array)
            .expect("milli array")
            .iter()
            .map(|v| C::from_milli(i32::try_from(v.as_i64().expect("integer")).expect("i32 milli")))
            .collect()
    };
    let edges: Vec<(usize, usize)> = case
        .get("edges")
        .and_then(serde_json::Value::as_array)
        .expect("edges array")
        .iter()
        .map(|edge| {
            let end = |i| {
                usize::try_from(
                    edge.get(i)
                        .and_then(serde_json::Value::as_u64)
                        .expect("endpoint"),
                )
                .expect("usize endpoint")
            };
            (end(0), end(1))
        })
        .collect();
    IsingGraph {
        h: coefficients("h_milli"),
        j: coefficients("j_milli"),
        edges: edges.into(),
    }
}

/// Every golden graph the pinned beta and CSR tests cover, as
/// `(section, index)`.
#[cfg(test)]
pub(crate) const GOLDEN_GRAPHS: [(&str, usize); 9] = [
    ("energy", 0),
    ("energy", 1),
    ("energy", 2),
    ("energy", 3),
    ("energy", 4),
    ("energy", 5),
    ("energy", 6),
    ("ising", 0),
    ("ising", 1),
];

#[cfg(test)]
mod tests {
    use super::{BetaSchedule, IsingGraph, SampleParams};

    /// The serde names are the `--solve` names, so a backend can carry the
    /// kind across its own process boundary in the same spelling.
    #[test]
    fn serde_round_trips_the_solve_names() {
        for kind in [BetaSchedule::Geometric, BetaSchedule::Linear] {
            let json = serde_json::to_string(&kind).expect("serialize");
            assert_eq!(json, format!("\"{}\"", kind.name()));
            let back: BetaSchedule = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn the_default_schedule_is_geometric() {
        assert_eq!(SampleParams::default().schedule, BetaSchedule::Geometric);
    }

    #[test]
    fn each_schedule_builds_its_own_ladder() {
        assert_eq!(
            BetaSchedule::Geometric.build(0.1, 10.0, 4),
            crate::beta::geometric_beta_schedule(0.1, 10.0, 4)
        );
        assert_eq!(
            BetaSchedule::Linear.build(0.1, 10.0, 4),
            crate::beta::linear_beta_schedule(0.1, 10.0, 4)
        );
    }

    #[test]
    fn num_nodes_counts_biases() {
        let graph = IsingGraph::new(
            vec![1.0, -0.5, 0.0, 0.25],
            vec![1.0, -1.0, -1.0, 1.0],
            vec![(0, 1), (1, 2), (2, 3), (0, 3)],
        );
        assert_eq!(graph.num_nodes(), 4);
        assert_eq!(graph.edges.len(), 4);
    }
}
