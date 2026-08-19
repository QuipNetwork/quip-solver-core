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
pub use ising::{Algorithm, IsingGraph, SampleParams, SamplerResult};
pub use session::{capabilities, run, BackendIdentity, OpenError};

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
pub struct StreamJob {
    /// Opaque job id from the coordinator (echoed on Result/Reject).
    pub job_id: Vec<u8>,
    /// Wire-parsed Ising problem for this job.
    pub graph: IsingGraph,
    /// Resolved sampling knobs for this job.
    pub params: SampleParams,
    /// Cancellation watermark for this job, or `None` when the job cannot be
    /// cancelled. The caller decides which jobs carry one.
    pub watermark: Option<u64>,
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
pub trait Sampler: Send + Sync + 'static {
    /// Sample one job.
    ///
    /// # Errors
    ///
    /// Returns a [`SampleError`] when the device cannot complete the job.
    /// Use `Capacity` for a size bound, `DeviceBusy` for transient load, and
    /// `DeviceFault` for a state that needs a restart.
    fn sample(
        &self,
        graph: &IsingGraph,
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
        mut jobs: tokio::sync::mpsc::Receiver<StreamJob>,
        out: tokio::sync::mpsc::Sender<StreamResult>,
        cancel: CancelToken,
    ) {
        while let Some(j) = jobs.blocking_recv() {
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
            if self.should_throttle() {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let t0 = std::time::Instant::now();
            let result = self.sample(&j.graph, &j.params);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "per-job sample duration in micros fits u64 for any realistic device runtime"
            )]
            let device_access_time_us = t0.elapsed().as_micros() as u64;
            if out
                .blocking_send(StreamResult {
                    job_id: j.job_id,
                    outcome: StreamOutcome::Completed(result),
                    device_access_time_us,
                })
                .is_err()
            {
                break;
            }
        }
    }

    /// Number of models the backend keeps in flight. Default 1 (serial).
    fn stream_width(&self) -> usize {
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
                spins: vec![1i8; graph.h.len()],
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
                spins: vec![1i8; graph.h.len()],
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
}
