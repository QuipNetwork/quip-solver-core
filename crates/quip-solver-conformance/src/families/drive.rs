//! The `--family` gate: the session script, its report, and its graders. The
//! script runs on the mock coordinator and records the transcript of each
//! case. The graders are pure functions over that transcript, so unit tests
//! feed them hand-built transcripts and need no binary.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::slice::from_ref;
use std::time::Duration;

use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits, SizeBounds};
use ising_generators::family::{admit_topology, BaseTopology, SaltFault};
use ising_generators::instance::Instance;
use ising_generators::Code;
use quip_proto::v1::{
    coord_msg, miner_msg, Cancel, Capabilities, CoordMsg, EdgeList, FamilyDescriptor,
    GeneratorAlgorithm, IsingProblemGenerator, Job, JobKind, MinerMsg, Ping, RejectReason,
    SetTarget, Shutdown, Topology, Welcome,
};
use quip_protocol::family::{reject_reason, support_code, CodePhase};
use quip_protocol::lease::{Generator, LeaseSpec};
use quip_protocol::scoring::energy_from_milli;
use quip_protocol::wire::decode_spins_packed;
use tokio::sync::mpsc;
use tonic::{Status, Streaming};

use super::layered::P_L2_F0;
use super::layered::{P_F5000, P_L3_F0, P_SPEC};
use super::{export_instances, reference_revision, ReferenceRevision, REFERENCE_REVISIONS};
use crate::driver::{
    configure, dispatch, drain_to_end, read_hello, read_until, run_miner, send, ScriptKind,
    SessionOutcome, Terminal, CONFIGURED_SWEEPS, PHASE_TIMEOUT,
};
use crate::solve::CaseOutcome;

/// One graded case of the `--family` gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyCase {
    /// Stable case name, for example `revision-plus-one`.
    pub name: &'static str,
    /// `family_id@revision` for a per-revision case, `None` for a session case.
    pub subject: Option<String>,
    /// How it ended.
    pub outcome: CaseOutcome,
}

/// Every case the gate ran against one binary.
#[derive(Clone, Debug, Default)]
pub struct FamilyReport {
    /// The graded cases, in run order.
    pub cases: Vec<FamilyCase>,
    /// The exit code of the solver binary.
    pub exit_code: i32,
    /// What the solver binary wrote to standard error.
    pub stderr: String,
}

impl FamilyReport {
    /// True when at least one case passed and none failed. A skip is not a
    /// pass for FAM-57, so a report of skips alone is not conformant.
    #[must_use]
    pub fn is_conformant(&self) -> bool {
        let mut passed = false;
        for case in &self.cases {
            match case.outcome {
                CaseOutcome::Failed(_) => return false,
                CaseOutcome::Passed => passed = true,
                CaseOutcome::Skipped(_) => {}
            }
        }
        passed
    }

    /// The outcome of the case with `name` and `subject`.
    #[must_use]
    pub fn outcome(&self, name: &str, subject: Option<&str>) -> Option<&CaseOutcome> {
        self.cases
            .iter()
            .find(|case| case.name == name && case.subject.as_deref() == subject)
            .map(|case| &case.outcome)
    }

    /// One line per case: `  [pass] name subject`, `  [FAIL] name subject: why`,
    /// or `  [skip] name subject: why`.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut out = String::new();
        for case in &self.cases {
            let (mark, why) = match &case.outcome {
                CaseOutcome::Passed => ("pass", None),
                CaseOutcome::Failed(why) => ("FAIL", Some(why)),
                CaseOutcome::Skipped(why) => ("skip", Some(why)),
            };
            let _ = write!(out, "  [{mark}] {}", case.name);
            if let Some(subject) = &case.subject {
                let _ = write!(out, " {subject}");
            }
            if let Some(why) = why {
                let _ = write!(out, ": {why}");
            }
            out.push('\n');
        }
        out
    }
}

/// What a lease must give: its inputs and the instance of each slot, by lease index.
pub(crate) struct ExpectedLease {
    pub(crate) spec: LeaseSpec,
    pub(crate) salts: Vec<Result<Instance, SaltFault>>,
}

/// `family_id@revision` for each advertised pair, in `Hello` order, without
/// repeats. The reference is `None` when the gate cannot drive the pair.
pub(crate) fn plan_subjects(
    caps: &Capabilities,
) -> Vec<(String, Option<&'static ReferenceRevision>)> {
    let mut subjects: Vec<(String, Option<&'static ReferenceRevision>)> = Vec::new();
    // FAM-72 sends a family lease only for a pair in `generated_families`.
    for entry in &caps.generated_families {
        for &revision in &entry.revisions {
            let subject = format!("{}@{revision}", entry.family_id);
            if subjects.iter().all(|(known, _)| *known != subject) {
                let reference = reference_revision(&entry.family_id, revision);
                subjects.push((subject, reference));
            }
        }
    }
    subjects
}

/// FAM-47 codes 5 to 13 as the gate predicts them for `caps`.
///
/// # Errors
///
/// The code the lease would get.
#[expect(
    clippy::too_many_arguments,
    reason = "the signature mirrors the lease inputs FAM-47 checks"
)]
pub(crate) fn reference_admission(
    caps: &Capabilities,
    reference: &ReferenceRevision,
    family_id: &str,
    revision: u32,
    params: &[u8],
    sent_params_digest: &[u8],
    topology: &BaseTopology,
    sent_order_digest: &[u8],
) -> Result<SizeBounds, Code> {
    let held = support_code(
        &caps.generated_families,
        family_id,
        revision,
        sent_params_digest,
    );
    if let Some(code @ (Code::FamilyUnsupported | Code::RevisionUnsupported)) = held {
        return Err(code);
    }
    let admitted = admit(
        from_ref(reference.contract),
        family_id.as_bytes(),
        revision,
        params,
        sent_params_digest,
    )?;
    if let Some(code) = held {
        return Err(code);
    }
    let advertised = AdvertisedLimits {
        max_nodes: u64::from(caps.max_nodes),
        max_edges: u64::from(caps.max_edges),
        max_degree: u64::from(caps.max_degree.unwrap_or(0)),
    };
    admit_topology(&admitted, topology, sent_order_digest, &advertised)
}

/// Grade the `Hello` capabilities: `(protocol version, generators)`.
pub(crate) fn grade_hello(caps: Option<&Capabilities>) -> (CaseOutcome, CaseOutcome) {
    let Some(caps) = caps else {
        let failed = CaseOutcome::Failed("no Hello".to_owned());
        return (failed.clone(), failed);
    };
    let version = if caps.protocol_version == 2 {
        CaseOutcome::Passed
    } else {
        CaseOutcome::Failed(format!(
            "protocol_version is {}, FAM-45 requires 2",
            caps.protocol_version
        ))
    };
    let lists_family = caps
        .generators
        .contains(&(GeneratorAlgorithm::FamilyChacha8V1 as i32));
    let generates = !caps.generated_families.is_empty();
    let generators = if lists_family == generates {
        CaseOutcome::Passed
    } else if generates {
        CaseOutcome::Failed(
            "generated_families is not empty but generators lacks FAMILY_CHACHA8_V1".to_owned(),
        )
    } else {
        CaseOutcome::Failed(
            "generators lists FAMILY_CHACHA8_V1 but generated_families is empty".to_owned(),
        )
    };
    (version, generators)
}

fn outcome_of(graded: Result<(), String>) -> CaseOutcome {
    match graded {
        Ok(()) => CaseOutcome::Passed,
        Err(why) => CaseOutcome::Failed(why),
    }
}

/// Positions of the `JobRequest` messages, with their credits.
fn job_requests(slice: &[miner_msg::Msg]) -> Vec<(usize, u32)> {
    let mut found = Vec::new();
    for (at, msg) in slice.iter().enumerate() {
        if let miner_msg::Msg::JobRequest(request) = msg {
            found.push((at, request.credits));
        }
    }
    found
}

/// Exactly one one-credit `JobRequest`, after position `after`.
fn check_refund(slice: &[miner_msg::Msg], after: Option<usize>) -> Result<(), String> {
    let requests = job_requests(slice);
    let [(at, credits)] = requests.as_slice() else {
        return Err(format!(
            "expected exactly one JobRequest, got {}",
            requests.len()
        ));
    };
    if *credits != 1 {
        return Err(format!("the JobRequest carries {credits} credits, not 1"));
    }
    if after.is_some_and(|position| *at < position) {
        return Err("the JobRequest came before the message it refunds".to_owned());
    }
    Ok(())
}

/// Grade a rejected lease: one `Reject` with reason `want`, no work, one refund.
pub(crate) fn grade_reject(slice: &[miner_msg::Msg], job_id: &[u8], want: i32) -> CaseOutcome {
    outcome_of(check_reject(slice, job_id, want))
}

fn check_reject(slice: &[miner_msg::Msg], job_id: &[u8], want: i32) -> Result<(), String> {
    let mut reject_at = Vec::new();
    for (at, msg) in slice.iter().enumerate() {
        match msg {
            miner_msg::Msg::Reject(reject) if reject.job_id == job_id => {
                if reject.reason != want {
                    return Err(format!("Reject reason {}, expected {want}", reject.reason));
                }
                reject_at.push(at);
            }
            miner_msg::Msg::Result(result) if result.job_id == job_id => {
                return Err("a Result followed a rejected lease".to_owned());
            }
            miner_msg::Msg::LeaseDone(done) if done.job_id == job_id => {
                return Err("a LeaseDone followed a rejected lease".to_owned());
            }
            _ => {}
        }
    }
    let [first] = reject_at.as_slice() else {
        return Err(format!(
            "expected exactly one Reject, got {}",
            reject_at.len()
        ));
    };
    check_refund(slice, Some(*first))
}

/// Grade a lease: the `Result` of every generated slot, the `LeaseDone`, one refund.
pub(crate) fn grade_lease(
    slice: &[miner_msg::Msg],
    job_id: &[u8],
    expected: &ExpectedLease,
) -> CaseOutcome {
    outcome_of(check_lease(slice, job_id, expected))
}

fn check_lease(
    slice: &[miner_msg::Msg],
    job_id: &[u8],
    expected: &ExpectedLease,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    let mut lowest = i64::MAX;
    let mut done_at = None;
    for (at, msg) in slice.iter().enumerate() {
        match msg {
            miner_msg::Msg::Reject(reject) => {
                return Err(format!(
                    "Reject with reason {} for a valid lease",
                    reject.reason
                ));
            }
            miner_msg::Msg::Result(result) if result.job_id == job_id => {
                if done_at.is_some() {
                    return Err("a Result came after the LeaseDone".to_owned());
                }
                let index = check_result(result, expected, &mut lowest)?;
                if !seen.insert(index) {
                    return Err(format!("slot {index} has a second Result"));
                }
            }
            miner_msg::Msg::LeaseDone(done) if done.job_id == job_id => {
                if done_at.is_some() {
                    return Err("a second LeaseDone".to_owned());
                }
                done_at = Some(at);
                if done.salts_done != expected.spec.salt_count {
                    return Err(format!(
                        "LeaseDone salts_done is {}, expected {}",
                        done.salts_done, expected.spec.salt_count
                    ));
                }
                if done.best_energy_milli != lowest {
                    return Err(format!(
                        "LeaseDone best_energy_milli is {}, the lowest reported energy is {lowest}",
                        done.best_energy_milli
                    ));
                }
            }
            _ => {}
        }
    }
    let Some(done_at) = done_at else {
        return Err("no LeaseDone".to_owned());
    };
    for (slot, instance) in expected.salts.iter().enumerate() {
        let present = u64::try_from(slot).is_ok_and(|index| seen.contains(&index));
        if instance.is_ok() && !present {
            return Err(format!(
                "slot {slot} generated an instance but has no Result"
            ));
        }
    }
    check_refund(slice, Some(done_at))
}

/// Check one `Result` and return its slot. Lowers `lowest` to its energies.
fn check_result(
    result: &quip_proto::v1::Result,
    expected: &ExpectedLease,
    lowest: &mut i64,
) -> Result<u64, String> {
    let salt: [u8; 32] = result
        .salt
        .as_slice()
        .try_into()
        .map_err(|_| "a Result salt is not 32 bytes".to_owned())?;
    let index = expected
        .spec
        .index_of(&salt)
        .ok_or_else(|| "a Result salt is not in the lease".to_owned())?;
    if expected
        .spec
        .nonce(index)
        .as_ref()
        .map(<[u8; 32]>::as_slice)
        != Some(result.nonce.as_slice())
    {
        return Err(format!(
            "slot {index}: nonce differs from the derived nonce"
        ));
    }
    let slot = usize::try_from(index).map_err(|_| format!("slot {index}: out of range"))?;
    let instance = match expected.salts.get(slot) {
        Some(Ok(instance)) => instance,
        Some(Err(_)) => return Err(format!("slot {index}: a Result for a skipped salt")),
        None => return Err(format!("slot {index}: not in the expected lease")),
    };
    if result.solutions.is_empty() {
        return Err(format!("slot {index}: a Result without a solution"));
    }
    let edges = instance
        .edges()
        .iter()
        .map(|&(a, b)| Ok((usize::try_from(a)?, usize::try_from(b)?)))
        .collect::<Result<Vec<_>, std::num::TryFromIntError>>()
        .map_err(|_| format!("slot {index}: an edge does not fit in usize"))?;
    for solution in &result.solutions {
        let spins = decode_spins_packed(&solution.spins, instance.node_ids().len())
            .map_err(|fault| format!("slot {index}: spins do not decode: {fault:?}"))?;
        let want = instance
            .offset_milli()
            .checked_add(energy_from_milli(
                &spins,
                instance.h(),
                instance.j(),
                &edges,
            ))
            .ok_or_else(|| format!("slot {index}: offset plus energy overflows i64"))?;
        if solution.energy_milli != want {
            return Err(format!(
                "slot {index}: energy_milli is {}, the instance scores {want} with its offset",
                solution.energy_milli
            ));
        }
        *lowest = (*lowest).min(solution.energy_milli);
    }
    Ok(index)
}

/// Grade a lease the miner abandoned: no work for it, one refund.
pub(crate) fn grade_stale(slice: &[miner_msg::Msg], job_id: &[u8]) -> CaseOutcome {
    outcome_of(check_stale(slice, job_id))
}

fn check_stale(slice: &[miner_msg::Msg], job_id: &[u8]) -> Result<(), String> {
    let mut lease_dones = 0_u32;
    for msg in slice {
        match msg {
            miner_msg::Msg::Result(result) if result.job_id == job_id => {
                return Err("a Result for a stale lease".to_owned());
            }
            miner_msg::Msg::Reject(reject) if reject.job_id == job_id => {
                return Err("a Reject for a stale lease".to_owned());
            }
            miner_msg::Msg::LeaseDone(done) if done.job_id == job_id => {
                lease_dones += 1;
                if lease_dones > 1 {
                    return Err("a second LeaseDone for a stale lease".to_owned());
                }
                if done.salts_done != 0 {
                    return Err(format!(
                        "a stale LeaseDone reports {} salts done",
                        done.salts_done
                    ));
                }
            }
            _ => {}
        }
    }
    check_refund(slice, None)
}

/// Generation the script cancels before it sends any family job.
const FAMILY_CANCEL_GENERATION: u64 = 3;

/// Generation of every live family job, above the cancel watermark.
const FAMILY_LIVE_GENERATION: u64 = FAMILY_CANCEL_GENERATION + 1;

/// Barriers outside the per-revision cases, each bounded by one
/// `PHASE_TIMEOUT`: the Hello, the two setup barriers, two per session case
/// (two session cases), and the drain after `Shutdown`.
const SESSION_BARRIERS: u32 = 1 + 2 + 2 * 2 + 1;

/// How long the gate waits for the solver to exit. A walk in which every
/// barrier times out must still finish, so the report names every case.
///
/// Each reference revision runs at most `REVISION_CASES.len()` cases of two
/// barriers each. With 10 cases and 2 reference revisions the budget is
/// `(8 + 2 * 20 + 1) * 10 s = 490 s`, the `+ 1` being one phase of slack so
/// the script ends before the child is killed.
fn family_child_timeout() -> Duration {
    let per_revision = u32::try_from(REVISION_CASES.len() * 2).unwrap_or(u32::MAX);
    let revisions = u32::try_from(REFERENCE_REVISIONS.len()).unwrap_or(u32::MAX);
    let phases = per_revision
        .saturating_mul(revisions)
        .saturating_add(SESSION_BARRIERS + 1);
    PHASE_TIMEOUT.saturating_mul(phases)
}

/// The hash the script caches `T_SPARSE` under.
const FAMILY_TOPOLOGY_HASH: [u8; 32] = [0x77; 32];

/// The `family-small` lease inputs.
const LAST_PROOF_BLOCK_HASH: [u8; 32] = [0x11; 32];
const MINER_ACCOUNT: [u8; 32] = [0x22; 32];
const SALT_START: u64 = 5;
const SALT_COUNT: u64 = 8;

/// Parameter bytes with a layout fault (code 7).
const P_MALFORMED: [u8; 1] = [0x02];

/// `T_SPARSE`: sparse native ids, one of them `u32::MAX`.
const T_SPARSE_NODES: [u32; 6] = [907, 41, 0, 4_294_967_295, 12, 3];
const T_SPARSE_EDGES: [(u32, u32); 7] = [
    (907, 41),
    (41, 0),
    (0, 4_294_967_295),
    (4_294_967_295, 12),
    (12, 3),
    (3, 907),
    (41, 12),
];

/// The per-revision cases, in run order.
const REVISION_CASES: [&str; 10] = [
    "supported-revision",
    "revision-plus-one",
    "params-malformed",
    "params-digest-mismatch",
    "params-out-of-bounds",
    "params-restricted",
    "topology-order-mismatch",
    "degree-bound-too-large",
    "stale-lease",
    "deadline-expired",
];

fn base_salt() -> [u8; 32] {
    let mut salt = [0x33; 32];
    salt[..8].fill(0);
    salt
}

fn t_sparse_wire() -> Topology {
    Topology {
        hash: FAMILY_TOPOLOGY_HASH.to_vec(),
        nodes: T_SPARSE_NODES.to_vec(),
        edges: Some(EdgeList {
            u: T_SPARSE_EDGES.iter().map(|&(u, _)| u).collect(),
            v: T_SPARSE_EDGES.iter().map(|&(_, v)| v).collect(),
        }),
        allowed_h_milli: vec![-1000, 0, 1000],
        allowed_j_milli: vec![-1000, 1000],
    }
}

/// `T_SPARSE`, or `T_REORDERED` (the first two edges swapped) when `reordered`.
fn base_topology(reordered: bool) -> BaseTopology {
    let mut edges = T_SPARSE_EDGES;
    if reordered {
        edges.swap(0, 1);
    }
    #[expect(
        clippy::expect_used,
        reason = "T_SPARSE is a fixed valid topology, so this cannot fail"
    )]
    BaseTopology::new(T_SPARSE_NODES.to_vec(), &edges).expect("T_SPARSE is valid")
}

/// The FAM-42 wire reason of an admission code, as a raw `i32`.
fn admission_reason(code: Code) -> i32 {
    // Codes 5 to 13 all have an admission reason. `Unspecified` never matches
    // a conformant reject, so a gap here fails the case instead of passing it.
    reject_reason(code, CodePhase::Admission)
        .map_or(RejectReason::Unspecified as i32, |reason| reason as i32)
}

/// The inputs of one family lease that the cases vary.
#[derive(Clone, Debug)]
struct FamilyLease {
    family_id: String,
    revision: u32,
    params: Vec<u8>,
    params_digest: Vec<u8>,
    order_digest: Vec<u8>,
    generation: u64,
    deadline_ms: u64,
}

impl FamilyLease {
    fn new(family_id: &str, revision: u32, params: &[u8], order_digest: &[u8]) -> Self {
        Self {
            family_id: family_id.to_owned(),
            revision,
            params: params.to_vec(),
            params_digest: params_digest(params).to_vec(),
            order_digest: order_digest.to_vec(),
            generation: FAMILY_LIVE_GENERATION,
            deadline_ms: 0,
        }
    }

    fn with_params(self, params: &[u8]) -> Self {
        Self {
            params: params.to_vec(),
            params_digest: params_digest(params).to_vec(),
            ..self
        }
    }

    fn job(&self, job_id: &str) -> Job {
        Job {
            job_id: job_id.as_bytes().to_vec(),
            kind: JobKind::IsingGenerate as i32,
            generation: self.generation,
            deadline_ms: self.deadline_ms,
            generator: Some(IsingProblemGenerator {
                algorithm: GeneratorAlgorithm::FamilyChacha8V1 as i32,
                topology_hash: FAMILY_TOPOLOGY_HASH.to_vec(),
                last_proof_block_hash: LAST_PROOF_BLOCK_HASH.to_vec(),
                miner_account: MINER_ACCOUNT.to_vec(),
                base_salt: base_salt().to_vec(),
                salt_start: SALT_START,
                salt_count: SALT_COUNT,
                family: Some(FamilyDescriptor {
                    family_id: self.family_id.clone(),
                    revision: self.revision,
                    params: self.params.clone(),
                    params_digest: self.params_digest.clone(),
                }),
                topology_order_digest: self.order_digest.clone(),
            }),
            ..Job::default()
        }
    }

    fn admission(
        &self,
        caps: &Capabilities,
        reference: &ReferenceRevision,
        topology: &BaseTopology,
    ) -> Result<SizeBounds, Code> {
        reference_admission(
            caps,
            reference,
            &self.family_id,
            self.revision,
            &self.params,
            &self.params_digest,
            topology,
            &self.order_digest,
        )
    }
}

/// How a job case is graded.
enum Grade {
    Lease(ExpectedLease),
    Reject(i32),
    Stale,
    /// The gate cannot build the expectation, so the case fails whatever
    /// the miner sends.
    Unavailable(String),
}

/// One per-revision case: its name, its lease, the reference admission it
/// needs (`None` means admitted), and its grader. The lease is `Err` with the
/// skip reason when no reference params set gives the needed code.
struct JobCase {
    name: &'static str,
    lease: Result<FamilyLease, String>,
    needs: Option<Code>,
    grade: Grade,
}

/// The expected lease of `lease` regenerated with `reference`.
fn expected_lease(
    reference: &ReferenceRevision,
    lease: &FamilyLease,
    topology: &BaseTopology,
) -> Result<ExpectedLease, String> {
    let spec = LeaseSpec::new(
        Generator::FamilyChacha8V1,
        LAST_PROOF_BLOCK_HASH,
        MINER_ACCOUNT,
        base_salt(),
        SALT_START,
        SALT_COUNT,
    )
    .map_err(|fault| format!("the lease inputs are invalid: {fault:?}"))?;
    let mut nonces = Vec::new();
    for index in 0..SALT_COUNT {
        nonces.push(
            spec.nonce(index)
                .ok_or_else(|| format!("slot {index} is outside the lease"))?,
        );
    }
    let salts = export_instances(reference.contract, &lease.params, topology, &nonces)
        .map_err(|code| format!("the reference does not regenerate: code {}", code.value()))?;
    Ok(ExpectedLease { spec, salts })
}

/// True when a `JobRequest` follows the first `Reject` or `LeaseDone` for `job_id`.
fn refunded_after_terminal(slice: &[miner_msg::Msg], job_id: &[u8]) -> bool {
    let terminal = slice.iter().position(|msg| match msg {
        miner_msg::Msg::Reject(reject) => reject.job_id == job_id,
        miner_msg::Msg::LeaseDone(done) => done.job_id == job_id,
        _ => false,
    });
    terminal.is_some_and(|at| {
        slice
            .iter()
            .skip(at + 1)
            .any(|msg| matches!(msg, miner_msg::Msg::JobRequest(_)))
    })
}

fn barrier_failure(outcome: &SessionOutcome, phase: &str) -> String {
    if outcome.stream_ended() {
        format!("the stream ended ({:?}) in phase {phase}", outcome.terminal)
    } else {
        format!("phase {phase} timed out")
    }
}

fn push_case(
    outcome: &mut SessionOutcome,
    name: &'static str,
    subject: Option<&str>,
    case_outcome: CaseOutcome,
) {
    outcome.family_cases.push(FamilyCase {
        name,
        subject: subject.map(str::to_owned),
        outcome: case_outcome,
    });
}

/// Dispatch `job`, wait for its refund and then a `Ping` fence, and return
/// where its transcript starts in `outcome.messages`.
async fn run_job(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    job: Job,
    stale: bool,
) -> Result<usize, String> {
    if outcome.stream_ended() {
        return Err(format!(
            "the stream ended ({:?}) before the job was sent",
            outcome.terminal
        ));
    }
    let start = outcome.messages.len();
    let job_id = job.job_id.clone();
    let case = String::from_utf8_lossy(&job_id).into_owned();
    let _ = dispatch(tx, outcome, job, None).await;
    let phase = format!("{case}->refund");
    let refunded = read_until(inbound, outcome, &phase, |o| {
        let slice = o.messages.get(start..).unwrap_or_default();
        if stale {
            slice
                .iter()
                .any(|msg| matches!(msg, miner_msg::Msg::JobRequest(_)))
        } else {
            refunded_after_terminal(slice, &job_id)
        }
    })
    .await;
    if !refunded {
        return Err(barrier_failure(outcome, &phase));
    }
    let before = outcome.statuses.len();
    let _ = send(tx, coord_msg::Msg::Ping(Ping {})).await;
    let phase = format!("{case}->ping");
    if !read_until(inbound, outcome, &phase, |o| o.statuses.len() > before).await {
        return Err(barrier_failure(outcome, &phase));
    }
    Ok(start)
}

/// Run one job case and record its outcome. The job id is `"{name}/{subject}"`,
/// or `"{name}"` for a session case.
async fn run_job_case(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    (name, subject): (&'static str, Option<&str>),
    lease: &FamilyLease,
    grade: &Grade,
) {
    let job_id = subject.map_or_else(|| name.to_owned(), |subject| format!("{name}/{subject}"));
    let stale = matches!(grade, Grade::Stale);
    let graded = match run_job(inbound, tx, outcome, lease.job(&job_id), stale).await {
        Err(why) => CaseOutcome::Failed(why),
        Ok(start) => {
            let slice = outcome.messages.get(start..).unwrap_or_default();
            let job_id = job_id.as_bytes();
            match grade {
                Grade::Lease(expected) => grade_lease(slice, job_id, expected),
                Grade::Reject(want) => grade_reject(slice, job_id, *want),
                Grade::Stale => grade_stale(slice, job_id),
                Grade::Unavailable(why) => CaseOutcome::Failed(why.clone()),
            }
        }
    };
    push_case(outcome, name, subject, graded);
}

/// The constant name of a reference params set, for skip reasons.
fn params_name(params: &[u8]) -> String {
    for (name, known) in [
        ("P_SPEC", &P_SPEC[..]),
        ("P_L2_F0", &P_L2_F0[..]),
        ("P_L3_F0", &P_L3_F0[..]),
        ("P_F5000", &P_F5000[..]),
    ] {
        if params == known {
            return name.to_owned();
        }
    }
    let mut hex = String::new();
    for byte in params {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn show_admission(admission: &Result<SizeBounds, Code>) -> String {
    match admission {
        Ok(_) => "admitted".to_owned(),
        Err(code) => format!("code {}", code.value()),
    }
}

/// True when `lease` fails only on the advertised degree: `D` is over
/// `max_degree`, and `N` and `E` are within their claims.
fn degree_is_the_cause(
    caps: &Capabilities,
    reference: &ReferenceRevision,
    lease: &FamilyLease,
    topology: &BaseTopology,
) -> bool {
    let unbounded = Capabilities {
        max_degree: None,
        ..caps.clone()
    };
    let Ok(bounds) = lease.admission(&unbounded, reference, topology) else {
        return false;
    };
    caps.max_degree
        .is_some_and(|degree| bounds.degree > u64::from(degree))
}

/// The supported lease with the first reference params set whose admission
/// gives `code` and passes `extra`.
///
/// # Errors
///
/// The skip reason, naming what each candidate gives.
fn first_params_with(
    caps: &Capabilities,
    reference: &ReferenceRevision,
    supported: &FamilyLease,
    topology: &BaseTopology,
    code: Code,
    extra: &dyn Fn(&FamilyLease) -> bool,
) -> Result<FamilyLease, String> {
    let mut tried = Vec::new();
    for params in reference.params {
        let lease = supported.clone().with_params(params);
        let admission = lease.admission(caps, reference, topology);
        if admission == Err(code) && extra(&lease) {
            return Ok(lease);
        }
        let mut gives = show_admission(&admission);
        if admission == Err(code) {
            gives.push_str(" for another reason");
        }
        tried.push(format!("{} gives {gives}", params_name(params)));
    }
    Err(format!(
        "no reference params set gives code {} ({})",
        code.value(),
        tried.join(", ")
    ))
}

/// The ten per-revision cases of one subject, built from its supported lease.
fn revision_cases(
    caps: &Capabilities,
    reference: &ReferenceRevision,
    supported: &FamilyLease,
    topology: &BaseTopology,
    reordered_digest: &[u8],
) -> Vec<JobCase> {
    let variant = |params: &[u8]| supported.clone().with_params(params);
    let first_with = |code: Code, extra: &dyn Fn(&FamilyLease) -> bool| {
        first_params_with(caps, reference, supported, topology, code, extra)
    };
    let reject = |code: Code| Grade::Reject(admission_reason(code));
    let grade_supported = match expected_lease(reference, supported, topology) {
        Ok(expected) => Grade::Lease(expected),
        Err(why) => Grade::Unavailable(why),
    };
    vec![
        JobCase {
            name: "supported-revision",
            lease: Ok(supported.clone()),
            needs: None,
            grade: grade_supported,
        },
        JobCase {
            name: "revision-plus-one",
            lease: Ok(FamilyLease {
                revision: supported.revision.saturating_add(1),
                ..supported.clone()
            }),
            needs: Some(Code::RevisionUnsupported),
            grade: reject(Code::RevisionUnsupported),
        },
        JobCase {
            name: "params-malformed",
            lease: Ok(variant(&P_MALFORMED)),
            needs: Some(Code::ParamsMalformed),
            grade: reject(Code::ParamsMalformed),
        },
        JobCase {
            name: "params-digest-mismatch",
            lease: Ok(FamilyLease {
                params_digest: vec![0; 32],
                ..supported.clone()
            }),
            needs: Some(Code::ParamsDigestMismatch),
            grade: reject(Code::ParamsDigestMismatch),
        },
        JobCase {
            name: "params-out-of-bounds",
            lease: Ok(variant(reference.out_of_bounds_params)),
            needs: Some(Code::ParamsOutOfBounds),
            grade: reject(Code::ParamsOutOfBounds),
        },
        JobCase {
            name: "params-restricted",
            lease: first_with(Code::ParamsRestricted, &|_| true),
            needs: Some(Code::ParamsRestricted),
            grade: reject(Code::ParamsRestricted),
        },
        JobCase {
            name: "topology-order-mismatch",
            lease: Ok(FamilyLease {
                order_digest: reordered_digest.to_vec(),
                ..supported.clone()
            }),
            needs: Some(Code::TopologyOrderMismatch),
            grade: reject(Code::TopologyOrderMismatch),
        },
        JobCase {
            name: "degree-bound-too-large",
            lease: first_with(Code::BoundsExceeded, &|lease| {
                degree_is_the_cause(caps, reference, lease, topology)
            }),
            needs: Some(Code::BoundsExceeded),
            grade: reject(Code::BoundsExceeded),
        },
        JobCase {
            name: "stale-lease",
            lease: Ok(FamilyLease {
                generation: FAMILY_CANCEL_GENERATION,
                ..supported.clone()
            }),
            needs: None,
            grade: Grade::Stale,
        },
        JobCase {
            name: "deadline-expired",
            lease: Ok(FamilyLease {
                deadline_ms: 1,
                ..supported.clone()
            }),
            needs: None,
            grade: Grade::Reject(RejectReason::Expired as i32),
        },
    ]
}

/// Run every case of one advertised `(subject, reference)` pair.
async fn run_subject(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    caps: &Capabilities,
    subject: &str,
    reference: Option<&ReferenceRevision>,
) {
    let Some(reference) = reference else {
        let skipped = CaseOutcome::Skipped("no reference contract".to_owned());
        push_case(outcome, "supported-revision", Some(subject), skipped);
        return;
    };
    let topology = base_topology(false);
    let reordered = base_topology(true);
    let contract = reference.contract;
    let supported_with = |params: &[u8]| {
        FamilyLease::new(
            contract.family_id,
            contract.revision,
            params,
            topology.order_digest(),
        )
    };
    let Some(supported) = reference
        .params
        .iter()
        .map(|params| supported_with(params))
        .find(|lease| lease.admission(caps, reference, &topology).is_ok())
    else {
        for name in REVISION_CASES {
            let why = "no reference params set admits under the advertised limits";
            push_case(
                outcome,
                name,
                Some(subject),
                CaseOutcome::Skipped(why.to_owned()),
            );
        }
        return;
    };
    let cases = revision_cases(
        caps,
        reference,
        &supported,
        &topology,
        reordered.order_digest(),
    );
    for case in &cases {
        let lease = match &case.lease {
            Ok(lease) => lease,
            Err(why) => {
                let skipped = CaseOutcome::Skipped(why.clone());
                push_case(outcome, case.name, Some(subject), skipped);
                continue;
            }
        };
        let admission = lease.admission(caps, reference, &topology);
        let met = match (&admission, case.needs) {
            (Ok(_), None) => true,
            (Err(code), Some(needs)) => *code == needs,
            (Ok(_), Some(_)) | (Err(_), None) => false,
        };
        if met {
            let at = (case.name, Some(subject));
            run_job_case(inbound, tx, outcome, at, lease, &case.grade).await;
        } else {
            let needs = case.needs.map_or_else(
                || "admitted".to_owned(),
                |code| format!("code {}", code.value()),
            );
            let why = format!(
                "reference admission gives {}, the case needs {needs}",
                show_admission(&admission)
            );
            push_case(outcome, case.name, Some(subject), CaseOutcome::Skipped(why));
        }
    }
}

/// The session cases that send a family lease the binary must refuse.
async fn run_session_cases(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    caps: &Capabilities,
) {
    let order_digest = *base_topology(false).order_digest();
    let grade = Grade::Reject(admission_reason(Code::FamilyUnsupported));
    let generates = !caps.generated_families.is_empty();
    let name = "no-generated-families-rejects-family-lease";
    if generates {
        let why = "the binary lists generated families".to_owned();
        push_case(outcome, name, None, CaseOutcome::Skipped(why));
    } else {
        let lease = FamilyLease::new("vectors/uniform-field", 1, &P_L2_F0, &order_digest);
        run_job_case(inbound, tx, outcome, (name, None), &lease, &grade).await;
    }
    let name = "unknown-family";
    let lists_none = caps
        .generated_families
        .iter()
        .any(|entry| entry.family_id == "vectors/none");
    if !generates {
        let why = "the binary lists no generated families".to_owned();
        push_case(outcome, name, None, CaseOutcome::Skipped(why));
    } else if lists_none {
        let why = "the binary lists vectors/none".to_owned();
        push_case(outcome, name, None, CaseOutcome::Skipped(why));
    } else {
        let lease = FamilyLease::new("vectors/none", 1, &P_L2_F0, &order_digest);
        run_job_case(inbound, tx, outcome, (name, None), &lease, &grade).await;
    }
}

/// Grade the session setup: `missed` names each setup barrier that did not hold.
fn grade_setup(missed: &[String]) -> CaseOutcome {
    if missed.is_empty() {
        CaseOutcome::Passed
    } else {
        CaseOutcome::Failed(missed.join(", "))
    }
}

/// The family session script: handshake, setup, the cancel watermark, the
/// session cases, the cases of each advertised revision, the credit ledger,
/// and `Shutdown`.
pub(crate) async fn run_family_script(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
) -> SessionOutcome {
    let mut outcome = SessionOutcome::default();

    // 1. The Hello and its capabilities.
    let hello = read_hello(inbound, &mut outcome).await;
    let caps = outcome
        .hello
        .as_ref()
        .and_then(|hello| hello.capabilities.clone());
    let (version, generators) = grade_hello(caps.as_ref());
    push_case(&mut outcome, "hello-protocol-version-2", None, version);
    push_case(
        &mut outcome,
        "generators-list-family-iff-generated",
        None,
        generators,
    );
    let (true, Some(caps)) = (hello, caps) else {
        return outcome;
    };

    // 2. Welcome, configuration, the topology, and the target. The barrier is
    //    Ready plus the initial credit grant. Both setup barriers grade as
    //    the `session-setup` case.
    if !send(
        tx,
        coord_msg::Msg::Welcome(Welcome {
            protocol_version: 2,
        }),
    )
    .await
    {
        let missed = ["the miner left before the Welcome".to_owned()];
        push_case(&mut outcome, "session-setup", None, grade_setup(&missed));
        return outcome;
    }
    let mut missed = Vec::new();
    let _ = send(tx, coord_msg::Msg::Configure(configure())).await;
    let _ = send(tx, coord_msg::Msg::Topology(t_sparse_wire())).await;
    let target = SetTarget {
        max_energy_milli: i64::MAX,
        min_solutions: 1,
        max_proof_solutions: 32,
        num_reads: 1,
        num_sweeps: CONFIGURED_SWEEPS,
        ..SetTarget::default()
    };
    let _ = send(tx, coord_msg::Msg::SetTarget(target)).await;
    if !read_until(inbound, &mut outcome, "configure->ready", |o| {
        o.ready_received && !o.job_request_credits.is_empty()
    })
    .await
    {
        missed.push(barrier_failure(&outcome, "configure->ready"));
    }

    // 3. The cancel watermark the stale-lease case sits at.
    let before = outcome.statuses.len();
    let _ = send(
        tx,
        coord_msg::Msg::Cancel(Cancel {
            max_generation: FAMILY_CANCEL_GENERATION,
        }),
    )
    .await;
    if !read_until(inbound, &mut outcome, "cancel->status", |o| {
        o.statuses.len() > before
    })
    .await
    {
        missed.push(barrier_failure(&outcome, "cancel->status"));
    }
    push_case(&mut outcome, "session-setup", None, grade_setup(&missed));

    // 4. Every job case below runs behind a refund barrier and a Ping fence
    //    (`run_job`). 5. The session cases, then 6. the cases of each
    //    advertised revision.
    run_session_cases(inbound, tx, &mut outcome, &caps).await;
    for (subject, reference) in plan_subjects(&caps) {
        run_subject(inbound, tx, &mut outcome, &caps, &subject, reference).await;
    }

    // 7. Every dispatched job returned exactly one credit.
    let refunded = outcome.credits_refunded();
    let dispatched = outcome.jobs_dispatched as u64;
    let ledger = if dispatched > 0 && refunded == dispatched {
        CaseOutcome::Passed
    } else {
        CaseOutcome::Failed(format!(
            "{refunded} credits refunded for {dispatched} jobs dispatched"
        ))
    };
    push_case(&mut outcome, "credit-ledger-balanced", None, ledger);

    // 8. Shutdown ends the session.
    let _ = send(tx, coord_msg::Msg::Shutdown(Shutdown { grace_ms: 1000 })).await;
    drain_to_end(inbound, &mut outcome).await;
    outcome
}

/// Run the family gate against `bin_path`, passing `args` after the session
/// arguments. The CLI passes none.
pub async fn drive_miner_family(bin_path: &str, args: &[&str], socket: &str) -> FamilyReport {
    let (mut outcome, exit_code, stderr) = run_miner(
        bin_path,
        args,
        socket,
        ScriptKind::Family,
        family_child_timeout(),
    )
    .await;
    if outcome.family_cases.is_empty() {
        // The script graded nothing: the binary never connected, or its
        // session produced no outcome before the harness gave up on it.
        let (version, generators) = if exit_code == -1 || !outcome.connected {
            grade_hello(None)
        } else {
            let lost = CaseOutcome::Failed("the session produced no outcome".to_owned());
            (lost.clone(), lost)
        };
        push_case(&mut outcome, "hello-protocol-version-2", None, version);
        push_case(
            &mut outcome,
            "generators-list-family-iff-generated",
            None,
            generators,
        );
    }
    let clean = if exit_code == 0 && outcome.terminal == Terminal::Closed {
        CaseOutcome::Passed
    } else {
        CaseOutcome::Failed(format!(
            "exit code {exit_code}, stream ended {:?}",
            outcome.terminal
        ))
    };
    push_case(&mut outcome, "clean-exit", None, clean);
    FamilyReport {
        cases: outcome.family_cases,
        exit_code,
        stderr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::families::export_instances;
    use crate::families::layered::{P_F5000, P_L2_F0, P_L3_F0, P_SPEC, UNIFORM_V1};
    use ising_generators::descriptor::params_digest;
    use quip_proto::v1::{FamilySupport, JobRequest, LeaseDone, Reject, RejectReason, Solution};
    use quip_protocol::lease::Generator;
    use quip_protocol::wire::encode_spins_packed;

    const JOB: &[u8] = b"job-1";

    fn mock_caps() -> Capabilities {
        let support = |family_id: &str, digests: Vec<Vec<u8>>| FamilySupport {
            family_id: family_id.to_owned(),
            revisions: vec![1],
            params_digests: digests,
        };
        let list = vec![
            support("vectors/uniform-field", vec![]),
            support(
                "vectors/restricted-field",
                vec![params_digest(&P_SPEC).to_vec()],
            ),
        ];
        Capabilities {
            max_degree: Some(4),
            generators: vec![1, 2],
            protocol_version: 2,
            generated_families: list.clone(),
            accepted_families: list,
            ..Capabilities::default()
        }
    }

    fn t_sparse(swap_first_edges: bool) -> BaseTopology {
        let nodes = vec![907, 41, 0, 4_294_967_295, 12, 3];
        let mut edges = vec![
            (907, 41),
            (41, 0),
            (0, 4_294_967_295),
            (4_294_967_295, 12),
            (12, 3),
            (3, 907),
            (41, 12),
        ];
        if swap_first_edges {
            edges.swap(0, 1);
        }
        BaseTopology::new(nodes, &edges).expect("T_SPARSE is a valid topology")
    }

    fn admission(
        caps: &Capabilities,
        family: &str,
        revision: u32,
        params: &[u8],
        digest: &[u8],
        order: &[u8],
    ) -> Option<u8> {
        // Revision 2 has no reference, so admission runs against revision 1.
        let reference = reference_revision(family, 1).expect("a reference revision exists");
        reference_admission(
            caps,
            reference,
            family,
            revision,
            params,
            digest,
            &t_sparse(false),
            order,
        )
        .err()
        .map(Code::value)
    }

    fn msg_reject(reason: RejectReason) -> miner_msg::Msg {
        miner_msg::Msg::Reject(Reject {
            job_id: JOB.to_vec(),
            reason: reason as i32,
        })
    }

    fn msg_request(credits: u32) -> miner_msg::Msg {
        miner_msg::Msg::JobRequest(JobRequest { credits })
    }

    fn lease_spec() -> LeaseSpec {
        let mut base_salt = [0x33; 32];
        base_salt[..8].fill(0);
        LeaseSpec::new(
            Generator::FamilyChacha8V1,
            [0x11; 32],
            [0x22; 32],
            base_salt,
            5,
            8,
        )
        .expect("a valid lease")
    }

    fn expected_lease() -> ExpectedLease {
        let spec = lease_spec();
        let nonces: Vec<[u8; 32]> = (0..8)
            .map(|i| spec.nonce(i).expect("slot in the lease"))
            .collect();
        let salts = export_instances(&UNIFORM_V1, &P_SPEC, &t_sparse(false), &nonces)
            .expect("P_SPEC admits");
        ExpectedLease { spec, salts }
    }

    fn result_of(expected: &ExpectedLease, slot: u64, with_offset: bool) -> miner_msg::Msg {
        let instance = expected
            .salts
            .get(usize::try_from(slot).expect("small"))
            .and_then(|salt| salt.as_ref().ok())
            .expect("the slot generated an instance");
        let spins = vec![1_i8; instance.node_ids().len()];
        let edges: Vec<(usize, usize)> = instance
            .edges()
            .iter()
            .map(|&(a, b)| (a as usize, b as usize))
            .collect();
        let mut energy = energy_from_milli(&spins, instance.h(), instance.j(), &edges);
        if with_offset {
            energy += instance.offset_milli();
        }
        miner_msg::Msg::Result(quip_proto::v1::Result {
            job_id: JOB.to_vec(),
            solutions: vec![Solution {
                energy_milli: energy,
                spins: encode_spins_packed(&spins),
            }],
            salt: expected.spec.salt(slot).expect("in lease").to_vec(),
            nonce: expected.spec.nonce(slot).expect("in lease").to_vec(),
            ..quip_proto::v1::Result::default()
        })
    }

    fn lease_done(expected: &ExpectedLease, salts_done: u64) -> miner_msg::Msg {
        let mut best = i64::MAX;
        for slot in [1, 2, 4, 5, 6, 7] {
            if let miner_msg::Msg::Result(result) = result_of(expected, slot, true) {
                for solution in result.solutions {
                    best = best.min(solution.energy_milli);
                }
            }
        }
        miner_msg::Msg::LeaseDone(LeaseDone {
            job_id: JOB.to_vec(),
            salts_done,
            best_energy_milli: best,
        })
    }

    fn good_transcript(expected: &ExpectedLease) -> Vec<miner_msg::Msg> {
        let mut slice: Vec<_> = [1, 2, 4, 5, 6, 7]
            .into_iter()
            .map(|slot| result_of(expected, slot, true))
            .collect();
        slice.push(lease_done(expected, 8));
        slice.push(msg_request(1));
        slice
    }

    fn failed(outcome: &CaseOutcome) -> bool {
        matches!(outcome, CaseOutcome::Failed(_))
    }

    #[test]
    fn protocol_version_must_be_two() {
        let mut caps = mock_caps();
        assert_eq!(grade_hello(Some(&caps)).0, CaseOutcome::Passed);
        caps.protocol_version = 1;
        assert!(failed(&grade_hello(Some(&caps)).0));
        assert_eq!(
            grade_hello(None),
            (
                CaseOutcome::Failed("no Hello".to_owned()),
                CaseOutcome::Failed("no Hello".to_owned())
            )
        );
    }

    #[test]
    fn generators_must_list_family_iff_generated() {
        let mut caps = mock_caps();
        for (lists, generates, passes) in [
            (true, true, true),
            (false, false, true),
            (true, false, false),
            (false, true, false),
        ] {
            caps.generators = if lists { vec![1, 2] } else { vec![1] };
            caps.generated_families = if generates {
                mock_caps().generated_families
            } else {
                vec![]
            };
            let graded = grade_hello(Some(&caps)).1;
            assert_eq!(graded == CaseOutcome::Passed, passes, "{lists} {generates}");
        }
    }

    #[test]
    fn a_reject_with_its_refund_passes() {
        let slice = [msg_reject(RejectReason::Malformed), msg_request(1)];
        let want = RejectReason::Malformed as i32;
        assert_eq!(grade_reject(&slice, JOB, want), CaseOutcome::Passed);
    }

    #[test]
    fn a_reject_without_a_refund_fails() {
        let slice = [msg_reject(RejectReason::Malformed)];
        let want = RejectReason::Malformed as i32;
        assert!(failed(&grade_reject(&slice, JOB, want)));
    }

    #[test]
    fn a_wrong_reject_reason_fails() {
        let slice = [msg_reject(RejectReason::Malformed), msg_request(1)];
        let want = RejectReason::Unspecified as i32;
        assert!(failed(&grade_reject(&slice, JOB, want)));
    }

    #[test]
    fn a_second_refund_fails_the_case() {
        let slice = [
            msg_reject(RejectReason::Malformed),
            msg_request(1),
            msg_request(1),
        ];
        let want = RejectReason::Malformed as i32;
        assert!(failed(&grade_reject(&slice, JOB, want)));
    }

    #[test]
    fn a_correct_lease_transcript_passes() {
        let expected = expected_lease();
        let skipped: Vec<usize> = expected
            .salts
            .iter()
            .enumerate()
            .filter(|(_, salt)| salt.is_err())
            .map(|(slot, _)| slot)
            .collect();
        assert_eq!(skipped, [0, 3]);
        let outcome = grade_lease(&good_transcript(&expected), JOB, &expected);
        assert_eq!(outcome, CaseOutcome::Passed);
    }

    #[test]
    fn a_lease_result_without_the_offset_fails() {
        let expected = expected_lease();
        let mut slice = good_transcript(&expected);
        *slice.first_mut().expect("non-empty") = result_of(&expected, 1, false);
        assert!(failed(&grade_lease(&slice, JOB, &expected)));
    }

    #[test]
    fn a_result_for_a_skipped_salt_fails() {
        let expected = expected_lease();
        let mut slice = good_transcript(&expected);
        let mut stray = result_of(&expected, 1, true);
        if let miner_msg::Msg::Result(result) = &mut stray {
            result.salt = expected.spec.salt(0).expect("in lease").to_vec();
            result.nonce = expected.spec.nonce(0).expect("in lease").to_vec();
        }
        slice.insert(0, stray);
        assert!(failed(&grade_lease(&slice, JOB, &expected)));
    }

    #[test]
    fn a_lease_done_with_a_wrong_count_fails() {
        let expected = expected_lease();
        let mut slice = good_transcript(&expected);
        *slice.get_mut(6).expect("the LeaseDone") = lease_done(&expected, 7);
        assert!(failed(&grade_lease(&slice, JOB, &expected)));
    }

    #[test]
    fn a_stale_lease_with_a_result_fails() {
        let expected = expected_lease();
        let slice = [result_of(&expected, 1, true), msg_request(1)];
        assert!(failed(&grade_stale(&slice, JOB)));
    }

    #[test]
    fn a_stale_lease_with_only_a_refund_passes() {
        assert_eq!(grade_stale(&[msg_request(1)], JOB), CaseOutcome::Passed);
    }

    #[test]
    fn reference_admission_follows_fam_47() {
        let caps = mock_caps();
        let uniform = "vectors/uniform-field";
        let restricted = "vectors/restricted-field";
        let order = *t_sparse(false).order_digest();
        let digest = |params: &[u8]| params_digest(params);
        let spec = admission(&caps, uniform, 1, &P_SPEC, &digest(&P_SPEC), &order);
        assert_eq!(spec, None);
        assert_eq!(
            admission(&caps, uniform, 2, &P_SPEC, &digest(&P_SPEC), &order),
            Some(6)
        );
        let mut vectors_none = mock_caps();
        vectors_none.generated_families = vec![];
        assert_eq!(
            admission(&vectors_none, uniform, 1, &P_SPEC, &digest(&P_SPEC), &order),
            Some(5)
        );
        assert_eq!(
            admission(&caps, uniform, 1, &[2], &digest(&[2]), &order),
            Some(7)
        );
        assert_eq!(
            admission(&caps, uniform, 1, &P_SPEC, &[0; 32], &order),
            Some(8)
        );
        assert_eq!(
            admission(&caps, uniform, 1, &P_F5000, &digest(&P_F5000), &order),
            Some(9)
        );
        assert_eq!(
            admission(&caps, restricted, 1, &P_L2_F0, &digest(&P_L2_F0), &order),
            Some(10)
        );
        let reordered = *t_sparse(true).order_digest();
        assert_eq!(
            admission(&caps, uniform, 1, &P_SPEC, &digest(&P_SPEC), &reordered),
            Some(11)
        );
        assert_eq!(
            admission(&caps, uniform, 1, &P_L3_F0, &digest(&P_L3_F0), &order),
            Some(13)
        );
        let mut unbounded = mock_caps();
        unbounded.max_degree = None;
        assert_eq!(
            admission(&unbounded, uniform, 1, &P_L3_F0, &digest(&P_L3_F0), &order),
            None
        );
    }

    #[test]
    fn advertised_revisions_without_a_reference_are_skipped() {
        let mut caps = mock_caps();
        caps.generated_families.push(FamilySupport {
            family_id: "vectors/other".to_owned(),
            revisions: vec![1],
            params_digests: vec![],
        });
        let subjects = plan_subjects(&caps);
        let names: Vec<&str> = subjects.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "vectors/uniform-field@1",
                "vectors/restricted-field@1",
                "vectors/other@1"
            ]
        );
        let references: Vec<bool> = subjects.iter().map(|(_, r)| r.is_some()).collect();
        assert_eq!(references, [true, true, false]);
    }

    fn case(name: &'static str, outcome: CaseOutcome) -> FamilyCase {
        FamilyCase {
            name,
            subject: Some("vectors/uniform-field@1".to_owned()),
            outcome,
        }
    }

    #[test]
    fn skips_do_not_fail_the_report() {
        let report = FamilyReport {
            cases: vec![
                case("a", CaseOutcome::Passed),
                case("b", CaseOutcome::Skipped("no reference".to_owned())),
            ],
            ..FamilyReport::default()
        };
        assert!(report.is_conformant());
        let only_skips = FamilyReport {
            cases: vec![case("b", CaseOutcome::Skipped("no reference".to_owned()))],
            ..FamilyReport::default()
        };
        assert!(!only_skips.is_conformant());
    }

    #[test]
    fn a_failed_case_fails_the_report() {
        let report = FamilyReport {
            cases: vec![
                case("a", CaseOutcome::Passed),
                case("b", CaseOutcome::Failed("why".to_owned())),
            ],
            ..FamilyReport::default()
        };
        assert!(!report.is_conformant());
        assert!(!FamilyReport::default().is_conformant());
    }

    #[test]
    fn summary_marks_each_case() {
        let report = FamilyReport {
            cases: vec![
                case("a", CaseOutcome::Passed),
                case("b", CaseOutcome::Failed("why".to_owned())),
                FamilyCase {
                    name: "c",
                    subject: None,
                    outcome: CaseOutcome::Skipped("because".to_owned()),
                },
            ],
            ..FamilyReport::default()
        };
        assert_eq!(
            report.summary(),
            "  [pass] a vectors/uniform-field@1\n  [FAIL] b vectors/uniform-field@1: why\n  [skip] c: because\n"
        );
        assert_eq!(
            report.outcome("b", Some("vectors/uniform-field@1")),
            Some(&CaseOutcome::Failed("why".to_owned()))
        );
        assert_eq!(report.outcome("b", None), None);
    }

    fn lease_failure(slice: &[miner_msg::Msg], expected: &ExpectedLease) -> bool {
        failed(&grade_lease(slice, JOB, expected))
    }

    fn with_result(
        expected: &ExpectedLease,
        edit: impl FnOnce(&mut quip_proto::v1::Result),
    ) -> Vec<miner_msg::Msg> {
        let mut slice = good_transcript(expected);
        if let Some(miner_msg::Msg::Result(result)) = slice.first_mut() {
            edit(result);
        }
        slice
    }

    #[test]
    fn a_result_after_the_lease_done_fails() {
        let expected = expected_lease();
        let mut slice = good_transcript(&expected);
        slice.insert(7, result_of(&expected, 1, true));
        assert!(lease_failure(&slice, &expected));
    }

    #[test]
    fn a_repeated_salt_fails() {
        let expected = expected_lease();
        let mut slice = good_transcript(&expected);
        slice.insert(0, result_of(&expected, 1, true));
        assert!(lease_failure(&slice, &expected));
    }

    #[test]
    fn a_nonce_mismatch_fails() {
        let expected = expected_lease();
        let slice = with_result(&expected, |result| result.nonce = vec![0; 32]);
        assert!(lease_failure(&slice, &expected));
    }

    #[test]
    fn a_generated_slot_without_a_result_fails() {
        let expected = expected_lease();
        let mut slice = good_transcript(&expected);
        let _ = slice.remove(0);
        assert!(lease_failure(&slice, &expected));
    }

    #[test]
    fn a_wrong_best_energy_fails() {
        let expected = expected_lease();
        let mut slice = good_transcript(&expected);
        if let Some(miner_msg::Msg::LeaseDone(done)) = slice.get_mut(6) {
            done.best_energy_milli += 1;
        }
        assert!(lease_failure(&slice, &expected));
    }

    #[test]
    fn undecodable_spins_fail() {
        let expected = expected_lease();
        let slice = with_result(&expected, |result| {
            if let Some(solution) = result.solutions.first_mut() {
                solution.spins = vec![];
            }
        });
        assert!(lease_failure(&slice, &expected));
    }

    #[test]
    fn a_stale_lease_done_with_work_fails() {
        let done = |salts_done| {
            miner_msg::Msg::LeaseDone(LeaseDone {
                job_id: JOB.to_vec(),
                salts_done,
                best_energy_milli: i64::MAX,
            })
        };
        assert!(failed(&grade_stale(&[done(1), msg_request(1)], JOB)));
        assert_eq!(
            grade_stale(&[done(0), msg_request(1)], JOB),
            CaseOutcome::Passed
        );
        assert!(failed(&grade_stale(
            &[done(0), done(0), msg_request(1)],
            JOB
        )));
    }

    #[test]
    fn a_reject_in_a_stale_slice_fails() {
        let slice = [msg_reject(RejectReason::Malformed), msg_request(1)];
        assert!(failed(&grade_stale(&slice, JOB)));
    }

    #[test]
    fn a_result_in_a_reject_slice_fails() {
        let expected = expected_lease();
        let slice = [
            msg_reject(RejectReason::Malformed),
            result_of(&expected, 1, true),
            msg_request(1),
        ];
        let want = RejectReason::Malformed as i32;
        assert!(failed(&grade_reject(&slice, JOB, want)));
    }

    #[test]
    fn a_missed_setup_barrier_fails_session_setup() {
        assert_eq!(grade_setup(&[]), CaseOutcome::Passed);
        let missed = ["phase configure->ready timed out".to_owned()];
        assert_eq!(
            grade_setup(&missed),
            CaseOutcome::Failed("phase configure->ready timed out".to_owned())
        );
        let both = [
            "phase configure->ready timed out".to_owned(),
            "phase cancel->status timed out".to_owned(),
        ];
        assert_eq!(
            grade_setup(&both),
            CaseOutcome::Failed(
                "phase configure->ready timed out, phase cancel->status timed out".to_owned()
            )
        );
    }

    #[test]
    fn the_child_budget_covers_every_reference_revision() {
        // 8 session barriers, 20 per reference revision, 1 phase of slack.
        let phases = 8 + 20 * REFERENCE_REVISIONS.len() + 1;
        let phases = u32::try_from(phases).expect("small");
        assert_eq!(family_child_timeout(), PHASE_TIMEOUT * phases);
    }

    #[test]
    fn an_accepted_only_pair_has_no_subject() {
        let mut caps = mock_caps();
        caps.generated_families = vec![];
        assert!(plan_subjects(&caps).is_empty());
    }
}
