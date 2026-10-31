//! Default salt expansion and lease completion accounting.
use crate::coefficient::Coefficient;
use crate::families::FamilyTable;
use crate::job::{miner, now_unix_ms, os_seed, pick_param, ExactEnergy, SessionTarget};
use crate::session::{BackendIdentity, Control, JobSender, PendingJob, PendingParams};
use crate::{
    CancelToken, IsingGraph, SampleError, SampleParams, Sampler, SamplerResult, StreamJob,
    StreamOutcome, StreamResult,
};
use ising_generators::descriptor::{
    admit, Admitted, AdvertisedLimits, Contract, Descriptor, IdContext, SizeBounds,
};
use ising_generators::family::{admit_topology, BaseTopology, Defect, SaltFault};
use ising_generators::instance::Instance;
use ising_generators::random::NonceDeriver;
use ising_generators::Code;
use quip_proto::v1::{
    miner_msg, Job, JobRequest, LeaseDone, MinerMsg, RejectReason, SamplerMeta, Solution, Topology,
};
use quip_protocol::family::{reject_reason, support_code, CodePhase};
use quip_protocol::lease::{Generator, LeaseSpec, TopologyView};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, MutexGuard,
};
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Copy)]
pub(crate) struct ShutdownDeadlines {
    pub(crate) close: tokio::time::Instant,
    pub(crate) teardown: tokio::time::Instant,
}

/// A decoded salt range and its deterministic generator.
#[derive(Clone, Debug)]
pub struct Lease {
    spec: LeaseSpec,
    /// Nonce state after the block every salt in this lease shares.
    nonces: NonceDeriver,
    family: Option<Arc<FamilyLease>>,
}
impl Lease {
    pub(crate) fn new(spec: LeaseSpec, family: Option<FamilyLease>) -> Self {
        Self {
            nonces: NonceDeriver::new(spec.last_proof_block_hash, spec.miner_account),
            spec,
            family: family.map(Arc::new),
        }
    }
    /// The family inputs, or `None` for a legacy lease.
    #[must_use]
    pub fn family(&self) -> Option<&FamilyLease> {
        self.family.as_deref()
    }
    /// Number of salts in the range.
    #[must_use]
    pub const fn salt_count(&self) -> u64 {
        self.spec.salt_count
    }
    /// Salt at `index`. Returns the zero array outside `0..salt_count()`.
    #[must_use]
    pub fn salt(&self, index: u64) -> [u8; 32] {
        self.spec.salt(index).unwrap_or([0; 32])
    }
    /// Nonce at `index`. Returns the zero array outside `0..salt_count()`.
    #[must_use]
    pub fn nonce(&self, index: u64) -> [u8; 32] {
        self.spec
            .salt(index)
            .map_or([0; 32], |salt| self.nonces.derive(salt))
    }
    /// Generator used by this lease.
    #[must_use]
    pub const fn generator(&self) -> Generator {
        self.spec.generator
    }
}

/// The family inputs of a lease, fixed at admission (FAM-47 to FAM-52).
///
/// Owns its parameter bytes and holds the base topology the session cached
/// when the lease was admitted, so a later `Job` or `Topology` cannot change
/// the instances of an admitted lease.
#[derive(Debug)]
pub struct FamilyLease {
    contract: &'static Contract,
    params: Vec<u8>,
    params_digest: [u8; 32],
    descriptor_digest: [u8; 32],
    topology: Arc<BaseTopology>,
    bounds: SizeBounds,
}
impl FamilyLease {
    /// The contract the lease was admitted against.
    #[must_use]
    pub const fn contract(&self) -> &'static Contract {
        self.contract
    }
    /// The family id of the descriptor.
    #[must_use]
    pub const fn family_id(&self) -> &'static str {
        self.contract.family_id
    }
    /// The revision of the descriptor.
    #[must_use]
    pub const fn revision(&self) -> u32 {
        self.contract.revision
    }
    /// The canonical parameter bytes of the descriptor.
    #[must_use]
    pub fn params(&self) -> &[u8] {
        &self.params
    }
    /// The descriptor digest (FAM-18).
    #[must_use]
    pub const fn descriptor_digest(&self) -> &[u8; 32] {
        &self.descriptor_digest
    }
    /// The base topology the lease builds on.
    #[must_use]
    pub fn topology(&self) -> &BaseTopology {
        &self.topology
    }
    /// The size bounds computed at admission.
    #[must_use]
    pub const fn size_bounds(&self) -> &SizeBounds {
        &self.bounds
    }
    /// The canonical instance of `nonce`. The coordinator regenerates the same one.
    ///
    /// # Errors
    /// See `ising_generators::family::generate_instance`.
    pub fn generate(&self, nonce: [u8; 32]) -> Result<Instance, SaltFault> {
        // The same inputs passed `admit` at admission, so this cannot fail.
        let admitted = admit(
            std::slice::from_ref(self.contract),
            self.contract.family_id.as_bytes(),
            self.contract.revision,
            &self.params,
            &self.params_digest,
        )
        .map_err(|code| SaltFault::Defect(Defect::ConstructionCode(code)))?;
        ising_generators::family::generate_instance(&admitted, &self.topology, &self.bounds, nonce)
    }
}

/// Salt indices a local sampler already pushed, stored as merged `start..end` ranges.
/// A backend that pushes in rough order keeps this to a few entries.
#[derive(Default)]
struct PushedSalts(std::collections::BTreeMap<u64, u64>);
impl PushedSalts {
    /// Record `index`. Returns `false` if it was already recorded.
    fn insert(&mut self, index: u64) -> bool {
        let before = self
            .0
            .range(..=index)
            .next_back()
            .map(|(&start, &end)| (start, end));
        if before.is_some_and(|(_, end)| index < end) {
            return false;
        }
        let mut start = index;
        if let Some((prev_start, prev_end)) = before {
            if prev_end == index {
                start = prev_start;
            }
        }
        // `index < salt_count <= u64::MAX`, so this never saturates in practice.
        let mut end = index.saturating_add(1);
        if let Some(next_end) = self.0.remove(&end) {
            end = next_end;
        }
        let _ = self.0.insert(start, end);
        true
    }
}

struct Progress {
    dispatched: u64,
    finished: u64,
    salts_done: u64,
    best_energy_milli: i64,
    closed: bool,
    done_sent: bool,
    pushed: PushedSalts,
}

pub(crate) struct LeaseState {
    job_id: Vec<u8>,
    lease: Lease,
    topology: Arc<TopologyView>,
    /// `topology.edges`, copied once so every drawn graph shares it.
    edges: Arc<[(usize, usize)]>,
    watermark: Option<u64>,
    deadline_ms: u64,
    aborted: Arc<AtomicBool>,
    jobs_done: Arc<AtomicU64>,
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
    pub(crate) fn stopped(&self, cancel: &CancelToken) -> bool {
        self.aborted.load(Ordering::Relaxed)
            || cancel.is_cancelled(self.watermark)
            || self.expired()
    }
    pub(crate) fn send_local(
        &self,
        msgs: Vec<MinerMsg>,
        permits: mpsc::PermitIterator<'_, MinerMsg>,
        cancel: &CancelToken,
        grace_expired: bool,
    ) {
        // Serialize the final send with fault(), which aborts under this lock.
        let _progress = self.progress();
        if self.aborted.load(Ordering::Relaxed) {
            return;
        }
        for (permit, msg) in permits.zip(msgs) {
            if matches!(msg.msg, Some(miner_msg::Msg::Result(_)))
                && (self.stopped(cancel) || grace_expired)
            {
                continue;
            }
            permit.send(msg);
        }
    }
    /// Count a salt that generation skipped (FAM-52). No `Result`, no energy,
    /// and `Status.jobs_done` is unchanged because nothing was sampled.
    pub(crate) fn skip_salt(&self) {
        let mut progress = self.progress();
        if !progress.done_sent {
            progress.salts_done += 1;
        }
    }
    fn try_finish(&self, force: bool) -> Option<MinerMsg> {
        self.finish_locked(&mut self.progress(), force)
    }
    fn finish_locked(&self, p: &mut Progress, force: bool) -> Option<MinerMsg> {
        if self.aborted.load(Ordering::Relaxed)
            || !p.closed
            || (!force && p.finished != p.dispatched)
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
    pub(crate) async fn send_done(
        &self,
        ctrl: &mpsc::Sender<Control>,
        force: bool,
    ) -> Result<(), ()> {
        if let Some(done) = self.try_finish(force) {
            ctrl.send(Control::summary(done, None))
                .await
                .map_err(|_| ())?;
        }
        Ok(())
    }

    /// Write a finished lease's summary and refund from inside the writer.
    pub(crate) async fn send_done_direct(
        &self,
        tx: &mpsc::Sender<MinerMsg>,
        force: bool,
    ) -> Result<(), ()> {
        if let Some(done) = self.try_finish(force) {
            let permits = tx.reserve_many(2).await.map_err(|_| ())?;
            for (permit, msg) in permits.zip([done, refund()]) {
                permit.send(msg);
            }
        }
        Ok(())
    }
}

fn refund() -> MinerMsg {
    miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 }))
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

#[cfg(test)]
pub(crate) fn test_state(watermark: Option<u64>) -> Arc<LeaseState> {
    let spec = LeaseSpec::new(Generator::Blake3Chacha8V1, [1; 32], [2; 32], [3; 32], 0, 1).unwrap();
    Arc::new(LeaseState {
        job_id: b"lease".to_vec(),
        lease: Lease::new(spec, None),
        topology: Arc::new(TopologyView {
            num_nodes: 1,
            edges: vec![],
            allowed_h_milli: vec![1000],
            allowed_j_milli: vec![],
        }),
        edges: Arc::new([]),
        watermark,
        deadline_ms: 0,
        aborted: Arc::new(AtomicBool::new(false)),
        jobs_done: Arc::new(AtomicU64::new(0)),
        progress: Mutex::new(Progress {
            dispatched: 0,
            finished: 0,
            salts_done: 0,
            best_energy_milli: i64::MAX,
            closed: false,
            done_sent: false,
            pushed: PushedSalts::default(),
        }),
    })
}

#[cfg(test)]
pub(crate) fn test_link() -> (LeaseLink, Arc<LeaseState>) {
    let state = test_state(None);
    {
        let mut progress = state.progress();
        progress.dispatched = 1;
        progress.closed = true;
    }
    let permit = Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap();
    (
        LeaseLink {
            state: Arc::clone(&state),
            index: 0,
            _permit: permit,
        },
        state,
    )
}

pub(crate) struct Expander<C: Coefficient> {
    pub(crate) jobs: JobSender<C>,
    pub(crate) pending: PendingParams,
    pub(crate) slots: Arc<Semaphore>,
    pub(crate) cancel: CancelToken,
    pub(crate) shutdown: watch::Receiver<Option<ShutdownDeadlines>>,
    pub(crate) ctrl: mpsc::Sender<Control>,
}

/// The session's cached `Topology`, as a lease reads it at admission.
pub(crate) struct LeaseTopology {
    pub(crate) hash: Vec<u8>,
    pub(crate) view: Result<Arc<TopologyView>, RejectReason>,
    /// Built only when the declaration lists a generated family.
    pub(crate) family: Option<Result<Arc<BaseTopology>, RejectReason>>,
}
impl LeaseTopology {
    pub(crate) fn new(t: &Topology, families: &FamilyTable) -> Self {
        let family = (!families.generated().is_empty()).then(|| {
            let (u, v) = t
                .edges
                .as_ref()
                .map_or((&[][..], &[][..]), |e| (&e.u[..], &e.v[..]));
            if u.len() != v.len() {
                return Err(RejectReason::Malformed);
            }
            let edges = u.iter().copied().zip(v.iter().copied()).collect::<Vec<_>>();
            BaseTopology::new(t.nodes.clone(), &edges)
                .map(Arc::new)
                .map_err(|_| RejectReason::Malformed)
        });
        Self {
            hash: t.hash.clone(),
            view: TopologyView::from_proto(t)
                .map(Arc::new)
                .map_err(|_| RejectReason::Malformed),
            family,
        }
    }
}

/// The wire reason of a family code at admission (FAM-42).
fn admission_reject(code: Code) -> RejectReason {
    tracing::debug!(code = code.value(), "family lease rejected");
    reject_reason(code, CodePhase::Admission).unwrap_or(RejectReason::Malformed)
}

/// Codes 3 to 10 of FAM-47, from the declaration's generated contracts.
/// Code 8 beats a pending code 10, which was computed with the sent digest.
fn admit_descriptor<'a>(
    family: &'a quip_proto::v1::FamilyDescriptor,
    families: &FamilyTable,
) -> Result<(&'static Contract, Admitted<'a>), Code> {
    let _ = Descriptor::new(
        family.family_id.as_bytes(),
        family.revision,
        &family.params,
        IdContext::Lease,
    )?;
    let pending = match support_code(
        families.generated(),
        &family.family_id,
        family.revision,
        &family.params_digest,
    ) {
        Some(Code::ParamsRestricted) => Some(Code::ParamsRestricted),
        Some(code) => return Err(code),
        None => None,
    };
    let contract = families
        .generated_contract(&family.family_id, family.revision)
        .ok_or(Code::FamilyUnsupported)?;
    let admitted = admit(
        std::slice::from_ref(contract),
        family.family_id.as_bytes(),
        family.revision,
        &family.params,
        &family.params_digest,
    )?;
    pending.map_or(Ok((contract, admitted)), Err)
}

/// Codes 11, 12, and the admission part of 13 (FAM-47 step 5) against the
/// cached base topology and the advertised limits, then the snapshot.
fn admit_family(
    contract: &'static Contract,
    admitted: &Admitted<'_>,
    topology: Arc<BaseTopology>,
    sent_order_digest: &[u8],
    id: &BackendIdentity,
) -> Result<FamilyLease, RejectReason> {
    let advertised = AdvertisedLimits {
        max_nodes: id.max_nodes.into(),
        max_edges: id.max_edges.into(),
        max_degree: id.limits.max_degree.map_or(0, u64::from),
    };
    let bounds = admit_topology(admitted, &topology, sent_order_digest, &advertised)
        .map_err(admission_reject)?;
    Ok(FamilyLease {
        contract,
        params: admitted.descriptor().params().to_vec(),
        params_digest: admitted.descriptor().params_digest(),
        descriptor_digest: admitted.descriptor().digest(),
        topology,
        bounds,
    })
}

/// Validate before starting a task or drawing any salt.
#[expect(
    clippy::too_many_arguments,
    reason = "lease admission resolves the same session inputs as plain admission"
)]
pub(crate) fn prepare(
    job: &Job,
    topology: Option<&Arc<LeaseTopology>>,
    target: Option<&SessionTarget>,
    id: &BackendIdentity,
    families: &FamilyTable,
    max_reads: u32,
    default_sweeps: usize,
    sweeps_per_beta: Option<usize>,
    aborted: Arc<AtomicBool>,
    jobs_done: Arc<AtomicU64>,
) -> Result<(Arc<LeaseState>, SampleParams), RejectReason> {
    if expired(job.deadline_ms) {
        return Err(RejectReason::Expired);
    }
    let cached = topology.ok_or(RejectReason::TopologyMissing)?;
    let target = target
        .filter(|t| t.max_proof_solutions != 0)
        .ok_or(RejectReason::TargetMissing)?;
    let generator = job.generator.as_ref().ok_or(RejectReason::Malformed)?;
    let spec = LeaseSpec::from_proto(generator).map_err(|_| RejectReason::Malformed)?;
    // `from_proto` guarantees a descriptor on a family lease (code 1).
    let descriptor = match (spec.generator, generator.family.as_ref()) {
        (Generator::FamilyChacha8V1, Some(family)) => {
            Some(admit_descriptor(family, families).map_err(admission_reject)?)
        }
        (Generator::FamilyChacha8V1, None) => return Err(RejectReason::Malformed),
        (Generator::Blake3Chacha8V1, _) => None,
    };
    if generator.topology_hash != cached.hash {
        return Err(RejectReason::TopologyMismatch);
    }
    let topology = cached.view.as_ref().map_err(|reason| *reason)?;
    let family = if let Some((contract, admitted)) = descriptor {
        let base = match &cached.family {
            Some(Ok(base)) => Arc::clone(base),
            Some(Err(_)) | None => return Err(RejectReason::Malformed),
        };
        Some(admit_family(
            contract,
            &admitted,
            base,
            &generator.topology_order_digest,
            id,
        )?)
    } else {
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
        None
    };
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
        lease: Lease::new(spec, family),
        topology: Arc::clone(topology),
        edges: topology.edges.as_slice().into(),
        watermark: (job.generation != 0).then_some(job.generation),
        deadline_ms: job.deadline_ms,
        aborted,
        jobs_done,
        progress: Mutex::new(Progress {
            dispatched: 0,
            finished: 0,
            salts_done: 0,
            best_energy_milli: i64::MAX,
            closed: false,
            done_sent: false,
            pushed: PushedSalts::default(),
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

/// Draw the salt at `index` as an [`IsingGraph`].
///
/// When every allowed value converts to `C` without loss, the draw selects
/// from the converted tables directly and no milli copy is kept. Otherwise it
/// draws milli values, converts each one, and keeps the milli coefficients
/// for exact rescoring.
fn draw_graph<C: Coefficient>(
    state: &LeaseState,
    index: u64,
) -> Result<(IsingGraph<C>, Option<ExactEnergy>), quip_protocol::chacha8::DrawError> {
    let topology = &*state.topology;
    let nonce = state.lease.nonce(index);
    let tables = (
        crate::encoding::exact_table::<C>(&topology.allowed_h_milli),
        crate::encoding::exact_table::<C>(&topology.allowed_j_milli),
    );
    if let (Some(h_table), Some(j_table)) = tables {
        let (h, j) = quip_protocol::chacha8::draw_ising(
            nonce,
            topology.num_nodes,
            topology.edges.len(),
            &h_table,
            &j_table,
        )?;
        let graph = IsingGraph {
            h,
            j,
            edges: Arc::clone(&state.edges),
        };
        return Ok((graph, None));
    }
    let (h, j) = topology.draw(nonce)?;
    let (h, j, exact_milli) = crate::encoding::convert_milli::<C>(h, j);
    let graph = IsingGraph {
        h,
        j,
        edges: Arc::clone(&state.edges),
    };
    let exact = exact_milli.map(|(h, j)| ExactEnergy::new(h, j, Arc::clone(&state.edges)));
    Ok((graph, exact))
}

/// A salt's graph, its exact coefficients when `C` loses precision, and its
/// instance offset (zero on a legacy lease).
type SaltGraph<C> = (IsingGraph<C>, Option<ExactEnergy>, i64);

/// The canonical instance of the salt at `index` as an [`IsingGraph`], with
/// its exact coefficients when `C` loses precision, and its offset.
fn family_graph<C: Coefficient>(
    family: &FamilyLease,
    nonce: [u8; 32],
) -> Result<SaltGraph<C>, SaltFault> {
    let instance = family.generate(nonce)?;
    let edges: Arc<[(usize, usize)]> = instance
        .edges()
        .iter()
        .map(|&(u, v)| (u as usize, v as usize))
        .collect();
    let (h, j, exact_milli) =
        crate::encoding::convert_milli::<C>(instance.h().to_vec(), instance.j().to_vec());
    let exact = exact_milli.map(|(h, j)| ExactEnergy::new(h, j, Arc::clone(&edges)));
    Ok((IsingGraph { h, j, edges }, exact, instance.offset_milli()))
}

/// The graph, exact coefficients, and offset of the salt at `index`.
/// `Ok(None)` skips the salt: codes 13 and 14 (FAM-52), and a construction
/// that breaks its contract (G1), which is also logged.
fn salt_graph<C: Coefficient>(
    state: &LeaseState,
    index: u64,
) -> Result<Option<SaltGraph<C>>, quip_protocol::chacha8::DrawError> {
    let Some(family) = state.lease.family() else {
        return draw_graph::<C>(state, index).map(|(graph, exact)| Some((graph, exact, 0)));
    };
    match family_graph::<C>(family, state.lease.nonce(index)) {
        Ok(drawn) => Ok(Some(drawn)),
        Err(SaltFault::Skip(_)) => Ok(None),
        Err(SaltFault::Defect(defect)) => {
            tracing::error!(
                ?defect,
                salt_index = index,
                "family construction broke its contract; skipping the salt"
            );
            Ok(None)
        }
    }
}

impl<C: Coefficient> Expander<C> {
    #[expect(
        clippy::too_many_lines,
        reason = "salt dispatch and the completion wait share one permit and shutdown loop"
    )]
    pub(crate) async fn run(mut self, state: Arc<LeaseState>, mut params: SampleParams) {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(50));
        for index in 0..state.lease.salt_count() {
            let permit = loop {
                if state.stopped(&self.cancel) || self.shutdown.borrow().is_some() {
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
            if state.stopped(&self.cancel) || self.shutdown.borrow().is_some() {
                break;
            }
            let (graph, exact_energy, offset_milli) = match salt_graph::<C>(&state, index) {
                Ok(Some(drawn)) => drawn,
                Ok(None) => {
                    drop(permit);
                    state.skip_salt();
                    continue;
                }
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
                offset_milli,
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
        // A stopped lease reports what finished without waiting for running salts.
        // The writer reports a lease that ends normally, so this loop exits on done_sent.
        let mut shutdown_open = true;
        loop {
            let deadlines = *self.shutdown.borrow();
            let close_at = deadlines.map(|d| d.close);
            let past_close = close_at.is_some_and(|d| tokio::time::Instant::now() >= d);
            let force = past_close || state.stopped(&self.cancel);
            if state.send_done(&self.ctrl, force).await.is_err() {
                break;
            }
            if state.progress().done_sent || state.aborted.load(Ordering::Relaxed) {
                break;
            }
            tokio::select! {
                _ = tick.tick() => {},
                changed = self.shutdown.changed(), if shutdown_open => shutdown_open = changed.is_ok(),
                () = wait_for_grace(close_at) => {},
            }
        }
    }
}

/// Report one salt. The writer sends the returned result before counting it finished.
/// Every completed salt with a non-empty read set becomes a `Result` carrying
/// its reads; solver-core does not filter by target here, either. The miner
/// and the coordinator decide what to do with what it reports.
pub(crate) fn handle_result(
    link: &LeaseLink,
    result: StreamResult,
    reads: u32,
    sweeps: u32,
) -> Option<MinerMsg> {
    let StreamOutcome::Completed(Ok(samples)) = result.outcome else {
        return None;
    };
    if !record_samples(&link.state, &samples) {
        return None;
    }
    package_result(
        &link.state,
        link.index,
        &samples,
        SamplerMeta {
            reads,
            sweeps,
            device_access_time_us: result.device_access_time_us,
            ..Default::default()
        },
    )
}
fn record_samples(state: &LeaseState, samples: &[SamplerResult]) -> bool {
    let mut progress = state.progress();
    if progress.done_sent {
        return false;
    }
    progress.salts_done += 1;
    let _ = state.jobs_done.fetch_add(1, Ordering::Relaxed);
    if let Some(best) = samples.iter().map(|s| s.energy_milli).min() {
        progress.best_energy_milli = progress.best_energy_milli.min(best);
    }
    true
}
/// Package a salt's reads into a wire `Result`, unfiltered. `None` for an
/// empty read set: a salt with no reads has nothing worth reporting.
fn package_result(
    state: &LeaseState,
    index: u64,
    samples: &[SamplerResult],
    meta: SamplerMeta,
) -> Option<MinerMsg> {
    if samples.is_empty() {
        return None;
    }
    let solutions = samples
        .iter()
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
pub(crate) async fn finish_result(
    link: &LeaseLink,
    tx: &mpsc::Sender<MinerMsg>,
    cancel: &CancelToken,
) -> Result<(), ()> {
    link.state.progress().finished += 1;
    link.state
        .send_done_direct(tx, link.state.stopped(cancel))
        .await
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

/// Receives locally generated salts and forwards the reads the backend reports.
pub struct LeaseSink {
    #[cfg(test)]
    pub(crate) before_send: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    pub(crate) state: Arc<LeaseState>,
    pub(crate) cancel: CancelToken,
    pub(crate) shutdown: watch::Receiver<Option<ShutdownDeadlines>>,
    pub(crate) target: watch::Receiver<Option<SessionTarget>>,
    // An uncooperative worker must not keep the control queue open after cancellation.
    pub(crate) ctrl: mpsc::WeakSender<Control>,
    pub(crate) device_faulted: Arc<AtomicBool>,
    pub(crate) params: SampleParams,
}
impl LeaseSink {
    /// Whether cancellation, shutdown, completion, or the deadline stopped this lease.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.state.stopped(&self.cancel)
            || self.shutdown.borrow().is_some()
            || self.ctrl.upgrade().is_none_or(|tx| tx.is_closed())
            || self.state.progress().closed
    }

    fn rejects_push(&self) -> bool {
        self.state.stopped(&self.cancel)
            || self
                .shutdown
                .borrow()
                .is_some_and(|deadlines| tokio::time::Instant::now() >= deadlines.close)
            || self.ctrl.upgrade().is_none_or(|tx| tx.is_closed())
            || self.state.progress().closed
    }

    /// Submit all reads for a finished salt from a blocking sampler thread.
    /// The backend decides which salts to report; every non-empty read set
    /// becomes a wire `Result`, forwarded without target filtering or
    /// re-verification. During shutdown, already running work may submit
    /// until the lease closes. A repeated `salt_index` is ignored and
    /// logged. It sends and counts nothing.
    ///
    /// # Errors
    /// Returns a stop if the lease has ended, the index is outside its
    /// range, or the writer has gone.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "public API takes ownership of the backend's read buffer"
    )]
    pub fn push(&self, salt_index: u64, reads: Vec<SamplerResult>) -> Result<(), LeaseStopped> {
        if self.rejects_push() || salt_index >= self.state.lease.salt_count() {
            return Err(LeaseStopped);
        }
        {
            let mut progress = self.state.progress();
            if progress.closed {
                return Err(LeaseStopped);
            }
            if !progress.pushed.insert(salt_index) {
                drop(progress);
                tracing::warn!(
                    salt_index,
                    "local sampler pushed a salt twice; ignoring the repeat"
                );
                return Ok(());
            }
            progress.dispatched += 1;
        }
        if !record_samples(&self.state, &reads) {
            self.state.progress().finished += 1;
            return Err(LeaseStopped);
        }
        let result = self.send_result(salt_index, &reads);
        self.state.progress().finished += 1;
        result
    }

    /// Count a salt the backend screened out, from a blocking sampler thread.
    /// Its energy folds into the lease's best energy. Sends nothing.
    /// A repeated `salt_index` is ignored and logged.
    ///
    /// # Errors
    /// Returns a stop if the lease has ended, the index is outside its range,
    /// or the writer has gone.
    pub fn screen(&self, salt_index: u64, energy_milli: i64) -> Result<(), LeaseStopped> {
        if self.rejects_push() || salt_index >= self.state.lease.salt_count() {
            return Err(LeaseStopped);
        }
        let mut progress = self.state.progress();
        if progress.closed || progress.done_sent {
            return Err(LeaseStopped);
        }
        if !progress.pushed.insert(salt_index) {
            drop(progress);
            tracing::warn!(
                salt_index,
                "local sampler screened a salt twice; ignoring the repeat"
            );
            return Ok(());
        }
        progress.dispatched += 1;
        progress.finished += 1;
        progress.salts_done += 1;
        progress.best_energy_milli = progress.best_energy_milli.min(energy_milli);
        drop(progress);
        let _ = self.state.jobs_done.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// The live session target's `max_energy_milli`, or `None` without one.
    #[must_use]
    pub fn target_energy_milli(&self) -> Option<i64> {
        self.target.borrow().as_ref().map(|t| t.max_energy_milli)
    }

    /// Package the backend's reads for `index` into a wire `Result` and send it.
    /// The backend decides which salts to report; solver-core forwards them
    /// without filtering by target or re-verifying energies. An empty read
    /// set carries nothing worth reporting, so it sends nothing.
    fn send_result(&self, index: u64, reads: &[SamplerResult]) -> Result<(), LeaseStopped> {
        let meta = SamplerMeta {
            reads: u32::try_from(self.params.num_reads).unwrap_or(u32::MAX),
            sweeps: u32::try_from(self.params.num_sweeps).unwrap_or(u32::MAX),
            ..Default::default()
        };
        let Some(reply) = package_result(&self.state, index, reads, meta) else {
            return Ok(());
        };
        if self.rejects_push() {
            return Err(LeaseStopped);
        }
        #[cfg(test)]
        if let Some(pause) = &*self
            .before_send
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            pause();
        }
        self.send(reply)
    }

    fn send(&self, msg: MinerMsg) -> Result<(), LeaseStopped> {
        self.ctrl
            .upgrade()
            .ok_or(LeaseStopped)?
            .blocking_send(if matches!(msg.msg, Some(miner_msg::Msg::Fatal(_))) {
                msg.into()
            } else {
                Control::local(msg, Arc::clone(&self.state), self.shutdown.clone())
            })
            .map_err(|_| LeaseStopped)
    }

    fn fault(&self, error: &SampleError) {
        {
            let mut progress = self.state.progress();
            self.state.aborted.store(true, Ordering::Relaxed);
            progress.closed = true;
        }
        if !self.device_faulted.swap(true, Ordering::Relaxed) {
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
        if let Err(error) = result {
            if error.is_fatal() {
                self.fault(&error);
                return;
            }
        }
        let done = {
            let mut progress = self.state.progress();
            progress.closed = true;
            self.state.finish_locked(&mut progress, false)
        };
        if let Some(done) = done {
            if let Some(ctrl) = self.ctrl.upgrade() {
                let _ = ctrl.blocking_send(Control::summary(
                    done,
                    Some((Arc::clone(&self.state), self.shutdown.clone())),
                ));
            }
        }
    }
}

/// Close stopped local leases even when the backend never returns.
pub(crate) async fn monitor_local(
    state: Arc<LeaseState>,
    cancel: CancelToken,
    mut shutdown: watch::Receiver<Option<ShutdownDeadlines>>,
    tx: mpsc::Sender<MinerMsg>,
) {
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(5));
    loop {
        let deadlines = *shutdown.borrow();
        let close_expired = deadlines.is_some_and(|d| tokio::time::Instant::now() >= d.close);
        let done = {
            let mut progress = state.progress();
            if state.stopped(&cancel) || close_expired {
                progress.closed = true;
            }
            state.finish_locked(&mut progress, close_expired)
        };
        if let Some(done) = done {
            // Stop summaries bypass queued results after cancellation or the
            // close deadline. The writer drops those results using the same state.
            let control = Control::summary(done, Some((Arc::clone(&state), shutdown.clone())));
            let send = crate::session::send_control(&tx, control, &cancel);
            tokio::pin!(send);
            loop {
                let teardown = shutdown.borrow().map(|d| d.teardown);
                tokio::select! {
                    biased;
                    _ = &mut send => break,
                    _ = shutdown.changed() => {},
                    () = wait_for_grace(teardown) => return,
                }
            }
        }
        if state.progress().done_sent || state.aborted.load(Ordering::Relaxed) {
            break;
        }
        tokio::select! {
            _ = tick.tick() => {},
            _ = shutdown.changed() => {},
            () = wait_for_grace(deadlines.map(|d| d.close)) => {},
        }
    }
}

async fn wait_for_grace(deadline: Option<tokio::time::Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline).await;
    } else {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushed_salts_merge_ranges_and_reject_repeats() {
        let mut pushed = PushedSalts::default();
        for index in [0, 1, 2, 5, 4, 3] {
            assert!(pushed.insert(index), "{index} is new");
        }
        assert_eq!(pushed.0.len(), 1, "0..6 must merge into one range");
        for index in 0..6 {
            assert!(!pushed.insert(index), "{index} repeats");
        }
        assert!(pushed.insert(u64::MAX - 1));
        assert!(!pushed.insert(u64::MAX - 1));
    }

    fn sink_for(
        state: &Arc<LeaseState>,
        target: Option<SessionTarget>,
        ctrl: &mpsc::Sender<Control>,
    ) -> LeaseSink {
        LeaseSink {
            before_send: Mutex::new(None),
            state: Arc::clone(state),
            cancel: CancelToken::default(),
            shutdown: watch::channel(None).1,
            target: watch::channel(target).1,
            ctrl: ctrl.downgrade(),
            device_faulted: Arc::new(AtomicBool::new(false)),
            params: SampleParams::default(),
        }
    }

    #[test]
    fn a_repeated_local_salt_counts_once() {
        let state = test_state(None);
        let (ctrl, _ctrl_rx) = mpsc::channel::<Control>(4);
        let sink = sink_for(&state, None, &ctrl);
        let read = || {
            vec![SamplerResult {
                spins: vec![1],
                energy_milli: -1000,
            }]
        };
        assert_eq!(sink.push(0, read()), Ok(()));
        assert_eq!(sink.push(0, read()), Ok(()));
        let progress = state.progress();
        assert_eq!(
            (progress.dispatched, progress.finished, progress.salts_done),
            (1, 1, 1)
        );
    }

    #[test]
    fn a_screened_salt_counts_and_folds_its_energy_without_sending() {
        let state = test_state(None);
        let (ctrl, mut ctrl_rx) = mpsc::channel::<Control>(4);
        let sink = sink_for(&state, None, &ctrl);
        assert_eq!(sink.screen(0, -700), Ok(()));
        assert_eq!(sink.screen(0, -900), Ok(()), "a repeat is ignored");
        assert_eq!(sink.screen(1, -900), Err(LeaseStopped), "outside the lease");
        let progress = state.progress();
        assert_eq!(
            (progress.dispatched, progress.finished, progress.salts_done),
            (1, 1, 1)
        );
        assert_eq!(progress.best_energy_milli, -700);
        drop(progress);
        assert!(ctrl_rx.try_recv().is_err(), "screen sends nothing");
    }

    #[test]
    fn a_closed_lease_refuses_screens() {
        let state = test_state(None);
        state.progress().closed = true;
        let (ctrl, _ctrl_rx) = mpsc::channel::<Control>(4);
        let sink = sink_for(&state, None, &ctrl);
        assert_eq!(sink.screen(0, -1), Err(LeaseStopped));
        assert_eq!(state.progress().salts_done, 0);
    }

    #[test]
    fn target_energy_follows_the_session_target() {
        let state = test_state(None);
        let (ctrl, _ctrl_rx) = mpsc::channel::<Control>(4);
        assert_eq!(sink_for(&state, None, &ctrl).target_energy_milli(), None);
        let target = SessionTarget {
            max_energy_milli: -1234,
            min_solutions: 0,
            max_proof_solutions: 0,
            num_reads: 0,
            num_sweeps: 0,
        };
        assert_eq!(
            sink_for(&state, Some(target), &ctrl).target_energy_milli(),
            Some(-1234)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn local_monitor_submits_one_summary_and_refund_at_grace_expiry() {
        use std::future::Future as _;

        let state = test_state(Some(1));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(100);
        let (_shutdown_tx, shutdown) = watch::channel(Some(ShutdownDeadlines {
            close: deadline,
            teardown: deadline,
        }));
        let (tx, mut rx) = mpsc::channel(2);
        let cancel = CancelToken::default();
        let monitor = monitor_local(
            Arc::clone(&state),
            cancel.clone(),
            shutdown.clone(),
            tx.clone(),
        );
        tokio::pin!(monitor);

        // Keep the worker unreturned and the transport unpolled throughout grace.
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(monitor.as_mut().poll(&mut context).is_pending());
        tokio::time::advance(std::time::Duration::from_millis(99)).await;
        assert!(monitor.as_mut().poll(&mut context).is_pending());
        assert!(!state.progress().closed);
        assert!(rx.try_recv().is_err());

        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        assert!(monitor.as_mut().poll(&mut context).is_ready());
        assert_eq!(tokio::time::Instant::now(), deadline);
        assert!(state.progress().closed);
        assert_eq!(
            rx.try_recv().unwrap().msg,
            Some(miner_msg::Msg::LeaseDone(LeaseDone {
                job_id: b"lease".to_vec(),
                salts_done: 0,
                best_energy_milli: i64::MAX,
            }))
        );
        assert_eq!(
            rx.try_recv().unwrap().msg,
            Some(miner_msg::Msg::JobRequest(JobRequest { credits: 1 }))
        );
        // A later observer must not submit a second completion or refund.
        monitor_local(state, cancel, shutdown, tx).await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn local_monitor_never_sends_a_summary_without_its_refund() {
        let state = test_state(Some(1));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(100);
        let (_shutdown_tx, shutdown) = watch::channel(Some(ShutdownDeadlines {
            close: deadline,
            teardown: deadline,
        }));
        // Two slots, one already taken: room for the summary but not its refund.
        let (tx, mut rx) = mpsc::channel(2);
        tx.send(miner(miner_msg::Msg::JobRequest(JobRequest { credits: 7 })))
            .await
            .unwrap();
        tokio::time::advance(std::time::Duration::from_millis(100)).await;
        monitor_local(Arc::clone(&state), CancelToken::default(), shutdown, tx).await;
        assert_eq!(
            rx.try_recv().unwrap().msg,
            Some(miner_msg::Msg::JobRequest(JobRequest { credits: 7 }))
        );
        assert!(
            rx.try_recv().is_err(),
            "a summary reached the writer without its refund"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn local_monitor_retries_summary_until_teardown_deadline() {
        let state = test_state(Some(1));
        let close = tokio::time::Instant::now() + std::time::Duration::from_millis(20);
        let teardown = close + std::time::Duration::from_millis(20);
        let (_shutdown_tx, shutdown) = watch::channel(Some(ShutdownDeadlines { close, teardown }));
        // One free slot remains, but a summary and refund need two slots.
        let (tx, mut rx) = mpsc::channel(2);
        tx.send(miner(miner_msg::Msg::JobRequest(JobRequest { credits: 7 })))
            .await
            .unwrap();
        let monitor = tokio::spawn(monitor_local(
            Arc::clone(&state),
            CancelToken::default(),
            shutdown,
            tx,
        ));

        tokio::time::advance(std::time::Duration::from_millis(20)).await;
        tokio::task::yield_now().await;
        assert!(state.progress().closed);
        assert_eq!(
            rx.try_recv().unwrap().msg,
            Some(miner_msg::Msg::JobRequest(JobRequest { credits: 7 }))
        );
        tokio::task::yield_now().await;
        assert!(
            matches!(
                rx.try_recv().unwrap().msg,
                Some(miner_msg::Msg::LeaseDone(_))
            ),
            "the summary must be sent after capacity returns during the flush margin"
        );
        assert_eq!(
            rx.try_recv().unwrap().msg,
            Some(miner_msg::Msg::JobRequest(JobRequest { credits: 1 }))
        );
        monitor.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn expander_reports_after_short_shutdown_changes_while_waiting_for_sampler() {
        use crate::coefficient::Milli;

        let state = test_state(None);
        let (jobs, _jobs_rx) = mpsc::channel::<StreamJob<Milli>>(1);
        let (ctrl, mut ctrl_rx) = mpsc::channel::<Control>(1);
        let (shutdown_tx, shutdown) = watch::channel(None);
        let expander = Expander {
            jobs: JobSender::Plain(jobs),
            pending: Arc::new(Mutex::new(std::collections::HashMap::new())),
            slots: Arc::new(Semaphore::new(1)),
            cancel: CancelToken::default(),
            shutdown,
            ctrl,
        };
        let run = tokio::spawn(expander.run(Arc::clone(&state), SampleParams::default()));
        tokio::task::yield_now().await;
        assert_eq!(state.progress().dispatched, 1);
        assert!(state.progress().closed);

        // The first interval tick ran during dispatch. Publish Shutdown just after it.
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        let teardown = tokio::time::Instant::now() + std::time::Duration::from_millis(10);
        let _ = shutdown_tx.send_replace(Some(ShutdownDeadlines {
            close: teardown - std::time::Duration::from_micros(2500),
            teardown,
        }));
        tokio::time::advance(std::time::Duration::from_millis(9)).await;

        let control = ctrl_rx
            .try_recv()
            .expect("shutdown change must force a summary before teardown");
        let (out, mut out_rx) = mpsc::channel(2);
        assert!(crate::session::send_control(&out, control, &CancelToken::default()).await);
        assert!(matches!(
            out_rx.try_recv().unwrap().msg,
            Some(miner_msg::Msg::LeaseDone(_))
        ));
        assert_eq!(
            out_rx.try_recv().unwrap().msg,
            Some(miner_msg::Msg::JobRequest(JobRequest { credits: 1 }))
        );
        run.await.unwrap();
    }

    #[test]
    #[ignore = "manual release-mode timing check on an Advantage2-sized synthetic topology"]
    #[expect(clippy::print_stdout, reason = "manual timing result for --nocapture")]
    fn lease_draw_advantage2_timing() -> Result<(), Box<dyn std::error::Error>> {
        use crate::coefficient::Fixed;
        use std::{hint::black_box, time::Instant};

        const NODES: usize = 4577;
        const EDGES: usize = 41_515;
        const SALTS: u64 = 200;
        let spec = LeaseSpec::new(
            Generator::Blake3Chacha8V1,
            [1; 32],
            [2; 32],
            [3; 32],
            0,
            SALTS,
        )?;
        let state = LeaseState {
            job_id: b"lease".to_vec(),
            lease: Lease::new(spec, None),
            topology: Arc::new(TopologyView {
                num_nodes: NODES,
                edges: (1..NODES)
                    .flat_map(|offset| (0..NODES).map(move |u| (u, (u + offset) % NODES)))
                    .take(EDGES)
                    .collect(),
                allowed_h_milli: vec![-1000, 0, 1000],
                allowed_j_milli: vec![-1000, 1000],
            }),
            edges: Arc::new([]),
            watermark: None,
            deadline_ms: 0,
            aborted: Arc::new(AtomicBool::new(false)),
            jobs_done: Arc::new(AtomicU64::new(0)),
            progress: Mutex::new(Progress {
                dispatched: 0,
                finished: 0,
                salts_done: 0,
                best_energy_milli: i64::MAX,
                closed: false,
                done_sent: false,
                pushed: PushedSalts::default(),
            }),
        };

        let start = Instant::now();
        for index in 0..SALTS {
            let (graph, exact) = draw_graph::<Fixed<i8, 1>>(&state, black_box(index))?;
            if exact.is_some() {
                return Err("draw did not convert exactly to Fixed<i8, 1>".into());
            }
            drop(black_box(graph));
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a 200-salt count is exact in f64"
        )]
        let mean = start.elapsed().as_secs_f64() / SALTS as f64;
        println!("lease_draw_advantage2_timing: {SALTS} salts, mean {mean:.9} s per salt");
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_bounds_an_expander_on_a_full_control_queue() {
        use crate::coefficient::Milli;
        let (ctrl, _stalled_outbound) = mpsc::channel(1);
        ctrl.send(miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 })).into())
            .await
            .unwrap();
        assert_eq!(ctrl.capacity(), 0);
        let spec =
            LeaseSpec::new(Generator::Blake3Chacha8V1, [1; 32], [2; 32], [3; 32], 0, 1).unwrap();
        let topology = TopologyView::from_proto(&Topology {
            nodes: vec![1],
            allowed_h_milli: vec![1000],
            ..Default::default()
        })
        .unwrap();
        let state = Arc::new(LeaseState {
            job_id: b"lease".to_vec(),
            lease: Lease::new(spec, None),
            edges: topology.edges.as_slice().into(),
            topology: Arc::new(topology),
            watermark: None,
            deadline_ms: 0,
            aborted: Arc::new(AtomicBool::new(false)),
            jobs_done: Arc::new(AtomicU64::new(0)),
            progress: Mutex::new(Progress {
                dispatched: 0,
                finished: 0,
                salts_done: 0,
                best_energy_milli: i64::MAX,
                closed: false,
                done_sent: false,
                pushed: PushedSalts::default(),
            }),
        });
        let (jobs, _jobs_rx) = mpsc::channel::<StreamJob<Milli>>(1);
        let expander = Expander {
            jobs: JobSender::Plain(jobs),
            pending: Arc::new(Mutex::new(std::collections::HashMap::new())),
            slots: Arc::new(Semaphore::new(1)),
            cancel: CancelToken::default(),
            shutdown: watch::channel(Some(ShutdownDeadlines {
                close: tokio::time::Instant::now(),
                teardown: tokio::time::Instant::now(),
            }))
            .1,
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
        let lease = Lease::new(spec, None);
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

#[cfg(test)]
mod family_tests {
    use super::*;
    use crate::families::{Declaration, FamilyClaim};
    use crate::layered;
    use ising_generators::descriptor::params_digest;
    use ising_generators::instance::InstanceLimits;
    use quip_proto::v1::{EdgeList, FamilyDescriptor, GeneratorAlgorithm, IsingProblemGenerator};

    const TIGHT_LIMITS: InstanceLimits = match InstanceLimits::new(
        16_777_216,
        67_108_864,
        2000,
        72_057_594_037_927_936,
        67_108_864,
    ) {
        Some(limits) => limits,
        None => InstanceLimits::FAM_37,
    };
    const UNIFORM_V1: Contract = Contract {
        family_id: "vectors/uniform-field",
        revision: 1,
        schema: layered::UNIFORM_FIELD,
        metadata_kinds: &[1],
        limits: TIGHT_LIMITS,
        size_bounds: layered::example_size_bounds,
        construction: Some(&layered::LAYERED),
    };
    const RESTRICTED_V1: Contract = Contract {
        family_id: "vectors/restricted-field",
        ..UNIFORM_V1
    };
    const P_SPEC: [u8; 19] = [
        0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0x24, 0xfa, 0xff, 0xff,
    ];
    const P_L2_F0: [u8; 19] = [0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0];
    const P_L3_F0: [u8; 19] = [0x01, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0];
    const P_F5000: [u8; 19] = [
        0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0x88, 0x13, 0, 0,
    ];
    const CLAIMS: &[FamilyClaim] = &[
        FamilyClaim {
            revisions: &[UNIFORM_V1],
            only_params: &[],
        },
        FamilyClaim {
            revisions: &[RESTRICTED_V1],
            only_params: &[&P_SPEC],
        },
    ];
    /// `T_SPARSE` edges in registration order.
    const SPARSE_EDGES: [(u32, u32); 7] = [
        (907, 41),
        (41, 0),
        (0, 4_294_967_295),
        (4_294_967_295, 12),
        (12, 3),
        (3, 907),
        (41, 12),
    ];

    fn table() -> FamilyTable {
        Declaration {
            solver_id: "quip-mock/family-lease",
            generated: CLAIMS,
            accepted: CLAIMS,
        }
        .validate()
        .expect("the test declaration is valid")
    }

    fn wire_topology(nodes: &[u32], edges: &[(u32, u32)]) -> Topology {
        Topology {
            hash: vec![7; 32],
            nodes: nodes.to_vec(),
            edges: Some(EdgeList {
                u: edges.iter().map(|e| e.0).collect(),
                v: edges.iter().map(|e| e.1).collect(),
            }),
            allowed_h_milli: vec![-1000, 0, 1000],
            allowed_j_milli: vec![-1000, 1000],
        }
    }

    fn sparse() -> Topology {
        wire_topology(&[907, 41, 0, 4_294_967_295, 12, 3], &SPARSE_EDGES)
    }

    fn digest_of(t: &Topology) -> Vec<u8> {
        let base = LeaseTopology::new(t, &table())
            .family
            .expect("built")
            .expect("valid");
        base.order_digest().to_vec()
    }

    fn identity() -> BackendIdentity {
        BackendIdentity {
            backend: quip_proto::v1::Backend::Mock,
            algorithm: quip_proto::v1::Algorithm::Sa,
            max_nodes: 0,
            max_edges: 0,
            features: &[],
            adapt: crate::adapt::AdaptBounds {
                min_sweeps: 64,
                max_sweeps: 4096,
                min_reads: 64,
                max_reads: 512,
                reads_solution_min_factor: 4,
                reads_solution_max_factor: 8,
                reads_solution_floor_factor: 0,
            },
            limits: crate::ModelLimits {
                max_degree: Some(4),
                ..crate::ModelLimits::UNSTATED
            },
        }
    }

    const TARGET: SessionTarget = SessionTarget {
        max_energy_milli: i64::MAX,
        min_solutions: 1,
        max_proof_solutions: 32,
        num_reads: 1,
        num_sweeps: 1,
    };

    fn family_job(contract: &Contract, params: &[u8]) -> Job {
        Job {
            job_id: b"lease".to_vec(),
            generation: 1,
            generator: Some(IsingProblemGenerator {
                algorithm: GeneratorAlgorithm::FamilyChacha8V1 as i32,
                topology_hash: vec![7; 32],
                last_proof_block_hash: vec![0x11; 32],
                miner_account: vec![0x22; 32],
                base_salt: [vec![0; 8], vec![0x33; 24]].concat(),
                salt_start: 5,
                salt_count: 4,
                family: Some(FamilyDescriptor {
                    family_id: contract.family_id.into(),
                    revision: contract.revision,
                    params: params.to_vec(),
                    params_digest: params_digest(params).to_vec(),
                }),
                topology_order_digest: digest_of(&sparse()),
            }),
            ..Default::default()
        }
    }

    fn run_prepare(
        job: &Job,
        topology: Option<&Arc<LeaseTopology>>,
        target: Option<&SessionTarget>,
        families: &FamilyTable,
        max_reads: u32,
    ) -> Result<(Arc<LeaseState>, SampleParams), RejectReason> {
        prepare(
            job,
            topology,
            target,
            &identity(),
            families,
            max_reads,
            64,
            None,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(0)),
        )
    }

    fn generator(job: &mut Job) -> &mut IsingProblemGenerator {
        job.generator.as_mut().expect("generator")
    }

    fn descriptor(job: &mut Job) -> &mut FamilyDescriptor {
        generator(job).family.as_mut().expect("family")
    }

    fn set_params(job: &mut Job, contract: &Contract, params: &[u8]) {
        let family = descriptor(job);
        contract.family_id.clone_into(&mut family.family_id);
        family.params = params.to_vec();
        family.params_digest = params_digest(params).to_vec();
    }

    #[test]
    #[expect(
        clippy::type_complexity,
        clippy::too_many_lines,
        reason = "one table of named mutations covers every FAM-47 step"
    )]
    fn admission_codes_follow_fam_47() {
        let families = table();
        let generic = Declaration::GENERIC.validate().expect("generic");
        let sparse_cache = Arc::new(LeaseTopology::new(&sparse(), &families));
        let mut doubled_edges = SPARSE_EDGES.to_vec();
        doubled_edges.push((41, 907));
        let doubled = wire_topology(&[907, 41, 0, 4_294_967_295, 12, 3], &doubled_edges);
        let doubled_cache = Arc::new(LeaseTopology::new(&doubled, &families));
        let mut reordered_edges = SPARSE_EDGES.to_vec();
        reordered_edges.swap(0, 1);
        let reordered = wire_topology(&[907, 41, 0, 4_294_967_295, 12, 3], &reordered_edges);
        let invalid = Arc::new(LeaseTopology::new(&wire_topology(&[1, 1], &[]), &families));
        let none = |_: &mut Job| {};
        let cases: Vec<(
            &str,
            Box<dyn Fn(&mut Job)>,
            Option<&Arc<LeaseTopology>>,
            bool,
            RejectReason,
        )> = vec![
            (
                "expired",
                Box::new(|j| j.deadline_ms = 1),
                None,
                false,
                RejectReason::Expired,
            ),
            (
                "topology missing",
                Box::new(none),
                None,
                true,
                RejectReason::TopologyMissing,
            ),
            (
                "target missing",
                Box::new(none),
                Some(&sparse_cache),
                false,
                RejectReason::TargetMissing,
            ),
            (
                "generator absent",
                Box::new(|j| j.generator = None),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "salt count zero",
                Box::new(|j| generator(j).salt_count = 0),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 1",
                Box::new(|j| generator(j).family = None),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 2",
                Box::new(|j| generator(j).algorithm = GeneratorAlgorithm::Blake3Chacha8V1 as i32),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 3",
                Box::new(|j| "Quip/X".clone_into(&mut descriptor(j).family_id)),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 4",
                Box::new(|j| descriptor(j).revision = 0),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 5",
                Box::new(|j| "vectors/none".clone_into(&mut descriptor(j).family_id)),
                Some(&sparse_cache),
                true,
                RejectReason::FamilyUnsupported,
            ),
            (
                "code 6",
                Box::new(|j| descriptor(j).revision = 2),
                Some(&sparse_cache),
                true,
                RejectReason::FamilyUnsupported,
            ),
            (
                "code 7",
                Box::new(|j| set_params(j, &UNIFORM_V1, &[0x02])),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 8",
                Box::new(|j| descriptor(j).params_digest = vec![0; 32]),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 9",
                Box::new(|j| set_params(j, &UNIFORM_V1, &P_F5000)),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 10",
                Box::new(|j| set_params(j, &RESTRICTED_V1, &P_L2_F0)),
                Some(&sparse_cache),
                true,
                RejectReason::FamilyUnsupported,
            ),
            (
                "code 8 beats a pending 10",
                Box::new(|j| {
                    set_params(j, &RESTRICTED_V1, &P_L2_F0);
                    descriptor(j).params_digest = params_digest(&P_SPEC).to_vec();
                }),
                Some(&sparse_cache),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 10 beats the topology hash",
                Box::new(|j| {
                    set_params(j, &RESTRICTED_V1, &P_L2_F0);
                    generator(j).topology_hash = vec![9; 32];
                }),
                Some(&sparse_cache),
                true,
                RejectReason::FamilyUnsupported,
            ),
            (
                "topology hash",
                Box::new(|j| generator(j).topology_hash = vec![9; 32]),
                Some(&sparse_cache),
                true,
                RejectReason::TopologyMismatch,
            ),
            (
                "invalid cached topology",
                Box::new(none),
                Some(&invalid),
                true,
                RejectReason::Malformed,
            ),
            (
                "code 11",
                Box::new(|j| generator(j).topology_order_digest = digest_of(&reordered)),
                Some(&sparse_cache),
                true,
                RejectReason::TopologyMismatch,
            ),
            (
                "code 12",
                Box::new(|j| {
                    generator(j).topology_order_digest = digest_of(&doubled);
                    set_params(j, &UNIFORM_V1, &P_L3_F0);
                }),
                Some(&doubled_cache),
                true,
                RejectReason::FamilyUnsupported,
            ),
            (
                "code 13",
                Box::new(|j| set_params(j, &UNIFORM_V1, &P_L3_F0)),
                Some(&sparse_cache),
                true,
                RejectReason::TooLarge,
            ),
        ];
        for (name, mutate, topology, with_target, expected) in cases {
            let mut job = family_job(&UNIFORM_V1, &P_SPEC);
            mutate(&mut job);
            let target = with_target.then_some(&TARGET);
            let got = run_prepare(&job, topology, target, &families, 512).map(|_| ());
            assert_eq!(got, Err(expected), "{name}");
        }
        let job = family_job(&UNIFORM_V1, &P_SPEC);
        assert_eq!(
            run_prepare(&job, Some(&sparse_cache), Some(&TARGET), &families, 0).map(|_| ()),
            Err(RejectReason::TooLarge),
            "max_reads is last"
        );
        let generic_cache = Arc::new(LeaseTopology::new(&sparse(), &generic));
        assert!(generic_cache.family.is_none());
        assert_eq!(
            run_prepare(&job, Some(&generic_cache), Some(&TARGET), &generic, 512).map(|_| ()),
            Err(RejectReason::FamilyUnsupported),
            "FAM-54"
        );
        let (state, _) = run_prepare(&job, Some(&sparse_cache), Some(&TARGET), &families, 512)
            .expect("the base lease is admitted");
        let family = state.lease.family().expect("a family lease");
        assert_eq!(
            (family.family_id(), family.revision(), family.params()),
            (UNIFORM_V1.family_id, 1, &P_SPEC[..])
        );
        assert_eq!(
            *family.size_bounds(),
            SizeBounds {
                nodes: 12,
                edges: 20,
                degree: 4
            }
        );
    }

    #[test]
    fn a_family_lease_snapshot_ignores_later_inputs() {
        let families = table();
        let mut cache = Arc::new(LeaseTopology::new(&sparse(), &families));
        let mut job = family_job(&UNIFORM_V1, &P_SPEC);
        let (state, _) =
            run_prepare(&job, Some(&cache), Some(&TARGET), &families, 512).expect("admitted");
        let nonce = state.lease.nonce(1);
        let family = state.lease.family().expect("a family lease");
        let before = family
            .generate(nonce)
            .expect("salt 1 is not skipped")
            .canonical_bytes()
            .expect("bytes");
        set_params(&mut job, &UNIFORM_V1, &P_L2_F0);
        generator(&mut job).topology_order_digest = vec![0; 32];
        let mut reordered_edges = SPARSE_EDGES.to_vec();
        reordered_edges.swap(0, 1);
        cache = Arc::new(LeaseTopology::new(
            &wire_topology(&[907, 41, 0, 4_294_967_295, 12, 3], &reordered_edges),
            &families,
        ));
        drop(cache);
        let after = family
            .generate(nonce)
            .expect("salt 1 is not skipped")
            .canonical_bytes()
            .expect("bytes");
        assert_eq!(before, after);
        assert_eq!(family.params(), &P_SPEC[..]);
        assert_eq!(
            family.topology().order_digest()[..],
            digest_of(&sparse())[..]
        );
    }

    #[test]
    fn a_skipped_salt_counts_without_energy() {
        let state = test_state(None);
        state.skip_salt();
        {
            let progress = state.progress();
            assert_eq!(
                (progress.salts_done, progress.dispatched, progress.finished),
                (1, 0, 0)
            );
            assert_eq!(progress.best_energy_milli, i64::MAX);
        }
        assert_eq!(state.jobs_done.load(Ordering::Relaxed), 0);
        state.progress().done_sent = true;
        state.skip_salt();
        assert_eq!(
            state.progress().salts_done,
            1,
            "a reported lease counts nothing"
        );
    }
}
