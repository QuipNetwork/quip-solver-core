//! Driver-mode input validation, exercised through the public API.
//!
//! `driver::solve` is the one-shot front door onto the same samplers the
//! session loop feeds, so it owes them the same guarantees `job::parse_ising`
//! provides on the coordinator path. These tests drive it directly rather than
//! spawning a miner, so they can assert on the [`SolveError`] variant and read
//! back what the sampler was actually handed.

use std::sync::Mutex;

use quip_solver_core::driver::{solve, SolveError};
use quip_solver_core::{IsingGraph, SampleError, SampleParams, Sampler, SamplerResult};

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
    let outcome = match solve(&sampler, input) {
        Ok(_) => "accepted".to_owned(),
        Err(SolveError::Malformed(detail)) => format!("malformed: {detail}"),
        Err(SolveError::Sample(e)) => format!("sample error: {e}"),
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
    let out = solve(&sampler, VALID).expect("the reference problem must solve");
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
        br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],
             "num_reads":5,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#,
    )
    .expect_err("5 reads against a cap of 4 must be rejected");
    assert!(matches!(err, SolveError::Malformed(_)), "{err:?}");
    assert!(sampler.seen().is_none());

    // Exactly at the cap still runs.
    let sampler = RecordingSampler::with_max_reads(4);
    assert!(solve(
        &sampler,
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
    let _ = solve(&sampler, VALID).expect("the reference problem must solve");
    assert_eq!(
        sampler.seen().expect("sampler called").beta_range,
        None,
        "null must mean auto, not a pinned range"
    );
}
