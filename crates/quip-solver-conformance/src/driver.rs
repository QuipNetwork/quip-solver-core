//! Mock coordinator: a tonic `MinerService` server that walks a miner binary
//! through a scripted protocol-conformance session over a Unix domain socket.
//!
//! The script is **phase-driven**: each phase sends its `CoordMsg`s and then
//! blocks on a barrier that reads inbound messages until the causally-required
//! reply arrives. Without that, a driver that fires the whole script and then
//! drains cannot attribute a reply to the message that caused it — every
//! `Status` looks like the `Cancel` acknowledgement, and a miner that answers
//! `Ping` but ignores `Cancel` grades the same as one that does both.

use quip_proto::v1::miner_service_server::{MinerService, MinerServiceServer};
use quip_proto::v1::{
    coord_msg, ising_problem, miner_msg, Cancel, Capabilities, Configure, CoordMsg, EdgeList,
    GetCapabilities, Hello, IsingProblem, IsingProblemGenerator, Job, JobKind, MinerMsg, Ping,
    RejectReason, SetTarget, Shutdown, Status as MinerStatus, Topology, Welcome,
};
use quip_protocol::lease::{verify_lease_result, TopologyView};
use quip_protocol::scoring::energy_milli;
use quip_protocol::target::Target;
use quip_protocol::wire::{decode_spins_packed, encode_i32_le, encode_spins_packed};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UnixListener;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::wrappers::{ReceiverStream, UnixListenerStream};
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};

/// How long one phase waits for the reply its `CoordMsg` must cause.
///
/// A phase that is never satisfied records itself in
/// [`DriverReport::timed_out_phases`] and the walk continues, so one
/// unimplemented feature surfaces as one named failure rather than a hang.
const PHASE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the harness waits for the miner process to exit.
///
/// Expressed in phase budgets rather than as a flat number: a walk in which
/// every phase times out must still run to completion, so the report names all
/// the phases that failed instead of reporting a single kill.
const CHILD_TIMEOUT: Duration = Duration::from_secs(PHASE_TIMEOUT.as_secs() * 13);

/// How long the harness waits for the session handler to hand back its outcome
/// once the child has exited.
const OUTCOME_TIMEOUT: Duration = Duration::from_secs(5);

/// `num_sweeps` the script pushes through `Configure.backend_toml`. Chosen
/// different from the SDK default (64) so an implementation that ignores
/// `backend_toml` and reports its default cannot pass by coincidence.
///
/// Public so a solver repository's tests can state expectations against the
/// same budget instead of mirroring the literal.
pub const CONFIGURED_SWEEPS: u32 = 512;

/// Sweep multiplier a `gibbs` solver applies to the configured budget.
///
/// SPEC.md: the pin is a sweep budget, not a literal count — a Gibbs solver
/// runs, and reports, twice the pinned number because Gibbs converges slower
/// per sweep. Mirrors the session's `GIBBS_SWEEP_MULTIPLIER` in
/// quip-solver-core, which this crate cannot depend on without a publish
/// cycle.
pub const GIBBS_SWEEP_MULTIPLIER: u32 = 2;

/// Generation carried by every ordinary job in the walk.
const LIVE_GENERATION: u64 = 2;

/// Generation used by the cancellation phases. Higher than
/// [`LIVE_GENERATION`], so cancelling it cannot retroactively abandon the
/// ordinary jobs — every one of those is already settled behind a barrier.
const CANCEL_GENERATION: u64 = 3;

const LEASE_JOB_ID: &[u8] = b"job-lease";
const LEASE_SALTS: u32 = 4;

/// Observations from the four-salt generation scenario.
#[derive(Debug, Clone, Default)]
pub struct LeaseOutcome {
    /// Number of lease results received.
    pub results: u32,
    /// Number of distinct salts verified before completion.
    pub results_verified: u32,
    /// `(salts_done, best_energy_milli)` from `LeaseDone`.
    pub lease_done: Option<(u64, i64)>,
    /// Whether one credit arrived after `LeaseDone`.
    pub credit_refunded: bool,
}

/// A reject observed during the scripted session, bound to its `job_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedReject {
    /// Job id the miner attached to the `Reject`.
    pub job_id: Vec<u8>,
    /// Proto `RejectReason` discriminant as `i32`.
    pub reason: i32,
}

/// A `Result` observed during the scripted session: its `job_id`, the reported
/// and independently re-scored energies, whether `SamplerMeta` was attached,
/// and the sweep count that meta declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedResult {
    /// Job id the miner attached to the `Result`.
    pub job_id: Vec<u8>,
    /// `energy_milli` the miner reported for each solution, in arrival order.
    pub solution_energies_milli: Vec<i64>,
    /// The driver's own score of the returned `spins` under the problem
    /// it sent, in arrival order. `None` for the whole vector when the driver
    /// has no problem on file for this job id (so it cannot re-score);
    /// `None` for one entry when that solution's spins did not decode to the
    /// problem's width.
    ///
    /// This is the consensus check the harness exists for: a solver may return
    /// any spins it likes, but the energy it claims for them must be the energy
    /// they have.
    pub rescored_energies_milli: Option<Vec<Option<i64>>>,
    /// Whether `SamplerMeta` was present on the `Result`.
    pub meta_present: bool,
    /// `SamplerMeta.sweeps`, or `0` when no meta was attached.
    pub meta_sweeps: u32,
}

/// How the miner's message stream ended. The three ways it can stop are
/// different failures, and folding them together hides which one happened.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Terminal {
    /// The stream was still open when the walk finished. For the full walk
    /// this is itself a failure: `Shutdown` must end the session.
    #[default]
    Open,
    /// The miner closed its send stream cleanly (`Ok(None)`). The expected end.
    Closed,
    /// A `MinerMsg` arrived with no `msg` oneof set — a well-formed envelope
    /// carrying nothing, which the protocol has no meaning for.
    EmptyMessage,
    /// The transport failed. Carries the gRPC status code and message.
    Transport(String),
}

/// Outcome of driving one miner through the conformance session.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "an observation record, not a state machine: each flag is an independent axis the walk grades, and collapsing them into enums would hide which one failed"
)]
pub struct DriverReport {
    /// Lease observations, populated only when generation is advertised.
    pub lease: Option<LeaseOutcome>,
    /// True when the first inbound message was a valid `Hello`.
    pub handshake_ok: bool,
    /// The `Hello` the miner opened with, when it sent one.
    pub hello: Option<Hello>,
    /// True when a `Ready` arrived after `Configure` was sent.
    pub ready_received: bool,
    /// Credits advertised on each `JobRequest`, in arrival order. The first is
    /// the initial grant (the reply to `Configure`); the rest are refunds.
    pub job_request_credits: Vec<u32>,
    /// How many `Job`s the script dispatched. Every one of them must return
    /// exactly one credit, whatever its terminal state.
    pub jobs_dispatched: usize,
    /// Every `Result` received (order preserved).
    pub results: Vec<ObservedResult>,
    /// Every `Reject`, bound to its `job_id` (not just the reason code).
    pub rejects: Vec<ObservedReject>,
    /// Every `Status` received (order preserved).
    pub statuses: Vec<MinerStatus>,
    /// True when a `Status` arrived in the `Cancel` phase, with no other
    /// coordinator message outstanding that could have caused it.
    pub cancel_acked: bool,
    /// True when a `Status` arrived in the `Ping` phase.
    pub ping_acked: bool,
    /// The `Capabilities` the miner returned for `GetCapabilities`.
    pub capabilities_received: Option<Capabilities>,
    /// Highest generation the script cancelled, or `0` when it cancelled none.
    pub cancelled_watermark: u64,
    /// `(exit_code, reason)` from a `Fatal`, if the miner sent one.
    pub fatal: Option<(i32, String)>,
    /// How the miner's message stream ended.
    pub terminal: Terminal,
    /// Phases whose causally-required reply never arrived, in script order.
    pub timed_out_phases: Vec<String>,
    /// Process exit code of the spawned miner binary.
    pub exit_code: i32,
    /// Everything the miner wrote to standard error during the session.
    pub stderr: String,
}

/// Job ids the full walk requires a `Result` for.
const REQUIRED_RESULTS: [&[u8]; 5] = [
    b"job-1",
    b"job-2",
    b"job-hash",
    b"job-sparse",
    b"job-seeded",
];

/// Job ids the full walk permits a `Result` for. `job-cancel` is cancelled
/// while live, which is a race the driver cannot win from outside the miner:
/// a fast sampler legitimately finishes before the `Cancel` lands. Its
/// *forbidden* outcomes are graded instead (see
/// [`live_cancel_conformant`](DriverReport::live_cancel_conformant)).
const PERMITTED_RESULTS: [&[u8]; 7] = [
    b"job-1",
    b"job-2",
    b"job-hash",
    b"job-sparse",
    b"job-seeded",
    b"job-warm-proof",
    b"job-cancel",
];

/// The feature string a solver lists in `Hello.features` when it uses
/// `IsingProblem.initial_spins`. Mirrors `quip_solver_core::INITIAL_SPINS_FEATURE`,
/// which this crate cannot depend on (see [`GIBBS_SWEEP_MULTIPLIER`]).
pub const INITIAL_SPINS_FEATURE: &str = "initial-spins";

/// Spins in the warm-start proof ring. A cold anneal leaves domain walls behind,
/// because 1-D coarsening grows domains only as the square root of the sweep
/// count, and the wall count grows with the ring. At 4096 spins a simulated
/// cold Metropolis anneal kept a median of 21 walls even at 4096 sweeps, and
/// never reached the ground state in 40 trials; at 1024 spins it did in 4 of
/// 40. A solver that advertises `initial-spins` must accept a job this size.
const WARM_PROOF_NODES: usize = 4096;

/// Start beta sent with the proof seed, milli-units. Cold enough that a seeded
/// anneal keeps the planted ground state.
const WARM_PROOF_START_BETA_MILLI: u32 = 10_000;

/// Reversal point sent with the proof seed, milli-units, for a reverse-anneal
/// backend. Shallow, so the anneal stays near the seed.
const WARM_PROOF_REVERSAL_S_MILLI: u32 = 900;

impl DriverReport {
    /// Job ids of the observed results, in arrival order. Convenience over
    /// [`results`](Self::results) for callers that only correlate job ids.
    #[must_use]
    pub fn result_job_ids(&self) -> Vec<Vec<u8>> {
        self.results.iter().map(|r| r.job_id.clone()).collect()
    }

    /// Full conformance verdict — every axis the harness grades.
    #[must_use]
    pub fn is_conformant(&self) -> bool {
        self.handshake_ok
            && self.ready_received
            && !self.job_request_credits.is_empty()
            && self.job_request_credits.iter().all(|&c| c > 0)
            && self.results_conformant()
            && self.has_reject(b"job-bad-h", RejectReason::Malformed)
            && self.has_reject(b"job-bad-j", RejectReason::Malformed)
            && self.has_reject(b"job-gate", RejectReason::UnsupportedKind)
            && self.has_reject(b"job-old", RejectReason::Expired)
            && self.warm_start_conformant()
            && self.lease_conformant()
            && self.cancel_acked
            && self.ping_acked
            && self.capabilities_conformant()
            && self.live_cancel_conformant()
            && self.credit_ledger_balanced()
            && self.terminal == Terminal::Closed
            && self.timed_out_phases.is_empty()
            && self.exit_code == 0
    }

    /// Grade salt leases only when the miner advertises `ISING_GENERATE`.
    #[must_use]
    pub fn lease_conformant(&self) -> bool {
        !advertises_lease(self.hello.as_ref())
            || self.lease.as_ref().is_some_and(|lease| {
                lease.results == LEASE_SALTS
                    && lease.results_verified == LEASE_SALTS
                    && lease
                        .lease_done
                        .is_some_and(|(salts, _)| salts == u64::from(LEASE_SALTS))
                    && lease.credit_refunded
            })
    }

    /// Every required `Result` present, no unexpected one, and each carrying
    /// solutions whose reported energy survives an independent re-score, plus
    /// a `SamplerMeta` echoing the configured sweep budget.
    #[must_use]
    pub fn results_conformant(&self) -> bool {
        REQUIRED_RESULTS
            .iter()
            .all(|id| self.results.iter().any(|r| r.job_id == *id))
            && self
                .results
                .iter()
                .all(|r| PERMITTED_RESULTS.iter().any(|id| r.job_id == *id))
            && self
                .results
                .iter()
                .all(|r| !r.solution_energies_milli.is_empty() && r.meta_present)
            && self.energies_rescore_clean()
            && self.sweeps_honoured()
    }

    /// Every reported `energy_milli` equals the driver's own score of the spins
    /// that came back with it.
    ///
    /// Both sides call `quip_protocol::scoring::energy_milli` on the same spins
    /// and the same problem, so the comparison is exact by construction: any
    /// difference is the solver reporting an energy its spins do not have.
    /// Results the driver has no problem on file for are not graded.
    #[must_use]
    pub fn energies_rescore_clean(&self) -> bool {
        self.results.iter().all(|r| {
            r.rescored_energies_milli.as_ref().is_none_or(|scored| {
                scored.len() == r.solution_energies_milli.len()
                    && scored
                        .iter()
                        .zip(&r.solution_energies_milli)
                        .all(|(mine, &theirs)| *mine == Some(theirs))
            })
        })
    }

    /// The `SamplerMeta.sweeps` a conformant solver reports: the configured
    /// budget, doubled when the `Hello` advertised the `gibbs` algorithm.
    ///
    /// Before a `Hello` arrives no `Result` can have arrived either, so the
    /// un-doubled budget is the right default.
    #[must_use]
    pub fn expected_meta_sweeps(&self) -> u32 {
        if self.hello.as_ref().is_some_and(|h| {
            h.capabilities
                .as_ref()
                .is_some_and(|c| c.algorithm == quip_proto::v1::Algorithm::Gibbs as i32)
        }) {
            CONFIGURED_SWEEPS * GIBBS_SWEEP_MULTIPLIER
        } else {
            CONFIGURED_SWEEPS
        }
    }

    /// Every `SamplerMeta` reports the sweep budget the script configured,
    /// adjusted for the advertised algorithm ([`Self::expected_meta_sweeps`]).
    #[must_use]
    pub fn sweeps_honoured(&self) -> bool {
        let expected = self.expected_meta_sweeps();
        self.results.iter().all(|r| r.meta_sweeps == expected)
    }

    /// The returned `Capabilities` agrees with the identity the miner
    /// advertised in its own `Hello`.
    ///
    /// Every field except features and stream width must match the handshake.
    #[must_use]
    pub fn capabilities_conformant(&self) -> bool {
        let (Some(c), Some(h)) = (self.capabilities_received.as_ref(), self.hello.as_ref()) else {
            return false;
        };
        let Some(h) = h.capabilities.as_ref() else {
            return false;
        };
        c.backend == h.backend
            && c.algorithm == h.algorithm
            && c.protocol_version == h.protocol_version
            && c.max_nodes == h.max_nodes
            && c.max_edges == h.max_edges
            && c.supported_kinds == h.supported_kinds
            && c.native_topology_hash == h.native_topology_hash
            && c.encodings == h.encodings
            && c.generators == h.generators
    }

    /// Cancellation is honoured: a job at or below the cancelled watermark
    /// yields neither a `Result` nor a `Reject`, the acknowledging `Status`
    /// carries the watermark, and the live-cancelled job is never *rejected*
    /// (cancelled work is reported through `Status`, never as a rejection).
    #[must_use]
    pub fn live_cancel_conformant(&self) -> bool {
        self.cancelled_watermark > 0
            && !self.results.iter().any(|r| r.job_id == b"job-stale")
            && !self.rejects.iter().any(|r| r.job_id == b"job-stale")
            && !self.rejects.iter().any(|r| r.job_id == b"job-cancel")
            && self.abandoned_watermark() >= self.cancelled_watermark
    }

    /// True when the `Hello` advertised [`INITIAL_SPINS_FEATURE`]. Only then
    /// is [`warm_start_conformant`](Self::warm_start_conformant) graded.
    #[must_use]
    pub fn advertises_initial_spins(&self) -> bool {
        self.hello.as_ref().is_some_and(|h| {
            h.capabilities
                .as_ref()
                .is_some_and(|c| c.features.iter().any(|f| f == INITIAL_SPINS_FEATURE))
        })
    }

    /// A solver that advertises [`INITIAL_SPINS_FEATURE`] rejects a malformed
    /// state as `Malformed`, and uses the states it is sent: seeded with the
    /// planted ground state of the proof ring, it returns that ground energy,
    /// which a cold anneal of the same budget does not reach.
    ///
    /// A solver that does not advertise the feature passes: it may ignore the
    /// field. The seeded `job-seeded`, which every solver must answer, already
    /// proves it still answers a seeded job.
    #[must_use]
    pub fn warm_start_conformant(&self) -> bool {
        if !self.advertises_initial_spins() {
            return true;
        }
        let ground = warm_proof_ground_milli();
        self.has_reject(b"job-bad-seed", RejectReason::Malformed)
            && self.results.iter().any(|r| {
                r.job_id == b"job-warm-proof"
                    && r.solution_energies_milli.iter().min() == Some(&ground)
            })
    }

    /// Highest `abandoned_generation` any `Status` reported.
    #[must_use]
    pub fn abandoned_watermark(&self) -> u64 {
        self.statuses
            .iter()
            .map(|s| s.abandoned_generation)
            .max()
            .unwrap_or(0)
    }

    /// Credits returned after the initial grant, summed over the whole walk.
    #[must_use]
    pub fn credits_refunded(&self) -> u64 {
        self.job_request_credits
            .iter()
            .skip(1)
            .map(|&c| u64::from(c))
            .sum()
    }

    /// The ledger invariant: every dispatched job returns exactly one credit,
    /// whether it ended in a `Result`, a `Reject`, or an abandonment. A solver
    /// that forgets one leaks a slot from the coordinator's
    /// consume-on-dispatch pool and slowly starves its own pipeline — a defect
    /// that is invisible in any single-job test.
    #[must_use]
    pub fn credit_ledger_balanced(&self) -> bool {
        self.jobs_dispatched > 0 && self.credits_refunded() == self.jobs_dispatched as u64
    }

    /// One line per graded axis, each marked pass or fail.
    ///
    /// The `Debug` rendering carries every observation, which is what a
    /// failing assertion needs but not what a person reading a CLI run needs:
    /// the axis that failed is buried in several hundred lines of spin bytes.
    #[must_use]
    pub fn summary(&self) -> String {
        let mark = |ok: bool| if ok { "pass" } else { "FAIL" };
        let mut out = String::new();
        for (axis, ok) in [
            ("handshake", self.handshake_ok),
            ("ready", self.ready_received),
            (
                "credits granted",
                !self.job_request_credits.is_empty()
                    && self.job_request_credits.iter().all(|&c| c > 0),
            ),
            ("results", self.results_conformant()),
            ("energies re-score", self.energies_rescore_clean()),
            ("configured sweeps", self.sweeps_honoured()),
            ("capabilities", self.capabilities_conformant()),
            (
                if self.advertises_initial_spins() {
                    "warm start used"
                } else {
                    "warm start (not advertised, not graded)"
                },
                self.warm_start_conformant(),
            ),
            ("ping ack", self.ping_acked),
            ("cancel ack", self.cancel_acked),
            ("cancellation honoured", self.live_cancel_conformant()),
            ("credit ledger", self.credit_ledger_balanced()),
            (
                if advertises_lease(self.hello.as_ref()) {
                    "salt lease"
                } else {
                    "salt lease (not advertised, skipped)"
                },
                self.lease_conformant(),
            ),
            ("clean stream end", self.terminal == Terminal::Closed),
            ("no phase timed out", self.timed_out_phases.is_empty()),
            ("exit code 0", self.exit_code == 0),
        ] {
            let _ = writeln!(out, "  [{}] {axis}", mark(ok));
        }
        let _ = writeln!(out, "  stream ended: {:?}", self.terminal);
        if let Some(lease) = &self.lease {
            let _ = writeln!(out, "  lease: {lease:?}");
        }
        if !self.timed_out_phases.is_empty() {
            let _ = writeln!(
                out,
                "  phases with no reply: {}",
                self.timed_out_phases.join(", ")
            );
        }
        let _ = writeln!(
            out,
            "  credits: granted {}, refunded {}, jobs dispatched {}",
            self.job_request_credits.first().copied().unwrap_or(0),
            self.credits_refunded(),
            self.jobs_dispatched,
        );
        out
    }

    /// True when a `Reject` with the given `job_id` and `reason` was observed.
    #[must_use]
    pub fn has_reject(&self, job_id: &[u8], reason: RejectReason) -> bool {
        self.rejects
            .iter()
            .any(|r| r.job_id == job_id && r.reason == reason as i32)
    }
}

/// The problem the driver sent with a job, in the miner's dense index space —
/// what re-scoring a returned solution needs.
#[derive(Debug, Clone)]
struct ScoreSpec {
    h: Vec<f64>,
    j: Vec<f64>,
    edges: Vec<(usize, usize)>,
}

/// Everything the scripted session observes, excluding the child's exit code.
#[derive(Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors DriverReport's independent observation axes"
)]
struct SessionOutcome {
    lease: Option<LeaseOutcome>,
    lease_salts: HashSet<Vec<u8>>,
    lease_refund_messages: u32,
    handshake_ok: bool,
    hello: Option<Hello>,
    ready_received: bool,
    job_request_credits: Vec<u32>,
    jobs_dispatched: usize,
    results: Vec<ObservedResult>,
    rejects: Vec<ObservedReject>,
    statuses: Vec<MinerStatus>,
    cancel_acked: bool,
    ping_acked: bool,
    capabilities_received: Option<Capabilities>,
    cancelled_watermark: u64,
    fatal: Option<(i32, String)>,
    terminal: Terminal,
    timed_out_phases: Vec<String>,
    /// Problems the script dispatched, keyed by job id, for re-scoring.
    expected: HashMap<Vec<u8>, ScoreSpec>,
}

impl SessionOutcome {
    /// Credits returned after the initial grant. Mirrors
    /// [`DriverReport::credits_refunded`]; the barriers need it mid-walk.
    fn credits_refunded(&self) -> u64 {
        self.job_request_credits
            .iter()
            .skip(1)
            .map(|&c| u64::from(c))
            .sum()
    }

    /// True once the miner's stream has ended, however it ended. No further
    /// barrier can be satisfied after this, so the script stops sending.
    fn stream_ended(&self) -> bool {
        self.terminal != Terminal::Open
    }
}

/// Which scripted session `MockCoordinator` plays against a connecting miner.
#[derive(Debug, Clone, Copy)]
enum ScriptKind {
    /// The full conformance walk: handshake, capabilities, ping, jobs,
    /// rejects, cancellation, shutdown.
    Full,
    /// Send a `Welcome` with an unsupported `protocol_version` and observe how
    /// the miner rejects it (expected: `Fatal` + exit `ConfigInvalid`).
    BadWelcome,
    /// Handshake, one valid job, then wait. No `Shutdown`. A miner that ends
    /// the session after a device fault closes on its own. A miner that keeps
    /// reading hangs until the harness timeout.
    OneJobThenWait,
    /// Read `Hello`, send `Welcome`, then drop the outbound stream without a
    /// `Shutdown`. The coordinator has vanished mid-session, which is not a
    /// clean end (expected: exit `InternalFatal`).
    CloseAfterWelcome,
    /// Read `Hello`, then drop the outbound stream without ever sending
    /// `Welcome`. From the miner's side this is indistinguishable from a
    /// coordinator that refused the session token (expected: exit
    /// `TokenRejected`).
    CloseBeforeWelcome,
}

struct MockCoordinator {
    outcome_tx: Mutex<Option<oneshot::Sender<SessionOutcome>>>,
    script: ScriptKind,
}

#[tonic::async_trait]
impl MinerService for MockCoordinator {
    type SessionStream = ReceiverStream<Result<CoordMsg, Status>>;

    async fn session(
        &self,
        request: Request<Streaming<MinerMsg>>,
    ) -> Result<Response<Self::SessionStream>, Status> {
        let mut inbound = request.into_inner();
        let (tx, rx) = mpsc::channel::<Result<CoordMsg, Status>>(64);
        let outcome_tx = self.outcome_tx.lock().await.take();
        let script = self.script;

        let _handle = tokio::spawn(async move {
            let outcome = match script {
                ScriptKind::Full => run_script(&mut inbound, &tx).await,
                ScriptKind::BadWelcome => run_script_bad_welcome(&mut inbound, &tx).await,
                ScriptKind::OneJobThenWait => run_script_one_job(&mut inbound, &tx).await,
                // These two take the sender by value: dropping it is the whole
                // point of the script, and a borrow cannot close the stream.
                ScriptKind::CloseAfterWelcome => {
                    run_script_close(&mut inbound, tx, /* send_welcome */ true).await
                }
                ScriptKind::CloseBeforeWelcome => {
                    run_script_close(&mut inbound, tx, /* send_welcome */ false).await
                }
            };
            if let Some(otx) = outcome_tx {
                let _ = otx.send(outcome);
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

fn now_unix_ms() -> u64 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "unix epoch millis fit in u64 for practical wall-clock times"
    )]
    {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

/// Build a `CoordMsg` from a oneof payload. Callers wrap it in `Ok` for the
/// channel, whose item type is `Result<_, Status>` per tonic's `SessionStream`;
/// this mock never emits a stream-level error.
fn coord(msg: coord_msg::Msg) -> CoordMsg {
    CoordMsg { msg: Some(msg) }
}

/// Hash of the dense `Topology` the script sends first; `job-hash` references it.
const TOPOLOGY_HASH: [u8; 32] = [0x11; 32];

/// Hash of the sparse `Topology` the script sends second; `job-sparse`
/// references it.
const SPARSE_TOPOLOGY_HASH: [u8; 32] = [0x22; 32];

/// Native node ids of the sparse topology. Deliberately neither contiguous nor
/// zero-based past the first entry: a solver that treats native ids as dense
/// indices reads out of bounds or scores the wrong spins, and only a sparse
/// topology catches it.
const SPARSE_NODES: [u32; 3] = [0, 12, 2400];

/// A minimal, well-formed two-spin Ising problem with an edge (0,1).
fn valid_ising() -> IsingProblem {
    IsingProblem {
        graph: Some(ising_problem::Graph::Edges(EdgeList {
            u: vec![0],
            v: vec![1],
        })),
        encoding: quip_proto::v1::CoefficientEncoding::I32 as i32,
        scale: 1000,
        h: encode_i32_le(&[1000, -1000]),
        j: encode_i32_le(&[500]),
        num_reads: 1,
        num_sweeps: 0,
        anneal_time_us: 0,
        ..Default::default()
    }
}

/// The dense-space problem `valid_ising` describes, for re-scoring.
fn valid_spec() -> ScoreSpec {
    ScoreSpec {
        h: vec![1.0, -1.0],
        j: vec![0.5],
        edges: vec![(0, 1)],
    }
}

/// Same problem as `valid_ising`, but referenced by `topology_hash` — the miner
/// must resolve it against the cached `Topology` (nodes [0,1], edge (0,1)).
fn hash_ising() -> IsingProblem {
    IsingProblem {
        graph: Some(ising_problem::Graph::TopologyHash(TOPOLOGY_HASH.to_vec())),
        encoding: quip_proto::v1::CoefficientEncoding::I32 as i32,
        scale: 1000,
        h: encode_i32_le(&[1000, -1000]),
        j: encode_i32_le(&[500]),
        num_reads: 1,
        num_sweeps: 0,
        anneal_time_us: 0,
        ..Default::default()
    }
}

/// A three-spin problem referenced by the *sparse* topology hash. The miner
/// must map native ids [0, 12, 2400] to dense positions [0, 1, 2] and score in
/// that space.
fn sparse_ising() -> IsingProblem {
    IsingProblem {
        graph: Some(ising_problem::Graph::TopologyHash(
            SPARSE_TOPOLOGY_HASH.to_vec(),
        )),
        encoding: quip_proto::v1::CoefficientEncoding::I32 as i32,
        scale: 1000,
        h: encode_i32_le(&[1000, -1000, 250]),
        j: encode_i32_le(&[500, -750]),
        num_reads: 1,
        num_sweeps: 0,
        anneal_time_us: 0,
        ..Default::default()
    }
}

/// The dense-space problem `sparse_ising` resolves to: native (0,12) and
/// (12,2400) become dense (0,1) and (1,2).
fn sparse_spec() -> ScoreSpec {
    ScoreSpec {
        h: vec![1.0, -1.0, 0.25],
        j: vec![0.5, -0.75],
        edges: vec![(0, 1), (1, 2)],
    }
}

/// `valid_ising` seeded with its ground state `[-1, +1]`. Every solver must
/// answer it: one that ignores the seed returns a cold result, which is valid.
fn seeded_ising() -> IsingProblem {
    IsingProblem {
        initial_spins: vec![encode_spins_packed(&[-1, 1])],
        ..valid_ising()
    }
}

/// The planted spin of node `i` in the proof ring. Deterministic and
/// irregular, so no simple cold start (all `+1`, alternating) matches it.
fn warm_proof_spin(i: usize) -> i8 {
    if (i.wrapping_mul(2_654_435_761) >> 7) & 1 == 1 {
        1
    } else {
        -1
    }
}

/// Coupling of ring edge `(i, i + 1)`: `-s_i * s_(i+1)` in milli-units, so the
/// planted state satisfies every edge. With `h = 0` that state and its flip are
/// the only ground states, at energy `-WARM_PROOF_NODES` units.
fn warm_proof_j_milli() -> Vec<i32> {
    (0..WARM_PROOF_NODES)
        .map(|i| {
            let (a, b) = (
                warm_proof_spin(i),
                warm_proof_spin((i + 1) % WARM_PROOF_NODES),
            );
            -1000 * i32::from(a) * i32::from(b)
        })
        .collect()
}

fn warm_proof_edges() -> Vec<(usize, usize)> {
    (0..WARM_PROOF_NODES)
        .map(|i| (i, (i + 1) % WARM_PROOF_NODES))
        .collect()
}

/// The proof ring, seeded with its planted ground state at a cold start.
fn warm_proof_ising() -> IsingProblem {
    let edges = warm_proof_edges();
    let planted: Vec<i8> = (0..WARM_PROOF_NODES).map(warm_proof_spin).collect();
    IsingProblem {
        graph: Some(ising_problem::Graph::Edges(EdgeList {
            u: edges.iter().map(|&(u, _)| as_node_id(u)).collect(),
            v: edges.iter().map(|&(_, v)| as_node_id(v)).collect(),
        })),
        encoding: quip_proto::v1::CoefficientEncoding::I32 as i32,
        scale: 1000,
        h: encode_i32_le(&vec![0; WARM_PROOF_NODES]),
        j: encode_i32_le(&warm_proof_j_milli()),
        num_reads: 1,
        initial_spins: vec![encode_spins_packed(&planted)],
        start_beta_milli: WARM_PROOF_START_BETA_MILLI,
        reversal_s_milli: WARM_PROOF_REVERSAL_S_MILLI,
        ..Default::default()
    }
}

fn warm_proof_spec() -> ScoreSpec {
    ScoreSpec {
        h: vec![0.0; WARM_PROOF_NODES],
        j: warm_proof_j_milli()
            .into_iter()
            .map(|m| f64::from(m) / 1000.0)
            .collect(),
        edges: warm_proof_edges(),
    }
}

/// Energy of the proof ring's planted state, scored the consensus way.
fn warm_proof_ground_milli() -> i64 {
    let spec = warm_proof_spec();
    let planted: Vec<i8> = (0..WARM_PROOF_NODES).map(warm_proof_spin).collect();
    energy_milli(&planted, &spec.h, &spec.j, &spec.edges)
}

/// A dense index as a wire node id. The proof ring is far below `u32::MAX`.
fn as_node_id(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// The dense topology `job-hash` resolves against.
fn dense_topology() -> Topology {
    Topology {
        allowed_j_milli: vec![],
        hash: TOPOLOGY_HASH.to_vec(),
        nodes: vec![0, 1],
        edges: Some(EdgeList {
            u: vec![0],
            v: vec![1],
        }),
        allowed_h_milli: vec![-1000, 0, 1000],
    }
}

/// The sparse topology `job-sparse` resolves against.
fn sparse_topology() -> Topology {
    Topology {
        allowed_j_milli: vec![],
        hash: SPARSE_TOPOLOGY_HASH.to_vec(),
        nodes: SPARSE_NODES.to_vec(),
        edges: Some(EdgeList {
            u: vec![SPARSE_NODES[0], SPARSE_NODES[1]],
            v: vec![SPARSE_NODES[1], SPARSE_NODES[2]],
        }),
        allowed_h_milli: vec![-1000, 0, 1000],
    }
}

/// The `Configure` the script sends. `backend_toml` is populated: an empty
/// string never proves the field is read at all.
fn configure() -> Configure {
    Configure {
        queue_depth: 3,
        idle_timeout_s: 300,
        heartbeat_s: 15,
        reconnect_window_s: 60,
        backend_toml: format!("num_sweeps = {CONFIGURED_SWEEPS}\n"),
    }
}

fn job_at(job_id: &[u8], deadline_ms: u64, generation: u64, ising: IsingProblem) -> Job {
    Job {
        generator: None,
        job_id: job_id.to_vec(),
        kind: JobKind::IsingSample as i32,
        generation,
        deadline_ms,
        ising: Some(ising),
        provenance: None,
    }
}

fn job(job_id: &[u8], deadline_ms: u64, ising: IsingProblem) -> Job {
    job_at(job_id, deadline_ms, LIVE_GENERATION, ising)
}

fn job_kind(job_id: &[u8], deadline_ms: u64, kind: JobKind, ising: IsingProblem) -> Job {
    Job {
        generator: None,
        job_id: job_id.to_vec(),
        kind: kind as i32,
        generation: LIVE_GENERATION,
        deadline_ms,
        ising: Some(ising),
        provenance: None,
    }
}

/// Read the first inbound message, which must be a valid `Hello`. Returns
/// `false` (without touching `outcome.handshake_ok`) if the stream ended or
/// the first message was something else.
async fn read_hello(inbound: &mut Streaming<MinerMsg>, outcome: &mut SessionOutcome) -> bool {
    match tokio::time::timeout(PHASE_TIMEOUT, inbound.message()).await {
        Ok(Ok(Some(MinerMsg {
            msg: Some(miner_msg::Msg::Hello(h)),
        }))) => {
            outcome.handshake_ok = h.session_token == "test-token"
                && h.capabilities
                    .as_ref()
                    .is_some_and(|c| c.protocol_version == 2);
            outcome.hello = Some(h);
            true
        }
        Ok(Ok(None)) => {
            outcome.terminal = Terminal::Closed;
            false
        }
        Ok(Err(s)) => {
            outcome.terminal = Terminal::Transport(format!("{}: {}", s.code(), s.message()));
            false
        }
        Ok(Ok(Some(MinerMsg { msg: None }))) => {
            outcome.terminal = Terminal::EmptyMessage;
            false
        }
        Ok(Ok(Some(_))) => false,
        Err(_) => {
            outcome.timed_out_phases.push("hello".to_owned());
            false
        }
    }
}

/// Fold one inbound message into the running outcome.
fn fold(outcome: &mut SessionOutcome, m: miner_msg::Msg) {
    match m {
        miner_msg::Msg::Ready(_) => outcome.ready_received = true,
        miner_msg::Msg::JobRequest(jr) => {
            outcome.job_request_credits.push(jr.credits);
            if let Some(lease) = &mut outcome.lease {
                if lease.lease_done.is_some() {
                    outcome.lease_refund_messages += 1;
                    lease.credit_refunded = jr.credits == 1 && outcome.lease_refund_messages == 1;
                }
            }
        }
        miner_msg::Msg::Result(r) if r.job_id == LEASE_JOB_ID && outcome.lease.is_some() => {
            if let Some(lease) = &mut outcome.lease {
                lease.results += 1;
                if let Ok(topology) = TopologyView::from_proto(&lease_topology()) {
                    let verified = verify_lease_result(
                        &lease_generator(),
                        &topology,
                        &Target::from_proto(&lease_target()),
                        &r,
                    )
                    .is_ok();
                    if verified && lease.lease_done.is_none() && outcome.lease_salts.insert(r.salt)
                    {
                        lease.results_verified += 1;
                    }
                }
            }
        }
        miner_msg::Msg::Result(r) => {
            let spec = outcome.expected.get(&r.job_id);
            let rescored = spec.map(|s| {
                r.solutions
                    .iter()
                    .map(|sol| {
                        // A solution whose spins do not decode, or that is the
                        // wrong width for the problem, cannot be re-scored.
                        // Recording `None` keeps it distinguishable from a
                        // genuine score of zero.
                        decode_spins_packed(&sol.spins, s.h.len())
                            .ok()
                            .filter(|v| v.len() == s.h.len())
                            .map(|v| energy_milli(&v, &s.h, &s.j, &s.edges))
                    })
                    .collect()
            });
            outcome.results.push(ObservedResult {
                job_id: r.job_id,
                solution_energies_milli: r.solutions.iter().map(|s| s.energy_milli).collect(),
                rescored_energies_milli: rescored,
                meta_present: r.meta.is_some(),
                meta_sweeps: r.meta.map_or(0, |m| m.sweeps),
            });
        }
        miner_msg::Msg::Reject(r) => outcome.rejects.push(ObservedReject {
            job_id: r.job_id,
            reason: r.reason,
        }),
        // Attribution to Ping or Cancel is the caller's job: the phase that was
        // waiting when this arrived is the one that caused it.
        miner_msg::Msg::Status(s) => outcome.statuses.push(s),
        miner_msg::Msg::Capabilities(c) => outcome.capabilities_received = Some(c),
        miner_msg::Msg::Fatal(f) => {
            outcome.fatal = Some((f.exit_code.cast_signed(), f.reason));
        }
        miner_msg::Msg::LeaseDone(done) => {
            if done.job_id == LEASE_JOB_ID {
                if let Some(lease) = &mut outcome.lease {
                    // A second completion cannot replace a malformed first one.
                    lease.lease_done = Some(if lease.lease_done.is_some() {
                        (0, done.best_energy_milli)
                    } else {
                        (done.salts_done, done.best_energy_milli)
                    });
                }
            }
        }
        miner_msg::Msg::Hello(_) => {}
    }
}

fn advertises_lease(hello: Option<&Hello>) -> bool {
    hello
        .and_then(|h| h.capabilities.as_ref())
        .is_some_and(|c| c.supported_kinds.contains(&(JobKind::IsingGenerate as i32)))
}

fn lease_topology() -> Topology {
    // An eight-node ring with four opposite-node chords: connected, degree three.
    Topology {
        hash: vec![0x33; 32],
        nodes: (0..8).collect(),
        edges: Some(EdgeList {
            u: vec![0, 1, 2, 3, 4, 5, 6, 0, 0, 1, 2, 3],
            v: vec![1, 2, 3, 4, 5, 6, 7, 7, 4, 5, 6, 7],
        }),
        allowed_h_milli: vec![-1000, 0, 1000],
        allowed_j_milli: vec![-1000, 1000],
    }
}

fn lease_generator() -> IsingProblemGenerator {
    IsingProblemGenerator {
        algorithm: quip_proto::v1::GeneratorAlgorithm::Blake3Chacha8V1 as i32,
        topology_hash: lease_topology().hash,
        last_proof_block_hash: vec![0x11; 32],
        miner_account: vec![0x22; 32],
        base_salt: vec![0x44; 32],
        salt_start: 10,
        salt_count: u64::from(LEASE_SALTS),
    }
}

fn lease_target() -> SetTarget {
    SetTarget {
        max_energy_milli: i64::MAX,
        min_solutions: 1,
        min_diversity_milli: 0,
        max_proof_solutions: 32,
        num_reads: 1,
        num_sweeps: CONFIGURED_SWEEPS,
        ..Default::default()
    }
}

async fn run_lease(
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    inbound: &mut Streaming<MinerMsg>,
    outcome: &mut SessionOutcome,
) {
    outcome.lease = Some(LeaseOutcome::default());
    let _ = send(tx, coord_msg::Msg::Topology(lease_topology())).await;
    let _ = send(tx, coord_msg::Msg::SetTarget(lease_target())).await;
    let _ = dispatch(
        tx,
        outcome,
        Job {
            job_id: LEASE_JOB_ID.to_vec(),
            kind: JobKind::IsingGenerate as i32,
            generation: CANCEL_GENERATION + 1,
            generator: Some(lease_generator()),
            // Zero means no deadline. The driver's timeout bounds this scenario.
            deadline_ms: 0,
            ..Default::default()
        },
        None,
    )
    .await;
    // Bound the whole scenario, including its refund, even if messages keep arriving.
    if tokio::time::timeout(
        PHASE_TIMEOUT,
        read_until(inbound, outcome, "salt-lease", |o| {
            o.lease
                .as_ref()
                .is_some_and(|l| l.lease_done.is_some() && l.credit_refunded)
        }),
    )
    .await
    .is_err()
    {
        outcome.timed_out_phases.push("salt-lease".to_owned());
    }
}

/// Read and fold inbound messages until `satisfied` holds, the stream ends, or
/// the phase budget runs out. Returns whether `satisfied` was reached.
///
/// This is the barrier that gives the walk its causality: nothing is sent for
/// the next phase until the reply this phase requires has actually arrived.
async fn read_until<F>(
    inbound: &mut Streaming<MinerMsg>,
    outcome: &mut SessionOutcome,
    phase: &str,
    mut satisfied: F,
) -> bool
where
    F: FnMut(&SessionOutcome) -> bool,
{
    loop {
        if satisfied(outcome) {
            return true;
        }
        if outcome.stream_ended() {
            return false;
        }
        match tokio::time::timeout(PHASE_TIMEOUT, inbound.message()).await {
            Ok(Ok(Some(MinerMsg { msg: Some(m) }))) => fold(outcome, m),
            Ok(Ok(Some(MinerMsg { msg: None }))) => outcome.terminal = Terminal::EmptyMessage,
            Ok(Ok(None)) => outcome.terminal = Terminal::Closed,
            Ok(Err(s)) => {
                outcome.terminal = Terminal::Transport(format!("{}: {}", s.code(), s.message()));
            }
            Err(_) => {
                outcome.timed_out_phases.push(phase.to_owned());
                return false;
            }
        }
    }
}

/// Read to the end of the miner's stream, folding everything that arrives.
/// Used after `Shutdown`, where the terminal condition is itself the assertion.
async fn drain_to_end(inbound: &mut Streaming<MinerMsg>, outcome: &mut SessionOutcome) {
    // Never satisfied: only the stream ending or the budget expiring stops it.
    let _ = read_until(inbound, outcome, "drain", |_| false).await;
}

/// Send one `CoordMsg`. Returns `false` when the miner is already gone, which
/// tells the script to stop rather than push into a closed channel.
async fn send(tx: &mpsc::Sender<Result<CoordMsg, Status>>, msg: coord_msg::Msg) -> bool {
    tx.send(Ok(coord(msg))).await.is_ok()
}

/// Dispatch a job, recording both the credit obligation it creates and the
/// problem needed to re-score whatever comes back.
async fn dispatch(
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    j: Job,
    spec: Option<ScoreSpec>,
) -> bool {
    if let Some(s) = spec {
        let _ = outcome.expected.insert(j.job_id.clone(), s);
    }
    outcome.jobs_dispatched += 1;
    send(tx, coord_msg::Msg::Job(j)).await
}

/// Drive the full scripted `CoordMsg` sequence and collect the miner's replies.
///
/// Each numbered step ends at a barrier. Steps are ordered so that every
/// assertion downstream of them is deterministic: all ordinary jobs settle
/// before anything is cancelled, so the cancellation phases cannot swallow a
/// result the walk still needs.
#[expect(
    clippy::too_many_lines,
    reason = "single scripted session walk kept linear for readability"
)]
async fn run_script(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
) -> SessionOutcome {
    let mut outcome = SessionOutcome::default();

    // 1. First inbound message must be a valid Hello.
    if !read_hello(inbound, &mut outcome).await {
        return outcome;
    }

    // 2. Handshake response, configuration, and the dense topology. The
    //    barrier is Ready plus the initial credit grant: no job may be sent
    //    before the miner has said it can take one.
    if !send(
        tx,
        coord_msg::Msg::Welcome(Welcome {
            protocol_version: 2,
        }),
    )
    .await
    {
        return outcome;
    }
    let _ = send(tx, coord_msg::Msg::Configure(configure())).await;
    let _ = send(tx, coord_msg::Msg::Topology(dense_topology())).await;
    let _ = read_until(inbound, &mut outcome, "configure->ready", |o| {
        o.ready_received && !o.job_request_credits.is_empty()
    })
    .await;
    if outcome.stream_ended() {
        return outcome;
    }

    // 3. GetCapabilities -> Capabilities. Sent on its own so the reply cannot
    //    be confused with an unprompted mid-session capability update.
    let _ = send(tx, coord_msg::Msg::GetCapabilities(GetCapabilities {})).await;
    let _ = read_until(inbound, &mut outcome, "get-capabilities", |o| {
        o.capabilities_received.is_some()
    })
    .await;

    // 4. Ping -> Status. Alone on the wire, so the Status it produces is
    //    attributable to the Ping and to nothing else.
    let before = outcome.statuses.len();
    let _ = send(tx, coord_msg::Msg::Ping(Ping {})).await;
    outcome.ping_acked = read_until(inbound, &mut outcome, "ping->status", |o| {
        o.statuses.len() > before
    })
    .await;

    // 5. Cancel a generation below every job this walk sends, so the ack can
    //    be observed without abandoning work the later phases still need. Its
    //    Status is attributable for the same reason the Ping's was.
    let before = outcome.statuses.len();
    let _ = send(tx, coord_msg::Msg::Cancel(Cancel { max_generation: 1 })).await;
    outcome.cancel_acked = read_until(inbound, &mut outcome, "cancel->status", |o| {
        o.statuses.len() > before
    })
    .await;
    outcome.cancelled_watermark = 1;

    // 6. Two valid jobs with far-future deadlines, plus the topology-hash job:
    //    same problem by hash, which the miner resolves against the cached
    //    dense Topology (regression guard for the session cache + resolution).
    let future = now_unix_ms() + 3_600_000;
    let _ = dispatch(
        tx,
        &mut outcome,
        job(b"job-1", future, valid_ising()),
        Some(valid_spec()),
    )
    .await;
    let _ = dispatch(
        tx,
        &mut outcome,
        job(b"job-2", future, valid_ising()),
        Some(valid_spec()),
    )
    .await;
    let _ = dispatch(
        tx,
        &mut outcome,
        job(b"job-hash", future, hash_ising()),
        Some(valid_spec()),
    )
    .await;
    // A seeded job goes to every solver. One built before `initial_spins`
    // existed skips the unknown field and answers it cold.
    let _ = dispatch(
        tx,
        &mut outcome,
        job(b"job-seeded", future, seeded_ising()),
        Some(valid_spec()),
    )
    .await;
    // Only a solver that says it uses the states is held to using them.
    let advertises_initial_spins = outcome.hello.as_ref().is_some_and(|h| {
        h.capabilities
            .as_ref()
            .is_some_and(|c| c.features.iter().any(|f| f == INITIAL_SPINS_FEATURE))
    });
    if advertises_initial_spins {
        let _ = dispatch(
            tx,
            &mut outcome,
            job(b"job-warm-proof", future, warm_proof_ising()),
            Some(warm_proof_spec()),
        )
        .await;
        let mut bad_seed = valid_ising();
        bad_seed.initial_spins = vec![vec![0x00, 0x00]]; // 2 bytes for 2 spins
        let _ = dispatch(
            tx,
            &mut outcome,
            job(b"job-bad-seed", future, bad_seed),
            None,
        )
        .await;
    }
    let expect_credits = outcome.jobs_dispatched as u64;
    let _ = read_until(inbound, &mut outcome, "valid-jobs", |o| {
        o.credits_refunded() >= expect_credits
    })
    .await;

    // 7. Sparse topology, then a hash job against it. Sent after the dense
    //    jobs settled, because the session caches one topology at a time and
    //    this replaces it.
    let _ = send(tx, coord_msg::Msg::Topology(sparse_topology())).await;
    let _ = dispatch(
        tx,
        &mut outcome,
        job(b"job-sparse", future, sparse_ising()),
        Some(sparse_spec()),
    )
    .await;
    let expect_credits = outcome.jobs_dispatched as u64;
    let _ = read_until(inbound, &mut outcome, "sparse-topology-job", |o| {
        o.credits_refunded() >= expect_credits
    })
    .await;

    // 8. The four rejection paths: h length not a multiple of 4, j length not
    //    a multiple of 4, an unsupported kind, and a deadline in the past.
    let mut malformed_h = valid_ising();
    malformed_h.h = vec![0x01, 0x02, 0x03]; // len 3, not a multiple of 4
    let _ = dispatch(
        tx,
        &mut outcome,
        job(b"job-bad-h", future, malformed_h),
        None,
    )
    .await;

    let mut malformed_j = valid_ising();
    malformed_j.j = vec![0x01, 0x02, 0x03];
    let _ = dispatch(
        tx,
        &mut outcome,
        job(b"job-bad-j", future, malformed_j),
        None,
    )
    .await;

    let _ = dispatch(
        tx,
        &mut outcome,
        job_kind(b"job-gate", future, JobKind::GateCircuit, valid_ising()),
        None,
    )
    .await;

    let past = now_unix_ms().saturating_sub(60_000);
    let _ = dispatch(tx, &mut outcome, job(b"job-old", past, valid_ising()), None).await;
    let expect_credits = outcome.jobs_dispatched as u64;
    let _ = read_until(inbound, &mut outcome, "reject-jobs", |o| {
        o.credits_refunded() >= expect_credits
    })
    .await;

    // 9. Live-generation cancel: dispatch a job and immediately cancel its own
    //    generation. Whether the sampler finishes first is a race no external
    //    driver can win, so this grades the outcomes that are wrong either
    //    way — a Reject for cancelled work, or a missing credit — rather than
    //    demanding the cancel land first.
    let _ = dispatch(
        tx,
        &mut outcome,
        job_at(b"job-cancel", future, CANCEL_GENERATION, valid_ising()),
        Some(valid_spec()),
    )
    .await;
    let before = outcome.statuses.len();
    let _ = send(
        tx,
        coord_msg::Msg::Cancel(Cancel {
            max_generation: CANCEL_GENERATION,
        }),
    )
    .await;
    outcome.cancelled_watermark = CANCEL_GENERATION;
    let expect_credits = outcome.jobs_dispatched as u64;
    let _ = read_until(inbound, &mut outcome, "live-cancel", |o| {
        o.credits_refunded() >= expect_credits && o.statuses.len() > before
    })
    .await;

    // 10. Stale job: dispatched after the Cancel, at a generation the
    //     watermark already covers. This one is not a race — the miner set the
    //     watermark before the job existed — so it must produce no Result and
    //     no Reject, only the credit refund.
    let _ = dispatch(
        tx,
        &mut outcome,
        job_at(b"job-stale", future, CANCEL_GENERATION, valid_ising()),
        Some(valid_spec()),
    )
    .await;
    let expect_credits = outcome.jobs_dispatched as u64;
    let _ = read_until(inbound, &mut outcome, "stale-after-cancel", |o| {
        o.credits_refunded() >= expect_credits
    })
    .await;

    // 11. Generation runs above the cancellation watermark from the plain jobs.
    if advertises_lease(outcome.hello.as_ref()) {
        run_lease(tx, inbound, &mut outcome).await;
    }

    // 12. Shutdown -> miner flushes and exits 0, closing its send stream.
    let _ = send(tx, coord_msg::Msg::Shutdown(Shutdown { grace_ms: 1000 })).await;
    drain_to_end(inbound, &mut outcome).await;

    outcome
}

/// Handshake, then send a `Welcome` advertising an unsupported protocol
/// version. A conformant miner rejects this cleanly: it emits `Fatal` and
/// disconnects instead of proceeding to `Configure`.
async fn run_script_bad_welcome(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
) -> SessionOutcome {
    let mut outcome = SessionOutcome::default();

    if !read_hello(inbound, &mut outcome).await {
        return outcome;
    }

    if !send(
        tx,
        coord_msg::Msg::Welcome(Welcome {
            protocol_version: 1,
        }),
    )
    .await
    {
        return outcome;
    }

    drain_to_end(inbound, &mut outcome).await;
    outcome
}

/// Handshake, one valid job, then wait for the miner to close. Unlike
/// [`run_script`], this does not send `Shutdown`, so a miner that stays in
/// its read loop after a device fault hangs until the harness timeout.
async fn run_script_one_job(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
) -> SessionOutcome {
    let mut outcome = SessionOutcome::default();

    if !read_hello(inbound, &mut outcome).await {
        return outcome;
    }

    if !send(
        tx,
        coord_msg::Msg::Welcome(Welcome {
            protocol_version: 2,
        }),
    )
    .await
    {
        return outcome;
    }
    let _ = send(tx, coord_msg::Msg::Configure(configure())).await;
    let _ = send(tx, coord_msg::Msg::Topology(dense_topology())).await;
    let _ = read_until(inbound, &mut outcome, "configure->ready", |o| {
        o.ready_received && !o.job_request_credits.is_empty()
    })
    .await;

    let future = now_unix_ms() + 3_600_000;
    let _ = dispatch(
        tx,
        &mut outcome,
        job(b"job-1", future, valid_ising()),
        Some(valid_spec()),
    )
    .await;

    drain_to_end(inbound, &mut outcome).await;
    outcome
}

/// Read `Hello`, optionally answer with `Welcome`, then drop the outbound
/// stream. No `Shutdown` is ever sent: the coordinator simply disappears.
///
/// `tx` is taken by value because dropping it is the assertion. Dropping the
/// only sender ends the `ReceiverStream` tonic is serving, which closes the
/// coordinator-to-miner direction of the bidi stream.
///
/// What happens to `inbound` after that drop is deliberately not graded. Once
/// the response stream ends, tonic completes the RPC, and whether the request
/// body yields another item, an error, or nothing is a transport-timing
/// detail, not solver behaviour. The graded observable is the miner's process
/// exit code, which [`drive_miner_with_script`] captures independently of this
/// stream. The drain is here to keep reading until the miner is gone, so a
/// `Fatal` it manages to emit before the close lands is still recorded.
async fn run_script_close(
    inbound: &mut Streaming<MinerMsg>,
    tx: mpsc::Sender<Result<CoordMsg, Status>>,
    send_welcome: bool,
) -> SessionOutcome {
    let mut outcome = SessionOutcome::default();

    if !read_hello(inbound, &mut outcome).await {
        return outcome;
    }

    if send_welcome
        && !send(
            &tx,
            coord_msg::Msg::Welcome(Welcome {
                protocol_version: 2,
            }),
        )
        .await
    {
        return outcome;
    }

    // The close. Ordering is safe: a `Welcome` already handed to the channel
    // is buffered ahead of the close, so the miner reads it before the stream
    // ends rather than racing against it.
    drop(tx);

    drain_to_end(inbound, &mut outcome).await;
    outcome
}

/// Bind a UDS mock coordinator, spawn `bin_path` as a miner client against it,
/// run the given scripted session, and report what was observed.
///
/// `socket` is a `unix://<path>` URI; the same value is passed to the miner via
/// `--quip-coordinator`.
async fn drive_miner_with_script(bin_path: &str, socket: &str, script: ScriptKind) -> DriverReport {
    let path = socket.strip_prefix("unix://").unwrap_or(socket).to_string();
    let _ = std::fs::remove_file(&path);
    #[expect(
        clippy::expect_used,
        reason = "test harness panics on bind failure rather than restructuring error flow"
    )]
    let uds = UnixListener::bind(&path).expect("bind unix socket");
    let incoming = UnixListenerStream::new(uds);

    let (otx, orx) = oneshot::channel::<SessionOutcome>();
    let svc = MockCoordinator {
        outcome_tx: Mutex::new(Some(otx)),
        script,
    };
    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(MinerServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await
    });

    #[expect(
        clippy::expect_used,
        reason = "test harness panics on spawn failure rather than restructuring error flow"
    )]
    let mut child = Command::new(bin_path)
        .arg("--quip-coordinator")
        .arg(socket)
        .arg("--miner-id")
        .arg("mock-0")
        .env("QUIP_SESSION_TOKEN", "test-token")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn miner");
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });

    // Bound the wait so a hung miner can't hang the test suite; on timeout,
    // kill the child and report a sentinel exit code so callers fail loudly
    // instead of blocking forever. The budget covers a walk whose every phase
    // times out, so a partial failure still produces a full phase list.
    let exit_code = if let Ok(status) = tokio::time::timeout(CHILD_TIMEOUT, child.wait()).await {
        #[expect(
            clippy::expect_used,
            reason = "test harness panics if wait fails after spawn succeeded"
        )]
        status.expect("wait for miner").code().unwrap_or(-1)
    } else {
        let _ = child.kill().await;
        -1
    };
    let stderr = match stderr_reader {
        Some(reader) => tokio::time::timeout(Duration::from_secs(5), reader)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default(),
        None => String::new(),
    };
    // The session handler sends the outcome as the miner closes its stream on
    // exit; the timeout guards a miner that dies before ever connecting.
    let outcome = tokio::time::timeout(OUTCOME_TIMEOUT, orx)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    server.abort();
    let _ = std::fs::remove_file(&path);

    DriverReport {
        lease: outcome.lease,
        handshake_ok: outcome.handshake_ok,
        hello: outcome.hello,
        ready_received: outcome.ready_received,
        job_request_credits: outcome.job_request_credits,
        jobs_dispatched: outcome.jobs_dispatched,
        results: outcome.results,
        rejects: outcome.rejects,
        statuses: outcome.statuses,
        cancel_acked: outcome.cancel_acked,
        ping_acked: outcome.ping_acked,
        capabilities_received: outcome.capabilities_received,
        cancelled_watermark: outcome.cancelled_watermark,
        fatal: outcome.fatal,
        terminal: outcome.terminal,
        timed_out_phases: outcome.timed_out_phases,
        exit_code,
        stderr,
    }
}

/// Run the full conformance walk against `bin_path`.
pub async fn drive_miner(bin_path: &str, socket: &str) -> DriverReport {
    drive_miner_with_script(bin_path, socket, ScriptKind::Full).await
}

/// Send a `Welcome` with an unsupported `protocol_version` and observe how
/// `bin_path` rejects it. A conformant miner exits `ConfigInvalid` (64) and
/// sends a `Fatal` before disconnecting.
pub async fn drive_miner_bad_welcome(bin_path: &str, socket: &str) -> DriverReport {
    drive_miner_with_script(bin_path, socket, ScriptKind::BadWelcome).await
}

/// Handshake, one valid job, then wait. Does not send `Shutdown`.
pub async fn drive_miner_one_job(bin_path: &str, socket: &str) -> DriverReport {
    drive_miner_with_script(bin_path, socket, ScriptKind::OneJobThenWait).await
}

/// Answer `Hello` with `Welcome`, then drop the stream without `Shutdown`.
/// A conformant miner treats the vanished coordinator as a failure and exits
/// `InternalFatal` (70), not `Clean`.
pub async fn drive_miner_close_after_welcome(bin_path: &str, socket: &str) -> DriverReport {
    drive_miner_with_script(bin_path, socket, ScriptKind::CloseAfterWelcome).await
}

/// Read `Hello`, then drop the stream without ever sending `Welcome`.
/// A conformant miner exits `TokenRejected` (77).
pub async fn drive_miner_close_before_welcome(bin_path: &str, socket: &str) -> DriverReport {
    drive_miner_with_script(bin_path, socket, ScriptKind::CloseBeforeWelcome).await
}

/// Exit code a miner must use when it refuses a `Welcome` it cannot speak.
pub const EXIT_CONFIG_INVALID: i32 = 64;

/// Exit code a miner must use when the coordinator vanishes after `Welcome`.
pub const EXIT_INTERNAL_FATAL: i32 = 70;

/// Exit code a miner must use when the coordinator vanishes before `Welcome`.
pub const EXIT_TOKEN_REJECTED: i32 = 77;

impl DriverReport {
    /// Verdict for [`drive_miner_bad_welcome`]: the miner must say *why* it is
    /// leaving before it leaves.
    ///
    /// A bare exit 64 is indistinguishable, from the coordinator's side, from
    /// a crash during startup. The `Fatal` is what turns an unexplained
    /// disconnect into a diagnosable one, so the exit code alone is not enough.
    #[must_use]
    pub fn bad_welcome_conformant(&self) -> bool {
        self.handshake_ok
            && self.exit_code == EXIT_CONFIG_INVALID
            && self
                .fatal
                .as_ref()
                .is_some_and(|(code, _)| *code == EXIT_CONFIG_INVALID)
            && self.results.is_empty()
    }

    /// Verdict for [`drive_miner_close_after_welcome`]: a coordinator that
    /// disappears mid-session is a failure, and the exit code has to say so.
    ///
    /// The distinction being graded is against exit `0`. `Shutdown` is the
    /// only clean end to a session; a stream that simply stops is a lost
    /// coordinator, and a miner that reports it as success tells its
    /// supervisor nothing went wrong when the session was in fact cut short.
    ///
    /// No `Fatal` is required. The channel the miner would send it on is the
    /// one that just closed, so demanding a `Fatal` here would demand the
    /// impossible.
    #[must_use]
    pub fn close_after_welcome_conformant(&self) -> bool {
        self.handshake_ok && self.exit_code == EXIT_INTERNAL_FATAL && self.results.is_empty()
    }

    /// Verdict for [`drive_miner_close_before_welcome`]: a coordinator that
    /// drops the connection before it ever answers `Hello` is reported as a
    /// rejected session, not as an internal fault and not as success.
    ///
    /// The two close verdicts differ only in the expected code, and that is
    /// the point: the miner has to distinguish "never accepted" from "accepted
    /// then lost", because they send an operator to different places.
    #[must_use]
    pub fn close_before_welcome_conformant(&self) -> bool {
        self.handshake_ok && self.exit_code == EXIT_TOKEN_REJECTED && self.results.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare_report() -> DriverReport {
        DriverReport {
            lease: None,
            handshake_ok: true,
            hello: None,
            ready_received: false,
            job_request_credits: vec![],
            jobs_dispatched: 0,
            results: vec![],
            rejects: vec![],
            statuses: vec![],
            cancel_acked: false,
            ping_acked: false,
            capabilities_received: None,
            cancelled_watermark: 0,
            fatal: None,
            terminal: Terminal::Open,
            timed_out_phases: vec![],
            exit_code: 0,
            stderr: String::new(),
        }
    }

    fn hello(backend: &str) -> Hello {
        Hello {
            miner_id: "mock-0".into(),
            session_token: "test-token".into(),
            capabilities: Some(caps(backend)),
        }
    }

    fn caps(backend: &str) -> Capabilities {
        Capabilities {
            backend: quip_protocol::session::backend_from_name(backend).unwrap() as i32,
            algorithm: quip_proto::v1::Algorithm::Sa as i32,
            supported_kinds: vec![JobKind::IsingSample as i32],
            max_nodes: 100,
            max_edges: 200,
            features: vec!["streaming".to_owned()],
            protocol_version: 2,
            stream_width: 1,
            native_topology_hash: None,
            encodings: vec![quip_proto::v1::CoefficientEncoding::I32 as i32],
            generators: vec![],
        }
    }

    fn status(abandoned: u64) -> MinerStatus {
        MinerStatus {
            miner_id: "mock-0".to_owned(),
            utilization: 0.0,
            jobs_done: 0,
            abandoned_generation: abandoned,
            sampler_stats: HashMap::new(),
        }
    }

    fn result(job_id: &[u8], reported: i64, rescored: Option<i64>) -> ObservedResult {
        ObservedResult {
            job_id: job_id.to_vec(),
            solution_energies_milli: vec![reported],
            rescored_energies_milli: Some(vec![rescored]),
            meta_present: true,
            meta_sweeps: CONFIGURED_SWEEPS,
        }
    }

    fn full_results() -> Vec<ObservedResult> {
        REQUIRED_RESULTS
            .iter()
            .map(|id| result(id, 500, Some(500)))
            .collect()
    }

    /// A report that passes every axis, to mutate one field at a time from.
    fn conformant_report() -> DriverReport {
        let mut r = bare_report();
        r.hello = Some(hello("mock"));
        r.ready_received = true;
        // Grant of 3, then one refund per dispatched job.
        r.job_request_credits = vec![3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
        r.jobs_dispatched = 10;
        r.results = full_results();
        r.rejects = vec![
            ObservedReject {
                job_id: b"job-bad-h".to_vec(),
                reason: RejectReason::Malformed as i32,
            },
            ObservedReject {
                job_id: b"job-bad-j".to_vec(),
                reason: RejectReason::Malformed as i32,
            },
            ObservedReject {
                job_id: b"job-gate".to_vec(),
                reason: RejectReason::UnsupportedKind as i32,
            },
            ObservedReject {
                job_id: b"job-old".to_vec(),
                reason: RejectReason::Expired as i32,
            },
        ];
        r.statuses = vec![status(1), status(CANCEL_GENERATION)];
        r.cancel_acked = true;
        r.ping_acked = true;
        r.capabilities_received = Some(caps("mock"));
        r.cancelled_watermark = CANCEL_GENERATION;
        r.terminal = Terminal::Closed;
        r
    }

    #[test]
    fn the_reference_report_is_conformant() {
        // Guards the negative tests below: each of them mutates exactly one
        // field, so this must pass or they prove nothing.
        let r = conformant_report();
        assert!(r.is_conformant(), "{r:?}");
    }

    fn lease_report() -> DriverReport {
        let mut r = conformant_report();
        let caps = r.hello.as_mut().unwrap().capabilities.as_mut().unwrap();
        caps.supported_kinds.push(JobKind::IsingGenerate as i32);
        r.capabilities_received = Some(caps.clone());
        r
    }

    #[test]
    fn advertised_lease_requires_an_outcome() {
        let r = lease_report();
        assert!(!r.is_conformant());
    }

    #[test]
    fn lease_requires_four_verified_results_done_and_refund() {
        let mut r = lease_report();
        r.lease = Some(LeaseOutcome {
            results: 4,
            results_verified: 4,
            lease_done: Some((4, -1000)),
            credit_refunded: true,
        });
        assert!(r.is_conformant());
        for bad in [
            LeaseOutcome {
                lease_done: Some((3, -1000)),
                ..r.lease.clone().unwrap()
            },
            LeaseOutcome {
                results: 3,
                ..r.lease.clone().unwrap()
            },
            LeaseOutcome {
                results_verified: 3,
                ..r.lease.clone().unwrap()
            },
            LeaseOutcome {
                lease_done: None,
                ..r.lease.clone().unwrap()
            },
            LeaseOutcome {
                credit_refunded: false,
                ..r.lease.clone().unwrap()
            },
        ] {
            let mut failed = r.clone();
            failed.lease = Some(bad);
            assert!(!failed.is_conformant());
        }
    }

    #[test]
    fn unadvertised_lease_is_not_graded() {
        let mut r = conformant_report();
        assert!(r.is_conformant());
        r.lease = Some(LeaseOutcome {
            results: 0,
            results_verified: 0,
            lease_done: None,
            credit_refunded: false,
        });
        assert!(r.is_conformant());
    }

    fn lease_result(index: u64) -> quip_proto::v1::Result {
        let generator = lease_generator();
        let spec = quip_protocol::lease::LeaseSpec::from_proto(&generator).unwrap();
        let topology = TopologyView::from_proto(&lease_topology()).unwrap();
        let nonce = spec.nonce(index).unwrap();
        let (h, j) = topology.draw(nonce).unwrap();
        let spins = vec![1; topology.num_nodes];
        quip_proto::v1::Result {
            job_id: LEASE_JOB_ID.to_vec(),
            salt: spec.salt(index).unwrap().to_vec(),
            nonce: nonce.to_vec(),
            solutions: vec![quip_proto::v1::Solution {
                spins: encode_spins_packed(&spins),
                energy_milli: quip_protocol::scoring::energy_from_milli(
                    &spins,
                    &h,
                    &j,
                    &topology.edges,
                ),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn lease_fold_verifies_distinct_salts_and_refund_order() {
        let mut o = SessionOutcome {
            lease: Some(LeaseOutcome::default()),
            ..Default::default()
        };
        // These credits belong to the earlier plain jobs, not the lease completion.
        fold(
            &mut o,
            miner_msg::Msg::JobRequest(quip_proto::v1::JobRequest { credits: 1 }),
        );
        assert!(!o.lease.as_ref().unwrap().credit_refunded);
        for index in 0..4 {
            fold(&mut o, miner_msg::Msg::Result(lease_result(index)));
        }
        assert!(o.results.is_empty());
        assert_eq!(o.lease.as_ref().unwrap().results_verified, 4);
        fold(
            &mut o,
            miner_msg::Msg::LeaseDone(quip_proto::v1::LeaseDone {
                job_id: LEASE_JOB_ID.to_vec(),
                salts_done: 4,
                best_energy_milli: -1000,
            }),
        );
        assert!(!o.lease.as_ref().unwrap().credit_refunded);
        fold(
            &mut o,
            miner_msg::Msg::JobRequest(quip_proto::v1::JobRequest { credits: 1 }),
        );
        assert!(o.lease.as_ref().unwrap().credit_refunded);
        let mut r = lease_report();
        r.lease = o.lease;
        assert!(r.is_conformant());
    }

    #[test]
    fn lease_fold_rejects_duplicate_salts_and_bad_proofs() {
        let mut o = SessionOutcome {
            lease: Some(LeaseOutcome::default()),
            ..Default::default()
        };
        fold(&mut o, miner_msg::Msg::Result(lease_result(0)));
        fold(&mut o, miner_msg::Msg::Result(lease_result(0)));
        let mut bad_nonce = lease_result(1);
        bad_nonce.nonce = vec![0; 32];
        fold(&mut o, miner_msg::Msg::Result(bad_nonce));
        let mut bad_energy = lease_result(2);
        bad_energy.solutions.first_mut().unwrap().energy_milli += 1;
        fold(&mut o, miner_msg::Msg::Result(bad_energy));
        let lease = o.lease.unwrap();
        assert_eq!(lease.results, 4);
        assert_eq!(lease.results_verified, 1);
    }

    #[test]
    fn lease_fold_rejects_results_after_completion() {
        let mut o = SessionOutcome {
            lease: Some(LeaseOutcome {
                lease_done: Some((4, -1000)),
                ..Default::default()
            }),
            ..Default::default()
        };
        fold(&mut o, miner_msg::Msg::Result(lease_result(0)));
        let lease = o.lease.unwrap();
        assert_eq!(lease.results, 1);
        assert_eq!(lease.results_verified, 0);
        let mut report = lease_report();
        report.lease = Some(lease);
        assert!(!report.is_conformant());
    }

    #[test]
    fn lease_fold_rejects_wrong_job_id_during_active_lease() {
        let mut o = SessionOutcome {
            lease: Some(LeaseOutcome::default()),
            ..Default::default()
        };
        let mut result = lease_result(0);
        result.job_id = b"wrong-lease".to_vec();
        fold(&mut o, miner_msg::Msg::Result(result));
        assert_eq!(o.lease.as_ref().unwrap().results, 0);
        assert_eq!(o.results.len(), 1);
        let mut report = lease_report();
        report.results.extend(o.results);
        assert!(!report.is_conformant());
    }

    struct LeaseTrafficCoordinator(Mutex<Option<oneshot::Sender<SessionOutcome>>>);
    #[tonic::async_trait]
    impl MinerService for LeaseTrafficCoordinator {
        type SessionStream = ReceiverStream<Result<CoordMsg, Status>>;
        async fn session(
            &self,
            request: Request<Streaming<MinerMsg>>,
        ) -> Result<Response<Self::SessionStream>, Status> {
            let sender = self.0.lock().await.take().unwrap();
            let (tx, rx) = mpsc::channel(8);
            let mut inbound = request.into_inner();
            let _task = tokio::spawn(async move {
                let mut outcome = SessionOutcome::default();
                run_lease(&tx, &mut inbound, &mut outcome).await;
                let _ = sender.send(outcome);
            });
            Ok(Response::new(ReceiverStream::new(rx)))
        }
    }

    #[tokio::test]
    async fn lease_collection_timeout_bounds_continuous_traffic() {
        use quip_proto::v1::miner_service_client::MinerServiceClient;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (done, outcome) = oneshot::channel();
        let server = tokio::spawn(
            Server::builder()
                .add_service(MinerServiceServer::new(LeaseTrafficCoordinator(
                    Mutex::new(Some(done)),
                )))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
        );
        let mut client = MinerServiceClient::connect(format!("http://{address}"))
            .await
            .unwrap();
        let (tx, rx) = mpsc::channel(8);
        let mut inbound = client
            .session(ReceiverStream::new(rx))
            .await
            .unwrap()
            .into_inner();
        for _ in 0..3 {
            assert!(inbound.message().await.unwrap().is_some());
        }
        let traffic = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            loop {
                let _ = tick.tick().await;
                if tx
                    .send(MinerMsg {
                        msg: Some(miner_msg::Msg::Status(MinerStatus::default())),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let result = tokio::time::timeout(PHASE_TIMEOUT + Duration::from_secs(2), outcome).await;
        traffic.abort();
        server.abort();
        let outcome = result
            .expect("continuous traffic must not renew the collection budget")
            .unwrap();
        assert_eq!(outcome.timed_out_phases, vec!["salt-lease"]);
        assert!(!outcome.statuses.is_empty());
    }

    #[test]
    fn unsolicited_lease_results_remain_unexpected_plain_results() {
        let mut o = SessionOutcome::default();
        fold(&mut o, miner_msg::Msg::Result(lease_result(0)));
        assert_eq!(o.results.len(), 1);
        let mut r = conformant_report();
        r.results.extend(o.results);
        assert!(!r.is_conformant());
    }

    #[test]
    fn extra_lease_refunds_stay_invalid() {
        let mut o = SessionOutcome {
            lease: Some(LeaseOutcome {
                lease_done: Some((4, -1000)),
                ..Default::default()
            }),
            ..Default::default()
        };
        for count in 1..=3 {
            fold(
                &mut o,
                miner_msg::Msg::JobRequest(quip_proto::v1::JobRequest { credits: 1 }),
            );
            assert_eq!(o.lease.as_ref().unwrap().credit_refunded, count == 1);
        }
    }

    #[test]
    fn under_asserted_report_is_not_conformant() {
        // Regression: handshake+exit alone used to green-light non-conformant miners.
        let mut r = bare_report();
        r.handshake_ok = true;
        r.exit_code = 0;
        assert!(
            !r.is_conformant(),
            "Ready / JobRequest / per-job rejects / Cancel ack are required"
        );
    }

    #[test]
    fn per_job_id_reject_binding() {
        let mut r = bare_report();
        r.rejects.push(ObservedReject {
            job_id: b"job-bad-h".to_vec(),
            reason: RejectReason::Malformed as i32,
        });
        assert!(r.has_reject(b"job-bad-h", RejectReason::Malformed));
        assert!(!r.has_reject(b"job-old", RejectReason::Malformed));
        assert!(!r.has_reject(b"job-bad-h", RejectReason::Expired));
    }

    #[test]
    fn results_require_solutions_and_meta() {
        let mut r = bare_report();
        r.results = full_results();
        assert!(
            r.results_conformant(),
            "results with energy+meta must pass: {:?}",
            r.results
        );

        // Empty solutions on one Result -> fails, even though meta is present.
        let mut empty_solutions = full_results();
        #[expect(
            clippy::indexing_slicing,
            reason = "full_results always has REQUIRED_RESULTS.len() entries; index 0 is in bounds"
        )]
        {
            empty_solutions[0].solution_energies_milli.clear();
            empty_solutions[0].rescored_energies_milli = Some(vec![]);
        }
        let mut r_empty = bare_report();
        r_empty.results = empty_solutions;
        assert!(
            !r_empty.results_conformant(),
            "empty solutions must fail conformance"
        );

        // Missing meta on one Result -> fails, even though solutions are present.
        let mut missing_meta = full_results();
        #[expect(
            clippy::indexing_slicing,
            reason = "full_results always has REQUIRED_RESULTS.len() entries; index 1 is in bounds"
        )]
        {
            missing_meta[1].meta_present = false;
        }
        let mut r_meta = bare_report();
        r_meta.results = missing_meta;
        assert!(
            !r_meta.results_conformant(),
            "missing SamplerMeta must fail conformance"
        );

        // The documented failing example: no solutions and no meta at all.
        let mut r_bare_result = bare_report();
        r_bare_result.results = vec![ObservedResult {
            job_id: b"job-1".to_vec(),
            solution_energies_milli: vec![],
            rescored_energies_milli: None,
            meta_present: false,
            meta_sweeps: 0,
        }];
        assert!(!r_bare_result.results_conformant());
    }

    #[test]
    fn a_missing_required_result_fails() {
        let mut r = conformant_report();
        r.results.retain(|x| x.job_id != b"job-sparse");
        assert!(
            !r.is_conformant(),
            "the sparse-topology result is required: {r:?}"
        );
    }

    #[test]
    fn an_unexpected_result_fails() {
        // A Result for the stale post-cancel job is exactly what the
        // cancellation phase forbids.
        let mut r = conformant_report();
        r.results.push(result(b"job-stale", 500, Some(500)));
        assert!(!r.results_conformant(), "job-stale must produce no Result");
        assert!(!r.is_conformant(), "{r:?}");
    }

    #[test]
    fn a_misreported_energy_fails() {
        let mut r = conformant_report();
        #[expect(
            clippy::indexing_slicing,
            reason = "conformant_report always has REQUIRED_RESULTS.len() results"
        )]
        {
            // Same spins, a different claimed energy: the whole point of the gate.
            r.results[0].solution_energies_milli = vec![-9_000];
        }
        assert!(!r.energies_rescore_clean());
        assert!(!r.is_conformant(), "{r:?}");
    }

    #[test]
    fn undecodable_spins_fail() {
        let mut r = conformant_report();
        #[expect(
            clippy::indexing_slicing,
            reason = "conformant_report always has REQUIRED_RESULTS.len() results"
        )]
        {
            r.results[0].rescored_energies_milli = Some(vec![None]);
        }
        assert!(
            !r.energies_rescore_clean(),
            "spins that do not decode cannot be accepted"
        );
    }

    #[test]
    fn a_result_the_driver_cannot_score_is_not_graded() {
        // No problem on file -> no re-score claim either way.
        let mut r = bare_report();
        r.results = vec![ObservedResult {
            job_id: b"job-1".to_vec(),
            solution_energies_milli: vec![123],
            rescored_energies_milli: None,
            meta_present: true,
            meta_sweeps: CONFIGURED_SWEEPS,
        }];
        assert!(r.energies_rescore_clean());
    }

    #[test]
    fn ignoring_backend_toml_fails() {
        let mut r = conformant_report();
        #[expect(
            clippy::indexing_slicing,
            reason = "conformant_report always has REQUIRED_RESULTS.len() results"
        )]
        {
            r.results[0].meta_sweeps = 64; // the SDK default, not the configured 512
        }
        assert!(
            !r.sweeps_honoured(),
            "a solver that ignores Configure.backend_toml must fail"
        );
        assert!(!r.is_conformant(), "{r:?}");
    }

    #[test]
    fn a_gibbs_solver_reporting_the_doubled_budget_is_conformant() {
        // SPEC.md: the pin is a budget, and a gibbs solver runs — and reports
        // — twice it. The driver must expect the doubled echo rather than
        // fail the solver for following the spec.
        let mut r = conformant_report();
        if let Some(h) = r.hello.as_mut() {
            h.capabilities.as_mut().unwrap().algorithm = quip_proto::v1::Algorithm::Gibbs as i32;
        }
        if let Some(c) = r.capabilities_received.as_mut() {
            c.algorithm = quip_proto::v1::Algorithm::Gibbs as i32;
        }
        for result in &mut r.results {
            result.meta_sweeps = CONFIGURED_SWEEPS * GIBBS_SWEEP_MULTIPLIER;
        }
        assert!(r.is_conformant(), "{r:?}");
    }

    #[test]
    fn a_gibbs_solver_echoing_the_raw_budget_fails() {
        // The un-doubled echo means the solver skipped its own 2x rule.
        let mut r = conformant_report();
        if let Some(h) = r.hello.as_mut() {
            h.capabilities.as_mut().unwrap().algorithm = quip_proto::v1::Algorithm::Gibbs as i32;
        }
        if let Some(c) = r.capabilities_received.as_mut() {
            c.algorithm = quip_proto::v1::Algorithm::Gibbs as i32;
        }
        assert!(!r.sweeps_honoured(), "{r:?}");
    }

    #[test]
    fn capabilities_must_match_the_hello_identity() {
        let mut r = conformant_report();
        r.capabilities_received = Some(caps("cuda"));
        assert!(
            !r.capabilities_conformant(),
            "Capabilities.backend must agree with Hello.backend"
        );

        let mut missing = conformant_report();
        missing.capabilities_received = None;
        assert!(
            !missing.capabilities_conformant(),
            "GetCapabilities must be answered"
        );

        let mut no_hello = conformant_report();
        no_hello.hello = None;
        assert!(!no_hello.capabilities_conformant());
    }

    #[test]
    fn an_unanswered_ping_fails() {
        let mut r = conformant_report();
        r.ping_acked = false;
        assert!(!r.is_conformant(), "{r:?}");
    }

    #[test]
    fn cancellation_must_be_honoured() {
        // A Reject for a cancelled job: cancelled work is reported through
        // Status, never as a rejection.
        let mut rejected = conformant_report();
        rejected.rejects.push(ObservedReject {
            job_id: b"job-stale".to_vec(),
            reason: RejectReason::Malformed as i32,
        });
        assert!(!rejected.live_cancel_conformant());

        // Status never carried the cancelled watermark.
        let mut no_watermark = conformant_report();
        no_watermark.statuses = vec![status(1)];
        assert!(
            !no_watermark.live_cancel_conformant(),
            "Status.abandoned_generation must carry the cancelled watermark"
        );
    }

    #[test]
    fn the_credit_ledger_must_balance() {
        let mut leaked = conformant_report();
        let _ = leaked.job_request_credits.pop(); // one job never refunded
        assert!(
            !leaked.credit_ledger_balanced(),
            "a dropped refund leaks a coordinator slot"
        );
        assert!(!leaked.is_conformant(), "{leaked:?}");

        let mut doubled = conformant_report();
        doubled.job_request_credits.push(1); // one refund too many
        assert!(
            !doubled.credit_ledger_balanced(),
            "an extra refund inflates the pool"
        );
    }

    #[test]
    fn the_terminal_condition_is_graded_and_distinguishable() {
        for bad in [
            Terminal::Open,
            Terminal::EmptyMessage,
            Terminal::Transport("Cancelled: peer went away".to_owned()),
        ] {
            let mut r = conformant_report();
            r.terminal = bad.clone();
            assert!(!r.is_conformant(), "terminal {bad:?} must fail");
        }
    }

    #[test]
    fn a_timed_out_phase_fails_and_is_named() {
        let mut r = conformant_report();
        r.timed_out_phases = vec!["ping->status".to_owned()];
        assert!(!r.is_conformant());
        assert!(format!("{r:?}").contains("ping->status"));
    }

    #[test]
    fn bad_welcome_requires_fatal_before_the_exit_code() {
        let mut silent = bare_report();
        silent.exit_code = EXIT_CONFIG_INVALID;
        assert!(
            !silent.bad_welcome_conformant(),
            "exit 64 without a Fatal is an unexplained disconnect"
        );

        let mut spoken = bare_report();
        spoken.exit_code = EXIT_CONFIG_INVALID;
        spoken.fatal = Some((
            EXIT_CONFIG_INVALID,
            "unsupported protocol version".to_owned(),
        ));
        assert!(spoken.bad_welcome_conformant(), "{spoken:?}");

        // A Fatal whose code disagrees with the process exit is still wrong.
        let mut mismatched = bare_report();
        mismatched.exit_code = EXIT_CONFIG_INVALID;
        mismatched.fatal = Some((70, "internal".to_owned()));
        assert!(!mismatched.bad_welcome_conformant());
    }

    #[test]
    fn a_vanished_coordinator_is_never_a_clean_exit() {
        // Exit 0 is the failure these two verdicts exist to catch: `Shutdown`
        // is the only clean end, so a stream that just stops must not report
        // success.
        let mut clean = bare_report();
        clean.exit_code = 0;
        assert!(
            !clean.close_after_welcome_conformant(),
            "a lost coordinator reported as success hides a cut-short session"
        );
        assert!(!clean.close_before_welcome_conformant());

        let mut after = bare_report();
        after.exit_code = EXIT_INTERNAL_FATAL;
        assert!(after.close_after_welcome_conformant(), "{after:?}");

        let mut before = bare_report();
        before.exit_code = EXIT_TOKEN_REJECTED;
        assert!(before.close_before_welcome_conformant(), "{before:?}");
    }

    #[test]
    fn the_two_close_points_do_not_share_an_exit_code() {
        // The whole value of the pair is that the miner tells them apart:
        // "never accepted" and "accepted then lost" send an operator to
        // different places, so each verdict must reject the other's code.
        assert_ne!(EXIT_INTERNAL_FATAL, EXIT_TOKEN_REJECTED);

        let mut swapped_after = bare_report();
        swapped_after.exit_code = EXIT_TOKEN_REJECTED;
        assert!(!swapped_after.close_after_welcome_conformant());

        let mut swapped_before = bare_report();
        swapped_before.exit_code = EXIT_INTERNAL_FATAL;
        assert!(!swapped_before.close_before_welcome_conformant());
    }

    #[test]
    fn a_close_verdict_needs_the_handshake_and_no_results() {
        let mut no_hello = bare_report();
        no_hello.handshake_ok = false;
        no_hello.exit_code = EXIT_INTERNAL_FATAL;
        assert!(
            !no_hello.close_after_welcome_conformant(),
            "the right exit code for the wrong reason is not conformance"
        );

        let mut with_result = bare_report();
        with_result.exit_code = EXIT_INTERNAL_FATAL;
        with_result.results.push(ObservedResult {
            job_id: b"job-1".to_vec(),
            solution_energies_milli: vec![0],
            rescored_energies_milli: None,
            meta_present: false,
            meta_sweeps: 0,
        });
        assert!(
            !with_result.close_after_welcome_conformant(),
            "no job was ever dispatched, so no result can be legitimate"
        );
    }
}
