//! Integer quadratic models: the `IQM_SAMPLE` job kind.
//!
//! A solver opts in with [`Sampler::accepts_iqm`] and samples with
//! [`Sampler::sample_iqm`]. The session and `--solve` parse the problem into
//! an [`IqmModel`], check every assignment the sampler returns against the
//! bounds, and score it themselves, so a reported energy is always the energy
//! of the values sent with it.

use crate::backend::BackendIdentity;
use crate::coefficient::Coefficient;
use crate::error::SampleError;
use crate::ising::SampleParams;
use crate::job::{deadline_passed, miner, os_seed, pick_param, SessionTarget};
use crate::pipeline::Control;
use crate::{CancelToken, Sampler};
use quip_proto::v1::{
    miner_msg, Fatal, IqmProblem, Job, JobRequest, MinerMsg, RejectReason, Result as JobResult,
    SamplerMeta, Solution,
};
use quip_protocol::scoring::iqm_energy_from_milli;
use quip_protocol::session::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// An integer quadratic model: variable `i` takes an integer value from `0`
/// to `upper_bounds[i]`, inclusive, and the energy of an assignment `x` is
/// `Σ h_i·x_i + Σ j_k·x_u·x_v` over the pairs `(u, v)` in `edges`.
///
/// Coefficients are exact milli values. The wire rejects any coefficient that
/// is not, so no conversion loses precision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IqmModel {
    /// Inclusive upper bound of each variable. The lower bound is 0.
    pub upper_bounds: Vec<u32>,
    /// Linear coefficient of each variable, in milli.
    pub h_milli: Vec<i32>,
    /// Coefficient of each pair in `edges`, in milli.
    pub j_milli: Vec<i32>,
    /// Pairs of distinct variable indices.
    pub edges: Vec<(usize, usize)>,
}

impl IqmModel {
    /// Number of variables.
    #[must_use]
    pub fn num_vars(&self) -> usize {
        self.upper_bounds.len()
    }

    /// The consensus energy of `values`, in milli.
    #[must_use]
    pub fn energy_milli(&self, values: &[u32]) -> i64 {
        iqm_energy_from_milli(values, &self.h_milli, &self.j_milli, &self.edges)
    }

    /// Check that `values` holds one value per variable, each within its
    /// bound.
    ///
    /// # Errors
    ///
    /// Returns why `values` is not an assignment of this model.
    pub fn check_values(&self, values: &[u32]) -> Result<(), String> {
        if values.len() != self.num_vars() {
            return Err(format!(
                "{} values for {} variables",
                values.len(),
                self.num_vars()
            ));
        }
        for (i, (&x, &bound)) in values.iter().zip(&self.upper_bounds).enumerate() {
            if x > bound {
                return Err(format!("variable {i} is {x}, above its bound {bound}"));
            }
        }
        Ok(())
    }

    /// The shape every model must have: one bias per bound, one coupling per
    /// pair, and every pair two distinct variables inside the model.
    pub(crate) fn check_shape(&self) -> Result<(), String> {
        if self.num_vars() == 0 {
            return Err("the model has no variables".into());
        }
        if self.h_milli.len() != self.num_vars() {
            return Err(format!(
                "h has {} entries for {} bounds",
                self.h_milli.len(),
                self.num_vars()
            ));
        }
        crate::job::validate_shape(self.num_vars(), self.j_milli.len(), &self.edges)
    }

    /// Parse and check a wire problem. Every defect is `Malformed`.
    pub(crate) fn from_proto(problem: &IqmProblem) -> Result<Self, RejectReason> {
        let edges = match &problem.edges {
            Some(e) => crate::job::dense_pairs(e)?,
            None => Vec::new(),
        };
        let decode = |bytes: &[u8]| {
            crate::encoding::decode_milli_bytes(problem.encoding, problem.scale, bytes)
        };
        let model = Self {
            upper_bounds: problem.upper_bounds.clone(),
            h_milli: decode(&problem.h)?,
            j_milli: decode(&problem.j)?,
            edges,
        };
        model.check_shape().map_err(|_| RejectReason::Malformed)?;
        Ok(model)
    }
}

/// Sample `model`, check each assignment, and score it.
///
/// # Errors
///
/// Returns the sampler's error, or a `DeviceFault` when it returned other
/// than `params.num_reads` assignments, or an assignment of the wrong length
/// or outside a bound: the sampler is broken.
pub(crate) fn sample<S: Sampler<C>, C: Coefficient>(
    sampler: &S,
    model: &IqmModel,
    params: &SampleParams,
) -> Result<Vec<(Vec<u32>, i64)>, SampleError> {
    let assignments = sampler.sample_iqm(model, params)?;
    if assignments.len() != params.num_reads {
        return Err(SampleError::DeviceFault(format!(
            "sample_iqm returned {} assignments for num_reads {}",
            assignments.len(),
            params.num_reads
        )));
    }
    assignments
        .into_iter()
        .map(|values| {
            model
                .check_values(&values)
                .map_err(|why| SampleError::DeviceFault(format!("sample_iqm returned {why}")))?;
            let energy = model.energy_milli(&values);
            Ok((values, energy))
        })
        .collect()
}

/// An `IQM_SAMPLE` job that passed every check, ready to sample.
pub(crate) struct PreparedIqm {
    pub(crate) job_id: Vec<u8>,
    pub(crate) model: IqmModel,
    pub(crate) params: SampleParams,
    /// The job's generation, or `None` for generation 0, which no `Cancel`
    /// abandons.
    pub(crate) watermark: Option<u64>,
}

/// Check one `IQM_SAMPLE` job and resolve its budget: per-job override, then
/// the `SetTarget` value, then the default. Refusals carry the job id.
pub(crate) fn prepare(
    job: Job,
    id: &BackendIdentity,
    max_reads: u32,
    default_sweeps: usize,
    sweeps_per_beta: Option<usize>,
    target: Option<&SessionTarget>,
) -> Result<PreparedIqm, (Vec<u8>, RejectReason)> {
    let job_id = job.job_id;
    let refuse = |reason| Err((job_id.clone(), reason));
    if deadline_passed(job.deadline_ms) {
        return refuse(RejectReason::Expired);
    }
    let Some(problem) = job.iqm else {
        return refuse(RejectReason::Malformed);
    };
    let model = match IqmModel::from_proto(&problem) {
        Ok(model) => model,
        Err(reason) => return refuse(reason),
    };
    let over = |len: usize, cap: u32| cap != 0 && len > cap as usize;
    if over(model.num_vars(), id.max_nodes) || over(model.edges.len(), id.max_edges) {
        return refuse(RejectReason::TooLarge);
    }
    let num_reads = pick_param(
        problem.num_reads,
        target.map_or(0, |t| t.num_reads),
        None,
        1,
    );
    if num_reads > max_reads {
        return refuse(RejectReason::TooLarge);
    }
    let num_sweeps = pick_param(
        problem.num_sweeps,
        target.map_or(0, |t| t.num_sweeps),
        None,
        u32::try_from(default_sweeps).unwrap_or(u32::MAX),
    );
    let Some(seed) = os_seed() else {
        return refuse(RejectReason::Overloaded);
    };
    Ok(PreparedIqm {
        job_id: job_id.clone(),
        model,
        params: SampleParams {
            num_reads: num_reads as usize,
            num_sweeps: num_sweeps as usize,
            sweeps_per_beta: sweeps_per_beta.unwrap_or(1),
            seed,
            ..SampleParams::default()
        },
        watermark: (job.generation != 0).then_some(job.generation),
    })
}

/// The replies to one sampled job, each path ending with one credit, except a
/// device fault, which ends the session with `Fatal`. A completed job whose
/// generation was cancelled sends only the credit.
fn replies(
    job: &PreparedIqm,
    outcome: Result<Vec<(Vec<u32>, i64)>, SampleError>,
    cancelled: bool,
    device_access_time_us: u64,
) -> Vec<MinerMsg> {
    let credit = || miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 }));
    let samples = match outcome {
        Ok(_) if cancelled => return vec![credit()],
        Ok(samples) => samples,
        Err(err) if err.is_fatal() => {
            return vec![
                crate::job::reject(job.job_id.clone(), err.to_reject_reason()),
                miner(miner_msg::Msg::Fatal(Fatal {
                    exit_code: ExitCode::InternalFatal as u32,
                    reason: err.to_string(),
                    restart_required: true,
                })),
            ];
        }
        Err(err) => {
            return vec![
                crate::job::reject(job.job_id.clone(), err.to_reject_reason()),
                credit(),
            ]
        }
    };
    let result = JobResult {
        job_id: job.job_id.clone(),
        solutions: samples
            .into_iter()
            .map(|(values, energy_milli)| Solution {
                energy_milli,
                spins: vec![],
                values,
            })
            .collect(),
        meta: Some(SamplerMeta {
            reads: u32::try_from(job.params.num_reads).unwrap_or(u32::MAX),
            sweeps: u32::try_from(job.params.num_sweeps).unwrap_or(u32::MAX),
            device_access_time_us,
            ..SamplerMeta::default()
        }),
        ..JobResult::default()
    };
    vec![miner(miner_msg::Msg::Result(result)), credit()]
}

/// The session state one IQM job reports into.
pub(crate) struct IqmSession {
    /// One permit per `sample_iqm` call allowed at once: the stream width.
    pub(crate) permits: Arc<tokio::sync::Semaphore>,
    pub(crate) ctrl: mpsc::Sender<Control>,
    pub(crate) cancel: CancelToken,
    pub(crate) jobs_done: Arc<AtomicU64>,
    pub(crate) device_faulted: Arc<AtomicBool>,
    pub(crate) aborted: Arc<AtomicBool>,
}

/// Answer one `IQM_SAMPLE` job: send the refusal [`prepare`] returned, or
/// sample the prepared job on a blocking thread and send its replies. Every
/// path but a device fault ends with one credit. A device fault marks the
/// session faulted, and the read loop then ends it.
pub(crate) async fn run<S: Sampler<C>, C: Coefficient>(
    sampler: Arc<S>,
    prepared: Result<PreparedIqm, (Vec<u8>, RejectReason)>,
    session: IqmSession,
) {
    let job = match prepared {
        Ok(job) => Arc::new(job),
        Err((job_id, reason)) => {
            let credit = miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 }));
            for reply in [crate::job::reject(job_id, reason), credit] {
                if session.ctrl.send(reply.into()).await.is_err() {
                    return;
                }
            }
            return;
        }
    };
    let worker = Arc::clone(&job);
    let Ok(permit) = Arc::clone(&session.permits).acquire_owned().await else {
        return;
    };
    let (outcome, device_us) = tokio::task::spawn_blocking(move || {
        // Held until the call returns, even when shutdown aborts this task.
        let _permit = permit;
        let started = Instant::now();
        let outcome = sample(&*sampler, &worker.model, &worker.params);
        let device_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        (outcome, device_us)
    })
    .await
    .unwrap_or_else(|error| {
        // A sampler that panics is in a state nothing can trust.
        let detail = format!("sample_iqm panicked: {error}");
        (Err(SampleError::DeviceFault(detail)), 0)
    });
    let completed = outcome.is_ok();
    let fatal = outcome.as_ref().is_err_and(SampleError::is_fatal);
    let cancelled = session.cancel.is_cancelled(job.watermark);
    for reply in replies(&job, outcome, cancelled, device_us) {
        if session.ctrl.send(reply.into()).await.is_err() {
            return;
        }
    }
    if fatal {
        session.device_faulted.store(true, Ordering::Relaxed);
        session.aborted.store(true, Ordering::Relaxed);
    } else if completed && !cancelled {
        let _ = session.jobs_done.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SampleError, SampleParams, Sampler};
    use quip_proto::v1::{CoefficientEncoding, EdgeList, IqmProblem, Job, JobKind, RejectReason};
    use quip_protocol::wire::encode_i32_le;

    fn problem() -> IqmProblem {
        IqmProblem {
            edges: Some(EdgeList {
                u: vec![0, 1],
                v: vec![1, 2],
            }),
            upper_bounds: vec![1, 4, 5],
            encoding: CoefficientEncoding::I32 as i32,
            scale: 1000,
            h: encode_i32_le(&[-2, 0, 3]),
            j: encode_i32_le(&[1, -2]),
            num_reads: 2,
            num_sweeps: 0,
        }
    }

    #[test]
    fn a_valid_problem_parses_to_milli_coefficients() {
        let model = IqmModel::from_proto(&problem()).expect("valid");
        assert_eq!(model.upper_bounds, [1, 4, 5]);
        assert_eq!(model.h_milli, [-2, 0, 3]);
        assert_eq!(model.j_milli, [1, -2]);
        assert_eq!(model.edges, [(0, 1), (1, 2)]);
        assert_eq!(model.num_vars(), 3);
        // h terms -2 + 0 + 15 = 13; pairs 1*1*4 = 4 and -2*4*5 = -40.
        assert_eq!(model.energy_milli(&[1, 4, 5]), -23);
    }

    #[test]
    fn integer_and_float_encodings_follow_the_ising_rules() {
        let mut i8_scaled = problem();
        i8_scaled.encoding = CoefficientEncoding::I8 as i32;
        i8_scaled.scale = 1;
        i8_scaled.h = vec![1, 0, 0xFF];
        i8_scaled.j = vec![2, 0];
        let model = IqmModel::from_proto(&i8_scaled).expect("i8 at scale 1");
        assert_eq!(model.h_milli, [1000, 0, -1000]);
        assert_eq!(model.j_milli, [2000, 0]);

        let mut f64_units = problem();
        f64_units.encoding = CoefficientEncoding::F64 as i32;
        f64_units.scale = 0;
        f64_units.h = [0.5_f64, 0.0, -1.25]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        f64_units.j = [0.001_f64, 2.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let model = IqmModel::from_proto(&f64_units).expect("f64 units");
        assert_eq!(model.h_milli, [500, 0, -1250]);
        assert_eq!(model.j_milli, [1, 2000]);
    }

    #[test]
    fn each_malformed_problem_is_refused() {
        type Edit = fn(&mut IqmProblem);
        let rows: [(&str, Edit); 9] = [
            ("one bound short", |p| p.upper_bounds.truncate(2)),
            ("one bias short", |p| p.h.truncate(8)),
            ("a partial element", |p| p.h.truncate(11)),
            ("one coupling short", |p| p.j.truncate(4)),
            ("unspecified encoding", |p| p.encoding = 0),
            ("integer scale 0", |p| p.scale = 0),
            ("self-loop", |p| {
                p.edges = Some(EdgeList {
                    u: vec![0, 1],
                    v: vec![1, 1],
                });
            }),
            ("endpoint out of range", |p| {
                p.edges = Some(EdgeList {
                    u: vec![0, 1],
                    v: vec![1, 3],
                });
            }),
            ("ragged edge list", |p| {
                p.edges = Some(EdgeList {
                    u: vec![0, 1],
                    v: vec![1],
                });
            }),
        ];
        for (label, edit) in rows {
            let mut p = problem();
            edit(&mut p);
            assert_eq!(
                IqmModel::from_proto(&p).err(),
                Some(RejectReason::Malformed),
                "{label}"
            );
        }
        // No edges at all is a model of independent variables.
        let mut no_edges = problem();
        no_edges.edges = None;
        no_edges.j.clear();
        assert!(IqmModel::from_proto(&no_edges).is_ok());
        // A model with no variables has nothing to sample, and its solutions
        // would look like empty Ising ones: refused.
        let empty = IqmProblem {
            upper_bounds: vec![],
            h: vec![],
            j: vec![],
            edges: None,
            ..problem()
        };
        assert_eq!(
            IqmModel::from_proto(&empty).err(),
            Some(RejectReason::Malformed)
        );
    }

    #[test]
    fn values_must_cover_every_variable_within_its_bound() {
        let model = IqmModel::from_proto(&problem()).expect("valid");
        assert_eq!(model.check_values(&[0, 4, 5]), Ok(()));
        assert!(model.check_values(&[0, 4]).is_err(), "too few");
        assert!(model.check_values(&[0, 4, 5, 0]).is_err(), "too many");
        let over = model.check_values(&[2, 0, 0]).unwrap_err();
        assert!(over.contains("variable 0"), "{over}");
    }

    fn job(problem: IqmProblem) -> Job {
        Job {
            job_id: b"iqm".to_vec(),
            kind: JobKind::IqmSample as i32,
            iqm: Some(problem),
            ..Default::default()
        }
    }

    fn identity() -> BackendIdentity {
        BackendIdentity {
            backend: quip_proto::v1::Backend::Mock,
            algorithm: quip_proto::v1::Algorithm::Sa,
            max_nodes: 100,
            max_edges: 200,
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

    fn reason(job: Job) -> Option<RejectReason> {
        prepare(job, &identity(), u32::MAX, 64, None, None)
            .err()
            .map(|(_, reason)| reason)
    }

    #[test]
    fn prepare_refuses_missing_large_and_expired_jobs() {
        assert_eq!(reason(job(problem())), None);
        let mut missing = job(problem());
        missing.iqm = None;
        assert_eq!(reason(missing), Some(RejectReason::Malformed));
        let mut expired = job(problem());
        expired.deadline_ms = 1;
        assert_eq!(reason(expired), Some(RejectReason::Expired));

        let small = BackendIdentity {
            max_nodes: 2,
            ..identity()
        };
        let refused = prepare(job(problem()), &small, u32::MAX, 64, None, None).err();
        assert_eq!(refused.map(|(_, r)| r), Some(RejectReason::TooLarge));
        let few_reads = prepare(job(problem()), &identity(), 1, 64, None, None).err();
        assert_eq!(few_reads.map(|(_, r)| r), Some(RejectReason::TooLarge));
    }

    #[test]
    fn prepare_resolves_reads_and_sweeps() {
        let prepared =
            prepare(job(problem()), &identity(), u32::MAX, 64, Some(3), None).expect("valid");
        assert_eq!(prepared.params.num_reads, 2, "the per-job override");
        assert_eq!(prepared.params.num_sweeps, 64, "the session default");
        assert_eq!(prepared.params.sweeps_per_beta, 3);
        assert_eq!(prepared.watermark, None, "generation 0 is never cancelled");
    }

    struct AtBounds;
    impl Sampler for AtBounds {
        fn sample(
            &self,
            _: &crate::IsingGraph,
            _: &SampleParams,
        ) -> Result<Vec<crate::SamplerResult>, SampleError> {
            Ok(vec![])
        }
        fn accepts_iqm() -> bool {
            true
        }
        fn sample_iqm(
            &self,
            model: &IqmModel,
            params: &SampleParams,
        ) -> Result<Vec<Vec<u32>>, SampleError> {
            Ok(vec![model.upper_bounds.clone(); params.num_reads])
        }
    }

    struct OutOfBounds;
    impl Sampler for OutOfBounds {
        fn sample(
            &self,
            _: &crate::IsingGraph,
            _: &SampleParams,
        ) -> Result<Vec<crate::SamplerResult>, SampleError> {
            Ok(vec![])
        }
        fn accepts_iqm() -> bool {
            true
        }
        fn sample_iqm(
            &self,
            model: &IqmModel,
            _: &SampleParams,
        ) -> Result<Vec<Vec<u32>>, SampleError> {
            Ok(vec![model.upper_bounds.iter().map(|b| b + 1).collect()])
        }
    }

    #[test]
    fn sample_scores_each_assignment_and_refuses_one_out_of_bounds() {
        let model = IqmModel::from_proto(&problem()).expect("valid");
        let params = SampleParams {
            num_reads: 2,
            ..SampleParams::default()
        };
        let scored = sample(&AtBounds, &model, &params).expect("in bounds");
        assert_eq!(scored, vec![(vec![1, 4, 5], -23); 2]);
        assert!(matches!(
            sample(&OutOfBounds, &model, &params),
            Err(SampleError::DeviceFault(_))
        ));
        // An assignment count other than num_reads is broken output too.
        for num_reads in [1, 3] {
            let other = SampleParams {
                num_reads,
                ..SampleParams::default()
            };
            assert!(
                matches!(
                    sample(&TwoReads, &model, &other),
                    Err(SampleError::DeviceFault(_))
                ),
                "{num_reads} reads"
            );
        }
        assert!(sample(&TwoReads, &model, &params).is_ok());
    }

    /// Answers every model with two all-zero assignments, whatever it is asked.
    struct TwoReads;
    impl Sampler for TwoReads {
        fn sample(
            &self,
            _: &crate::IsingGraph,
            _: &SampleParams,
        ) -> Result<Vec<crate::SamplerResult>, SampleError> {
            Ok(vec![])
        }
        fn accepts_iqm() -> bool {
            true
        }
        fn sample_iqm(
            &self,
            model: &IqmModel,
            _: &SampleParams,
        ) -> Result<Vec<Vec<u32>>, SampleError> {
            Ok(vec![vec![0; model.num_vars()]; 2])
        }
    }

    /// The trait default declines: a sampler that lists `IQM_SAMPLE` must
    /// override `sample_iqm`.
    #[test]
    fn the_default_sample_iqm_declines() {
        struct Plain;
        impl Sampler for Plain {
            fn sample(
                &self,
                _: &crate::IsingGraph,
                _: &SampleParams,
            ) -> Result<Vec<crate::SamplerResult>, SampleError> {
                Ok(vec![])
            }
        }
        assert!(!Plain::accepts_iqm());
        let model = IqmModel::from_proto(&problem()).expect("valid");
        assert!(matches!(
            Plain.sample_iqm(&model, &SampleParams::default()),
            Err(SampleError::DeviceFault(_))
        ));
    }
}
