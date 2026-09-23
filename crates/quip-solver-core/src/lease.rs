//! Default salt expansion and lease completion accounting.
use crate::coefficient::Coefficient;
use crate::job::{miner, now_unix_ms, os_seed, pick_param, ExactEnergy, SessionTarget};
use crate::session::{BackendIdentity, JobSender, PendingJob, PendingParams};
use crate::{
    CancelToken, IsingGraph, SampleError, SampleParams, Sampler, SamplerResult, StreamJob,
    StreamOutcome, StreamResult,
};
use quip_proto::v1::{
    miner_msg, Job, JobRequest, LeaseDone, MinerMsg, RejectReason, SamplerMeta, Solution,
};
use quip_protocol::lease::{Generator, LeaseSpec, TopologyView};
use quip_protocol::target::meets_target;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};

/// A decoded salt range and its deterministic generator.
#[derive(Clone, Debug)]
pub struct Lease(LeaseSpec);
impl Lease {
    /// Number of salts in the range.
    #[must_use]
    pub const fn salt_count(&self) -> u64 {
        self.0.salt_count
    }
    /// Salt at `index`. Returns the zero array outside `0..salt_count()`.
    #[must_use]
    pub fn salt(&self, index: u64) -> [u8; 32] {
        self.0.salt(index).unwrap_or([0; 32])
    }
    /// Nonce at `index`. Returns the zero array outside `0..salt_count()`.
    #[must_use]
    pub fn nonce(&self, index: u64) -> [u8; 32] {
        self.0.nonce(index).unwrap_or([0; 32])
    }
    /// Generator used by this lease.
    #[must_use]
    pub const fn generator(&self) -> Generator {
        self.0.generator
    }
}

struct Progress {
    dispatched: u64,
    finished: u64,
    salts_done: u64,
    best_energy_milli: i64,
    closed: bool,
    done_sent: bool,
}

pub(crate) struct LeaseState {
    job_id: Vec<u8>,
    lease: Lease,
    topology: Arc<TopologyView>,
    watermark: Option<u64>,
    deadline_ms: u64,
    aborted: Arc<AtomicBool>,
    progress: Mutex<Progress>,
}
impl LeaseState {
    fn progress(&self) -> MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(|err| {
            tracing::error!("lease progress lock poisoned; recovering counters");
            err.into_inner()
        })
    }
    pub(crate) fn expired(&self) -> bool {
        expired(self.deadline_ms)
    }
    fn stopped(&self, cancel: &CancelToken) -> bool {
        self.aborted.load(Ordering::Relaxed)
            || cancel.is_cancelled(self.watermark)
            || self.expired()
    }
    fn try_finish(&self) -> Option<MinerMsg> {
        self.finish_locked(&mut self.progress())
    }
    fn finish_locked(&self, p: &mut Progress) -> Option<MinerMsg> {
        if self.aborted.load(Ordering::Relaxed)
            || !p.closed
            || p.finished != p.dispatched
            || p.done_sent
        {
            return None;
        }
        p.done_sent = true;
        Some(miner(miner_msg::Msg::LeaseDone(LeaseDone {
            job_id: self.job_id.clone(),
            salts_done: p.salts_done,
            best_energy_milli: p.best_energy_milli,
        })))
    }
    pub(crate) async fn send_done(&self, tx: &mpsc::Sender<MinerMsg>) -> Result<(), ()> {
        if let Some(done) = self.try_finish() {
            tx.send(done).await.map_err(|_| ())?;
            tx.send(miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 })))
                .await
                .map_err(|_| ())?;
        }
        Ok(())
    }
}
pub(crate) fn expired(deadline: u64) -> bool {
    deadline != 0 && now_unix_ms().is_none_or(|now| now > deadline)
}
pub(crate) struct LeaseLink {
    pub(crate) state: Arc<LeaseState>,
    index: u64,
    // Kept until the writer finishes this salt, including its wire send.
    _permit: OwnedSemaphorePermit,
}

pub(crate) struct Expander<C: Coefficient> {
    pub(crate) jobs: JobSender<C>,
    pub(crate) pending: PendingParams,
    pub(crate) slots: Arc<Semaphore>,
    pub(crate) cancel: CancelToken,
    pub(crate) shutdown: watch::Receiver<bool>,
    pub(crate) ctrl: mpsc::Sender<MinerMsg>,
}

pub(crate) type CachedLeaseTopology = (Vec<u8>, Result<Arc<TopologyView>, RejectReason>);

/// Validate before starting a task or drawing any salt.
#[expect(
    clippy::too_many_arguments,
    reason = "lease admission resolves the same session inputs as plain admission"
)]
pub(crate) fn prepare(
    job: &Job,
    topology: Option<&CachedLeaseTopology>,
    target: Option<&SessionTarget>,
    id: &BackendIdentity,
    max_reads: u32,
    default_sweeps: usize,
    sweeps_per_beta: Option<usize>,
    aborted: Arc<AtomicBool>,
) -> Result<(Arc<LeaseState>, SampleParams), RejectReason> {
    if expired(job.deadline_ms) {
        return Err(RejectReason::Expired);
    }
    let (hash, topology) = topology.ok_or(RejectReason::TopologyMissing)?;
    let target = target
        .filter(|t| t.max_proof_solutions != 0)
        .ok_or(RejectReason::TargetMissing)?;
    let generator = job.generator.as_ref().ok_or(RejectReason::Malformed)?;
    let spec = LeaseSpec::from_proto(generator).map_err(|_| RejectReason::Malformed)?;
    if generator.topology_hash != *hash {
        return Err(RejectReason::TopologyMismatch);
    }
    let topology = topology.as_ref().map_err(|reason| *reason)?;
    if (topology.num_nodes != 0 && topology.allowed_h_milli.is_empty())
        || (!topology.edges.is_empty() && topology.allowed_j_milli.is_empty())
    {
        return Err(RejectReason::Malformed);
    }
    if (id.max_nodes != 0 && topology.num_nodes > id.max_nodes as usize)
        || (id.max_edges != 0 && topology.edges.len() > id.max_edges as usize)
    {
        return Err(RejectReason::TooLarge);
    }
    let adapt = crate::adapt::adapt_params(
        target.max_energy_milli,
        target.min_solutions.max(1),
        topology.num_nodes,
        topology.edges.len(),
        &topology.allowed_h_milli,
        &id.adapt,
    );
    let num_reads = pick_param(0, target.num_reads, Some(adapt.num_reads), 1);
    let mut num_sweeps = pick_param(
        0,
        target.num_sweeps,
        Some(adapt.num_sweeps),
        u32::try_from(default_sweeps).unwrap_or(u32::MAX),
    );
    if id.algorithm == quip_proto::v1::Algorithm::Gibbs {
        num_sweeps = num_sweeps.saturating_mul(2);
    }
    if num_reads > max_reads {
        return Err(RejectReason::TooLarge);
    }
    let state = Arc::new(LeaseState {
        job_id: job.job_id.clone(),
        lease: Lease(spec),
        topology: Arc::clone(topology),
        watermark: (job.generation != 0).then_some(job.generation),
        deadline_ms: job.deadline_ms,
        aborted,
        progress: Mutex::new(Progress {
            dispatched: 0,
            finished: 0,
            salts_done: 0,
            best_energy_milli: i64::MAX,
            closed: false,
            done_sent: false,
        }),
    });
    Ok((
        state,
        SampleParams {
            num_reads: num_reads as usize,
            num_sweeps: num_sweeps as usize,
            sweeps_per_beta: sweeps_per_beta.unwrap_or(1),
            ..Default::default()
        },
    ))
}

/// Convert each draw and retain milli coefficients only when conversion loses precision.
fn draw_graph<C: Coefficient>(
    state: &LeaseState,
    index: u64,
) -> Result<(IsingGraph<C>, Option<ExactEnergy>), quip_protocol::chacha8::DrawError> {
    let (h, j) = state.topology.draw(state.lease.nonce(index))?;
    let (h, j, exact_milli) = crate::encoding::convert_milli::<C>(h, j);
    let graph = IsingGraph {
        h,
        j,
        edges: state.topology.edges.clone(),
    };
    let exact = exact_milli.map(|(h, j)| ExactEnergy::new(h, j, graph.edges.clone()));
    Ok((graph, exact))
}

impl<C: Coefficient> Expander<C> {
    pub(crate) async fn run(mut self, state: Arc<LeaseState>, mut params: SampleParams) {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
        for index in 0..state.lease.salt_count() {
            let permit = loop {
                if state.stopped(&self.cancel) || *self.shutdown.borrow() {
                    break None;
                }
                tokio::select! {
                    biased;
                    _ = self.shutdown.changed() => {},
                    _ = tick.tick() => {},
                    permit = Arc::clone(&self.slots).acquire_owned() => break permit.ok(),
                }
            };
            let Some(permit) = permit else {
                break;
            };
            if state.stopped(&self.cancel) || *self.shutdown.borrow() {
                break;
            }
            let (graph, exact_energy) = match draw_graph::<C>(&state, index) {
                Ok(drawn) => drawn,
                Err(error) => {
                    tracing::error!(%error, "validated lease could not draw");
                    break;
                }
            };
            let Some(seed) = os_seed() else {
                break;
            };
            params.seed = seed;
            let job_id = crate::session::pending_id(&state.job_id, Some(index));
            let entry = PendingJob {
                exact_energy,
                num_reads: u32::try_from(params.num_reads).unwrap_or(u32::MAX),
                num_sweeps: u32::try_from(params.num_sweeps).unwrap_or(u32::MAX),
                started: std::time::Instant::now(),
                max_energy_milli: None,
                min_solutions: 0,
                watermark: state.watermark,
                lease: Some(LeaseLink {
                    state: Arc::clone(&state),
                    index,
                    _permit: permit,
                }),
            };
            {
                let mut pending = self
                    .pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let _ = pending.insert(job_id.clone(), entry);
            }
            state.progress().dispatched += 1;
            if self
                .jobs
                .send(
                    StreamJob {
                        job_id: job_id.clone(),
                        graph,
                        params: params.clone(),
                        watermark: state.watermark,
                    },
                    None,
                )
                .await
                .is_err()
            {
                let _ = self
                    .pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&job_id);
                state.progress().finished += 1;
                break;
            }
        }
        state.progress().closed = true;
        let _ = state.send_done(&self.ctrl).await;
    }
}

/// Score one salt. The writer sends the returned winner before counting it finished.
pub(crate) fn handle_result(
    link: &LeaseLink,
    result: StreamResult,
    target: Option<&SessionTarget>,
    reads: u32,
    sweeps: u32,
) -> Option<MinerMsg> {
    let StreamOutcome::Completed(Ok(samples)) = result.outcome else {
        return None;
    };
    record_samples(&link.state, &samples);
    winner(
        &link.state,
        link.index,
        &samples,
        target,
        SamplerMeta {
            reads,
            sweeps,
            device_access_time_us: result.device_access_time_us,
            ..Default::default()
        },
    )
}
fn record_samples(state: &LeaseState, samples: &[SamplerResult]) {
    let mut progress = state.progress();
    progress.salts_done += 1;
    if let Some(best) = samples.iter().map(|s| s.energy_milli).min() {
        progress.best_energy_milli = progress.best_energy_milli.min(best);
    }
}
fn winner(
    state: &LeaseState,
    index: u64,
    samples: &[SamplerResult],
    target: Option<&SessionTarget>,
    meta: SamplerMeta,
) -> Option<MinerMsg> {
    let target = target?.to_target();
    let pairs = samples
        .iter()
        .map(|s| (s.spins.as_slice(), s.energy_milli))
        .collect::<Vec<_>>();
    let proof = meets_target(&pairs, &target).ok()?;
    let solutions = proof
        .indices
        .into_iter()
        .filter_map(|i| samples.get(i))
        .map(|s| Solution {
            spins: quip_protocol::wire::encode_spins_packed(&s.spins),
            energy_milli: s.energy_milli,
        })
        .collect();
    Some(miner(miner_msg::Msg::Result(quip_proto::v1::Result {
        job_id: state.job_id.clone(),
        salt: state.lease.salt(index).to_vec(),
        nonce: state.lease.nonce(index).to_vec(),
        solutions,
        meta: Some(meta),
    })))
}
pub(crate) async fn finish_result(link: &LeaseLink, tx: &mpsc::Sender<MinerMsg>) -> Result<(), ()> {
    link.state.progress().finished += 1;
    link.state.send_done(tx).await
}

/// A lease no longer accepts reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseStopped;
impl std::fmt::Display for LeaseStopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("lease stopped")
    }
}
impl std::error::Error for LeaseStopped {}

/// Receives locally generated salts and verifies winning reads on the host.
pub struct LeaseSink {
    pub(crate) state: Arc<LeaseState>,
    pub(crate) cancel: CancelToken,
    pub(crate) shutdown: watch::Receiver<bool>,
    pub(crate) target: watch::Receiver<Option<SessionTarget>>,
    // An uncooperative worker must not keep the control queue open after cancellation.
    pub(crate) ctrl: mpsc::WeakSender<MinerMsg>,
    pub(crate) device_faulted: Arc<AtomicBool>,
    pub(crate) params: SampleParams,
}
impl LeaseSink {
    /// Whether cancellation, shutdown, completion, or the deadline stopped this lease.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.state.stopped(&self.cancel)
            || *self.shutdown.borrow()
            || self.ctrl.upgrade().is_none_or(|tx| tx.is_closed())
            || self.state.progress().closed
    }

    /// Submit all reads for a finished salt from a blocking sampler thread.
    ///
    /// # Errors
    /// Returns a stop if the lease has ended, the index is outside its range,
    /// the writer has gone, or host verification detects a device fault.
    pub fn push(&self, salt_index: u64, reads: Vec<SamplerResult>) -> Result<(), LeaseStopped> {
        if self.is_stopped() || salt_index >= self.state.lease.salt_count() {
            return Err(LeaseStopped);
        }
        {
            let mut progress = self.state.progress();
            if progress.closed {
                return Err(LeaseStopped);
            }
            progress.dispatched += 1;
        }
        record_samples(&self.state, &reads);
        let result = self.push_winner(salt_index, reads);
        self.state.progress().finished += 1;
        result
    }

    fn push_winner(&self, index: u64, reads: Vec<SamplerResult>) -> Result<(), LeaseStopped> {
        let target = self.target.borrow().clone();
        let meta = SamplerMeta {
            reads: u32::try_from(self.params.num_reads).unwrap_or(u32::MAX),
            sweeps: u32::try_from(self.params.num_sweeps).unwrap_or(u32::MAX),
            ..Default::default()
        };
        let Some(target) = target else {
            return Ok(());
        };
        let pairs = reads
            .iter()
            .map(|read| (read.spins.as_slice(), read.energy_milli))
            .collect::<Vec<_>>();
        if meets_target(&pairs, &target.to_target()).is_err() {
            return Ok(());
        }
        let (h, j) = self
            .state
            .topology
            .draw(self.state.lease.nonce(index))
            .map_err(|error| {
                self.fault(&SampleError::DeviceFault(error.to_string()));
                LeaseStopped
            })?;
        let mut rescored = Vec::with_capacity(reads.len());
        for read in reads {
            let energy = quip_protocol::scoring::energy_from_milli(
                &read.spins,
                &h,
                &j,
                &self.state.topology.edges,
            );
            if read.spins.len() != self.state.topology.num_nodes
                || read.spins.iter().any(|spin| !matches!(spin, -1 | 1))
                || energy != read.energy_milli
            {
                self.fault(&SampleError::DeviceFault(
                    "local lease draw disagrees with host energy".into(),
                ));
                return Err(LeaseStopped);
            }
            rescored.push(SamplerResult {
                spins: read.spins,
                energy_milli: energy,
            });
        }
        if self.is_stopped() {
            return Err(LeaseStopped);
        }
        let target = self.target.borrow().clone();
        if let Some(reply) = winner(&self.state, index, &rescored, target.as_ref(), meta) {
            self.send(reply)?;
        }
        Ok(())
    }

    fn send(&self, msg: MinerMsg) -> Result<(), LeaseStopped> {
        self.ctrl
            .upgrade()
            .ok_or(LeaseStopped)?
            .blocking_send(msg)
            .map_err(|_| LeaseStopped)
    }

    fn fault(&self, error: &SampleError) {
        self.state.progress().closed = true;
        if !self.device_faulted.swap(true, Ordering::Relaxed) {
            self.state.aborted.store(true, Ordering::Relaxed);
            let _ = self.send(miner(miner_msg::Msg::Fatal(quip_proto::v1::Fatal {
                exit_code: quip_protocol::session::ExitCode::InternalFatal as u32,
                reason: error.to_string(),
                restart_required: true,
            })));
        }
    }

    pub(crate) fn run<S: Sampler<C>, C: Coefficient>(&self, sampler: &S) {
        // A backend may leave its own state inconsistent after a panic. The
        // fatal path ends the session rather than reusing that sampler.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sampler.sample_lease(&self.state.lease, &self.state.topology, &self.params, self)
        }))
        .unwrap_or_else(|panic| {
            let payload = crate::session::panic_payload_message(&*panic);
            tracing::error!(panic = %payload, "lease thread panicked");
            Err(SampleError::DeviceFault(format!(
                "lease thread panicked: {payload}"
            )))
        });
        let Some(ctrl) = self.ctrl.upgrade() else {
            return;
        };
        let done = {
            let mut progress = self.state.progress();
            progress.closed = true;
            self.state.finish_locked(&mut progress)
        };
        if let Some(done) = done {
            let _ = ctrl.blocking_send(done);
            match result {
                Err(error) if error.is_fatal() => self.fault(&error),
                _ => {
                    let _ = ctrl.blocking_send(miner(miner_msg::Msg::JobRequest(JobRequest {
                        credits: 1,
                    })));
                }
            }
        } else if let Err(error) = result {
            if error.is_fatal() {
                self.fault(&error);
            }
        }
    }
}

/// Close cancelled local leases even when the backend never returns.
pub(crate) async fn monitor_local(
    state: Arc<LeaseState>,
    cancel: CancelToken,
    mut shutdown: watch::Receiver<bool>,
    ctrl: mpsc::Sender<MinerMsg>,
) {
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(5));
    loop {
        if state.stopped(&cancel) || *shutdown.borrow() {
            state.progress().closed = true;
        }
        let _ = state.send_done(&ctrl).await;
        if state.progress().done_sent || state.aborted.load(Ordering::Relaxed) {
            break;
        }
        tokio::select! {
            _ = tick.tick() => {},
            _ = shutdown.changed() => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn shutdown_bounds_an_expander_on_a_full_control_queue() {
        use crate::coefficient::Milli;
        let (ctrl, _stalled_outbound) = mpsc::channel(1);
        ctrl.send(miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 })))
            .await
            .unwrap();
        assert_eq!(ctrl.capacity(), 0);
        let spec =
            LeaseSpec::new(Generator::Blake3Chacha8V1, [1; 32], [2; 32], [3; 32], 0, 1).unwrap();
        let topology = TopologyView::from_proto(&quip_proto::v1::Topology {
            nodes: vec![1],
            allowed_h_milli: vec![1000],
            ..Default::default()
        })
        .unwrap();
        let state = Arc::new(LeaseState {
            job_id: b"lease".to_vec(),
            lease: Lease(spec),
            topology: Arc::new(topology),
            watermark: None,
            deadline_ms: 0,
            aborted: Arc::new(AtomicBool::new(false)),
            progress: Mutex::new(Progress {
                dispatched: 0,
                finished: 0,
                salts_done: 0,
                best_energy_milli: i64::MAX,
                closed: false,
                done_sent: false,
            }),
        });
        let (jobs, _jobs_rx) = mpsc::channel::<StreamJob<Milli>>(1);
        let expander = Expander {
            jobs: JobSender::Plain(jobs),
            pending: Arc::new(Mutex::new(std::collections::HashMap::new())),
            slots: Arc::new(Semaphore::new(1)),
            cancel: CancelToken::default(),
            shutdown: watch::channel(true).1,
            ctrl: ctrl.clone(),
        };
        let mut expanders = tokio::task::JoinSet::new();
        let _ = expanders.spawn(expander.run(Arc::clone(&state), SampleParams::default()));
        tokio::task::yield_now().await;
        assert!(state.progress().done_sent);
        assert_eq!(ctrl.capacity(), 0);
        let grace = std::time::Duration::from_millis(200);
        let start = tokio::time::Instant::now();
        let ended = tokio::time::timeout(
            grace + std::time::Duration::from_millis(50),
            crate::session::join_expanders(&mut expanders, start + grace),
        )
        .await;
        assert!(
            ended.is_ok(),
            "session shutdown blocked before the grace timeout"
        );
        assert!(start.elapsed() <= grace);
        assert!(expanders.is_empty());
        assert_eq!(
            ctrl.strong_count(),
            1,
            "aborted expander must release its sender"
        );
    }

    #[test]
    fn lease_wrapper_maps_out_of_range_indices_to_zero() {
        let spec =
            LeaseSpec::new(Generator::Blake3Chacha8V1, [1; 32], [2; 32], [3; 32], 10, 2).unwrap();
        let lease = Lease(spec);
        assert_eq!(lease.salt_count(), 2);
        assert_eq!(lease.generator(), Generator::Blake3Chacha8V1);
        for index in 0..2 {
            assert_eq!(Some(lease.salt(index)), spec.salt(index));
            assert_eq!(Some(lease.nonce(index)), spec.nonce(index));
        }
        for index in [2, u64::MAX] {
            assert_eq!(lease.salt(index), [0; 32]);
            assert_eq!(lease.nonce(index), [0; 32]);
        }
    }
}
