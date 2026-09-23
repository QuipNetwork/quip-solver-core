//! The Quip solver contract.
//!
//! Holds the [`Sampler`] trait every solver implements, the base Ising types,
//! the CSR representation GPU backends upload, the beta ladder, the adaptive
//! sampling budget, and the generic gRPC session loop. A solver provides a
//! [`Sampler`] and calls [`run`]; this crate owns everything else (Hello and
//! Welcome, Configure, credits, reject reasons, Status, Shutdown, exit codes).

pub mod adapt;
pub mod beta;
pub mod cli;
pub mod coefficient;
pub mod config;
pub mod csr;
mod display;
pub mod driver;
pub mod error;
pub mod ising;
mod job;
pub mod logging;
mod session;

pub use cli::CommonArgs;
pub use csr::CsrGraph;
pub use error::SampleError;
pub use ising::{Algorithm, IsingGraph, SampleParams, SamplerResult, WarmStart};
pub use session::{capabilities, run, run_code, BackendIdentity, OpenError, INITIAL_SPINS_FEATURE};

/// The generated protobuf and tonic stubs for the wire contract.
///
/// Re-exported so a solver depends on this one crate. Reaching for
/// `quip-proto` directly only pins a second version of the same code.
pub use quip_proto;
/// The consensus primitives: energy and diversity scoring, the `ChaCha8` draw,
/// nonce derivation, wire codecs, and the handshake.
///
/// Re-exported for the same reason as [`quip_proto`]. A solver that scores its
/// own solutions calls `quip_solver_core::quip_protocol::scoring::energy_milli`
/// rather than adding a second dependency that must move in lockstep.
pub use quip_protocol;
/// Process exit codes from SPEC section 2, and the values `Fatal.exit_code`
/// carries.
pub use quip_protocol::session::ExitCode;

use crate::coefficient::Coefficient;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Cancellation point for in-flight work.
///
/// The session raises it when the coordinator abandons a round; the solver
/// reads it to skip work nobody is waiting for. Cheap to clone (one
/// `Arc<AtomicU64>`) and monotonic, so out-of-order or repeated cancels are
/// idempotent.
///
/// Watermarks are opaque and start at 1. The stored value `0` is the sentinel
/// for "nothing cancelled yet", so a fresh token cancels nothing. Which jobs
/// carry a watermark at all is a caller decision: a job built with
/// `watermark: None` is never cancelled.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicU64>);

impl CancelToken {
    /// Abandon every watermark at or below `watermark`.
    pub fn cancel_through(&self, watermark: u64) {
        let _ = self.0.fetch_max(watermark, Ordering::Relaxed);
    }

    /// The highest watermark abandoned so far. `0` means nothing is abandoned.
    #[must_use]
    pub fn abandoned(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    /// True when this job's watermark has been abandoned.
    #[must_use]
    pub fn is_cancelled(&self, watermark: Option<u64>) -> bool {
        let Some(w) = watermark else { return false };
        let point = self.0.load(Ordering::Relaxed);
        point != 0 && w <= point
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::CancelToken;

    #[test]
    fn marks_watermarks_at_or_below_the_cancel_point() {
        let t = CancelToken::default();
        assert!(!t.is_cancelled(Some(5))); // nothing cancelled yet
        t.cancel_through(5);
        assert!(t.is_cancelled(Some(5)));
        assert!(t.is_cancelled(Some(4)));
        assert!(!t.is_cancelled(Some(6)));
    }

    #[test]
    fn a_job_with_no_watermark_is_never_cancelled() {
        let t = CancelToken::default();
        t.cancel_through(u64::MAX);
        assert!(!t.is_cancelled(None));
    }

    #[test]
    fn the_cancel_point_is_monotonic() {
        let t = CancelToken::default();
        t.cancel_through(7);
        t.cancel_through(3); // a lower value must not lower the cancel point
        assert!(t.is_cancelled(Some(7)));
        assert!(t.is_cancelled(Some(4)));
    }

    #[test]
    fn a_fresh_token_cancels_nothing_at_all() {
        // The zero sentinel must not read as "watermark 0 is cancelled".
        let t = CancelToken::default();
        assert!(!t.is_cancelled(Some(0)));
        assert!(!t.is_cancelled(Some(1)));
    }
}

/// One job entering the streaming sampler.
pub struct StreamJob<C: Coefficient = f64> {
    /// Opaque job id from the coordinator (echoed on Result/Reject).
    pub job_id: Vec<u8>,
    /// Wire-parsed Ising problem for this job.
    pub graph: IsingGraph<C>,
    /// Resolved sampling knobs for this job.
    pub params: SampleParams,
    /// Cancellation watermark for this job, or `None` when the job cannot be
    /// cancelled. The caller decides which jobs carry one.
    pub watermark: Option<u64>,
}

/// One job entering [`Sampler::sample_stream_warm`]: the plain job, plus its
/// warm start when the coordinator sent one.
#[non_exhaustive]
pub struct WarmStreamJob<C: Coefficient = f64> {
    /// The job, exactly as [`Sampler::sample_stream`] would receive it.
    pub job: StreamJob<C>,
    /// Start states and anneal start point, or `None` for a cold job.
    pub warm_start: Option<WarmStart>,
}

/// One job leaving the streaming sampler, in completion order.
pub struct StreamResult {
    /// Opaque job id, matching the inbound [`StreamJob`].
    pub job_id: Vec<u8>,
    /// Completed samples, a device error, or a cancelled watermark.
    pub outcome: StreamOutcome,
    /// Per-model device/sample time in microseconds, reported in `SamplerMeta`.
    pub device_access_time_us: u64,
}

/// Outcome of one streamed job.
pub enum StreamOutcome {
    /// Ran to completion, or failed with a device condition.
    Completed(Result<Vec<SamplerResult>, SampleError>),
    /// Abandoned because its watermark was cancelled; the caller has moved
    /// on, so nothing is sent upstream — only the local credit is refunded to
    /// keep pipeline depth for the live round.
    Cancelled,
}

/// A backend that samples Ising problems for the miner harness.
///
/// Implementations own their device and algorithm. Only [`sample`](Sampler::sample)
/// is required; the other methods default to a no-governor, uncapped backend
/// (the CPU miner's shape).
pub trait Sampler<C: Coefficient = f64>: Send + Sync + 'static {
    /// Sample one job.
    ///
    /// # Errors
    ///
    /// Returns a [`SampleError`] when the device cannot complete the job.
    /// Use `Capacity` for a size bound, `DeviceBusy` for transient load, and
    /// `DeviceFault` for a state that needs a restart.
    fn sample(
        &self,
        graph: &IsingGraph<C>,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError>;

    /// Stream-process jobs: pull from `jobs`, keep up to [`stream_width`] models
    /// in flight, emit each result to `out` in completion order. Blocks until
    /// `jobs` closes and all in-flight finish. Runs on a blocking thread (uses
    /// `blocking_recv`/`blocking_send`), so it must not be called from an async
    /// task directly. Default: serial loop over [`sample`](Sampler::sample).
    ///
    /// [`stream_width`]: Sampler::stream_width
    fn sample_stream(
        &self,
        jobs: tokio::sync::mpsc::Receiver<StreamJob<C>>,
        out: tokio::sync::mpsc::Sender<StreamResult>,
        cancel: CancelToken,
    ) {
        serial_stream(self, jobs, &out, &cancel, |j| (j, None));
    }

    /// Whether this backend uses warm-start states (`IsingProblem` fields 9 to
    /// 12). Answered without a device, like
    /// [`declared_stream_width`](Sampler::declared_stream_width), because
    /// `--capabilities` reads it too.
    ///
    /// Default `false`: the session feeds [`sample_stream`](Sampler::sample_stream)
    /// and the states never reach the backend. When `true`, the session
    /// advertises the `initial-spins` feature and feeds
    /// [`sample_stream_warm`](Sampler::sample_stream_warm) instead. Override it
    /// together with [`sample_warm`](Sampler::sample_warm), or with
    /// `sample_stream_warm` for a backend that keeps several models in flight.
    #[must_use]
    fn accepts_warm_start() -> bool
    where
        Self: Sized,
    {
        false
    }

    /// Sample one seeded job. Called only when
    /// [`accepts_warm_start`](Sampler::accepts_warm_start) is `true`.
    /// Default: ignore `warm` and call [`sample`](Sampler::sample).
    ///
    /// # Errors
    ///
    /// Same as [`sample`](Sampler::sample).
    fn sample_warm(
        &self,
        graph: &IsingGraph<C>,
        params: &SampleParams,
        warm: &WarmStart,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let _ = warm;
        self.sample(graph, params)
    }

    /// The warm-start form of [`sample_stream`](Sampler::sample_stream), with
    /// the same contract. Called instead of `sample_stream` only when
    /// [`accepts_warm_start`](Sampler::accepts_warm_start) is `true`.
    /// Default: serial loop over [`sample_warm`](Sampler::sample_warm) for a
    /// seeded job and [`sample`](Sampler::sample) for a cold one.
    fn sample_stream_warm(
        &self,
        jobs: tokio::sync::mpsc::Receiver<WarmStreamJob<C>>,
        out: tokio::sync::mpsc::Sender<StreamResult>,
        cancel: CancelToken,
    ) {
        serial_stream(self, jobs, &out, &cancel, |w| (w.job, w.warm_start));
    }

    /// Number of models the backend keeps in flight. Default 1 (serial).
    fn stream_width(&self) -> usize {
        1
    }

    /// The stream width this backend advertises, answered without a device.
    ///
    /// `--capabilities` must not open the device, so it cannot call
    /// [`stream_width`](Sampler::stream_width), which takes `&self`. Both the
    /// flag and the in-session `Capabilities` reply read this instead, so the
    /// two answers are the same number by construction — SPEC section 8 says
    /// they are the same message.
    ///
    /// A backend that keeps several models in flight overrides this and
    /// [`stream_width`](Sampler::stream_width) with the same value. The session
    /// logs an error when they disagree, because the advertised number is then
    /// a misdeclaration.
    ///
    /// A backend whose width is a property of the opened device — a lane count
    /// derived from the GPU's multiprocessor count, say — has no honest static
    /// answer and declares `0`: width unknown until the device opens. The
    /// session then skips the mismatch check, and the in-session
    /// `Capabilities` reply carries the live width while `--capabilities`
    /// keeps the `0`.
    #[must_use]
    fn declared_stream_width() -> u32
    where
        Self: Sized,
    {
        1
    }

    /// Current utilization for Status messages. `0.0` when no governor.
    fn utilization(&self) -> f64 {
        0.0
    }

    /// Whether to briefly back off before the next job (governor backpressure).
    fn should_throttle(&self) -> bool {
        false
    }

    /// Largest `num_reads` this backend accepts. Defaults to no cap; a backend
    /// with a device-memory bound overrides it.
    fn max_reads(&self) -> u32 {
        u32::MAX
    }

    /// Apply this backend's configuration from `Configure.backend_toml` — the
    /// verbatim `config.toml` subsection the coordinator forwards. Called once
    /// when `Configure` arrives, before any job. Each backend parses against its
    /// own schema, applies recognized fields (config overrides CLI, see
    /// [`config::config_override`]), and warns on unknown fields
    /// ([`config::warn_unknown_fields`]). Default: no configurable settings (the
    /// CPU miner's shape).
    fn apply_config(&self, _backend_toml: &str) {}
}

/// The serial loop behind the default [`Sampler::sample_stream`] and
/// [`Sampler::sample_stream_warm`]. `split` pulls the plain job and its
/// optional warm start out of the channel item.
fn serial_stream<S, J, C>(
    sampler: &S,
    mut jobs: tokio::sync::mpsc::Receiver<J>,
    out: &tokio::sync::mpsc::Sender<StreamResult>,
    cancel: &CancelToken,
    split: impl Fn(J) -> (StreamJob<C>, Option<WarmStart>),
) where
    S: Sampler<C> + ?Sized,
    C: Coefficient,
{
    while let Some(item) = jobs.blocking_recv() {
        let (j, warm) = split(item);
        // Skip a job the coordinator abandoned on reseed; refund the credit
        // (via the session's Cancelled handling) so the pipeline keeps depth.
        // The serial default can only check at dequeue; backends that own a
        // sweep/read loop poll `cancel` at their finer checkpoints.
        if cancel.is_cancelled(j.watermark) {
            if out
                .blocking_send(StreamResult {
                    job_id: j.job_id,
                    outcome: StreamOutcome::Cancelled,
                    device_access_time_us: 0,
                })
                .is_err()
            {
                break;
            }
            continue;
        }
        if sampler.should_throttle() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let t0 = std::time::Instant::now();
        let result = match &warm {
            Some(w) => sampler.sample_warm(&j.graph, &j.params, w),
            None => sampler.sample(&j.graph, &j.params),
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "per-job sample duration in micros fits u64 for any realistic device runtime"
        )]
        let device_access_time_us = t0.elapsed().as_micros() as u64;
        // A `Cancel` that lands while `sample` runs abandons this job too.
        // Checking only at dequeue lets the finished samples out as a
        // `Result` for a generation the coordinator has already moved past,
        // which SPEC section 5 forbids: an abandoned generation gets no
        // `Result` at all. The credit is still refunded, by the same
        // `Cancelled` path the dequeue check uses.
        //
        // A device fault is the exception. It describes the device, not the
        // job, and the session ends on it, so swallowing it here would hide
        // a wedged device behind an ordinary cancellation.
        let cancelled =
            cancel.is_cancelled(j.watermark) && !matches!(&result, Err(e) if e.is_fatal());
        let outcome = if cancelled {
            StreamOutcome::Cancelled
        } else {
            StreamOutcome::Completed(result)
        };
        if out
            .blocking_send(StreamResult {
                job_id: j.job_id,
                outcome,
                device_access_time_us,
            })
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    struct OneResultSampler;
    impl Sampler for OneResultSampler {
        fn sample(
            &self,
            graph: &IsingGraph,
            _params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            Ok(vec![SamplerResult {
                spins: vec![1i8; graph.num_nodes()],
                energy_milli: 0,
            }])
        }
    }

    fn tiny_graph() -> IsingGraph {
        IsingGraph::new(vec![1.0, -1.0], vec![1.0], vec![(0, 1)])
    }

    #[test]
    fn default_sample_stream_returns_every_result_once() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt");
        let (job_tx, job_rx) = tokio::sync::mpsc::channel::<StreamJob>(8);
        let (res_tx, mut res_rx) = tokio::sync::mpsc::channel::<StreamResult>(8);

        let worker = std::thread::spawn(move || {
            OneResultSampler.sample_stream(job_rx, res_tx, CancelToken::default());
        });

        rt.block_on(async {
            for i in 0u8..5 {
                job_tx
                    .send(StreamJob {
                        job_id: vec![i],
                        graph: tiny_graph(),
                        params: SampleParams::default(),
                        watermark: Some(1),
                    })
                    .await
                    .expect("send job");
            }
            drop(job_tx); // close the stream so the worker exits

            let mut seen: Vec<u8> = Vec::new();
            while let Some(r) = res_rx.recv().await {
                assert!(matches!(r.outcome, StreamOutcome::Completed(Ok(_))));
                #[expect(
                    clippy::indexing_slicing,
                    reason = "test jobs use single-byte ids (vec![i])"
                )]
                seen.push(r.job_id[0]);
            }
            seen.sort_unstable();
            assert_eq!(seen, vec![0, 1, 2, 3, 4]);
        });
        worker.join().expect("worker join");
    }

    struct CountingSampler(Arc<std::sync::atomic::AtomicUsize>);
    impl Sampler for CountingSampler {
        fn sample(
            &self,
            graph: &IsingGraph,
            _params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            use std::sync::atomic::Ordering;
            let _ = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(vec![SamplerResult {
                spins: vec![1i8; graph.num_nodes()],
                energy_milli: 0,
            }])
        }
    }

    #[test]
    fn sample_stream_skips_cancelled_generation_without_sampling() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt");
        let (job_tx, job_rx) = tokio::sync::mpsc::channel::<StreamJob>(8);
        let (res_tx, mut res_rx) = tokio::sync::mpsc::channel::<StreamResult>(8);
        let calls = Arc::new(AtomicUsize::new(0));
        let cancel = CancelToken::default();
        cancel.cancel_through(3); // generations 1..=3 abandoned

        let sampler = CountingSampler(Arc::clone(&calls));
        let worker = std::thread::spawn(move || sampler.sample_stream(job_rx, res_tx, cancel));

        rt.block_on(async {
            job_tx
                .send(StreamJob {
                    job_id: vec![1],
                    graph: tiny_graph(),
                    params: SampleParams::default(),
                    watermark: Some(2), // stale
                })
                .await
                .expect("send");
            job_tx
                .send(StreamJob {
                    job_id: vec![2],
                    graph: tiny_graph(),
                    params: SampleParams::default(),
                    watermark: Some(5), // live
                })
                .await
                .expect("send");
            drop(job_tx);

            let mut cancelled: std::collections::HashMap<u8, bool> =
                std::collections::HashMap::new();
            while let Some(r) = res_rx.recv().await {
                #[expect(
                    clippy::indexing_slicing,
                    reason = "test jobs use single-byte ids (vec![1]/vec![2])"
                )]
                let _ =
                    cancelled.insert(r.job_id[0], matches!(r.outcome, StreamOutcome::Cancelled));
            }
            assert_eq!(cancelled.get(&1), Some(&true)); // stale -> Cancelled
            assert_eq!(cancelled.get(&2), Some(&false)); // live -> Completed
        });
        worker.join().expect("worker join");
        assert_eq!(calls.load(Ordering::SeqCst), 1); // sample ran only for the live job
    }

    /// A sampler that raises the cancel watermark from inside `sample`, which
    /// is what a `Cancel` arriving mid-job looks like to `sample_stream`.
    struct CancelWhileSampling {
        cancel: CancelToken,
        outcome: Result<Vec<SamplerResult>, SampleError>,
    }

    impl Sampler for CancelWhileSampling {
        fn sample(
            &self,
            _graph: &IsingGraph,
            _params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            self.cancel.cancel_through(9);
            self.outcome.clone()
        }
    }

    /// Drive one job with watermark `Some(5)` through a sampler that cancels
    /// generation 9 while sampling, and report the outcome.
    fn outcome_after_cancel_during_sample(
        result: Result<Vec<SamplerResult>, SampleError>,
    ) -> StreamOutcome {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt");
        let (job_tx, job_rx) = tokio::sync::mpsc::channel::<StreamJob>(4);
        let (res_tx, mut res_rx) = tokio::sync::mpsc::channel::<StreamResult>(4);
        let cancel = CancelToken::default();
        let sampler = CancelWhileSampling {
            cancel: cancel.clone(),
            outcome: result,
        };
        let worker = std::thread::spawn(move || sampler.sample_stream(job_rx, res_tx, cancel));

        let outcome = rt.block_on(async {
            job_tx
                .send(StreamJob {
                    job_id: vec![1],
                    graph: tiny_graph(),
                    params: SampleParams::default(),
                    watermark: Some(5),
                })
                .await
                .expect("send");
            drop(job_tx);
            let r = res_rx.recv().await.expect("one result");
            assert!(res_rx.recv().await.is_none(), "exactly one result per job");
            r.outcome
        });
        worker.join().expect("worker join");
        outcome
    }

    /// SPEC section 5: a generation abandoned while its job was sampling gets
    /// no `Result`. Checking the watermark only at dequeue lets the finished
    /// samples out anyway, which is the bug this pins.
    #[test]
    fn a_cancel_arriving_during_sample_suppresses_the_result() {
        let outcome = outcome_after_cancel_during_sample(Ok(vec![SamplerResult {
            spins: vec![1i8, -1],
            energy_milli: -1000,
        }]));
        assert!(
            matches!(outcome, StreamOutcome::Cancelled),
            "a job cancelled while sampling must not report Completed"
        );
    }

    /// The carve-out: a device fault describes the device, not the job. A
    /// cancellation must not swallow it, or a wedged device keeps taking work.
    #[test]
    fn a_cancel_during_sample_still_reports_a_device_fault() {
        let outcome = outcome_after_cancel_during_sample(Err(SampleError::DeviceFault(
            "nvml: GPU 0 lost".to_owned(),
        )));
        assert!(
            matches!(
                outcome,
                StreamOutcome::Completed(Err(SampleError::DeviceFault(_)))
            ),
            "a device fault must survive a concurrent cancel"
        );
    }

    /// A non-fatal device error for an abandoned generation is still abandoned:
    /// the coordinator has moved on, so a stale `Reject` helps nobody.
    #[test]
    fn a_cancel_during_sample_suppresses_a_non_fatal_error() {
        let outcome = outcome_after_cancel_during_sample(Err(SampleError::DeviceBusy));
        assert!(matches!(outcome, StreamOutcome::Cancelled));
    }

    /// Reports which entry point handled each job through the read's energy:
    /// `sample_warm` returns the first seed's first spin, `sample` returns 0.
    struct WarmEcho;
    impl Sampler for WarmEcho {
        fn sample(
            &self,
            graph: &IsingGraph,
            _params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            Ok(vec![SamplerResult {
                spins: vec![1i8; graph.num_nodes()],
                energy_milli: 0,
            }])
        }

        fn accepts_warm_start() -> bool {
            true
        }

        fn sample_warm(
            &self,
            _graph: &IsingGraph,
            _params: &SampleParams,
            warm: &WarmStart,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            let first = warm.spins.first().cloned().unwrap_or_default();
            Ok(vec![SamplerResult {
                energy_milli: first.first().copied().map_or(0, i64::from),
                spins: first,
            }])
        }
    }

    /// The default warm stream sends a seeded job to `sample_warm` and a cold
    /// one to `sample`.
    #[test]
    fn default_sample_stream_warm_routes_by_warm_start() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt");
        let (job_tx, job_rx) = tokio::sync::mpsc::channel::<WarmStreamJob>(4);
        let (res_tx, mut res_rx) = tokio::sync::mpsc::channel::<StreamResult>(4);
        let worker = std::thread::spawn(move || {
            WarmEcho.sample_stream_warm(job_rx, res_tx, CancelToken::default());
        });

        let energies = rt.block_on(async {
            for (id, warm_start) in [
                (
                    1u8,
                    Some(WarmStart {
                        spins: vec![vec![-1, 1]],
                        start_beta: None,
                        reversal_s: None,
                        reversal_pause_us: None,
                    }),
                ),
                (2u8, None),
            ] {
                job_tx
                    .send(WarmStreamJob {
                        job: StreamJob {
                            job_id: vec![id],
                            graph: tiny_graph(),
                            params: SampleParams::default(),
                            watermark: None,
                        },
                        warm_start,
                    })
                    .await
                    .expect("send");
            }
            drop(job_tx);
            let mut energies = std::collections::HashMap::new();
            while let Some(r) = res_rx.recv().await {
                let StreamOutcome::Completed(Ok(reads)) = r.outcome else {
                    panic!("both jobs complete");
                };
                let _ = energies.insert(r.job_id, reads.first().map(|x| x.energy_milli));
            }
            energies
        });
        worker.join().expect("worker join");
        assert_eq!(energies.get(&vec![1u8]), Some(&Some(-1))); // sample_warm
        assert_eq!(energies.get(&vec![2u8]), Some(&Some(0))); // sample
    }

    /// The advertised width defaults to the live width, so a backend that
    /// overrides neither cannot advertise a number its sampler contradicts.
    #[test]
    fn the_declared_stream_width_defaults_to_the_live_one() {
        let declared = OneResultSampler::declared_stream_width();
        assert_eq!(
            usize::try_from(declared),
            Ok(OneResultSampler.stream_width())
        );
    }

    struct NarrowFixedSampler;
    impl Sampler<coefficient::Fixed<i8, 1>> for NarrowFixedSampler {
        fn sample(
            &self,
            graph: &IsingGraph<coefficient::Fixed<i8, 1>>,
            _params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, SampleError> {
            Ok(vec![SamplerResult {
                spins: vec![1; graph.num_nodes()],
                energy_milli: 7,
            }])
        }
    }

    /// The default warm stream forwards one cold narrow job through `sample`.
    #[test]
    fn default_sample_stream_warm_runs_a_narrow_job() {
        use coefficient::Fixed;

        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt");
        let (job_tx, job_rx) = tokio::sync::mpsc::channel::<WarmStreamJob<Fixed<i8, 1>>>(8);
        let (res_tx, mut res_rx) = tokio::sync::mpsc::channel::<StreamResult>(8);
        let worker = std::thread::spawn(move || {
            NarrowFixedSampler.sample_stream_warm(job_rx, res_tx, CancelToken::default());
        });

        rt.block_on(async {
            job_tx
                .send(WarmStreamJob {
                    job: StreamJob {
                        job_id: b"n1".to_vec(),
                        graph: IsingGraph::<Fixed<i8, 1>> {
                            h: vec![Fixed(1), Fixed(-1)],
                            j: vec![Fixed(1)],
                            edges: vec![(0, 1)],
                        },
                        params: SampleParams::default(),
                        watermark: None,
                    },
                    warm_start: None,
                })
                .await
                .expect("send job");
            drop(job_tx);

            let result = res_rx.recv().await.expect("one result");
            assert!(res_rx.recv().await.is_none(), "exactly one result");
            assert_eq!(result.job_id, b"n1");
            let StreamOutcome::Completed(Ok(reads)) = result.outcome else {
                panic!("expected Completed(Ok)");
            };
            assert_eq!(reads.len(), 1);
            let read = reads.first().expect("one read");
            assert_eq!(read.spins.len(), 2);
        });
        worker.join().expect("worker join");
    }
}
