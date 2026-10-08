//! Driver-mode input validation, exercised through the public API.
//!
//! `driver::solve` is the one-shot front door onto the same samplers the
//! session loop feeds, so it owes them the same guarantees `job::parse_ising`
//! provides on the coordinator path. These tests drive it directly rather than
//! spawning a miner, so they can assert on the [`SolveError`] variant and read
//! back what the sampler was actually handed.

use std::sync::Mutex;

use quip_solver_core::adapt::AdaptBounds;
use quip_solver_core::driver::{solve, SolveError};
use quip_solver_core::quip_proto::v1::{Algorithm, Backend};
use quip_solver_core::{
    BackendIdentity, BetaSchedule, ExitCode, IsingGraph, ModelLimits, SampleError, SampleParams,
    Sampler, SamplerResult, EXACT_COEFFICIENTS_FEATURE, LINEAR_SCHEDULE_FEATURE,
};

/// A solver identity with no caps, no limits and no optional features, for
/// tests that vary one thing at a time.
fn identity() -> BackendIdentity {
    BackendIdentity {
        backend: Backend::Mock,
        algorithm: Algorithm::Sa,
        max_nodes: 0,
        max_edges: 0,
        features: &[],
        adapt: AdaptBounds {
            min_sweeps: 1,
            max_sweeps: 1,
            min_reads: 1,
            max_reads: 1,
            reads_solution_min_factor: 1,
            reads_solution_max_factor: 1,
            reads_solution_floor_factor: 0,
        },
        limits: ModelLimits::UNSTATED,
    }
}

/// Records the params of the last `sample` call, so a test can assert on what
/// crossed the boundary rather than only on what came back.
#[derive(Default)]
struct RecordingSampler {
    seen: Mutex<Option<SampleParams>>,
    max_reads: Option<u32>,
}

impl RecordingSampler {
    fn with_max_reads(max_reads: u32) -> Self {
        Self {
            seen: Mutex::new(None),
            max_reads: Some(max_reads),
        }
    }

    /// The params of the last `sample` call, or `None` when it was never
    /// reached — which is itself the assertion for a rejected problem.
    ///
    /// Poison-tolerant: a panic in one test must surface as that test's own
    /// failure, not as a lock error in the next assertion.
    fn seen(&self) -> Option<SampleParams> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Sampler for RecordingSampler {
    fn sample(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        *self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(params.clone());
        Ok(vec![
            SamplerResult {
                spins: vec![1i8; graph.h.len()],
                energy_milli: 0,
            };
            params.num_reads
        ])
    }

    fn max_reads(&self) -> u32 {
        self.max_reads.unwrap_or(u32::MAX)
    }
}

/// A problem body with every field spelled correctly, for tests that vary one
/// thing at a time.
const VALID: &[u8] = br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],
    "num_reads":2,"num_sweeps":8,"sweeps_per_beta":1,"beta_range":null,"seed":7}"#;

/// Solve `input`, require that it was rejected as malformed without reaching
/// the sampler, and hand back the detail message for the caller to assert on.
fn reject_reason(input: &[u8]) -> String {
    let sampler = RecordingSampler::default();
    let outcome = match solve(&sampler, &identity(), input) {
        Ok(_) => "accepted".to_owned(),
        Err(SolveError::Malformed(detail)) => format!("malformed: {detail}"),
        Err(e) => format!("other error: {e}"),
    };
    assert!(
        outcome.starts_with("malformed: "),
        "expected a malformed rejection, got: {outcome}"
    );
    assert!(
        sampler.seen().is_none(),
        "a rejected problem must never reach the sampler"
    );
    outcome
}

#[test]
fn the_reference_problem_solves() {
    let sampler = RecordingSampler::default();
    let out = solve(&sampler, &identity(), VALID).expect("the reference problem must solve");
    assert!(!out.is_empty());
    assert!(
        sampler.seen().is_some(),
        "the sampler must have been called"
    );
}

/// quip-solver-core-l12 / -3eb: driver mode accepted a `j`/`edges` pair that
/// cannot describe a graph. Reaching a C-ABI sampler with these two disagreeing
/// is an out-of-bounds read, so it must be rejected before `IsingGraph` is even
/// built.
#[test]
fn couplings_that_do_not_pair_with_edges_are_rejected() {
    let detail = reject_reason(
        br#"{"h":[1.0],"j":[1.0,2.0],"edges":[],
             "num_reads":1,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#,
    );
    assert!(
        detail.contains("2 couplings") && detail.contains("0 edges"),
        "the error should name both counts: {detail}"
    );
}

#[test]
fn an_edge_endpoint_outside_h_is_rejected() {
    let detail = reject_reason(
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,2]],
             "num_reads":1,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#,
    );
    assert!(
        detail.contains("outside h"),
        "the error should name the bound: {detail}"
    );

    // The last in-range index is still accepted, so the check is not off by one.
    let sampler = RecordingSampler::default();
    assert!(solve(
        &sampler,
        &identity(),
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],
             "num_reads":1,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#,
    )
    .is_ok());
}

/// quip-solver-core-l12: `ProblemJson` had no `deny_unknown_fields`, so a
/// misspelled key was dropped and its field silently took a default.
#[test]
fn an_unknown_key_is_rejected_rather_than_ignored() {
    let detail = reject_reason(
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],"num_reads":1,"num_sweeps":1,
             "sweeps_per_beta":1,"beta_range":null,"seed":1,"num_sweep":99}"#,
    );
    assert!(
        detail.contains("num_sweep"),
        "the error should name the offending key: {detail}"
    );
}

#[test]
fn num_reads_must_be_at_least_one() {
    let detail = reject_reason(
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],
             "num_reads":0,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#,
    );
    assert!(detail.contains("num_reads"), "{detail}");
}

/// The session path caps `num_reads` at the backend's own bound in
/// `prepare_job`; driver mode owes a backend with a device-memory limit the
/// same, before it allocates for the read count.
#[test]
fn num_reads_beyond_the_backend_maximum_is_rejected() {
    let sampler = RecordingSampler::with_max_reads(4);
    let err = solve(
        &sampler,
        &identity(),
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],
             "num_reads":5,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#,
    )
    .expect_err("5 reads against a cap of 4 must be rejected");
    assert!(matches!(err, SolveError::Capacity(_)), "{err:?}");
    assert_eq!(err.exit_code(), ExitCode::Capacity);
    assert!(sampler.seen().is_none());

    // Exactly at the cap still runs.
    let sampler = RecordingSampler::with_max_reads(4);
    assert!(solve(
        &sampler,
        &identity(),
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],
             "num_reads":4,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#,
    )
    .is_ok());
}

/// `--sweeps-per-beta` enforces `>= 1`; the JSON door onto the same field must
/// agree, since backends divide the sweep budget by it.
#[test]
fn sweeps_per_beta_must_be_at_least_one() {
    let detail = reject_reason(
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],
             "num_reads":1,"num_sweeps":1,"sweeps_per_beta":0,"beta_range":null,"seed":1}"#,
    );
    assert!(detail.contains("sweeps_per_beta"), "{detail}");
}

/// `beta_range` threads from the JSON into `SampleParams` untouched. It had no
/// coverage at all: a `solve` that dropped it would still return plausible
/// solutions, just annealed on the wrong ladder.
#[test]
fn beta_range_reaches_the_sampler_unchanged() {
    let sampler = RecordingSampler::default();
    let _ = solve(
        &sampler,
        &identity(),
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],"num_reads":1,"num_sweeps":3,
             "sweeps_per_beta":2,"beta_range":[0.25,9.5],"seed":42}"#,
    )
    .expect("a problem with an explicit beta range must solve");

    let seen = sampler.seen().expect("the sampler must have been called");
    assert_eq!(seen.beta_range, Some((0.25, 9.5)));
    // The neighbouring knobs travel with it, so a field-order slip is caught.
    assert_eq!(seen.num_reads, 1);
    assert_eq!(seen.num_sweeps, 3);
    assert_eq!(seen.sweeps_per_beta, 2);
    assert_eq!(seen.seed, 42);
}

/// `null` is the documented "auto from biases" value and must stay distinct
/// from a pinned range.
#[test]
fn a_null_beta_range_reaches_the_sampler_as_none() {
    let sampler = RecordingSampler::default();
    let _ = solve(&sampler, &identity(), VALID).expect("the reference problem must solve");
    assert_eq!(
        sampler.seen().expect("sampler called").beta_range,
        None,
        "null must mean auto, not a pinned range"
    );
}

#[test]
fn a_self_loop_is_rejected() {
    let detail = reject_reason(
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[1,1]],
             "num_reads":1,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#,
    );
    assert!(detail.contains("self-loop"), "{detail}");
}

/// Solve `input` against `id`, require a capacity refusal that never reached
/// the sampler, and hand back its detail.
fn capacity_reason(id: &BackendIdentity, input: &[u8]) -> String {
    let sampler = RecordingSampler::default();
    let outcome = solve(&sampler, id, input);
    assert!(
        sampler.seen().is_none(),
        "a refused model must not be sampled"
    );
    assert!(
        matches!(outcome, Err(SolveError::Capacity(_))),
        "expected a capacity refusal, got {outcome:?}"
    );
    match outcome {
        Err(e @ SolveError::Capacity(_)) => {
            assert_eq!(e.exit_code(), ExitCode::Capacity);
            e.to_string()
        }
        _ => String::new(),
    }
}

#[test]
fn the_schedule_defaults_to_geometric() {
    let sampler = RecordingSampler::default();
    let _ = solve(&sampler, &identity(), VALID).expect("the reference problem must solve");
    assert_eq!(
        sampler.seen().expect("sampler called").schedule,
        BetaSchedule::Geometric
    );
}

#[test]
fn a_linear_schedule_reaches_a_solver_that_lists_the_feature() {
    let id = BackendIdentity {
        features: &[LINEAR_SCHEDULE_FEATURE],
        ..identity()
    };
    let sampler = RecordingSampler::default();
    let out = solve(
        &sampler,
        &id,
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],"num_reads":1,"num_sweeps":4,
             "sweeps_per_beta":1,"beta_range":null,"beta_schedule_type":"linear","seed":1}"#,
    )
    .expect("a linear schedule must solve on a solver that lists the feature");
    assert_eq!(
        sampler.seen().expect("sampler called").schedule,
        BetaSchedule::Linear
    );
    let v: serde_json::Value = serde_json::from_slice(&out).expect("JSON");
    assert_eq!(
        v.pointer("/0/sampler/beta_schedule_type"),
        Some(&serde_json::json!("linear"))
    );
}

/// A solver that does not list `linear-schedule` would run geometric and say
/// nothing; the request is refused instead, as an older SDK refuses the key.
#[test]
fn a_linear_schedule_is_refused_without_the_feature() {
    let detail = reject_reason(
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],"num_reads":1,"num_sweeps":4,
             "sweeps_per_beta":1,"beta_range":null,"beta_schedule_type":"linear","seed":1}"#,
    );
    assert!(detail.contains(LINEAR_SCHEDULE_FEATURE), "{detail}");
}

#[test]
fn an_unknown_schedule_name_is_malformed() {
    let detail = reject_reason(
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],"num_reads":1,"num_sweeps":4,
             "sweeps_per_beta":1,"beta_range":null,"beta_schedule_type":"cosine","seed":1}"#,
    );
    assert!(detail.contains("cosine"), "{detail}");
}

#[test]
fn every_solution_carries_timing_and_sampler_metadata() {
    let sampler = RecordingSampler::default();
    let out = solve(&sampler, &identity(), VALID).expect("the reference problem must solve");
    let v: serde_json::Value = serde_json::from_slice(&out).expect("JSON");
    let reads = v.as_array().expect("an array of solutions");
    assert_eq!(reads.len(), 2);
    for read in reads {
        let timing = read.get("timing").expect("timing");
        let sample_us = timing.get("sample_us").and_then(serde_json::Value::as_u64);
        let total_us = timing.get("total_us").and_then(serde_json::Value::as_u64);
        assert!(sample_us <= total_us, "{timing}");
        assert_eq!(
            read.get("sampler"),
            Some(&serde_json::json!({
                "backend": "mock",
                "algorithm": "sa",
                "num_reads": 2,
                "num_sweeps": 8,
                "sweeps_per_beta": 1,
                "beta_range": null,
                "beta_schedule_type": "geometric",
                "seed": 7,
            }))
        );
    }
}

#[test]
fn node_and_edge_caps_are_capacity() {
    let nodes = BackendIdentity {
        max_nodes: 1,
        ..identity()
    };
    assert!(capacity_reason(&nodes, VALID).contains("nodes"));
    let edges = BackendIdentity {
        max_edges: 1,
        ..identity()
    };
    let two_edges = br#"{"h":[1.0,-1.0,0.0],"j":[1.0,1.0],"edges":[[0,1],[1,2]],
        "num_reads":1,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#;
    assert!(capacity_reason(&edges, two_edges).contains("edges"));
}

/// A star with centre 0 and three leaves, couplings of one unit.
const STAR: &[u8] = br#"{"h":[0.5,0.0,0.0,0.0],"j":[1.0,-1.0,1.0],"edges":[[0,1],[0,2],[0,3]],
    "num_reads":1,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#;

fn limited(limits: ModelLimits) -> BackendIdentity {
    BackendIdentity {
        limits,
        ..identity()
    }
}

#[test]
fn a_model_at_the_degree_limit_solves_and_one_above_is_capacity() {
    let at = limited(ModelLimits {
        max_degree: Some(3),
        ..ModelLimits::UNSTATED
    });
    assert!(solve(&RecordingSampler::default(), &at, STAR).is_ok());
    let below = limited(ModelLimits {
        max_degree: Some(2),
        ..ModelLimits::UNSTATED
    });
    assert!(capacity_reason(&below, STAR).contains("degree 3"));
}

#[test]
fn a_model_at_the_field_limit_solves_and_one_above_is_capacity() {
    // Centre: |0.5| + 3 × 1.0 = 3.5 units.
    let at = limited(ModelLimits {
        max_local_field_milli: Some(3500),
        ..ModelLimits::UNSTATED
    });
    assert!(solve(&RecordingSampler::default(), &at, STAR).is_ok());
    let below = limited(ModelLimits {
        max_local_field_milli: Some(3499),
        ..ModelLimits::UNSTATED
    });
    assert!(capacity_reason(&below, STAR).contains("local field 3500"));
}

/// The resolution is information until the solver lists
/// `exact-coefficients`; then an off-step coefficient is refused.
#[test]
fn an_off_step_coefficient_is_capacity_only_for_an_exact_solver() {
    let step = ModelLimits {
        exact_resolution_milli: Some(1000),
        ..ModelLimits::UNSTATED
    };
    assert!(solve(&RecordingSampler::default(), &limited(step), STAR).is_ok());
    let exact = BackendIdentity {
        features: &[EXACT_COEFFICIENTS_FEATURE],
        ..limited(step)
    };
    assert!(capacity_reason(&exact, STAR).contains("500"));
}
