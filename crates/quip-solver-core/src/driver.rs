//! Driver mode: solve one problem and exit.
//!
//! A one-shot caller hands a problem in and reads solutions out. There is no
//! session, no coordinator, and no credits. The JSON shape matches what
//! `quip-solver-exec` already sends to an external solver, so the two are duals
//! and the schema stays single.

use crate::backend::{BackendIdentity, EXACT_COEFFICIENTS_FEATURE, LINEAR_SCHEDULE_FEATURE};
use crate::coefficient::Coefficient;
use crate::error::SampleError;
use crate::ising::{BetaLadder, BetaSchedule, IsingGraph, SampleParams};
use crate::Sampler;
use quip_protocol::session::{algorithm_name, backend_name};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// One problem, as read from stdin.
///
/// `deny_unknown_fields` because every field but `beta_schedule_type` is
/// required and none carries a serde default: a caller who misspells one gets
/// a missing-field error rather than a silent default, and an unrecognized key
/// (a typo alongside the real one, or a field from a newer schema) is refused
/// rather than dropped. A caller reads `--capabilities` before sending a key
/// an older binary would refuse.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProblemJson {
    /// Linear biases, one per variable.
    pub h: Vec<f64>,
    /// Couplings aligned with `edges`.
    pub j: Vec<f64>,
    /// Undirected edge list. A self-loop `(u, u)` is malformed.
    pub edges: Vec<(usize, usize)>,
    /// Independent reads to produce.
    pub num_reads: usize,
    /// Annealing sweeps per read.
    pub num_sweeps: usize,
    /// Sweeps spent at each beta rung.
    pub sweeps_per_beta: usize,
    /// Optional explicit `(hot_beta, cold_beta)`.
    pub beta_range: Option<(f64, f64)>,
    /// Shape of the ladder, `"geometric"` or `"linear"`. Optional: absent is
    /// geometric. `"linear"` needs a binary that lists `linear-schedule`.
    #[serde(default)]
    pub beta_schedule_type: BetaSchedule,
    /// PRNG seed.
    pub seed: u64,
    /// Per-algorithm options: names the solver lists in `--capabilities`
    /// under `solveOptions`, each with a JSON scalar of the listed type.
    /// Optional: absent is no options. A name the solver does not list, or a
    /// value of another type, is refused before sampling.
    pub options: Option<serde_json::Map<String, serde_json::Value>>,
}

/// One completed read, as written to stdout.
#[derive(Debug, Serialize)]
pub struct SolutionJson<'a> {
    /// Spin configuration, values in `{-1, +1}`.
    pub spins: Vec<i8>,
    /// Consensus energy in milli units.
    pub energy_milli: i64,
    /// How long the solve took. The same for every read of one solve.
    pub timing: TimingJson,
    /// Which sampler ran, with which knobs. The same for every read of one
    /// solve.
    pub sampler: &'a SamplerJson,
}

/// Wall-clock timing of one solve, in microseconds.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct TimingJson {
    /// Time inside the sampler.
    pub sample_us: u64,
    /// Time from reading the problem to the last scored read, sampling
    /// included.
    pub total_us: u64,
}

/// The sampler and the knobs one solve ran with.
#[derive(Clone, Debug, Serialize)]
pub struct SamplerJson {
    /// Backend name, as `--capabilities` spells it.
    pub backend: &'static str,
    /// Algorithm name, as `--capabilities` spells it.
    pub algorithm: &'static str,
    /// Reads requested.
    pub num_reads: usize,
    /// Sweeps per read requested.
    pub num_sweeps: usize,
    /// Sweeps per beta rung.
    pub sweeps_per_beta: usize,
    /// `(hot_beta, cold_beta)` as sent, or `null` when the binary chose.
    pub beta_range: Option<(f64, f64)>,
    /// `"geometric"` or `"linear"`.
    pub beta_schedule_type: &'static str,
    /// The ladder the solve ran, as the sampler reported it or as built from
    /// the request. Its `kind` equals `beta_schedule_type`.
    pub beta_ladder: ReportedLadder,
    /// PRNG seed.
    pub seed: u64,
}

/// A ladder and who stated it.
#[derive(Clone, Debug, Serialize)]
pub struct ReportedLadder {
    /// The kind and rungs.
    #[serde(flatten)]
    pub ladder: BetaLadder,
    /// `"sampler"` when [`Sampler::beta_ladder`] supplied the ladder, `"sdk"`
    /// when the SDK built it from the request.
    pub reported_by: &'static str,
}

/// The ladder `sampler` states for this job, or the one built from the request.
fn reported_ladder<S: Sampler<C>, C: Coefficient>(
    sampler: &S,
    graph: &IsingGraph<C>,
    params: &SampleParams,
) -> ReportedLadder {
    match sampler.beta_ladder(graph, params) {
        Some(ladder) => ReportedLadder {
            ladder,
            reported_by: "sampler",
        },
        None => ReportedLadder {
            ladder: BetaLadder::from_params(graph, params),
            reported_by: "sdk",
        },
    }
}

/// Why [`solve`] could not produce solutions.
#[derive(Debug)]
pub enum SolveError {
    /// The input was not valid problem JSON. A caller error, not a device
    /// condition: retrying the same bytes fails again no matter the device
    /// state.
    Malformed(String),
    /// The model exceeds what this binary solves: its node, edge or read caps,
    /// or its advertised [`ModelLimits`](crate::ModelLimits). The same model
    /// fails again here and may succeed on another binary.
    Capacity(String),
    /// The sampler could not complete the job.
    Sample(SampleError),
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "malformed problem JSON: {detail}"),
            Self::Capacity(detail) => write!(f, "model exceeds this solver: {detail}"),
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

impl SolveError {
    /// The process exit code `--solve` reports for this error (SPEC section 2).
    #[must_use]
    pub fn exit_code(&self) -> crate::ExitCode {
        match self {
            Self::Malformed(_) => crate::ExitCode::ConfigInvalid,
            Self::Capacity(_) | Self::Sample(SampleError::Capacity) => crate::ExitCode::Capacity,
            Self::Sample(_) => crate::ExitCode::InternalFatal,
        }
    }
}

fn convert_units<C: Coefficient>(name: &str, values: &[f64]) -> Result<Vec<C>, SolveError> {
    values
        .iter()
        .enumerate()
        .map(|(index, &unit)| {
            C::from_unit(unit).ok_or_else(|| {
                SolveError::Malformed(format!(
                    "{name}[{index}] cannot be represented by this coefficient type"
                ))
            })
        })
        .collect()
}

/// The `options` object checked against what `S` declares, or none.
fn checked_options<S: Sampler<C>, C: Coefficient>(
    raw: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<crate::options::SolveOptions, SolveError> {
    raw.map_or_else(
        || Ok(crate::options::SolveOptions::new()),
        |raw| crate::options::validate(raw, S::solve_options()).map_err(SolveError::Malformed),
    )
}

/// Microseconds since `start`, saturating.
fn micros_since(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// Check everything about a parsed problem except its coefficients: the
/// shape, the read and sweep floors, the schedule kind against the solver's
/// features, and the read, node and edge caps.
fn check_request<S: Sampler<C>, C: Coefficient>(
    sampler: &S,
    id: &BackendIdentity,
    p: &ProblemJson,
) -> Result<(), SolveError> {
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
    // Mirrors the `--sweeps-per-beta` floor in `CommonArgs`: the same field,
    // reached through the other door. Backends divide the sweep budget by it.
    if p.sweeps_per_beta == 0 {
        return Err(SolveError::Malformed(
            "sweeps_per_beta must be at least 1".into(),
        ));
    }
    // A binary that does not read the schedule kind would run geometric and
    // report nothing, so a linear request reaches only one that says it can.
    if p.beta_schedule_type == BetaSchedule::Linear
        && !id.features.contains(&LINEAR_SCHEDULE_FEATURE)
    {
        return Err(SolveError::Malformed(format!(
            "beta_schedule_type \"linear\" needs a solver that lists \"{LINEAR_SCHEDULE_FEATURE}\""
        )));
    }

    // The session path applies these caps in `prepare_job`; a backend with a
    // device-memory bound is entitled to them here too, before it allocates.
    // `0` in either size cap means no limit, as in the session.
    let max_reads = usize::try_from(sampler.max_reads()).unwrap_or(usize::MAX);
    if p.num_reads > max_reads {
        return Err(SolveError::Capacity(format!(
            "num_reads {} exceeds this backend's maximum of {max_reads}",
            p.num_reads
        )));
    }
    let over = |len: usize, cap: u32| cap != 0 && len > usize::try_from(cap).unwrap_or(usize::MAX);
    if over(p.h.len(), id.max_nodes) {
        return Err(SolveError::Capacity(format!(
            "{} nodes exceed this backend's maximum of {}",
            p.h.len(),
            id.max_nodes
        )));
    }
    if over(p.edges.len(), id.max_edges) {
        return Err(SolveError::Capacity(format!(
            "{} edges exceed this backend's maximum of {}",
            p.edges.len(),
            id.max_edges
        )));
    }
    Ok(())
}

/// Solve one JSON problem for the solver `id` describes, and render its
/// solutions as JSON.
///
/// `id` supplies the caps and limits the problem is checked against, the
/// features that decide what it may ask for, and the names the output reports.
///
/// # Errors
///
/// Returns [`SolveError::Malformed`] when `input` is not valid problem JSON or
/// does not describe a coherent problem, [`SolveError::Capacity`] when the
/// model exceeds what this solver solves, and [`SolveError::Sample`] when the
/// sampler cannot complete the job. [`SolveError::exit_code`] maps each to its
/// exit code; only the last is a device condition.
///
/// # Panics
///
/// Never. Serializing `Vec<SolutionJson>` cannot fail.
pub fn solve<S: Sampler<C>, C: Coefficient>(
    sampler: &S,
    id: &BackendIdentity,
    input: &[u8],
) -> Result<Vec<u8>, SolveError> {
    let started = Instant::now();
    let p: ProblemJson =
        serde_json::from_slice(input).map_err(|e| SolveError::Malformed(e.to_string()))?;

    check_request(sampler, id, &p)?;
    let options = checked_options::<S, C>(p.options.as_ref())?;

    // JSON also accepts sub-milli values and values outside the wire range.
    // Preserve those inputs, while wire-representable models share the session
    // conversion and exactness check.
    let milli = |values: &[f64]| {
        values
            .iter()
            .copied()
            .map(crate::encoding::float_milli)
            .collect::<Result<Vec<_>, _>>()
    };
    let (h_milli, j_milli) = (milli(&p.h), milli(&p.j));

    // The limits read milli. A wire-representable value uses its exact milli;
    // any other keeps its fraction, so it is off every exact-resolution step.
    let limit_view = |exact: &Result<Vec<i32>, _>, units: &[f64]| match exact {
        Ok(values) => values.iter().map(|&v| f64::from(v)).collect::<Vec<_>>(),
        Err(_) => units.iter().map(|v| v * 1000.0).collect(),
    };
    let exact_only = id.features.contains(&EXACT_COEFFICIENTS_FEATURE);
    id.limits
        .check(
            &limit_view(&h_milli, &p.h),
            &limit_view(&j_milli, &p.j),
            &p.edges,
            exact_only,
        )
        .map_err(SolveError::Capacity)?;

    let edges: std::sync::Arc<[(usize, usize)]> = p.edges.into();
    let (h, j, exact_energy, rescore_units) = match (h_milli, j_milli) {
        (Ok(h), Ok(j)) => {
            let (h, j, original) = crate::encoding::convert_milli::<C>(h, j);
            let exact_energy = original
                .map(|(h, j)| crate::job::ExactEnergy::new(h, j, std::sync::Arc::clone(&edges)));
            (h, j, exact_energy, false)
        }
        _ => (
            convert_units::<C>("h", &p.h)?,
            convert_units::<C>("j", &p.j)?,
            None,
            !C::EXACT,
        ),
    };
    // Refuse-not-quantize: a model this coefficient type would round or
    // saturate is one the binary does not represent exactly.
    if exact_only && (exact_energy.is_some() || rescore_units) {
        return Err(SolveError::Capacity(
            "a coefficient is not exactly representable by this solver".into(),
        ));
    }
    let graph = IsingGraph::<C> { h, j, edges };
    let params = SampleParams {
        num_reads: p.num_reads,
        num_sweeps: p.num_sweeps,
        sweeps_per_beta: p.sweeps_per_beta,
        beta_range: p.beta_range,
        schedule: p.beta_schedule_type,
        seed: p.seed,
        options,
    };
    let sample_started = Instant::now();
    let mut reads = sampler.sample(&graph, &params)?;
    let sample_us = micros_since(sample_started);
    if let Some(original) = exact_energy {
        original.rescore(&mut reads);
    }
    if rescore_units {
        for read in &mut reads {
            read.energy_milli =
                quip_protocol::scoring::energy_milli(&read.spins, &p.h, &p.j, &graph.edges);
        }
    }
    let timing = TimingJson {
        sample_us,
        total_us: micros_since(started),
    };
    let beta_ladder = reported_ladder(sampler, &graph, &params);
    let sampler_json = SamplerJson {
        backend: backend_name(id.backend),
        algorithm: algorithm_name(id.algorithm),
        num_reads: params.num_reads,
        num_sweeps: params.num_sweeps,
        sweeps_per_beta: params.sweeps_per_beta,
        beta_range: params.beta_range,
        beta_schedule_type: beta_ladder.ladder.kind.name(),
        beta_ladder,
        seed: params.seed,
    };
    let out: Vec<SolutionJson<'_>> = reads
        .into_iter()
        .map(|read| SolutionJson {
            spins: read.spins,
            energy_milli: read.energy_milli,
            timing,
            sampler: &sampler_json,
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
    use crate::backend::LINEAR_SCHEDULE_FEATURE;
    use crate::error::SampleError;
    use crate::ising::BetaSchedule;
    use crate::ising::{IsingGraph, SampleParams, SamplerResult};
    use crate::Sampler;

    fn id() -> crate::BackendIdentity {
        crate::BackendIdentity {
            backend: quip_proto::v1::Backend::Mock,
            algorithm: quip_proto::v1::Algorithm::Sa,
            max_nodes: 0,
            max_edges: 0,
            features: &[],
            adapt: crate::adapt::AdaptBounds {
                min_sweeps: 1,
                max_sweeps: 1,
                min_reads: 1,
                max_reads: 1,
                reads_solution_min_factor: 1,
                reads_solution_max_factor: 1,
                reads_solution_floor_factor: 0,
            },
            limits: crate::ModelLimits::UNSTATED,
        }
    }

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
        let err = solve(&StubSampler, &id(), b"not json").expect_err("malformed input must fail");
        assert!(
            matches!(err, SolveError::Malformed(_)),
            "a malformed problem document is a caller error, not {err:?}"
        );
    }

    /// Records the options it was given, in the first read's `spins`.
    struct OptionsProbe;

    impl Sampler for OptionsProbe {
        fn sample(
            &self,
            graph: &IsingGraph,
            params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            // One read whose spins encode `restarts` as a count of `+1`s.
            let restarts = match params.options.get("restarts") {
                Some(crate::OptionValue::Int(n)) => usize::try_from(*n).unwrap_or(0),
                _ => 0,
            };
            let mut spins = vec![-1_i8; graph.h.len()];
            spins.iter_mut().take(restarts).for_each(|s| *s = 1);
            Ok(vec![SamplerResult {
                spins,
                energy_milli: 0,
            }])
        }

        fn solve_options() -> &'static [crate::OptionDecl] {
            &[
                crate::OptionDecl {
                    name: "restarts",
                    kind: quip_proto::v1::OptionType::Int,
                },
                crate::OptionDecl {
                    name: "greedy",
                    kind: quip_proto::v1::OptionType::Bool,
                },
            ]
        }
    }

    fn problem_with(options: &str) -> Vec<u8> {
        format!(
            r#"{{"h":[0,0,0],"j":[],"edges":[],"num_reads":1,"num_sweeps":1,
                "sweeps_per_beta":1,"beta_range":null,"seed":1{options}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn declared_options_reach_the_sampler() {
        let out = solve(
            &OptionsProbe,
            &id(),
            &problem_with(r#","options":{"restarts":2}"#),
        )
        .expect("declared option is accepted");
        let v: serde_json::Value = serde_json::from_slice(&out).expect("json");
        assert_eq!(
            v.pointer("/0/spins").expect("spins"),
            &serde_json::json!([1, 1, -1])
        );
    }

    #[test]
    fn absent_null_and_empty_options_are_no_options() {
        for extra in ["", r#","options":null"#, r#","options":{}"#] {
            let out = solve(&OptionsProbe, &id(), &problem_with(extra)).expect("accepted");
            let v: serde_json::Value = serde_json::from_slice(&out).expect("json");
            assert_eq!(
                v.pointer("/0/spins").expect("spins"),
                &serde_json::json!([-1, -1, -1]),
                "{extra}"
            );
        }
    }

    #[test]
    fn undeclared_and_wrong_type_options_are_refused_before_sampling() {
        for extra in [
            r#","options":{"nope":1}"#,
            r#","options":{"restarts":"2"}"#,
            r#","options":{"restarts":1.5}"#,
            r#","options":{"greedy":1}"#,
            r#","options":[1]"#,
        ] {
            let err = solve(&OptionsProbe, &id(), &problem_with(extra)).expect_err(extra);
            assert!(matches!(err, SolveError::Malformed(_)), "{extra}: {err:?}");
            assert_eq!(err.exit_code(), crate::ExitCode::ConfigInvalid);
        }
    }

    #[test]
    fn a_sampler_that_declares_no_options_refuses_any() {
        let err = solve(
            &StubSampler,
            &id(),
            &problem_with(r#","options":{"restarts":1}"#),
        )
        .expect_err("no options are declared");
        assert!(matches!(err, SolveError::Malformed(_)), "{err:?}");
    }

    struct WrongLossy;
    impl Sampler<crate::coefficient::Fixed<i8, 1>> for WrongLossy {
        fn sample(
            &self,
            graph: &IsingGraph<crate::coefficient::Fixed<i8, 1>>,
            _: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            Ok(vec![SamplerResult {
                spins: vec![1; graph.num_nodes()],
                energy_milli: 777,
            }])
        }
    }

    struct UnchangedFloat;
    impl Sampler for UnchangedFloat {
        #[expect(
            clippy::panic_in_result_fn,
            reason = "test sampler asserts that JSON floats reach the graph with their exact bits"
        )]
        fn sample(
            &self,
            graph: &IsingGraph,
            _: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            assert_eq!(
                graph
                    .h
                    .iter()
                    .copied()
                    .map(f64::to_bits)
                    .collect::<Vec<_>>(),
                vec![
                    0.0004_f64.to_bits(),
                    1e300_f64.to_bits(),
                    (-0.0_f64).to_bits()
                ]
            );
            Ok(vec![SamplerResult {
                spins: vec![1; graph.num_nodes()],
                energy_milli: 777,
            }])
        }
    }

    fn input_with_biases(h: &str) -> Vec<u8> {
        format!(
            r#"{{"h":{h},"j":[],"edges":[],"num_reads":1,
            "num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":0}}"#
        )
        .into_bytes()
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test assertions check behavior while Result propagates unexpected errors"
    )]
    fn lossy_json_uses_original_milli_and_full_scoring_range(
    ) -> Result<(), Box<dyn std::error::Error>> {
        for (biases, expected) in [
            ("[0.0005]", 1_i64),
            ("[-0.0005]", -1),
            ("[0.499,-0.501]", -2),
            ("[2147483.648]", 2_147_483_648),
            ("[-2147483.649]", -2_147_483_649),
            ("[1e300]", i64::MAX),
            ("[-1e300]", i64::MIN),
            ("[1e300,-1e300]", -1),
            // An exact integer problem keeps the sampler's own energy.
            ("[-1,0,1]", 777),
        ] {
            let output = solve(&WrongLossy, &id(), &input_with_biases(biases))
                .map_err(|e| format!("{biases}: solve failed: {e:?}"))?;
            let results: serde_json::Value = serde_json::from_slice(&output)
                .map_err(|e| format!("{biases}: output is not JSON: {e}"))?;
            assert_eq!(
                results
                    .pointer("/0/energy_milli")
                    .and_then(serde_json::Value::as_i64),
                Some(expected),
                "{biases}"
            );
        }
        let input = br#"{"h":[0.499,-0.501],"j":[1.501],"edges":[[0,1]],
            "num_reads":1,"num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":0}"#;
        let results: serde_json::Value =
            serde_json::from_slice(&solve(&WrongLossy, &id(), input)?)?;
        assert_eq!(
            results
                .pointer("/0/energy_milli")
                .and_then(serde_json::Value::as_i64),
            Some(1499)
        );
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test assertions check behavior while Result propagates unexpected errors"
    )]
    fn exact_json_preserves_values_and_sampler_energy() -> Result<(), Box<dyn std::error::Error>> {
        let output = solve(
            &UnchangedFloat,
            &id(),
            &input_with_biases("[0.0004,1e300,-0.0]"),
        )?;
        let results: serde_json::Value = serde_json::from_slice(&output)?;
        assert_eq!(
            results
                .pointer("/0/energy_milli")
                .and_then(serde_json::Value::as_i64),
            Some(777)
        );
        Ok(())
    }

    #[test]
    fn fixed_nonfinite_conversion_is_a_malformed_input() {
        use crate::coefficient::Fixed;
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(matches!(
                super::convert_units::<Fixed<i8, 1>>("h", &[value]),
                Err(SolveError::Malformed(_))
            ));
        }
    }

    /// One all-`+1` read, with the default ladder report.
    struct OneRead;
    impl Sampler for OneRead {
        fn sample(
            &self,
            graph: &IsingGraph,
            _: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            Ok(vec![SamplerResult {
                spins: vec![1; graph.num_nodes()],
                energy_milli: 0,
            }])
        }
    }

    /// A kernel that ignores the schedule kind and keeps a geometric ladder.
    struct OwnLadder;
    impl Sampler for OwnLadder {
        fn sample(
            &self,
            graph: &IsingGraph,
            params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            OneRead.sample(graph, params)
        }

        fn beta_ladder(&self, _: &IsingGraph, _: &SampleParams) -> Option<crate::BetaLadder> {
            Some(crate::BetaLadder {
                kind: BetaSchedule::Geometric,
                betas: vec![0.5, 1.0],
            })
        }
    }

    fn ladder_input(schedule: &str) -> Vec<u8> {
        format!(
            r#"{{"h":[0.0],"j":[],"edges":[],"num_reads":1,"num_sweeps":5,
            "sweeps_per_beta":2,"beta_range":[0.5,2.0],"beta_schedule_type":"{schedule}","seed":0}}"#
        )
        .into_bytes()
    }

    fn reported_sampler<S: Sampler>(sampler: &S) -> serde_json::Value {
        let id = crate::BackendIdentity {
            features: &[LINEAR_SCHEDULE_FEATURE],
            ..id()
        };
        let out = solve(sampler, &id, &ladder_input("linear")).expect("solve");
        let reads: serde_json::Value = serde_json::from_slice(&out).expect("json");
        reads.pointer("/0/sampler").cloned().expect("sampler")
    }

    #[test]
    fn a_sampler_that_builds_no_ladder_reports_the_one_built_from_the_request() {
        let sampler = reported_sampler(&OneRead);
        // 5 sweeps at 2 per rung is 2 rungs, rounded down.
        assert_eq!(
            sampler.get("beta_ladder"),
            Some(&serde_json::json!({"kind": "linear", "betas": [0.5, 2.0], "reported_by": "sdk"}))
        );
        assert_eq!(sampler.get("beta_schedule_type"), Some(&"linear".into()));
    }

    #[test]
    fn a_sampler_that_builds_its_own_ladder_reports_it_and_its_kind() {
        let sampler = reported_sampler(&OwnLadder);
        assert_eq!(
            sampler.get("beta_ladder"),
            Some(
                &serde_json::json!({"kind": "geometric", "betas": [0.5, 1.0], "reported_by": "sampler"})
            )
        );
        assert_eq!(
            sampler.get("beta_schedule_type"),
            Some(&"geometric".into()),
            "the echo states what ran, not what was asked"
        );
    }

    #[test]
    fn the_gate_ladder_is_the_linear_ladder_of_the_core() {
        use quip_solver_conformance::solve::{expected_linear_ladder, LADDER_RANGE, LADDER_RUNGS};
        assert_eq!(
            expected_linear_ladder(),
            BetaSchedule::Linear.build(LADDER_RANGE.0, LADDER_RANGE.1, LADDER_RUNGS)
        );
    }
}

#[cfg(test)]
mod exact_tests {
    use super::{solve, SolveError};
    use crate::coefficient::Fixed;
    use crate::error::SampleError;
    use crate::ising::{IsingGraph, SampleParams, SamplerResult};
    use crate::Sampler;

    struct Narrow;
    impl Sampler<Fixed<i8, 1>> for Narrow {
        fn sample(
            &self,
            graph: &IsingGraph<Fixed<i8, 1>>,
            _: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            Ok(vec![SamplerResult {
                spins: vec![1; graph.num_nodes()],
                energy_milli: 0,
            }])
        }
    }

    fn exact_id() -> crate::BackendIdentity {
        crate::BackendIdentity {
            backend: quip_proto::v1::Backend::Mock,
            algorithm: quip_proto::v1::Algorithm::Sa,
            max_nodes: 0,
            max_edges: 0,
            features: &[crate::EXACT_COEFFICIENTS_FEATURE],
            adapt: crate::adapt::AdaptBounds {
                min_sweeps: 1,
                max_sweeps: 1,
                min_reads: 1,
                max_reads: 1,
                reads_solution_min_factor: 1,
                reads_solution_max_factor: 1,
                reads_solution_floor_factor: 0,
            },
            limits: crate::ModelLimits::UNSTATED,
        }
    }

    fn problem(h: &str) -> Vec<u8> {
        format!(
            r#"{{"h":{h},"j":[],"edges":[],"num_reads":1,
            "num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":0}}"#
        )
        .into_bytes()
    }

    /// Refuse-not-quantize without per-binary code: the coefficient type alone
    /// decides what an exact solver can take.
    #[test]
    fn an_exact_solver_refuses_what_its_type_would_round_or_saturate() {
        for h in ["[0.5]", "[200]", "[0.0004]"] {
            let err = solve(&Narrow, &exact_id(), &problem(h)).expect_err(h);
            assert!(matches!(err, SolveError::Capacity(_)), "{h}: {err:?}");
        }
        assert!(solve(&Narrow, &exact_id(), &problem("[-128,127,0]")).is_ok());
    }

    struct Wide;
    impl Sampler<crate::coefficient::Milli> for Wide {
        #[expect(
            clippy::panic_in_result_fn,
            reason = "test sampler asserts the whole milli value reaches the graph"
        )]
        fn sample(
            &self,
            graph: &IsingGraph<crate::coefficient::Milli>,
            _: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            assert_eq!(graph.h, vec![Fixed(536_870_910)]);
            Ok(vec![SamplerResult {
                spins: vec![1; graph.num_nodes()],
                energy_milli: 0,
            }])
        }
    }

    /// `536870.91 * 1000.0` is `536870910.00000006` in f64, but the value is a
    /// whole number of milli and sits on a step of 1.
    #[test]
    fn an_exact_solver_accepts_whole_milli_that_multiplies_inexactly() {
        let id = crate::BackendIdentity {
            limits: crate::ModelLimits {
                exact_resolution_milli: Some(1),
                ..crate::ModelLimits::UNSTATED
            },
            ..exact_id()
        };
        let result = solve(&Wide, &id, &problem("[536870.91]"));
        assert!(result.is_ok(), "whole milli on the step: {result:?}");
    }
}
