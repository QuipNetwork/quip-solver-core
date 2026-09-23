//! Job validation and dispatch: wire fields → [`IsingGraph`] → [`Sampler`] →
//! `Result`/`Reject`.

use crate::adapt::adapt_params;
use crate::coefficient::Coefficient;
use crate::ising::{IsingGraph, SampleParams, WarmStart};
use crate::session::BackendIdentity;
use crate::Sampler;
use crate::{StreamJob, StreamOutcome, StreamResult};
use quip_proto::v1::{
    ising_problem, miner_msg, Fatal, IsingProblem, Job, JobKind, JobRequest, MinerMsg, Reject,
    RejectReason, Result as JobResult, SamplerMeta, Solution, Status, Topology,
};
use quip_protocol::session::ExitCode;
use quip_protocol::wire::{decode_i32_le, decode_spins_packed, encode_spins_packed};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Session-cached topology, used to resolve `TopologyHash` jobs to dense edges.
///
/// Built once from the `Topology` message at Configure. `pos` maps each native
/// node id to its dense position (id → index in received order), so sparse
/// D-Wave qubit ids resolve without a per-job remap.
pub(crate) struct TopologyCache {
    hash: Vec<u8>,
    edges: Vec<(u32, u32)>,
    pos: HashMap<u32, usize>,
    allowed_h: Vec<i32>,
    /// `Some(reason)` when the `Topology` message itself did not validate.
    ///
    /// `Topology` arrives outside any job, so there is no message to reject at
    /// the moment the fault is found; the cache remembers it and every job that
    /// names this topology is rejected with it. Keeping a silently truncated or
    /// collapsed cache is the failure this replaces — the miner would sample a
    /// *different* graph than the coordinator scored against and return
    /// confident, wrong energies on the live mining path.
    invalid: Option<RejectReason>,
}

impl TopologyCache {
    /// Build from a `Topology` message. Node ids map to their received-order
    /// position; edges keep received order (the consensus `j`-zip invariant).
    ///
    /// The inline-edge path in [`resolve_edges`] already rejects an unequal
    /// `u`/`v` pair, and this path now holds the same bar: a cached topology is
    /// the *more* dangerous of the two, because one bad `Configure` misdirects
    /// every job in the session rather than one job.
    pub(crate) fn from_proto(t: &Topology) -> Self {
        let mut invalid = None;
        let mut pos = HashMap::with_capacity(t.nodes.len());
        for (i, &node) in t.nodes.iter().enumerate() {
            // A repeated id collapses two dense positions onto one, leaving
            // `pos` shorter than `nodes` and silently renumbering every node
            // after it.
            if pos.insert(node, i).is_some() {
                invalid = Some(RejectReason::Malformed);
            }
        }
        let edges: Vec<(u32, u32)> = match t.edges.as_ref() {
            Some(e) => {
                // `zip` stops at the shorter side, so an unequal u/v pair drops
                // the tail of the longer one without a trace.
                if e.u.len() != e.v.len() {
                    invalid = Some(RejectReason::Malformed);
                }
                e.u.iter().zip(&e.v).map(|(&u, &v)| (u, v)).collect()
            }
            None => Vec::new(),
        };
        Self {
            hash: t.hash.clone(),
            edges,
            pos,
            allowed_h: t.allowed_h_milli.clone(),
            invalid,
        }
    }

    pub(crate) fn allowed_h(&self) -> &[i32] {
        &self.allowed_h
    }

    /// Number of nodes in the cached topology. Equal to `nodes.len()`, since
    /// [`Self::from_proto`] marks a duplicate id invalid rather than dropping it.
    pub(crate) fn num_nodes(&self) -> usize {
        self.pos.len()
    }
}

/// Session difficulty target from `SetTarget`. The miner adapts its sampling
/// budget from `max_energy_milli`; the `num_*` fields are optional overrides.
#[derive(Clone)]
pub(crate) struct SessionTarget {
    pub(crate) max_energy_milli: i64,
    pub(crate) min_solutions: u32,
    pub(crate) min_diversity_milli: u32,
    pub(crate) max_proof_solutions: u32,
    pub(crate) num_reads: u32,
    pub(crate) num_sweeps: u32,
}

impl SessionTarget {
    pub(crate) fn to_target(&self) -> quip_protocol::target::Target {
        quip_protocol::target::Target {
            max_energy_milli: self.max_energy_milli,
            min_solutions: self.min_solutions,
            min_diversity_milli: self.min_diversity_milli,
            max_proof_solutions: self.max_proof_solutions,
        }
    }

    // anneal_time_us is ignored on the SA/GPU Rust path (QPU adapt lives in the
    // Python dwave miner).
    pub(crate) fn from_proto(s: &quip_proto::v1::SetTarget) -> Self {
        Self {
            max_energy_milli: s.max_energy_milli,
            min_solutions: s.min_solutions,
            min_diversity_milli: s.min_diversity_milli,
            max_proof_solutions: s.max_proof_solutions,
            num_reads: s.num_reads,
            num_sweeps: s.num_sweeps,
        }
    }
}

/// Resolve one sampling param: per-job override, else `SetTarget` override,
/// else the adapted value, else the fallback. `0` means "unset".
pub(crate) fn pick_param(job: u32, target: u32, adapt: Option<u32>, fallback: u32) -> u32 {
    if job != 0 {
        job
    } else if target != 0 {
        target
    } else {
        adapt.unwrap_or(fallback)
    }
}

pub(crate) const DEFAULT_NUM_SWEEPS: usize = 64;

// v0.2 parity: the Python GPU miners ran Gibbs at 2x the SA sweep budget
// (`GIBBS_SWEEP_MULTIPLIER` in GPU/cuda_miner.py, GPU/metal_miner.py) because
// Gibbs converges slower per sweep than SA. The v0.3 adapt path resolves an
// algorithm-agnostic `num_sweeps`, so this gate restores that parity for all
// backends (cpu/cuda/metal) sharing this code path.
const GIBBS_SWEEP_MULTIPLIER: u32 = 2;

/// Wall-clock milliseconds since the Unix epoch, or `None` when the clock is
/// before the epoch and no deadline can be judged against it.
///
/// The `None` is not theoretical: a container that starts before NTP sets the
/// clock, or a board with a dead RTC, reports a pre-epoch time. Defaulting to
/// `0` there — which is what this used to do — reads every deadline as "far in
/// the future" and silently accepts expired work for as long as the clock is
/// wrong. Callers fail closed instead.
pub(crate) fn now_unix_ms() -> Option<u64> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "unix ms since epoch fits u64 for the lifetime of this protocol"
    )]
    {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as u64)
    }
}

/// Fresh per-job seed from OS entropy, or `None` when the OS CSPRNG failed.
///
/// Two jobs issued in the same millisecond must not sample identically, which a
/// wall-clock-derived seed would cause (solver-core is shared by
/// cpu/cuda/metal), so there is no fallback source: a job without a real seed is
/// not sampled at all.
///
/// Failure here used to panic. A panic unwinds to the process boundary and
/// exits 101, which is not one of the SPEC section 2 codes a supervisor knows
/// how to read, and it takes down a miner over a condition that is usually
/// transient (exhausted descriptors, a seccomp filter, an unseeded early-boot
/// pool). Rejecting the one job instead keeps the miner up and tells the
/// coordinator to place the work elsewhere.
pub(crate) fn os_seed() -> Option<u64> {
    let mut bytes = [0u8; 8];
    match getrandom::getrandom(&mut bytes) {
        Ok(()) => Some(u64::from_le_bytes(bytes)),
        Err(e) => {
            tracing::error!("OS CSPRNG unavailable, rejecting job: {e}");
            None
        }
    }
}

pub(crate) fn miner(msg: miner_msg::Msg) -> MinerMsg {
    MinerMsg { msg: Some(msg) }
}

pub(crate) fn status_msg(
    miner_id: &str,
    jobs_done: u64,
    utilization: f64,
    abandoned_generation: u64,
) -> MinerMsg {
    miner(miner_msg::Msg::Status(Status {
        miner_id: miner_id.into(),
        utilization,
        jobs_done,
        abandoned_generation,
        sampler_stats: HashMap::default(),
    }))
}

pub(crate) fn reject(job_id: Vec<u8>, reason: RejectReason) -> MinerMsg {
    miner(miner_msg::Msg::Reject(Reject {
        job_id,
        reason: reason as i32,
    }))
}

fn resolve_edges(
    ising: &IsingProblem,
    cache: Option<&TopologyCache>,
) -> Result<Vec<(usize, usize)>, RejectReason> {
    match &ising.graph {
        // Inline edges use dense 0..n-1 ids straight off the wire.
        Some(ising_problem::Graph::Edges(e)) => {
            if e.u.len() != e.v.len() {
                return Err(RejectReason::Malformed);
            }
            Ok(e.u
                .iter()
                .zip(&e.v)
                .map(|(&u, &v)| (u as usize, v as usize))
                .collect())
        }
        // Topology-hash jobs resolve against the session-cached topology,
        // mapping native (possibly sparse) node ids to dense positions.
        Some(ising_problem::Graph::TopologyHash(h)) => {
            let cache = cache.ok_or(RejectReason::TopologyMissing)?;
            if *h != cache.hash {
                return Err(RejectReason::TopologyMismatch);
            }
            // Hash checked first: it is the more precise answer, and a cache
            // that failed validation still carries the hash verbatim. Only once
            // the job really names *this* topology does its validity matter.
            if let Some(reason) = cache.invalid {
                return Err(reason);
            }
            let mut out = Vec::with_capacity(cache.edges.len());
            for &(u, v) in &cache.edges {
                let pu = *cache.pos.get(&u).ok_or(RejectReason::Malformed)?;
                let pv = *cache.pos.get(&v).ok_or(RejectReason::Malformed)?;
                out.push((pu, pv));
            }
            Ok(out)
        }
        None => Ok(Vec::new()),
    }
}

/// The shape invariant every Ising problem must satisfy, whichever entry point
/// it arrived by: exactly one coupling per edge, and every edge endpoint inside
/// `h`.
///
/// This is a memory-safety boundary, not a tidiness check. `CSampler::sample`
/// (quip-solver-c) hands the C callback `num_edges = j.len()` alongside an
/// `edges` array built from `edges.len()` pairs, so a problem where those two
/// disagree makes the callback read `2 * j.len()` u32 out of an array that is
/// shorter — reproduced under `ASan` as a SEGV driven by a coordinator message.
/// The `!edges.is_empty() &&` short-circuit this replaces let the worst case
/// straight through: an `IsingProblem` with no graph oneof at all resolves to
/// zero edges, so a nonempty `j` was paired with `edges.as_ptr()` on an *empty*
/// `Vec`, i.e. a dangling pointer.
///
/// Both callers reject on any error; the message exists so driver mode can tell
/// its caller which of the two invariants failed.
pub(crate) fn validate_shape(
    h_len: usize,
    j_len: usize,
    edges: &[(usize, usize)],
) -> Result<(), String> {
    if j_len != edges.len() {
        return Err(format!(
            "j has {j_len} couplings but the graph has {} edges",
            edges.len()
        ));
    }
    for &(u, v) in edges {
        if u >= h_len || v >= h_len {
            return Err(format!(
                "edge ({u}, {v}) references a node outside h (h has {h_len} entries)"
            ));
        }
    }
    Ok(())
}

/// Validate wire fields and build the base Ising graph, or a reject reason.
fn parse_ising<C: Coefficient>(
    ising: &IsingProblem,
    max_nodes: u32,
    max_edges: u32,
    cache: Option<&TopologyCache>,
) -> Result<ParsedIsing<C>, RejectReason> {
    if ising.encoding != quip_proto::v1::CoefficientEncoding::I32 as i32 || ising.scale != 1000 {
        return Err(RejectReason::Malformed);
    }
    let h_milli = decode_i32_le(&ising.h).map_err(|_| RejectReason::Malformed)?;
    let j_milli = decode_i32_le(&ising.j).map_err(|_| RejectReason::Malformed)?;
    let edges = resolve_edges(ising, cache)?;
    let n = h_milli.len();

    // A topology-hash job names the cached graph, so its biases must cover
    // exactly that graph's nodes. The endpoint bounds in `validate_shape` only
    // catch an `h` that is too *short*; an over-long `h` would otherwise sample
    // a graph padded with phantom, unconnected variables and report an energy
    // for it. `cache` is always Some here — resolve_edges returned
    // TopologyMissing otherwise.
    let is_hash_job = matches!(&ising.graph, Some(ising_problem::Graph::TopologyHash(_)));
    if is_hash_job && cache.is_some_and(|c| n != c.num_nodes()) {
        return Err(RejectReason::Malformed);
    }

    // Shape before size: a malformed problem is malformed at any size, and
    // answering TooLarge would invite the coordinator to retry it on a bigger
    // miner, where it fails exactly the same way.
    validate_shape(n, j_milli.len(), &edges).map_err(|_| RejectReason::Malformed)?;

    // `0` means "no limit" in the advertised caps (see Capabilities in
    // quip_protocol::session; the coordinator's router reads it the same way).
    // A zero-initialized C identity therefore advertises "unlimited", and must
    // not then reject every job it is sent as TooLarge.
    let over_nodes = max_nodes != 0 && n > max_nodes as usize;
    let over_edges = max_edges != 0 && edges.len() > max_edges as usize;
    if over_nodes || over_edges {
        return Err(RejectReason::TooLarge);
    }

    let graph = IsingGraph {
        h: h_milli.iter().copied().map(C::from_milli).collect(),
        j: j_milli.iter().copied().map(C::from_milli).collect(),
        edges,
    };
    let exact_energy = if C::EXACT {
        None
    } else {
        Some(ExactEnergy::new(h_milli, j_milli, graph.edges.clone()))
    };
    Ok(ParsedIsing {
        graph,
        exact_energy,
    })
}

/// Validate the warm-start fields (`IsingProblem` 9 to 12) against a job of
/// `num_nodes` variables, and decode them when `decode` is set.
///
/// Validation runs for every solver, so one malformed job gets the same
/// `Malformed` everywhere, whether or not the solver uses the states. Decoding
/// runs only for a solver that does. The start-point fields mean nothing
/// without a state, so a job with no state yields `None` whatever they hold.
/// The states are cut to `num_reads`: a state past the last read has no read to
/// seed.
fn parse_warm_start(
    ising: &IsingProblem,
    num_nodes: usize,
    num_reads: usize,
    decode: bool,
) -> Result<Option<WarmStart>, RejectReason> {
    if ising.initial_spins.is_empty() {
        return Ok(None);
    }
    // `s` is a fraction of the anneal, strictly inside (0, 1); 0 means unset.
    if ising.reversal_s_milli >= 1000 {
        return Err(RejectReason::Malformed);
    }
    let mut spins = Vec::new();
    for packed in &ising.initial_spins {
        let state = decode_spins_packed(packed, num_nodes).map_err(|_| RejectReason::Malformed)?;
        if decode && spins.len() < num_reads {
            spins.push(state);
        }
    }
    if !decode {
        return Ok(None);
    }
    let milli = |v: u32| (v != 0).then(|| f64::from(v) / 1000.0);
    Ok(Some(WarmStart {
        spins,
        start_beta: milli(ising.start_beta_milli),
        reversal_s: milli(ising.reversal_s_milli),
        reversal_pause_us: (ising.reversal_pause_us != 0).then_some(ising.reversal_pause_us),
    }))
}

/// Read the session loop's `num_sweeps` out of `Configure.backend_toml`.
///
/// The hand-rolled line scan this replaces was wrong in both directions. It
/// accepted `num_sweeps_extra = 512` (a prefix match, not a key match) and any
/// `num_sweeps` nested under a backend's own `[table]`, and it silently
/// rejected quoted values, underscore separators, and any `#` inside a quoted
/// string. Every rejection fell through to [`DEFAULT_NUM_SWEEPS`] without a
/// word, so a coordinator that asked for 512 sweeps and got 64 had no way to
/// find out — a wrong-answer bug that looks exactly like a slow miner.
///
/// The `toml` crate is already compiled in this workspace (cbindgen builds the
/// quip-solver-c header with it), so the 0.9 line adds no new crate to the
/// dependency tree — only a direct edge to one already in it.
pub(crate) fn num_sweeps_from_toml(backend_toml: &str) -> usize {
    let table: toml::Table = match backend_toml.parse() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(
                "config: backend_toml does not parse as TOML ({e}); \
                 using num_sweeps = {DEFAULT_NUM_SWEEPS}"
            );
            return DEFAULT_NUM_SWEEPS;
        }
    };
    // Only a top-level key configures the session loop. A `num_sweeps` under
    // `[some_backend]` belongs to that backend's own schema.
    let Some(value) = table.get("num_sweeps") else {
        return DEFAULT_NUM_SWEEPS;
    };
    match value.as_integer().and_then(|n| usize::try_from(n).ok()) {
        Some(n) if n > 0 => n,
        // Present but unusable: wrong type, zero, or negative. Warn rather than
        // quietly substituting the default (see above).
        _ => {
            tracing::warn!(
                "config: num_sweeps = {value} is not a positive integer; \
                 using {DEFAULT_NUM_SWEEPS}"
            );
            DEFAULT_NUM_SWEEPS
        }
    }
}

/// Original wire coefficients retained only for lossy samplers.
#[derive(Debug)]
pub(crate) struct ExactEnergy {
    h: Vec<i32>,
    j: Vec<i32>,
    edges: Vec<(usize, usize)>,
}

impl ExactEnergy {
    pub(crate) fn new(h: Vec<i32>, j: Vec<i32>, edges: Vec<(usize, usize)>) -> Self {
        Self { h, j, edges }
    }

    pub(crate) fn rescore(&self, reads: &mut [crate::SamplerResult]) {
        for read in reads {
            read.energy_milli = quip_protocol::scoring::energy_from_milli(
                &read.spins,
                &self.h,
                &self.j,
                &self.edges,
            );
        }
    }
}

#[derive(Debug)]
struct ParsedIsing<C: Coefficient> {
    graph: IsingGraph<C>,
    exact_energy: Option<ExactEnergy>,
}

/// A validated job ready to sample, or an immediate reject reply.
pub(crate) enum Prepared<C: Coefficient> {
    /// Reject reply to send now (no sampling).
    Reject(MinerMsg),
    /// Hand to the streaming sampler; `num_reads`/`num_sweeps` are the resolved
    /// values, carried so [`finalize_result`] can build `SamplerMeta`.
    Sample {
        job: StreamJob<C>,
        /// Decoded start states, `Some` only for a seeded job sent to a
        /// sampler whose `accepts_warm_start` is true.
        warm_start: Option<WarmStart>,
        /// Original coefficients when sampler conversion loses information.
        exact_energy: Option<ExactEnergy>,
        num_reads: u32,
        num_sweeps: u32,
    },
}

/// Validate + resolve one job: reject reasons short-circuit; otherwise resolve
/// the sampling budget (per-job override > `SetTarget` override > adapt > default)
/// and return a [`StreamJob`] for the streaming sampler.
pub(crate) fn prepare_job<S: Sampler<C>, C: Coefficient>(
    job: Job,
    sampler: &S,
    id: &BackendIdentity,
    default_sweeps: usize,
    sweeps_per_beta: Option<usize>,
    cache: Option<&TopologyCache>,
    target: Option<&SessionTarget>,
) -> Prepared<C> {
    let job_id = job.job_id.clone();

    if job.kind != JobKind::IsingSample as i32 {
        return Prepared::Reject(reject(job_id, RejectReason::UnsupportedKind));
    }
    // deadline_ms == 0 means "no deadline" (only mempool/chain jobs carry one).
    if job.deadline_ms != 0 {
        // Fail closed on an unusable clock: it cannot say the job is still
        // live, and sampling work that has already expired spends device time
        // on results the coordinator discards.
        let expired = now_unix_ms().map_or_else(
            || {
                tracing::error!(
                    "system clock is before the Unix epoch; rejecting deadlined jobs as Expired"
                );
                true
            },
            |now| job.deadline_ms < now,
        );
        if expired {
            return Prepared::Reject(reject(job_id, RejectReason::Expired));
        }
    }
    let Some(ising) = job.ising else {
        return Prepared::Reject(reject(job_id, RejectReason::Malformed));
    };

    let ParsedIsing {
        graph,
        exact_energy,
    } = match parse_ising::<C>(&ising, id.max_nodes, id.max_edges, cache) {
        Ok(parsed) => parsed,
        Err(reason) => return Prepared::Reject(reject(job_id, reason)),
    };

    // adapt runs only when a target is set (uses the parsed problem's node/edge
    // counts and the topology's allowed_h).
    let adapt = target.map(|t| {
        let allowed_h = cache.map_or(&[][..], TopologyCache::allowed_h);
        adapt_params(
            t.max_energy_milli,
            t.min_solutions.max(1),
            graph.num_nodes(),
            graph.edges.len(),
            allowed_h,
            &id.adapt,
        )
    });
    let t_reads = target.map_or(0, |t| t.num_reads);
    let t_sweeps = target.map_or(0, |t| t.num_sweeps);
    let num_reads = pick_param(ising.num_reads, t_reads, adapt.map(|a| a.num_reads), 1);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "default_sweeps comes from config/CLI and is a small sweep budget well under u32::MAX"
    )]
    let num_sweeps = pick_param(
        ising.num_sweeps,
        t_sweeps,
        adapt.map(|a| a.num_sweeps),
        default_sweeps as u32,
    );
    let num_sweeps = if id.algorithm == quip_proto::v1::Algorithm::Gibbs {
        num_sweeps.saturating_mul(GIBBS_SWEEP_MULTIPLIER)
    } else {
        num_sweeps
    };

    // Shape before size, as in parse_ising.
    let warm_start = match parse_warm_start(
        &ising,
        graph.num_nodes(),
        num_reads as usize,
        S::accepts_warm_start(),
    ) {
        Ok(w) => w,
        Err(reason) => return Prepared::Reject(reject(job_id, reason)),
    };

    if num_reads > sampler.max_reads() {
        return Prepared::Reject(reject(job_id, RejectReason::TooLarge));
    }

    // Overloaded, not Malformed: the job is fine, this miner just cannot serve
    // it right now. That is the reason code that asks the coordinator to retry
    // the work somewhere else.
    let Some(seed) = os_seed() else {
        return Prepared::Reject(reject(job_id, RejectReason::Overloaded));
    };

    let params = SampleParams {
        num_reads: num_reads as usize,
        num_sweeps: num_sweeps as usize,
        seed,
        sweeps_per_beta: sweeps_per_beta.unwrap_or(1),
        ..Default::default()
    };
    Prepared::Sample {
        job: StreamJob {
            job_id,
            graph,
            params,
            // Generation 0 marks a mempool job, which a reseed never cancels. That
            // is a chain rule, so it is applied here rather than inside
            // CancelToken.
            watermark: (job.generation != 0).then_some(job.generation),
        },
        warm_start,
        exact_energy,
        num_reads,
        num_sweeps,
    }
}

/// Turn a [`StreamResult`] into the reply(s): a `Result` on completion, a
/// `Reject` on sampler error, or nothing upstream when the job's generation was
/// cancelled. Every path refunds one credit (`JobRequest{1}`) so the
/// coordinator's consume-on-dispatch pool never leaks a slot. `num_reads`/
/// `num_sweeps` are the resolved values from [`prepare_job`], echoed into
/// `SamplerMeta`.
pub(crate) fn finalize_result(
    sr: StreamResult,
    num_reads: u32,
    num_sweeps: u32,
    jobs_done: &mut u64,
) -> Vec<MinerMsg> {
    let samples = match sr.outcome {
        StreamOutcome::Completed(Ok(s)) => s,
        // A reject is terminal for this job too, so replace its credit like a
        // completion does — otherwise the coordinator's consume-on-dispatch
        // pool leaks one slot per reject and the pipeline slowly starves.
        //
        // A DeviceFault is the exception: the device will not recover, so the
        // session sends Fatal instead of asking for more work.
        StreamOutcome::Completed(Err(err)) => {
            let reason = err.to_reject_reason();
            if err.is_fatal() {
                return vec![
                    reject(sr.job_id, reason),
                    miner(miner_msg::Msg::Fatal(Fatal {
                        exit_code: ExitCode::InternalFatal as u32,
                        reason: err.to_string(),
                        restart_required: true,
                    })),
                ];
            }
            return vec![
                reject(sr.job_id, reason),
                miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 })),
            ];
        }
        // Generation abandoned on reseed: the coordinator has moved on, so send
        // no stale Result/Reject — only refund the credit to keep pipeline depth.
        StreamOutcome::Cancelled => {
            return vec![miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 }))]
        }
    };
    let solutions: Vec<Solution> = samples
        .into_iter()
        .map(|r| Solution {
            spins: encode_spins_packed(&r.spins),
            energy_milli: r.energy_milli,
        })
        .collect();

    *jobs_done = jobs_done.saturating_add(1);

    let result = JobResult {
        salt: vec![],
        nonce: vec![],
        job_id: sr.job_id,
        solutions,
        meta: Some(SamplerMeta {
            reads: num_reads,
            sweeps: num_sweeps,
            device_access_time_us: sr.device_access_time_us,
            qpu_access_us: 0,
            extra: HashMap::default(),
        }),
    };

    vec![
        miner(miner_msg::Msg::Result(result)),
        miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 })),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapt::AdaptBounds;
    use crate::ising::{IsingGraph, SamplerResult};
    use quip_proto::v1::{ising_problem::Graph, EdgeList, Topology};
    use quip_protocol::wire::encode_i32_le;

    struct StubSampler;

    impl Sampler for StubSampler {
        fn sample(
            &self,
            _graph: &IsingGraph,
            _params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, crate::SampleError> {
            Ok(vec![])
        }
    }

    const TEST_ADAPT: AdaptBounds = AdaptBounds {
        min_sweeps: 1,
        max_sweeps: u32::MAX,
        min_reads: 1,
        max_reads: u32::MAX,
        reads_solution_min_factor: 1,
        reads_solution_max_factor: 1,
        reads_solution_floor_factor: 0,
    };

    fn identity(algorithm: &'static str) -> BackendIdentity {
        BackendIdentity {
            backend: quip_proto::v1::Backend::Mock,
            algorithm: quip_protocol::session::algorithm_from_name(algorithm).unwrap(),
            max_nodes: 100_000,
            max_edges: 1_000_000,
            features: &[],
            adapt: TEST_ADAPT,
        }
    }

    /// A minimal valid `IsingSample` job with inline edges (no topology cache
    /// needed) and `num_sweeps` left unset (0), so `prepare_job` falls back to
    /// `default_sweeps`.
    /// `edges_job` with an explicit chain generation, for the watermark rule.
    fn edges_job_at_generation(job_id: u8, generation: u64) -> Job {
        Job {
            generation,
            ..edges_job(job_id)
        }
    }

    fn edges_job(job_id: u8) -> Job {
        Job {
            generator: None,
            job_id: vec![job_id],
            kind: JobKind::IsingSample as i32,
            generation: 0,
            deadline_ms: now_unix_ms().map_or(u64::MAX, |now| now + 60_000),
            ising: Some(IsingProblem {
                graph: Some(Graph::Edges(EdgeList {
                    u: vec![0],
                    v: vec![1],
                })),
                encoding: quip_proto::v1::CoefficientEncoding::I32 as i32,
                scale: 1000,
                h: encode_i32_le(&[1000, 1000]),
                j: encode_i32_le(&[1000]),
                num_reads: 0,
                num_sweeps: 0,
                anneal_time_us: 0,
                ..Default::default()
            }),
            provenance: None,
        }
    }

    fn hash_job(hash: Vec<u8>, h_milli: &[i32], j_milli: &[i32]) -> IsingProblem {
        IsingProblem {
            graph: Some(Graph::TopologyHash(hash)),
            encoding: quip_proto::v1::CoefficientEncoding::I32 as i32,
            scale: 1000,
            h: encode_i32_le(h_milli),
            j: encode_i32_le(j_milli),
            num_reads: 0,
            num_sweeps: 0,
            anneal_time_us: 0,
            ..Default::default()
        }
    }

    #[test]
    fn wire_v2_accepts_only_i32_at_scale_1000() {
        let mut ising = edges_job(1).ising.unwrap();
        for (encoding, scale) in [
            (0, 1000),
            (2, 1000),
            (3, 1000),
            (4, 0),
            (5, 0),
            (6, 0),
            (99, 1000),
            (1, 0),
            (1, 1),
        ] {
            ising.encoding = encoding;
            ising.scale = scale;
            assert_eq!(
                parse_ising::<f64>(&ising, 100, 100, None).unwrap_err(),
                RejectReason::Malformed
            );
        }
        ising.encoding = quip_proto::v1::CoefficientEncoding::I32 as i32;
        ising.scale = 1000;
        assert!(parse_ising::<f64>(&ising, 100, 100, None).is_ok());
    }

    #[test]
    fn generator_jobs_are_not_accepted_before_lease_support() {
        let mut job = edges_job(1);
        job.kind = JobKind::IsingGenerate as i32;
        let Prepared::Reject(message) =
            prepare_job(job, &StubSampler, &identity("sa"), 64, None, None, None)
        else {
            panic!("generator job must be rejected");
        };
        let Some(miner_msg::Msg::Reject(reject)) = message.msg else {
            panic!("expected rejection");
        };
        assert_eq!(reject.reason, RejectReason::UnsupportedKind as i32);
    }

    #[test]
    fn topology_cache_maps_sparse_ids_to_positions() {
        let topo = Topology {
            allowed_j_milli: vec![],
            hash: vec![0xAB; 32],
            nodes: vec![0, 12, 2400],
            allowed_h_milli: vec![],
            edges: Some(EdgeList {
                u: vec![0, 12],
                v: vec![12, 2400],
            }),
        };
        let c = TopologyCache::from_proto(&topo);
        assert_eq!(c.pos.get(&0), Some(&0));
        assert_eq!(c.pos.get(&12), Some(&1));
        assert_eq!(c.pos.get(&2400), Some(&2));
        // received order, unsorted
        assert_eq!(c.edges, vec![(0, 12), (12, 2400)]);
        assert_eq!(c.hash, vec![0xAB; 32]);
    }

    #[test]
    fn hash_job_resolves_sparse_edges_to_positions() {
        let topo = Topology {
            allowed_j_milli: vec![],
            hash: vec![7; 32],
            nodes: vec![0, 12, 2400],
            allowed_h_milli: vec![],
            edges: Some(EdgeList {
                u: vec![0, 12],
                v: vec![12, 2400],
            }),
        };
        let cache = TopologyCache::from_proto(&topo);
        let ising = hash_job(vec![7; 32], &[1000, -1000, 1000], &[1000, -1000]);
        let g = parse_ising::<f64>(&ising, 100_000, 1_000_000, Some(&cache))
            .unwrap()
            .graph;
        assert_eq!(g.edges, vec![(0, 1), (1, 2)]);
        assert_eq!(g.h, vec![1.0, -1.0, 1.0]);
        assert_eq!(g.j, vec![1.0, -1.0]);
    }

    #[test]
    fn hash_job_without_cache_rejects_missing() {
        let ising = hash_job(vec![7; 32], &[1000], &[]);
        let err = parse_ising::<f64>(&ising, 100_000, 1_000_000, None).unwrap_err();
        assert_eq!(err, RejectReason::TopologyMissing);
    }

    #[test]
    fn hash_job_wrong_hash_rejects_mismatch() {
        let topo = Topology {
            allowed_j_milli: vec![],
            hash: vec![1; 32],
            nodes: vec![0],
            allowed_h_milli: vec![],
            edges: None,
        };
        let cache = TopologyCache::from_proto(&topo);
        let ising = hash_job(vec![2; 32], &[1000], &[]);
        let err = parse_ising::<f64>(&ising, 100_000, 1_000_000, Some(&cache)).unwrap_err();
        assert_eq!(err, RejectReason::TopologyMismatch);
    }

    #[test]
    fn param_precedence_job_over_target_over_adapt_over_fallback() {
        // per-job override wins over everything
        assert_eq!(pick_param(5, 7, Some(268), 1), 5);
        // SetTarget override wins when no per-job override
        assert_eq!(pick_param(0, 7, Some(268), 1), 7);
        // adapt value when neither override set
        assert_eq!(pick_param(0, 0, Some(268), 1), 268);
        // fallback when no target/adapt (e.g. no SetTarget cached)
        assert_eq!(pick_param(0, 0, None, 64), 64);
    }

    #[test]
    fn session_target_from_proto_carries_target_and_overrides() {
        let s = quip_proto::v1::SetTarget {
            max_proof_solutions: 0,
            max_energy_milli: -14_700_000,
            min_solutions: 5,
            min_diversity_milli: 200,
            num_reads: 0,
            num_sweeps: 99,
            anneal_time_us: 0,
        };
        let t = SessionTarget::from_proto(&s);
        assert_eq!(t.max_energy_milli, -14_700_000);
        assert_eq!(t.min_solutions, 5);
        assert_eq!(t.num_sweeps, 99);
        assert_eq!(t.num_reads, 0);
    }

    #[test]
    fn hash_job_edge_id_absent_from_map_rejects_malformed() {
        // edge references id 999, which is not in nodes → no position.
        let topo = Topology {
            allowed_j_milli: vec![],
            hash: vec![7; 32],
            nodes: vec![0, 1],
            allowed_h_milli: vec![],
            edges: Some(EdgeList {
                u: vec![0],
                v: vec![999],
            }),
        };
        let cache = TopologyCache::from_proto(&topo);
        let ising = hash_job(vec![7; 32], &[1000, 1000], &[1000]);
        let err = parse_ising::<f64>(&ising, 100_000, 1_000_000, Some(&cache)).unwrap_err();
        assert_eq!(err, RejectReason::Malformed);
    }

    /// Build an inline-edge problem straight from parts, bypassing the helpers
    /// so a test can state a deliberately inconsistent shape.
    fn inline_job(h_milli: &[i32], j_milli: &[i32], u: Vec<u32>, v: Vec<u32>) -> IsingProblem {
        IsingProblem {
            graph: Some(Graph::Edges(EdgeList { u, v })),
            encoding: quip_proto::v1::CoefficientEncoding::I32 as i32,
            scale: 1000,
            h: encode_i32_le(h_milli),
            j: encode_i32_le(j_milli),
            num_reads: 0,
            num_sweeps: 0,
            anneal_time_us: 0,
            ..Default::default()
        }
    }

    /// The headline bug (quip-solver-core-3eb). An `IsingProblem` carrying no
    /// graph oneof resolves to zero edges, and the old
    /// `!edges.is_empty() && j.len() != edges.len()` guard skipped the length
    /// check entirely for it. `CSampler::sample` then passed the callback
    /// `num_edges = 3` against an `edges` array built from an empty `Vec`,
    /// which is a dangling pointer — an out-of-bounds read reached from a
    /// coordinator message.
    #[test]
    fn a_problem_with_no_graph_but_couplings_is_malformed() {
        let ising = IsingProblem {
            graph: None,
            encoding: quip_proto::v1::CoefficientEncoding::I32 as i32,
            scale: 1000,
            h: encode_i32_le(&[1000, 1000, 1000]),
            j: encode_i32_le(&[1000, -1000, 1000]),
            num_reads: 0,
            num_sweeps: 0,
            anneal_time_us: 0,
            ..Default::default()
        };
        assert_eq!(
            parse_ising::<f64>(&ising, 100_000, 1_000_000, None).unwrap_err(),
            RejectReason::Malformed,
            "3 couplings against 0 edges must never reach a sampler"
        );
    }

    /// The same invariant from the other side: no graph and no couplings is a
    /// legitimate (if trivial) problem, so the fix must not reject it.
    #[test]
    fn a_problem_with_no_graph_and_no_couplings_is_accepted() {
        let ising = IsingProblem {
            graph: None,
            encoding: quip_proto::v1::CoefficientEncoding::I32 as i32,
            scale: 1000,
            h: encode_i32_le(&[1000, 1000]),
            j: encode_i32_le(&[]),
            num_reads: 0,
            num_sweeps: 0,
            anneal_time_us: 0,
            ..Default::default()
        };
        let g = parse_ising::<f64>(&ising, 100_000, 1_000_000, None)
            .expect("an edgeless problem is fine")
            .graph;
        assert_eq!(g.num_nodes(), 2);
        assert!(g.edges.is_empty());
        assert!(g.j.is_empty());
    }

    #[test]
    fn inline_edges_with_unequal_u_and_v_are_malformed() {
        // resolve_edges rejects the pair before it can zip-truncate.
        let ising = inline_job(&[1000, 1000, 1000], &[1000, 1000], vec![0, 1], vec![1]);
        assert_eq!(
            parse_ising::<f64>(&ising, 100_000, 1_000_000, None).unwrap_err(),
            RejectReason::Malformed
        );
    }

    #[test]
    fn inline_edges_with_a_coupling_count_mismatch_are_malformed() {
        // 2 edges, 1 coupling.
        let ising = inline_job(&[1000, 1000, 1000], &[1000], vec![0, 1], vec![1, 2]);
        assert_eq!(
            parse_ising::<f64>(&ising, 100_000, 1_000_000, None).unwrap_err(),
            RejectReason::Malformed
        );
        // 1 edge, 2 couplings — the direction that used to reach the C ABI.
        let ising = inline_job(&[1000, 1000], &[1000, 1000], vec![0], vec![1]);
        assert_eq!(
            parse_ising::<f64>(&ising, 100_000, 1_000_000, None).unwrap_err(),
            RejectReason::Malformed
        );
    }

    #[test]
    fn an_edge_endpoint_outside_h_is_malformed() {
        let ising = inline_job(&[1000, 1000], &[1000], vec![0], vec![7]);
        assert_eq!(
            parse_ising::<f64>(&ising, 100_000, 1_000_000, None).unwrap_err(),
            RejectReason::Malformed
        );
    }

    // ---- quip-solver-core-oul: cached topology validation ----

    fn topology(nodes: Vec<u32>, u: Vec<u32>, v: Vec<u32>) -> Topology {
        Topology {
            allowed_j_milli: vec![],
            hash: vec![7; 32],
            nodes,
            allowed_h_milli: vec![],
            edges: Some(EdgeList { u, v }),
        }
    }

    /// `zip` truncates, so an unequal u/v pair silently dropped the tail. The
    /// inline path already rejected this; the cached path is worse, because one
    /// bad Configure misdirects every job for the rest of the session.
    #[test]
    fn a_topology_with_unequal_u_and_v_rejects_every_job() {
        let cache = TopologyCache::from_proto(&topology(vec![0, 1, 2], vec![0, 1], vec![1]));
        let ising = hash_job(vec![7; 32], &[1000, 1000, 1000], &[1000]);
        assert_eq!(
            parse_ising::<f64>(&ising, 100_000, 1_000_000, Some(&cache)).unwrap_err(),
            RejectReason::Malformed
        );
    }

    /// A repeated node id collapses two dense positions onto one, renumbering
    /// every node after it — the miner would sample a different graph than the
    /// coordinator scored.
    #[test]
    fn a_topology_with_duplicate_node_ids_rejects_every_job() {
        let cache = TopologyCache::from_proto(&topology(vec![0, 1, 1], vec![0], vec![1]));
        let ising = hash_job(vec![7; 32], &[1000, 1000, 1000], &[1000]);
        assert_eq!(
            parse_ising::<f64>(&ising, 100_000, 1_000_000, Some(&cache)).unwrap_err(),
            RejectReason::Malformed
        );
    }

    /// A wrong hash is still reported as a mismatch even when the cache itself
    /// is invalid: it is the more precise answer, and the hash is copied
    /// verbatim regardless.
    #[test]
    fn an_invalid_cache_still_reports_a_hash_mismatch_first() {
        let cache = TopologyCache::from_proto(&topology(vec![0, 1, 1], vec![0], vec![1]));
        let ising = hash_job(vec![9; 32], &[1000, 1000, 1000], &[1000]);
        assert_eq!(
            parse_ising::<f64>(&ising, 100_000, 1_000_000, Some(&cache)).unwrap_err(),
            RejectReason::TopologyMismatch
        );
    }

    /// A hash job names the cached graph, so `h` must cover exactly its nodes.
    /// An over-long `h` passes every endpoint bound and still samples the wrong
    /// problem.
    #[test]
    fn a_hash_job_whose_h_does_not_cover_the_topology_is_malformed() {
        let cache = TopologyCache::from_proto(&topology(vec![0, 1, 2], vec![0, 1], vec![1, 2]));
        for h in [
            vec![1000, 1000],                   // too short
            vec![1000, 1000, 1000, 1000],       // too long
            vec![1000, 1000, 1000, 1000, 1000], // much too long
        ] {
            let ising = hash_job(vec![7; 32], &h, &[1000, 1000]);
            assert_eq!(
                parse_ising::<f64>(&ising, 100_000, 1_000_000, Some(&cache)).unwrap_err(),
                RejectReason::Malformed,
                "h of {} against a 3-node topology must not sample",
                h.len()
            );
        }
        // The matching length still works.
        let ising = hash_job(vec![7; 32], &[1000, 1000, 1000], &[1000, 1000]);
        let g = parse_ising::<f64>(&ising, 100_000, 1_000_000, Some(&cache))
            .expect("matching h")
            .graph;
        assert_eq!(g.num_nodes(), 3);
    }

    // ---- quip-solver-core-vva: `0` means unlimited ----

    #[test]
    fn a_size_cap_admits_a_job_exactly_at_the_limit_and_rejects_one_past_it() {
        // 3 nodes, 2 edges.
        let ising = inline_job(&[1000, 1000, 1000], &[1000, 1000], vec![0, 1], vec![1, 2]);
        assert!(
            parse_ising::<f64>(&ising, 3, 2, None).is_ok(),
            "n == max_nodes"
        );
        assert_eq!(
            parse_ising::<f64>(&ising, 2, 2, None).unwrap_err(),
            RejectReason::TooLarge,
            "n == max_nodes + 1"
        );
        assert_eq!(
            parse_ising::<f64>(&ising, 3, 1, None).unwrap_err(),
            RejectReason::TooLarge,
            "edges == max_edges + 1"
        );
    }

    /// `0` is "no limit" in the advertised Hello caps. Treating it as a hard
    /// zero made a zero-initialized C identity advertise unlimited and then
    /// reject every job it was sent.
    #[test]
    fn a_zero_size_cap_means_unlimited_not_a_zero_ceiling() {
        let ising = inline_job(&[1000, 1000, 1000], &[1000, 1000], vec![0, 1], vec![1, 2]);
        assert!(
            parse_ising::<f64>(&ising, 0, 0, None).is_ok(),
            "max_nodes/max_edges of 0 must accept a large problem, not reject every one"
        );
        // Each bound is independently unlimited.
        assert!(parse_ising::<f64>(&ising, 0, 2, None).is_ok());
        assert!(parse_ising::<f64>(&ising, 3, 0, None).is_ok());
        // A zero cap on one axis does not excuse a real cap on the other.
        assert_eq!(
            parse_ising::<f64>(&ising, 0, 1, None).unwrap_err(),
            RejectReason::TooLarge
        );
    }

    // ---- quip-solver-core-0qb: num_sweeps from backend_toml ----

    #[test]
    fn num_sweeps_is_read_from_well_formed_toml() {
        for (input, want) in [
            ("num_sweeps = 512", 512),
            ("num_sweeps=512", 512),
            ("num_sweeps = 512 # trailing comment", 512),
            // TOML digit separators, which the old line scan could not parse.
            ("num_sweeps = 1_024", 1024),
            ("other = 1\nnum_sweeps = 512\n", 512),
        ] {
            assert_eq!(num_sweeps_from_toml(input), want, "input: {input:?}");
        }
    }

    #[test]
    fn an_unusable_or_absent_num_sweeps_falls_back_to_the_default() {
        for input in [
            "num_sweeps = 0",         // zero is not a sweep budget
            "num_sweeps = \"128\"",   // a string is not an integer
            "num_sweeps = -5",        // negative
            "num_sweeps = 1.5",       // not an integer
            "",                       // nothing configured
            "num_sweeps_extra = 512", // prefix match: the old scan took this as num_sweeps
            "extra_num_sweeps = 512",
            "not valid toml at all {{{", // unparseable
        ] {
            assert_eq!(
                num_sweeps_from_toml(input),
                DEFAULT_NUM_SWEEPS,
                "input: {input:?}"
            );
        }
    }

    /// A `num_sweeps` under a backend's own table belongs to that backend, not
    /// to the session loop. The old line scan matched it and applied it anyway.
    #[test]
    fn num_sweeps_under_another_table_does_not_configure_the_session() {
        assert_eq!(
            num_sweeps_from_toml("[other]\nnum_sweeps = 512"),
            DEFAULT_NUM_SWEEPS
        );
        // A top-level key still wins when it precedes a table.
        assert_eq!(
            num_sweeps_from_toml("num_sweeps = 128\n[other]\nnum_sweeps = 512"),
            128
        );
    }

    /// A `#` inside a quoted string is not a comment. The old scan cut the line
    /// there and mangled whatever followed.
    #[test]
    fn a_hash_inside_a_quoted_string_is_not_treated_as_a_comment() {
        assert_eq!(
            num_sweeps_from_toml("name = \"a # b\"\nnum_sweeps = 512"),
            512
        );
    }

    #[test]
    fn gibbs_job_resolves_2x_the_sweeps_of_the_same_sa_job() {
        let sampler = StubSampler;
        let default_sweeps = 64;

        let sa_id = identity("sa");
        let Prepared::Sample {
            num_sweeps: sa_sweeps,
            ..
        } = prepare_job(
            edges_job(1),
            &sampler,
            &sa_id,
            default_sweeps,
            None,
            None,
            None,
        )
        else {
            panic!("expected Sample");
        };

        let gibbs_id = identity("gibbs");
        let Prepared::Sample {
            num_sweeps: gibbs_sweeps,
            ..
        } = prepare_job(
            edges_job(2),
            &sampler,
            &gibbs_id,
            default_sweeps,
            None,
            None,
            None,
        )
        else {
            panic!("expected Sample");
        };

        #[expect(
            clippy::cast_possible_truncation,
            reason = "test default_sweeps is a small constant well under u32::MAX"
        )]
        {
            assert_eq!(sa_sweeps, default_sweeps as u32);
            assert_eq!(gibbs_sweeps, sa_sweeps * GIBBS_SWEEP_MULTIPLIER);
        }
    }

    /// The doubled gibbs budget must reach the wire: `SamplerMeta.sweeps`
    /// echoes what `prepare_job` resolved, one hop past the previous test.
    #[test]
    fn a_finalized_gibbs_result_reports_the_doubled_sweeps() {
        let sampler = StubSampler;
        let gibbs_id = identity("gibbs");
        let Prepared::Sample {
            job,
            num_reads,
            num_sweeps,
            ..
        } = prepare_job(edges_job(1), &sampler, &gibbs_id, 64, None, None, None)
        else {
            panic!("expected Sample");
        };

        let mut jobs_done = 0;
        let msgs = finalize_result(
            StreamResult {
                job_id: job.job_id,
                outcome: StreamOutcome::Completed(Ok(vec![SamplerResult {
                    spins: vec![1, -1],
                    energy_milli: -1000,
                }])),
                device_access_time_us: 7,
            },
            num_reads,
            num_sweeps,
            &mut jobs_done,
        );

        let sweeps: Vec<u32> = msgs
            .iter()
            .filter_map(|m| {
                if let Some(miner_msg::Msg::Result(r)) = &m.msg {
                    r.meta.as_ref().map(|meta| meta.sweeps)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(sweeps, vec![64 * GIBBS_SWEEP_MULTIPLIER]);
    }

    #[test]
    fn back_to_back_jobs_get_different_os_entropy_seeds() {
        let sampler = StubSampler;
        let id = identity("sa");

        let Prepared::Sample { job: job1, .. } =
            prepare_job(edges_job(1), &sampler, &id, 64, None, None, None)
        else {
            panic!("expected Sample");
        };
        let Prepared::Sample { job: job2, .. } =
            prepare_job(edges_job(2), &sampler, &id, 64, None, None, None)
        else {
            panic!("expected Sample");
        };

        assert_ne!(job1.params.seed, job2.params.seed);
    }

    #[test]
    fn sweeps_per_beta_flag_flows_into_sample_params() {
        let sampler = StubSampler;
        let id = identity("sa");

        // The miner-local CLI value threads through to SampleParams.
        let Prepared::Sample { job, .. } =
            prepare_job(edges_job(1), &sampler, &id, 64, Some(3), None, None)
        else {
            panic!("expected Sample");
        };
        assert_eq!(job.params.sweeps_per_beta, 3);

        // Unset falls back to the default of 1.
        let Prepared::Sample { job, .. } =
            prepare_job(edges_job(2), &sampler, &id, 64, None, None, None)
        else {
            panic!("expected Sample");
        };
        assert_eq!(job.params.sweeps_per_beta, 1);
    }

    /// The chain rule lives here, not in `CancelToken`: generation 0 means a
    /// mempool job, and a mempool job is never reseed-cancelled. Without this
    /// test an accidental unconditional `Some(job.generation)` compiles, passes
    /// clippy, and passes every other test, while silently making mempool jobs
    /// cancellable.
    #[test]
    fn a_mempool_job_gets_no_watermark() {
        let sampler = StubSampler;
        let id = identity("sa");
        let Prepared::Sample { job, .. } = prepare_job(
            edges_job_at_generation(1, 0),
            &sampler,
            &id,
            64,
            None,
            None,
            None,
        ) else {
            panic!("expected Sample");
        };
        assert_eq!(
            job.watermark, None,
            "generation 0 is a mempool job and must never carry a watermark"
        );
    }

    #[test]
    fn a_chain_job_carries_its_generation_as_the_watermark() {
        let sampler = StubSampler;
        let id = identity("sa");
        for generation in [1_u64, 2, 7, u64::MAX] {
            let Prepared::Sample { job, .. } = prepare_job(
                edges_job_at_generation(1, generation),
                &sampler,
                &id,
                64,
                None,
                None,
                None,
            ) else {
                panic!("expected Sample");
            };
            assert_eq!(
                job.watermark,
                Some(generation),
                "a chain job must carry its generation as the watermark"
            );
        }
    }

    #[test]
    fn status_carries_the_abandoned_watermark() {
        let Some(miner_msg::Msg::Status(status)) = status_msg("miner", 3, 0.5, 7).msg else {
            panic!("expected Status");
        };
        assert_eq!(
            status.abandoned_generation, 7,
            "a non-zero watermark must be carried into Status"
        );

        let Some(miner_msg::Msg::Status(status)) = status_msg("miner", 3, 0.5, 0).msg else {
            panic!("expected Status");
        };
        assert_eq!(
            status.abandoned_generation, 0,
            "a zero watermark must stay zero"
        );
    }

    /// A sampler that opts in to warm starts. `sample` is never reached here;
    /// these tests stop at `prepare_job`.
    struct WarmSampler;

    impl Sampler for WarmSampler {
        fn sample(
            &self,
            _graph: &IsingGraph,
            _params: &SampleParams,
        ) -> Result<Vec<SamplerResult>, crate::SampleError> {
            Ok(vec![])
        }

        fn accepts_warm_start() -> bool {
            true
        }
    }

    /// `edges_job` (2 nodes) carrying the given packed states and start point,
    /// with `num_reads` pinned.
    fn seeded_job(states: Vec<Vec<u8>>, num_reads: u32, start: (u32, u32, u32)) -> Job {
        let mut job = edges_job(1);
        if let Some(ising) = job.ising.as_mut() {
            ising.initial_spins = states;
            ising.num_reads = num_reads;
            ising.start_beta_milli = start.0;
            ising.reversal_s_milli = start.1;
            ising.reversal_pause_us = start.2;
        }
        job
    }

    fn prepared_warm_start<S: Sampler<C>, C: Coefficient>(
        job: Job,
        sampler: &S,
    ) -> Result<Option<WarmStart>, i32> {
        match prepare_job(job, sampler, &identity("sa"), 64, None, None, None) {
            Prepared::Sample { warm_start, .. } => Ok(warm_start),
            Prepared::Reject(MinerMsg {
                msg: Some(miner_msg::Msg::Reject(r)),
            }) => Err(r.reason),
            // A reject with no Reject payload; no reason code can match -1.
            Prepared::Reject(_) => Err(-1),
        }
    }

    #[test]
    fn a_warm_sampler_gets_decoded_states_cut_to_num_reads() {
        // Node 0 is bit 0: 0b01 = [+1, -1], 0b10 = [-1, +1], 0b11 = [+1, +1].
        let job = seeded_job(vec![vec![0b01], vec![0b10], vec![0b11]], 2, (2500, 400, 7));
        let warm = prepared_warm_start(job, &WarmSampler)
            .expect("a valid seeded job samples")
            .expect("a warm sampler receives the states");
        assert_eq!(warm.spins, vec![vec![1, -1], vec![-1, 1]]);
        assert_eq!(warm.start_beta, Some(2.5));
        assert_eq!(warm.reversal_s, Some(0.4));
        assert_eq!(warm.reversal_pause_us, Some(7));
    }

    #[test]
    fn unset_start_point_fields_decode_to_none() {
        let warm = prepared_warm_start(seeded_job(vec![vec![0b01]], 1, (0, 0, 0)), &WarmSampler)
            .expect("samples")
            .expect("seeded");
        assert_eq!(
            (warm.start_beta, warm.reversal_s, warm.reversal_pause_us),
            (None, None, None)
        );
    }

    #[test]
    fn a_plain_sampler_never_receives_the_states() {
        let job = seeded_job(vec![vec![0b01]], 1, (2500, 0, 0));
        assert_eq!(prepared_warm_start(job, &StubSampler), Ok(None));
    }

    #[test]
    fn a_start_point_without_a_state_is_a_cold_job() {
        let job = seeded_job(vec![], 1, (2500, 400, 7));
        assert_eq!(prepared_warm_start(job, &WarmSampler), Ok(None));
    }

    /// Validation is the same for every solver on this version, so one
    /// malformed job gets one answer whether or not the solver uses the states.
    #[test]
    fn a_malformed_state_is_rejected_by_every_sampler() {
        let malformed = RejectReason::Malformed as i32;
        for states in [
            vec![vec![0b01, 0x00]],        // 2 bytes for 2 nodes
            vec![vec![]],                  // empty entry
            vec![vec![0b01], vec![0b100]], // padding bit set in the second state
        ] {
            let job = || seeded_job(states.clone(), 1, (0, 0, 0));
            assert_eq!(prepared_warm_start(job(), &WarmSampler), Err(malformed));
            assert_eq!(prepared_warm_start(job(), &StubSampler), Err(malformed));
        }
    }

    #[test]
    fn a_reversal_point_outside_the_anneal_is_malformed() {
        let job = seeded_job(vec![vec![0b01]], 1, (0, 1000, 0));
        assert_eq!(
            prepared_warm_start(job, &WarmSampler),
            Err(RejectReason::Malformed as i32)
        );
    }

    #[test]
    fn lossy_decode_keeps_originals_but_exact_types_do_not() {
        use crate::coefficient::{Fixed, Milli};
        let problem = hash_job(vec![], &[499, -501], &[1501]);
        let problem = IsingProblem {
            graph: Some(Graph::Edges(EdgeList {
                u: vec![0],
                v: vec![1],
            })),
            ..problem
        };
        let parsed = parse_ising::<Fixed<i8, 1>>(&problem, 0, 0, None).expect("valid");
        assert_eq!(parsed.graph.h, vec![Fixed(0), Fixed(-1)]);
        assert_eq!(parsed.graph.j, vec![Fixed(2)]);
        let mut reads = vec![SamplerResult {
            spins: vec![1, -1],
            energy_milli: 123,
        }];
        parsed
            .exact_energy
            .expect("lossy input retained")
            .rescore(&mut reads);
        assert_eq!(reads.first().map(|r| r.energy_milli), Some(-501));
        let float = parse_ising::<f64>(&problem, 0, 0, None).expect("valid");
        let milli = parse_ising::<Milli>(&problem, 0, 0, None).expect("valid");
        assert!(float.exact_energy.is_none());
        assert!(milli.exact_energy.is_none());
        assert_eq!(milli.graph.h, vec![Fixed(499), Fixed(-501)]);
    }
}
