//! The `--family` gate: the session script, its report, and its graders. The
//! script runs on the mock coordinator and records the transcript of each
//! case. The graders are pure functions over that transcript, so unit tests
//! feed them hand-built transcripts and need no binary.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::slice::from_ref;
use std::time::Duration;

use ising_generators::descriptor::{
    admit, check_family_id, params_digest, AdvertisedLimits, Contract, IdContext, SizeBounds,
};
use ising_generators::family::{admit_topology, BaseTopology, SaltFault};
use ising_generators::instance::{Instance, InstanceParts};
use ising_generators::Code;
use quip_proto::v1::{
    coord_msg, ising_problem::Graph, miner_msg, Cancel, Capabilities, CoefficientEncoding,
    CoordMsg, EdgeList, FamilyBinding, FamilyDescriptor, GeneratorAlgorithm, IsingProblem,
    IsingProblemGenerator, Job, JobKind, MinerMsg, Ping, RejectReason, SetTarget, Shutdown,
    Topology, Welcome,
};
use quip_protocol::family::{reject_reason, support_code, CodePhase};
use quip_protocol::lease::{Generator, LeaseSpec};
use quip_protocol::scoring::energy_from_milli;
use quip_protocol::wire::{decode_spins_packed, encode_i32_le};
use tokio::sync::mpsc;
use tonic::{Status, Streaming};

use super::charge::{P_CHARGED_A251, P_CHARGED_ADJ_A100_B45_C256, P_CHARGED_Z22_A250_B31_C128};
use super::fields::{P_FIELDS_ADJ_G45_F1000_D16, P_FIELDS_F1001, P_FIELDS_Z22_G125_F500_D16};
use super::ice::{P_ICE_ADJ_125, P_ICE_GLASS_1000, P_ICE_Z12_200};
use super::layered::P_L2_F0;
use super::layered::{P_F5000, P_L3_F0, P_SPEC};
use super::polariton::{P_POLARITON_SIDE1, P_POLARITON_Z24_L2_P0, P_POLARITON_Z34_L3_P2};
use super::routing::{P_ROUTING_D3, P_ROUTING_Z24_D4, P_ROUTING_Z34_D6};
use super::twisted::{P_TWISTED_ADJ_W45_P6, P_TWISTED_W1000, P_TWISTED_Z22_W62_P8};
use super::{
    export_instances, reference_revision, ReferenceRevision, REFERENCE_REVISIONS, T_SPARSE_TOPOLOGY,
};
use crate::driver::{
    configure, dispatch, drain_to_end, read_hello, read_until, run_miner, send, ScriptKind,
    SessionOutcome, Terminal, CONFIGURED_SWEEPS, PHASE_TIMEOUT, STDERR_DRAIN_TIMEOUT,
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

/// `family_id@revision` for each pair in `accepted_families` that the gate
/// has a reference revision for, in `Hello` order, without repeats. The gate
/// sends these pairs explicit family jobs (FAM-65). A pair with no reference
/// revision has no cases, because the gate cannot build its instance.
pub(crate) fn plan_explicit_subjects(
    caps: &Capabilities,
) -> Vec<(String, &'static ReferenceRevision)> {
    let mut subjects: Vec<(String, &'static ReferenceRevision)> = Vec::new();
    for entry in &caps.accepted_families {
        for &revision in &entry.revisions {
            let subject = format!("{}@{revision}", entry.family_id);
            let Some(reference) = reference_revision(&entry.family_id, revision) else {
                continue;
            };
            if subjects.iter().all(|(known, _)| *known != subject) {
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

/// The edges of `instance` as the scorer takes them.
fn instance_edges(instance: &Instance) -> Result<Vec<(usize, usize)>, std::num::TryFromIntError> {
    instance
        .edges()
        .iter()
        .map(|&(a, b)| Ok((usize::try_from(a)?, usize::try_from(b)?)))
        .collect()
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
    let edges = instance_edges(instance)
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

/// Grade a solved explicit job: one `Result` whose solutions score, on the plain
/// model and without the offset, the energy they carry (FAM-64), then one refund.
pub(crate) fn grade_explicit_solved(
    slice: &[miner_msg::Msg],
    job_id: &[u8],
    instance: &Instance,
) -> CaseOutcome {
    outcome_of(check_explicit_solved(slice, job_id, instance))
}

fn check_explicit_solved(
    slice: &[miner_msg::Msg],
    job_id: &[u8],
    instance: &Instance,
) -> Result<(), String> {
    let edges = instance_edges(instance)
        .map_err(|_| "an edge of the instance does not fit in usize".to_owned())?;
    let mut result_at = None;
    for (at, msg) in slice.iter().enumerate() {
        match msg {
            miner_msg::Msg::Reject(reject) if reject.job_id == job_id => {
                return Err(format!(
                    "Reject with reason {} for a valid explicit job",
                    reject.reason
                ));
            }
            miner_msg::Msg::LeaseDone(done) if done.job_id == job_id => {
                return Err("a LeaseDone followed an explicit job".to_owned());
            }
            miner_msg::Msg::Result(result) if result.job_id == job_id => {
                if result_at.is_some() {
                    return Err("a second Result for an explicit job".to_owned());
                }
                result_at = Some(at);
                check_explicit_result(result, instance, &edges)?;
            }
            _ => {}
        }
    }
    let Some(result_at) = result_at else {
        return Err("no Result for a valid explicit job".to_owned());
    };
    check_refund(slice, Some(result_at))
}

fn check_explicit_result(
    result: &quip_proto::v1::Result,
    instance: &Instance,
    edges: &[(usize, usize)],
) -> Result<(), String> {
    if result.solutions.is_empty() {
        return Err("a Result without a solution".to_owned());
    }
    for solution in &result.solutions {
        let spins = decode_spins_packed(&solution.spins, instance.node_ids().len())
            .map_err(|fault| format!("spins do not decode: {fault:?}"))?;
        let want = energy_from_milli(&spins, instance.h(), instance.j(), edges);
        if solution.energy_milli != want {
            return Err(format!(
                "energy_milli is {}, the plain model scores {want} without the offset",
                solution.energy_milli
            ));
        }
    }
    Ok(())
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
/// Each reference revision runs at most `REVISION_CASES.len()` lease cases and
/// `EXPLICIT_CASES.len()` explicit cases of two barriers each. With 15 cases
/// and 6 reference revisions the budget is `(8 + 6 * 30 + 1) * 10 s`, the
/// `+ 1` being one phase of slack so the script ends before the child is
/// killed.
fn family_child_timeout() -> Duration {
    let cases = REVISION_CASES.len() + EXPLICIT_CASES.len();
    let per_revision = u32::try_from(cases * 2).unwrap_or(u32::MAX);
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

/// The explicit-job cases of each accepted revision, in run order.
const EXPLICIT_CASES: [&str; 5] = [
    "explicit-supported",
    "explicit-digest-mismatch",
    "explicit-metadata-invalid",
    "explicit-params-out-of-bounds",
    "explicit-topology-hash-form",
];

fn base_salt() -> [u8; 32] {
    let mut salt = [0x33; 32];
    salt[..8].fill(0);
    salt
}

/// A base topology in the forms the script needs: the stored ids, the
/// topology itself, its reordered twin (the first two edges swapped), and the
/// hash the session caches it under.
struct GateTopology {
    stored: ising_generators::blob::Topology,
    plain: BaseTopology,
    reordered: BaseTopology,
    hash: [u8; 32],
}

impl GateTopology {
    /// Decode the topology file that `reference` names. `T_SPARSE` keeps the
    /// hash `FAMILY_TOPOLOGY_HASH`. Any other topology uses its own order digest,
    /// so each distinct topology has a distinct hash.
    ///
    /// # Errors
    ///
    /// A message when the embedded file does not decode or is not valid.
    fn load(reference: &str) -> Result<Self, String> {
        let stored = crate::topology(reference)
            .map_err(|why| format!("{reference} failed to load: {why}"))?;
        let build = |swap: bool| {
            let mut edges = stored.edges.clone();
            if swap {
                edges.swap(0, 1);
            }
            BaseTopology::new(stored.nodes.clone(), &edges)
                .map_err(|why| format!("{reference} is not a valid topology: {why:?}"))
        };
        let plain = build(false)?;
        let reordered = build(true)?;
        let hash = if reference == T_SPARSE_TOPOLOGY {
            FAMILY_TOPOLOGY_HASH
        } else {
            *plain.order_digest()
        };
        Ok(Self {
            stored,
            plain,
            reordered,
            hash,
        })
    }

    fn wire(&self) -> Topology {
        Topology {
            hash: self.hash.to_vec(),
            nodes: self.stored.nodes.clone(),
            edges: Some(EdgeList {
                u: self.stored.edges.iter().map(|&(u, _)| u).collect(),
                v: self.stored.edges.iter().map(|&(_, v)| v).collect(),
            }),
            allowed_h_milli: vec![-1000, 0, 1000],
            allowed_j_milli: vec![-1000, 1000],
        }
    }
}

/// The FAM-42 wire reason of an admission code, as a raw `i32`.
fn admission_reason(code: Code) -> i32 {
    // Codes 5 to 13 all have an admission reason. `Unspecified` never matches
    // a conformant reject, so a gap here fails the case instead of passing it.
    reject_reason(code, CodePhase::Admission)
        .map_or(RejectReason::Unspecified as i32, |reason| reason as i32)
}

/// The FAM-42 wire reason of an explicit-job code, as a raw `i32`.
fn explicit_reason(code: Code) -> i32 {
    reject_reason(code, CodePhase::ExplicitJob)
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
    topology_hash: [u8; 32],
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
            topology_hash: FAMILY_TOPOLOGY_HASH,
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
                topology_hash: self.topology_hash.to_vec(),
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

/// The gate lease inputs and the nonce of each of its slots.
fn lease_inputs() -> Result<(LeaseSpec, Vec<[u8; 32]>), String> {
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
    Ok((spec, nonces))
}

/// The expected lease of `lease` regenerated with `reference`.
fn expected_lease(
    reference: &ReferenceRevision,
    lease: &FamilyLease,
    topology: &BaseTopology,
) -> Result<ExpectedLease, String> {
    let (spec, nonces) = lease_inputs()?;
    let salts = export_instances(reference.contract, &lease.params, topology, &nonces)
        .map_err(|code| format!("the reference does not regenerate: code {}", code.value()))?;
    Ok(ExpectedLease { spec, salts })
}

/// True when a `JobRequest` follows the first `Reject` or `LeaseDone` for
/// `job_id`. For an explicit job a `Result` is also terminal, because the job
/// has no `LeaseDone`.
fn refunded_after_terminal(slice: &[miner_msg::Msg], job_id: &[u8], explicit: bool) -> bool {
    let terminal = slice.iter().position(|msg| match msg {
        miner_msg::Msg::Reject(reject) => reject.job_id == job_id,
        miner_msg::Msg::LeaseDone(done) => done.job_id == job_id,
        miner_msg::Msg::Result(result) => explicit && result.job_id == job_id,
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

/// What ends a job before its refund counts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Await {
    /// A `Reject` or the `LeaseDone` of a lease.
    Lease,
    /// Any refund: the job gets no other message.
    Stale,
    /// A `Reject` or the `Result` of an explicit job.
    Explicit,
}

/// Dispatch `job`, wait for its refund and then a `Ping` fence, and return
/// where its transcript starts in `outcome.messages`.
async fn run_job(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    job: Job,
    ends: Await,
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
        if ends == Await::Stale {
            slice
                .iter()
                .any(|msg| matches!(msg, miner_msg::Msg::JobRequest(_)))
        } else {
            refunded_after_terminal(slice, &job_id, ends == Await::Explicit)
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
    let ends = if matches!(grade, Grade::Stale) {
        Await::Stale
    } else {
        Await::Lease
    };
    let graded = match run_job(inbound, tx, outcome, lease.job(&job_id), ends).await {
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
        ("P_ICE_ADJ_125", &P_ICE_ADJ_125[..]),
        ("P_ICE_Z12_200", &P_ICE_Z12_200[..]),
        ("P_ICE_GLASS_1000", &P_ICE_GLASS_1000[..]),
        ("P_TWISTED_Z22_W62_P8", &P_TWISTED_Z22_W62_P8[..]),
        ("P_TWISTED_ADJ_W45_P6", &P_TWISTED_ADJ_W45_P6[..]),
        ("P_TWISTED_W1000", &P_TWISTED_W1000[..]),
        (
            "P_CHARGED_Z22_A250_B31_C128",
            &P_CHARGED_Z22_A250_B31_C128[..],
        ),
        (
            "P_CHARGED_ADJ_A100_B45_C256",
            &P_CHARGED_ADJ_A100_B45_C256[..],
        ),
        ("P_CHARGED_A251", &P_CHARGED_A251[..]),
        (
            "P_FIELDS_Z22_G125_F500_D16",
            &P_FIELDS_Z22_G125_F500_D16[..],
        ),
        (
            "P_FIELDS_ADJ_G45_F1000_D16",
            &P_FIELDS_ADJ_G45_F1000_D16[..],
        ),
        ("P_FIELDS_F1001", &P_FIELDS_F1001[..]),
        ("P_ROUTING_Z24_D4", &P_ROUTING_Z24_D4[..]),
        ("P_ROUTING_Z34_D6", &P_ROUTING_Z34_D6[..]),
        ("P_ROUTING_D3", &P_ROUTING_D3[..]),
        ("P_POLARITON_Z24_L2_P0", &P_POLARITON_Z24_L2_P0[..]),
        ("P_POLARITON_Z34_L3_P2", &P_POLARITON_Z34_L3_P2[..]),
        ("P_POLARITON_SIDE1", &P_POLARITON_SIDE1[..]),
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

/// The supported lease at the next revision.
///
/// # Errors
///
/// The skip reason when the binary lists the next revision, because it then
/// may generate it.
fn next_revision_lease(
    caps: &Capabilities,
    supported: &FamilyLease,
) -> Result<FamilyLease, String> {
    let next = supported.revision.saturating_add(1);
    let listed = caps
        .generated_families
        .iter()
        .any(|entry| entry.family_id == supported.family_id && entry.revisions.contains(&next));
    if listed {
        return Err(format!("the binary lists revision {next}"));
    }
    Ok(FamilyLease {
        revision: next,
        ..supported.clone()
    })
}

/// The FAM-61 rule that the `generated_families` entry of `family_id`
/// breaks, if any. An entry that breaks FAM-61 claims nothing.
fn fam61_fault(caps: &Capabilities, family_id: &str) -> Option<String> {
    let entries: Vec<_> = caps
        .generated_families
        .iter()
        .filter(|entry| entry.family_id == family_id)
        .collect();
    let [entry] = entries.as_slice() else {
        return Some(format!(
            "generated_families lists {family_id} {} times, so the list claims nothing for it (FAM-61)",
            entries.len()
        ));
    };
    if check_family_id(family_id.as_bytes(), IdContext::Lease).is_err() {
        return Some(format!(
            "the family_id {family_id} breaks FAM-01 or FAM-02 or uses the example namespace (FAM-61)"
        ));
    }
    if entry.revisions.is_empty() {
        return Some(format!(
            "the entry for {family_id} lists no revision (FAM-61)"
        ));
    }
    if entry.revisions.contains(&0) {
        return Some(format!(
            "the entry for {family_id} lists revision 0 (FAM-61)"
        ));
    }
    if entry.params_digests.iter().any(|digest| digest.len() != 32) {
        return Some(format!(
            "the entry for {family_id} has a params digest that is not 32 bytes (FAM-61)"
        ));
    }
    None
}

/// The cases to record when the gate cannot drive a listed revision:
/// `supported-revision` fails, and the other cases of a revision with a
/// reference are skipped with the same reason.
fn unavailable_cases(has_reference: bool, why: &str) -> Vec<(&'static str, CaseOutcome)> {
    let mut cases = vec![("supported-revision", CaseOutcome::Failed(why.to_owned()))];
    if has_reference {
        for name in REVISION_CASES.iter().skip(1) {
            cases.push((*name, CaseOutcome::Skipped(why.to_owned())));
        }
    }
    cases
}

/// The supported lease of a listed subject, or the cases to record when the
/// gate cannot drive it. A listed revision that the gate cannot drive fails
/// `supported-revision` (FAM-57). The other cases of a revision with a
/// reference are skipped with the same reason.
fn select_supported<'r>(
    caps: &Capabilities,
    subject: &str,
    reference: Option<&'r ReferenceRevision>,
    topology: &BaseTopology,
) -> Result<(FamilyLease, &'r ReferenceRevision), Vec<(&'static str, CaseOutcome)>> {
    let family_id = subject.rsplit_once('@').map_or(subject, |(id, _)| id);
    let cases_for = |why: String| unavailable_cases(reference.is_some(), &why);
    if let Some(fault) = fam61_fault(caps, family_id) {
        return Err(cases_for(fault));
    }
    let Some(reference) = reference else {
        return Err(cases_for("no reference contract".to_owned()));
    };
    let contract = reference.contract;
    let mut tried = Vec::new();
    for params in reference.params {
        let lease = FamilyLease::new(
            contract.family_id,
            contract.revision,
            params,
            topology.order_digest(),
        );
        let admission = lease.admission(caps, reference, topology);
        if admission.is_ok() {
            return Ok((lease, reference));
        }
        tried.push(format!(
            "{} gives {}",
            params_name(params),
            show_admission(&admission)
        ));
    }
    Err(cases_for(format!(
        "no reference params set admits under the advertised limits ({})",
        tried.join(", ")
    )))
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
            lease: next_revision_lease(caps, supported),
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

/// The inputs of one explicit family job that the cases vary.
#[derive(Clone, Debug)]
struct ExplicitJob {
    family: FamilyDescriptor,
    nonce: Vec<u8>,
    instance_digest: Vec<u8>,
    metadata: Vec<u8>,
    order_digest: Vec<u8>,
    node_ids: Vec<u32>,
    offset_milli: i64,
    graph: Graph,
    h: Vec<i32>,
    j: Vec<i32>,
}

impl ExplicitJob {
    /// The valid job of `instance`, bound to `family_id@revision` and `params`.
    fn new(contract: &Contract, params: &[u8], instance: &Instance) -> Self {
        Self {
            family: FamilyDescriptor {
                family_id: contract.family_id.to_owned(),
                revision: contract.revision,
                params: params.to_vec(),
                params_digest: params_digest(params).to_vec(),
            },
            nonce: instance
                .nonce()
                .map(|nonce| nonce.to_vec())
                .unwrap_or_default(),
            instance_digest: instance.digest().to_vec(),
            metadata: instance.metadata().to_vec(),
            order_digest: instance.topology_order_digest().to_vec(),
            node_ids: instance.node_ids().to_vec(),
            offset_milli: instance.offset_milli(),
            graph: Graph::Edges(EdgeList {
                u: instance.edges().iter().map(|&(u, _)| u).collect(),
                v: instance.edges().iter().map(|&(_, v)| v).collect(),
            }),
            h: instance.h().to_vec(),
            j: instance.j().to_vec(),
        }
    }

    fn job(&self, job_id: &str) -> Job {
        Job {
            job_id: job_id.as_bytes().to_vec(),
            kind: JobKind::IsingSample as i32,
            generation: FAMILY_LIVE_GENERATION,
            ising: Some(IsingProblem {
                graph: Some(self.graph.clone()),
                encoding: CoefficientEncoding::I32 as i32,
                scale: 1000,
                h: encode_i32_le(&self.h),
                j: encode_i32_le(&self.j),
                family: Some(FamilyBinding {
                    family: Some(self.family.clone()),
                    nonce: self.nonce.clone(),
                    instance_digest: self.instance_digest.clone(),
                    metadata: self.metadata.clone(),
                    topology_order_digest: self.order_digest.clone(),
                    node_ids: self.node_ids.clone(),
                    offset_milli: self.offset_milli,
                }),
                ..IsingProblem::default()
            }),
            ..Job::default()
        }
    }
}

/// How an explicit job case is graded.
#[derive(Clone, Debug)]
enum ExplicitGrade {
    /// A solution scored on the plain model of this instance.
    Solved(Box<Instance>),
    /// One `Reject` with this wire reason.
    Reject(i32),
}

/// One explicit-job case. `plan` is `Err` with the outcome to record when the
/// gate cannot build the job.
struct ExplicitCase {
    name: &'static str,
    plan: Result<(ExplicitJob, ExplicitGrade), CaseOutcome>,
}

/// The first reference params set that the accepted claims admit, with the
/// first instance of the gate lease that it generates.
///
/// # Errors
///
/// The reason, naming what each params set gives.
fn select_explicit(
    caps: &Capabilities,
    reference: &'static ReferenceRevision,
    topology: &BaseTopology,
) -> Result<(&'static [u8], Instance), String> {
    // Admission reads `generated_families`; the explicit job reads the accepted list.
    let accepted = Capabilities {
        generated_families: caps.accepted_families.clone(),
        ..caps.clone()
    };
    let contract = reference.contract;
    let (_, nonces) = lease_inputs()?;
    let mut tried = Vec::new();
    for &params in reference.params {
        let admission = reference_admission(
            &accepted,
            reference,
            contract.family_id,
            contract.revision,
            params,
            &params_digest(params),
            topology,
            topology.order_digest(),
        );
        if let Err(code) = admission {
            tried.push(format!(
                "{} gives code {}",
                params_name(params),
                code.value()
            ));
            continue;
        }
        let salts = export_instances(contract, params, topology, &nonces)
            .map_err(|code| format!("the reference does not regenerate: code {}", code.value()))?;
        if let Some(instance) = salts.into_iter().find_map(Result::ok) {
            return Ok((params, instance));
        }
        tried.push(format!("{} skips every salt", params_name(params)));
    }
    Err(format!(
        "no reference params set gives an explicit instance under the accepted claims ({})",
        tried.join(", ")
    ))
}

/// The metadata of `instance` with one byte changed so the layout holds and
/// the construction check fails, with the instance digest of the changed bytes.
///
/// # Errors
///
/// The reason when no one-byte change does both.
fn corrupt_metadata(
    reference: &ReferenceRevision,
    params: &[u8],
    instance: &Instance,
) -> Result<(Vec<u8>, [u8; 32]), String> {
    let contract = reference.contract;
    let construction = contract
        .construction
        .ok_or_else(|| "the reference has no construction".to_owned())?;
    let admitted = admit(
        from_ref(contract),
        contract.family_id.as_bytes(),
        contract.revision,
        params,
        &params_digest(params),
    )
    .map_err(|code| format!("the reference params do not admit: code {}", code.value()))?;
    let region = instance.metadata();
    for at in (0..region.len()).rev() {
        let mut changed = region.to_vec();
        if let Some(byte) = changed.get_mut(at) {
            *byte ^= 0x01;
        }
        let parts = InstanceParts {
            descriptor_digest: *instance.descriptor_digest(),
            topology_order_digest: *instance.topology_order_digest(),
            nonce: instance.nonce().copied(),
            node_ids: instance.node_ids().to_vec(),
            edges: instance.edges().to_vec(),
            h: instance.h().to_vec(),
            j: instance.j().to_vec(),
            offset_milli: instance.offset_milli(),
            metadata: changed.clone(),
        };
        let Ok(rebuilt) = Instance::new(parts, &contract.limits, contract.metadata_kinds, None)
        else {
            continue;
        };
        if !(construction.check_metadata)(admitted.params(), &rebuilt) {
            return Ok((changed, rebuilt.digest()));
        }
    }
    Err("no one-byte change of the metadata keeps its layout and fails the family check".to_owned())
}

/// The explicit-job cases to record when the gate cannot build the base
/// instance: `explicit-supported` fails and the others are skipped.
fn unavailable_explicit(why: &str) -> Vec<ExplicitCase> {
    EXPLICIT_CASES
        .iter()
        .enumerate()
        .map(|(at, &name)| {
            let outcome = if at == 0 {
                CaseOutcome::Failed(why.to_owned())
            } else {
                CaseOutcome::Skipped(why.to_owned())
            };
            ExplicitCase {
                name,
                plan: Err(outcome),
            }
        })
        .collect()
}

/// The five explicit-job cases of one accepted revision, in run order.
fn explicit_cases(
    caps: &Capabilities,
    reference: &'static ReferenceRevision,
    gate: &GateTopology,
) -> Vec<ExplicitCase> {
    let topology = &gate.plain;
    let (params, instance) = match select_explicit(caps, reference, topology) {
        Ok(selected) => selected,
        Err(why) => return unavailable_explicit(&why),
    };
    let contract = reference.contract;
    let base = ExplicitJob::new(contract, params, &instance);
    let reject = |code: Code| ExplicitGrade::Reject(explicit_reason(code));

    let mut wrong_digest = base.instance_digest.clone();
    if let Some(byte) = wrong_digest.first_mut() {
        *byte ^= 0x01;
    }
    let metadata =
        corrupt_metadata(reference, params, &instance).map(|(metadata, digest)| ExplicitJob {
            metadata,
            instance_digest: digest.to_vec(),
            ..base.clone()
        });
    let out_of_bounds = reference.out_of_bounds_params;
    let oob_gives = admit(
        from_ref(contract),
        contract.family_id.as_bytes(),
        contract.revision,
        out_of_bounds,
        &params_digest(out_of_bounds),
    )
    .err();
    let out_of_bounds_job = if oob_gives == Some(Code::ParamsOutOfBounds) {
        Ok(ExplicitJob {
            family: ExplicitJob::new(contract, out_of_bounds, &instance).family,
            ..base.clone()
        })
    } else {
        Err(format!(
            "the reference out-of-bounds params give {}, not code 9",
            oob_gives.map_or_else(
                || "admitted".to_owned(),
                |code| format!("code {}", code.value())
            )
        ))
    };
    // The cached topology fixes the plain model of the hash form, so any
    // coefficients that fit it work. The binding stays the valid one.
    let hash_form = ExplicitJob {
        graph: Graph::TopologyHash(gate.hash.to_vec()),
        h: vec![0; topology.node_ids().len()],
        j: vec![1000; topology.edges().len()],
        ..base.clone()
    };
    let unavailable = |why: String| CaseOutcome::Failed(why);
    vec![
        ExplicitCase {
            name: "explicit-supported",
            plan: Ok((base.clone(), ExplicitGrade::Solved(Box::new(instance)))),
        },
        ExplicitCase {
            name: "explicit-digest-mismatch",
            plan: Ok((
                ExplicitJob {
                    instance_digest: wrong_digest,
                    ..base
                },
                reject(Code::InstanceDigestMismatch),
            )),
        },
        ExplicitCase {
            name: "explicit-metadata-invalid",
            plan: metadata
                .map(|job| (job, reject(Code::MetadataInvalid)))
                .map_err(unavailable),
        },
        ExplicitCase {
            name: "explicit-params-out-of-bounds",
            plan: out_of_bounds_job
                .map(|job| (job, reject(Code::ParamsOutOfBounds)))
                .map_err(unavailable),
        },
        ExplicitCase {
            name: "explicit-topology-hash-form",
            plan: Ok((hash_form, reject(Code::InstanceDigestMismatch))),
        },
    ]
}

/// Run one explicit job case and record its outcome. The job id is `"{name}/{subject}"`.
async fn run_explicit_case(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    (name, subject): (&'static str, &str),
    plan: &(ExplicitJob, ExplicitGrade),
) {
    let (explicit, grade) = plan;
    let job_id = format!("{name}/{subject}");
    let graded = match run_job(inbound, tx, outcome, explicit.job(&job_id), Await::Explicit).await {
        Err(why) => CaseOutcome::Failed(why),
        Ok(start) => {
            let slice = outcome.messages.get(start..).unwrap_or_default();
            let job_id = job_id.as_bytes();
            match grade {
                ExplicitGrade::Solved(instance) => grade_explicit_solved(slice, job_id, instance),
                ExplicitGrade::Reject(want) => grade_reject(slice, job_id, *want),
            }
        }
    };
    push_case(outcome, name, Some(subject), graded);
}

/// Send `gate` to the session unless the session already holds it. A session
/// holds one topology, the last one sent, so a subject sends its own before its
/// first job.
async fn hold_topology(
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    held: &mut [u8; 32],
    gate: &GateTopology,
) {
    if *held != gate.hash {
        let _ = send(tx, coord_msg::Msg::Topology(gate.wire())).await;
        *held = gate.hash;
    }
}

/// Run the explicit-job cases of one accepted `(subject, reference)` pair.
async fn run_explicit_subject(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    caps: &Capabilities,
    held: &mut [u8; 32],
    (subject, reference): (&str, &'static ReferenceRevision),
) {
    let gate = match GateTopology::load(reference.topology) {
        Ok(gate) => gate,
        Err(why) => {
            for case in unavailable_explicit(&why) {
                if let Err(graded) = case.plan {
                    push_case(outcome, case.name, Some(subject), graded);
                }
            }
            return;
        }
    };
    for case in explicit_cases(caps, reference, &gate) {
        match &case.plan {
            Err(graded) => push_case(outcome, case.name, Some(subject), graded.clone()),
            Ok(plan) => {
                hold_topology(tx, held, &gate).await;
                run_explicit_case(inbound, tx, outcome, (case.name, subject), plan).await;
            }
        }
    }
}

/// Run every case of one advertised `(subject, reference)` pair on the base
/// topology of its reference. `sparse` stands in when there is no reference.
async fn run_subject(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    caps: &Capabilities,
    (sparse, held): (&GateTopology, &mut [u8; 32]),
    (subject, reference): (&str, Option<&'static ReferenceRevision>),
) {
    let loaded;
    let gate = match reference.map(|reference| GateTopology::load(reference.topology)) {
        None => sparse,
        Some(Ok(gate)) => {
            loaded = gate;
            &loaded
        }
        Some(Err(why)) => {
            for (name, case_outcome) in unavailable_cases(true, &why) {
                push_case(outcome, name, Some(subject), case_outcome);
            }
            return;
        }
    };
    let topology = &gate.plain;
    let reordered = &gate.reordered;
    let (mut supported, reference) = match select_supported(caps, subject, reference, topology) {
        Ok(selected) => selected,
        Err(cases) => {
            for (name, case_outcome) in cases {
                push_case(outcome, name, Some(subject), case_outcome);
            }
            return;
        }
    };
    supported.topology_hash = gate.hash;
    let cases = revision_cases(
        caps,
        reference,
        &supported,
        topology,
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
        let admission = lease.admission(caps, reference, topology);
        let met = match (&admission, case.needs) {
            (Ok(_), None) => true,
            (Err(code), Some(needs)) => *code == needs,
            (Ok(_), Some(_)) | (Err(_), None) => false,
        };
        if met {
            hold_topology(tx, held, gate).await;
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

/// Run the explicit-job cases of every accepted revision.
async fn run_explicit_subjects(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    caps: &Capabilities,
    held: &mut [u8; 32],
) {
    for (subject, reference) in plan_explicit_subjects(caps) {
        run_explicit_subject(inbound, tx, outcome, caps, held, (&subject, reference)).await;
    }
}

/// The session cases that send a family lease the binary must refuse.
async fn run_session_cases(
    inbound: &mut Streaming<MinerMsg>,
    tx: &mpsc::Sender<Result<CoordMsg, Status>>,
    outcome: &mut SessionOutcome,
    caps: &Capabilities,
    sparse: &GateTopology,
) {
    let order_digest = *sparse.plain.order_digest();
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

/// Load `T_SPARSE`, or grade `session-setup` as failed and give `None`.
fn load_sparse(outcome: &mut SessionOutcome) -> Option<GateTopology> {
    match GateTopology::load(T_SPARSE_TOPOLOGY) {
        Ok(sparse) => Some(sparse),
        Err(why) => {
            push_case(outcome, "session-setup", None, CaseOutcome::Failed(why));
            None
        }
    }
}

/// The family session script: handshake, setup, the cancel watermark, the
/// session cases, the cases of each advertised revision, the explicit-job
/// cases of each accepted revision, the credit ledger, and `Shutdown`.
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
    let Some(sparse) = load_sparse(&mut outcome) else {
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
    let _ = send(tx, coord_msg::Msg::Topology(sparse.wire())).await;
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
    run_session_cases(inbound, tx, &mut outcome, &caps, &sparse).await;
    let mut held = sparse.hash;
    for (subject, reference) in plan_subjects(&caps) {
        run_subject(
            inbound,
            tx,
            &mut outcome,
            &caps,
            (&sparse, &mut held),
            (&subject, reference),
        )
        .await;
    }

    // 7. The explicit jobs of each accepted revision, after every lease case.
    run_explicit_subjects(inbound, tx, &mut outcome, &caps, &mut held).await;

    // 8. Every dispatched job, explicit jobs included, returned exactly one credit.
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

    // 9. Shutdown ends the session.
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
        STDERR_DRAIN_TIMEOUT,
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
        let stored = crate::t_sparse().expect("T_SPARSE loads");
        let mut edges = stored.edges;
        if swap_first_edges {
            edges.swap(0, 1);
        }
        BaseTopology::new(stored.nodes, &edges).expect("T_SPARSE is a valid topology")
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

    /// Asserts that `outcome` passes, or that it fails, as the row says.
    fn assert_graded(label: &str, outcome: &CaseOutcome, passes: bool) {
        if passes {
            assert_eq!(outcome, &CaseOutcome::Passed, "{label}");
        } else {
            assert!(failed(outcome), "{label}: {outcome:?}");
        }
    }

    #[test]
    fn grade_reject_cases() {
        let expected = expected_lease();
        let malformed = RejectReason::Malformed as i32;
        let unspecified = RejectReason::Unspecified as i32;
        // (label, slice, wanted reason, whether it passes)
        let rows = [
            (
                "a reject with its refund",
                vec![msg_reject(RejectReason::Malformed), msg_request(1)],
                malformed,
                true,
            ),
            (
                "a reject without a refund",
                vec![msg_reject(RejectReason::Malformed)],
                malformed,
                false,
            ),
            (
                "a wrong reject reason",
                vec![msg_reject(RejectReason::Malformed), msg_request(1)],
                unspecified,
                false,
            ),
            (
                "a second refund",
                vec![
                    msg_reject(RejectReason::Malformed),
                    msg_request(1),
                    msg_request(1),
                ],
                malformed,
                false,
            ),
            (
                "a result in a reject slice",
                vec![
                    msg_reject(RejectReason::Malformed),
                    result_of(&expected, 1, true),
                    msg_request(1),
                ],
                malformed,
                false,
            ),
        ];
        for (label, slice, want, passes) in rows {
            assert_graded(label, &grade_reject(&slice, JOB, want), passes);
        }
    }

    #[test]
    fn lease_transcripts_grade_as_expected() {
        type Edit = fn(&ExpectedLease, &mut Vec<miner_msg::Msg>);
        let expected = expected_lease();
        let skipped: Vec<usize> = expected
            .salts
            .iter()
            .enumerate()
            .filter(|(_, salt)| salt.is_err())
            .map(|(slot, _)| slot)
            .collect();
        assert_eq!(skipped, [0, 3], "the skipped slots of the lease");
        // (label, edit of the good transcript, whether it passes)
        let rows: [(&str, Edit, bool); 10] = [
            ("a correct transcript", |_, _| {}, true),
            (
                "a result without the offset",
                |e, slice| {
                    if let Some(first) = slice.first_mut() {
                        *first = result_of(e, 1, false);
                    }
                },
                false,
            ),
            (
                "a result for a skipped salt",
                |e, slice| {
                    let mut stray = result_of(e, 1, true);
                    if let miner_msg::Msg::Result(result) = &mut stray {
                        result.salt = e.spec.salt(0).expect("in lease").to_vec();
                        result.nonce = e.spec.nonce(0).expect("in lease").to_vec();
                    }
                    slice.insert(0, stray);
                },
                false,
            ),
            (
                "a LeaseDone with a wrong count",
                |e, slice| {
                    if let Some(done) = slice.get_mut(6) {
                        *done = lease_done(e, 7);
                    }
                },
                false,
            ),
            (
                "a result after the LeaseDone",
                |e, slice| {
                    slice.insert(7, result_of(e, 1, true));
                },
                false,
            ),
            (
                "a repeated salt",
                |e, slice| {
                    slice.insert(0, result_of(e, 1, true));
                },
                false,
            ),
            (
                "a nonce mismatch",
                |_, slice| {
                    if let Some(miner_msg::Msg::Result(result)) = slice.first_mut() {
                        result.nonce = vec![0; 32];
                    }
                },
                false,
            ),
            (
                "a generated slot without a result",
                |_, slice| {
                    let _ = slice.remove(0);
                },
                false,
            ),
            (
                "a wrong best energy",
                |_, slice| {
                    if let Some(miner_msg::Msg::LeaseDone(done)) = slice.get_mut(6) {
                        done.best_energy_milli += 1;
                    }
                },
                false,
            ),
            (
                "undecodable spins",
                |_, slice| {
                    if let Some(miner_msg::Msg::Result(result)) = slice.first_mut() {
                        if let Some(solution) = result.solutions.first_mut() {
                            solution.spins = vec![];
                        }
                    }
                },
                false,
            ),
        ];
        for (label, edit, passes) in rows {
            let mut slice = good_transcript(&expected);
            edit(&expected, &mut slice);
            assert_graded(label, &grade_lease(&slice, JOB, &expected), passes);
        }
    }

    #[test]
    fn grade_stale_cases() {
        let expected = expected_lease();
        let done = |salts_done| {
            miner_msg::Msg::LeaseDone(LeaseDone {
                job_id: JOB.to_vec(),
                salts_done,
                best_energy_milli: i64::MAX,
            })
        };
        // (label, slice, whether it passes)
        let rows = [
            (
                "a stale lease with a result",
                vec![result_of(&expected, 1, true), msg_request(1)],
                false,
            ),
            (
                "a stale lease with only a refund",
                vec![msg_request(1)],
                true,
            ),
            (
                "a stale LeaseDone with work",
                vec![done(1), msg_request(1)],
                false,
            ),
            (
                "a stale LeaseDone without work",
                vec![done(0), msg_request(1)],
                true,
            ),
            (
                "two stale LeaseDone messages",
                vec![done(0), done(0), msg_request(1)],
                false,
            ),
            (
                "a reject in a stale slice",
                vec![msg_reject(RejectReason::Malformed), msg_request(1)],
                false,
            ),
        ];
        for (label, slice, passes) in rows {
            assert_graded(label, &grade_stale(&slice, JOB), passes);
        }
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
    fn advertised_revisions_without_a_reference_fail_supported_revision() {
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
        let cases = select_supported(&caps, "vectors/other@1", None, &t_sparse(false))
            .expect_err("the gate cannot drive vectors/other@1");
        assert_eq!(
            cases,
            [(
                "supported-revision",
                CaseOutcome::Failed("no reference contract".to_owned())
            )]
        );
    }

    fn uniform_reference() -> &'static ReferenceRevision {
        reference_revision("vectors/uniform-field", 1).expect("a reference revision exists")
    }

    #[test]
    fn supported_revision_failures_name_the_reason() {
        let ice = reference_revision("quip/ice-glass", 1).expect("a reference revision");
        let mut ice_caps = mock_caps();
        ice_caps.generated_families.push(FamilySupport {
            family_id: "quip/ice-glass".to_owned(),
            revisions: vec![1],
            params_digests: vec![],
        });
        let mut repeated = mock_caps();
        let repeat = repeated
            .generated_families
            .first()
            .cloned()
            .expect("an entry");
        repeated.generated_families.push(repeat);
        let mut zero_revision = mock_caps();
        if let Some(entry) = zero_revision.generated_families.first_mut() {
            entry.revisions = vec![0, 1];
        }
        let uniform = "vectors/uniform-field@1";
        // (label, caps, subject, reference, reason of the first case, whether the
        // case list has ten cases, whether the other cases skip with the reason)
        let rows = [
            (
                "no params set admits",
                Capabilities {
                    max_nodes: 1,
                    ..mock_caps()
                },
                uniform,
                uniform_reference(),
                "no reference params set admits under the advertised limits \
                 (P_SPEC gives code 13, P_L2_F0 gives code 13, P_L3_F0 gives code 13)",
                true,
                true,
            ),
            (
                "T_SPARSE without ice tiles",
                ice_caps,
                "quip/ice-glass@1",
                ice,
                "no reference params set admits under the advertised limits \
                 (P_ICE_ADJ_125 gives code 12, P_ICE_Z12_200 gives code 12)",
                true,
                false,
            ),
            (
                "a repeated entry",
                repeated,
                uniform,
                uniform_reference(),
                "generated_families lists vectors/uniform-field 2 times, \
                 so the list claims nothing for it (FAM-61)",
                false,
                false,
            ),
            (
                "an entry that lists revision 0",
                zero_revision,
                uniform,
                uniform_reference(),
                "the entry for vectors/uniform-field lists revision 0 (FAM-61)",
                false,
                false,
            ),
        ];
        let topology = t_sparse(false);
        for (label, caps, subject, reference, why, ten_cases, rest_skipped) in rows {
            let selected = select_supported(&caps, subject, Some(reference), &topology);
            assert!(selected.is_err(), "{label}: the gate accepted the revision");
            let cases = selected.expect_err("checked above");
            if ten_cases {
                assert_eq!(cases.len(), 10, "{label}");
            }
            assert_eq!(
                cases.first(),
                Some(&("supported-revision", CaseOutcome::Failed(why.to_owned()))),
                "{label}"
            );
            if rest_skipped {
                for (_, outcome) in cases.iter().skip(1) {
                    assert_eq!(outcome, &CaseOutcome::Skipped(why.to_owned()), "{label}");
                }
            }
        }
    }

    #[test]
    fn revision_plus_one_is_skipped_when_the_binary_lists_it() {
        let mut caps = mock_caps();
        if let Some(entry) = caps.generated_families.first_mut() {
            entry.revisions = vec![1, 2];
        }
        let topology = t_sparse(false);
        let reordered = t_sparse(true);
        let (supported, reference) = select_supported(
            &caps,
            "vectors/uniform-field@1",
            Some(uniform_reference()),
            &topology,
        )
        .expect("revision 1 admits");
        let cases = revision_cases(
            &caps,
            reference,
            &supported,
            &topology,
            reordered.order_digest(),
        );
        let next = cases
            .iter()
            .find(|case| case.name == "revision-plus-one")
            .expect("the case exists");
        assert_eq!(
            next.lease.as_ref().err().map(String::as_str),
            Some("the binary lists revision 2")
        );
        let listed_once = mock_caps();
        let cases = revision_cases(
            &listed_once,
            reference,
            &supported,
            &topology,
            reordered.order_digest(),
        );
        let next = cases
            .iter()
            .find(|case| case.name == "revision-plus-one")
            .expect("the case exists");
        assert_eq!(next.lease.as_ref().map(|lease| lease.revision), Ok(2));
    }

    fn case(name: &'static str, outcome: CaseOutcome) -> FamilyCase {
        FamilyCase {
            name,
            subject: Some("vectors/uniform-field@1".to_owned()),
            outcome,
        }
    }

    #[test]
    fn report_conformance_follows_the_outcomes() {
        // (label, cases, whether the report is conformant)
        let rows = [
            (
                "a pass and a skip",
                vec![
                    case("a", CaseOutcome::Passed),
                    case("b", CaseOutcome::Skipped("no reference".to_owned())),
                ],
                true,
            ),
            (
                "only a skip",
                vec![case("b", CaseOutcome::Skipped("no reference".to_owned()))],
                false,
            ),
            (
                "a pass and a failure",
                vec![
                    case("a", CaseOutcome::Passed),
                    case("b", CaseOutcome::Failed("why".to_owned())),
                ],
                false,
            ),
            ("no cases", vec![], false),
        ];
        for (label, cases, conformant) in rows {
            let report = FamilyReport {
                cases,
                ..FamilyReport::default()
            };
            assert_eq!(report.is_conformant(), conformant, "{label}");
        }
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
        // 8 session barriers, 30 per reference revision (ten lease cases and
        // five explicit cases, two barriers each), 1 phase of slack.
        let phases = 8 + 30 * REFERENCE_REVISIONS.len() + 1;
        let phases = u32::try_from(phases).expect("small");
        assert_eq!(family_child_timeout(), PHASE_TIMEOUT * phases);
    }

    #[test]
    fn subject_plans_follow_the_lists() {
        type PlanRow<'a> = (
            &'a str,
            Capabilities,
            Option<usize>,
            usize,
            Option<&'a [&'a str]>,
        );
        let mut no_generated = mock_caps();
        no_generated.generated_families = vec![];
        let mut no_accepted = mock_caps();
        no_accepted.accepted_families = vec![];
        let mut no_reference = mock_caps();
        no_reference.accepted_families = vec![FamilySupport {
            family_id: "vectors/other".to_owned(),
            revisions: vec![1, 1],
            params_digests: vec![],
        }];
        let mut repeated = mock_caps();
        let repeat = repeated.accepted_families.clone();
        repeated.accepted_families.extend(repeat);
        // (label, caps, number of lease subjects if checked, number of explicit
        // subjects, names of the explicit subjects if checked)
        let rows: [PlanRow<'_>; 4] = [
            (
                "accepted only",
                no_generated,
                Some(0),
                2,
                Some(&["vectors/uniform-field@1", "vectors/restricted-field@1"]),
            ),
            ("generated only", no_accepted, Some(2), 0, None),
            ("accepted without a reference", no_reference, None, 0, None),
            ("accepted listed twice", repeated, None, 2, None),
        ];
        for (label, caps, lease_count, explicit_count, explicit_names) in rows {
            if let Some(count) = lease_count {
                assert_eq!(plan_subjects(&caps).len(), count, "{label}: lease subjects");
            }
            let subjects = plan_explicit_subjects(&caps);
            assert_eq!(subjects.len(), explicit_count, "{label}: explicit subjects");
            if let Some(want) = explicit_names {
                let names: Vec<&str> = subjects.iter().map(|(name, _)| name.as_str()).collect();
                assert_eq!(names, want, "{label}: explicit subject names");
            }
        }
    }

    fn explicit_base() -> Instance {
        select_explicit(&mock_caps(), uniform_reference(), &t_sparse(false))
            .expect("the uniform reference gives an explicit instance")
            .1
    }

    fn explicit_result(instance: &Instance, with_offset: bool) -> miner_msg::Msg {
        let spins = vec![-1_i8; instance.node_ids().len()];
        let edges = instance_edges(instance).expect("small");
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
            ..quip_proto::v1::Result::default()
        })
    }

    fn solved(slice: &[miner_msg::Msg], instance: &Instance) -> CaseOutcome {
        grade_explicit_solved(slice, JOB, instance)
    }

    #[test]
    fn explicit_solved_transcripts_grade_as_expected() {
        type Build = fn(&Instance) -> Vec<miner_msg::Msg>;
        let instance = explicit_base();
        assert_ne!(
            instance.offset_milli(),
            0,
            "the offset check needs an offset"
        );
        // (label, slice built from the instance, whether it passes)
        let rows: [(&str, Build, bool); 11] = [
            (
                "a solved job with its refund",
                |i| vec![explicit_result(i, false), msg_request(1)],
                true,
            ),
            (
                "a result scored with the offset",
                |i| vec![explicit_result(i, true), msg_request(1)],
                false,
            ),
            ("a job without a result", |_| vec![msg_request(1)], false),
            (
                "a rejected valid job",
                |_| vec![msg_reject(RejectReason::Malformed), msg_request(1)],
                false,
            ),
            (
                "a result without a refund",
                |i| vec![explicit_result(i, false)],
                false,
            ),
            (
                "an early refund",
                |i| vec![msg_request(1), explicit_result(i, false)],
                false,
            ),
            (
                "two refunds",
                |i| vec![explicit_result(i, false), msg_request(1), msg_request(1)],
                false,
            ),
            (
                "a second result",
                |i| {
                    vec![
                        explicit_result(i, false),
                        explicit_result(i, false),
                        msg_request(1),
                    ]
                },
                false,
            ),
            (
                "a result without solutions",
                |i| {
                    let mut empty = explicit_result(i, false);
                    if let miner_msg::Msg::Result(result) = &mut empty {
                        result.solutions.clear();
                    }
                    vec![empty, msg_request(1)]
                },
                false,
            ),
            (
                "a result with bad spins",
                |i| {
                    let mut bad = explicit_result(i, false);
                    if let miner_msg::Msg::Result(result) = &mut bad {
                        if let Some(solution) = result.solutions.first_mut() {
                            solution.spins = vec![];
                        }
                    }
                    vec![bad, msg_request(1)]
                },
                false,
            ),
            (
                "a LeaseDone",
                |i| {
                    let done = miner_msg::Msg::LeaseDone(LeaseDone {
                        job_id: JOB.to_vec(),
                        salts_done: 0,
                        best_energy_milli: 0,
                    });
                    vec![explicit_result(i, false), done, msg_request(1)]
                },
                false,
            ),
        ];
        for (label, build, passes) in rows {
            let slice = build(&instance);
            assert_graded(label, &solved(&slice, &instance), passes);
        }
    }

    fn explicit_plans(caps: &Capabilities) -> Vec<ExplicitCase> {
        let gate = gate_of(uniform_reference());
        explicit_cases(caps, uniform_reference(), &gate)
    }

    fn plan_of(cases: &[ExplicitCase], name: &str) -> (ExplicitJob, ExplicitGrade) {
        cases
            .iter()
            .find(|case| case.name == name)
            .and_then(|case| case.plan.clone().ok())
            .unwrap_or_else(|| panic!("{name}: the case is missing or does not build"))
    }

    fn check_supported_explicit_plan(cases: &[ExplicitCase], instance: &Instance) {
        // explicit-supported
        let (base, grade) = plan_of(cases, "explicit-supported");
        assert_eq!(
            base.instance_digest,
            instance.digest().to_vec(),
            "explicit-supported"
        );
        assert_eq!(
            base.offset_milli,
            instance.offset_milli(),
            "explicit-supported"
        );
        assert_eq!(base.node_ids, instance.node_ids(), "explicit-supported");
        assert_eq!(base.nonce.len(), 32, "explicit-supported");
        assert!(
            matches!(grade, ExplicitGrade::Solved(_)),
            "explicit-supported grades as solved"
        );
        let wire = base.job("explicit-supported/x");
        assert_eq!(wire.kind, JobKind::IsingSample as i32, "explicit-supported");
        assert_eq!(
            wire.generation, FAMILY_LIVE_GENERATION,
            "explicit-supported"
        );
        let ising = wire.ising.expect("explicit-supported: an IsingProblem");
        assert!(
            matches!(ising.graph, Some(Graph::Edges(_))),
            "explicit-supported sends the edges"
        );
        assert_eq!(ising.scale, 1000, "explicit-supported");
        let binding = ising.family.expect("explicit-supported: a binding");
        assert_eq!(
            binding.instance_digest,
            instance.digest().to_vec(),
            "explicit-supported"
        );
    }

    fn check_digest_explicit_plan(cases: &[ExplicitCase]) {
        let (base, _) = plan_of(cases, "explicit-supported");
        // explicit-digest-mismatch
        let (job, grade) = plan_of(cases, "explicit-digest-mismatch");
        let differing = job
            .instance_digest
            .iter()
            .zip(&base.instance_digest)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(differing, 1, "explicit-digest-mismatch");
        assert_eq!(job.instance_digest.len(), 32, "explicit-digest-mismatch");
        let malformed = RejectReason::Malformed as i32;
        assert!(
            matches!(grade, ExplicitGrade::Reject(want) if want == malformed),
            "explicit-digest-mismatch expects the malformed reject"
        );
    }

    fn check_metadata_explicit_plan(cases: &[ExplicitCase], instance: &Instance) {
        // explicit-metadata-invalid
        let (job, grade) = plan_of(cases, "explicit-metadata-invalid");
        assert_ne!(
            job.metadata,
            instance.metadata(),
            "explicit-metadata-invalid"
        );
        let contract = &UNIFORM_V1;
        let parts = InstanceParts {
            descriptor_digest: *instance.descriptor_digest(),
            topology_order_digest: *instance.topology_order_digest(),
            nonce: instance.nonce().copied(),
            node_ids: instance.node_ids().to_vec(),
            edges: instance.edges().to_vec(),
            h: instance.h().to_vec(),
            j: instance.j().to_vec(),
            offset_milli: instance.offset_milli(),
            metadata: job.metadata.clone(),
        };
        let sent: [u8; 32] = job
            .instance_digest
            .as_slice()
            .try_into()
            .expect("explicit-metadata-invalid: 32 bytes");
        let rebuilt = Instance::new(
            parts,
            &contract.limits,
            contract.metadata_kinds,
            Some(&sent),
        )
        .expect("explicit-metadata-invalid: the layout and the digest are valid");
        let admitted = admit(
            from_ref(contract),
            contract.family_id.as_bytes(),
            1,
            &P_SPEC,
            &params_digest(&P_SPEC),
        )
        .expect("explicit-metadata-invalid: P_SPEC admits");
        let check = contract
            .construction
            .expect("explicit-metadata-invalid: a construction")
            .check_metadata;
        assert!(
            !check(admitted.params(), &rebuilt),
            "explicit-metadata-invalid fails the family check"
        );
        let invalid = explicit_reason(Code::MetadataInvalid);
        assert!(
            matches!(grade, ExplicitGrade::Reject(want) if want == invalid),
            "explicit-metadata-invalid expects the metadata reject"
        );
    }

    fn check_params_explicit_plan(cases: &[ExplicitCase]) {
        let (base, _) = plan_of(cases, "explicit-supported");
        // explicit-params-out-of-bounds
        let (job, grade) = plan_of(cases, "explicit-params-out-of-bounds");
        assert_eq!(job.family.params, P_F5000, "explicit-params-out-of-bounds");
        assert_eq!(
            job.family.params_digest,
            params_digest(&P_F5000).to_vec(),
            "explicit-params-out-of-bounds"
        );
        assert_eq!(
            job.instance_digest, base.instance_digest,
            "explicit-params-out-of-bounds"
        );
        let want = explicit_reason(Code::ParamsOutOfBounds);
        assert!(
            matches!(grade, ExplicitGrade::Reject(got) if got == want),
            "explicit-params-out-of-bounds expects the params reject"
        );
    }

    fn check_hash_form_explicit_plan(cases: &[ExplicitCase]) {
        // explicit-topology-hash-form
        let (job, grade) = plan_of(cases, "explicit-topology-hash-form");
        let topology = t_sparse(false);
        assert_eq!(
            job.graph,
            Graph::TopologyHash(FAMILY_TOPOLOGY_HASH.to_vec()),
            "explicit-topology-hash-form"
        );
        assert_eq!(
            job.h.len(),
            topology.node_ids().len(),
            "explicit-topology-hash-form"
        );
        assert_eq!(
            job.j.len(),
            topology.edges().len(),
            "explicit-topology-hash-form"
        );
        let wire = job.job("explicit-topology-hash-form/x");
        assert!(
            wire.ising.and_then(|ising| ising.family).is_some(),
            "explicit-topology-hash-form carries a binding"
        );
        let want = explicit_reason(Code::InstanceDigestMismatch);
        assert!(
            matches!(grade, ExplicitGrade::Reject(got) if got == want),
            "explicit-topology-hash-form expects the digest mismatch reject"
        );
    }

    #[test]
    fn explicit_case_plans_match_their_rules() {
        let cases = explicit_plans(&mock_caps());
        let instance = explicit_base();
        // The order of the cases.
        let names: Vec<&str> = cases.iter().map(|case| case.name).collect();
        assert_eq!(names, EXPLICIT_CASES, "the order of the cases");
        for case in &cases {
            assert!(
                case.plan.is_ok(),
                "{}: the case does not build: {:?}",
                case.name,
                case.plan.as_ref().err()
            );
        }
        check_supported_explicit_plan(&cases, &instance);
        check_digest_explicit_plan(&cases);
        check_metadata_explicit_plan(&cases, &instance);
        check_params_explicit_plan(&cases);
        check_hash_form_explicit_plan(&cases);
    }

    #[test]
    fn an_unbuildable_explicit_instance_fails_the_first_case_and_skips_the_rest() {
        let caps = Capabilities {
            max_nodes: 1,
            ..mock_caps()
        };
        let cases = explicit_plans(&caps);
        assert_eq!(cases.len(), EXPLICIT_CASES.len());
        let outcomes: Vec<&CaseOutcome> = cases
            .iter()
            .map(|case| case.plan.as_ref().expect_err("nothing builds"))
            .collect();
        let (first, rest) = outcomes.split_first().expect("five cases");
        assert!(failed(first));
        for skipped in rest {
            assert!(matches!(skipped, CaseOutcome::Skipped(_)));
        }
    }

    #[test]
    fn a_restricted_accepted_entry_picks_a_params_set_it_allows() {
        let reference = reference_revision("vectors/restricted-field", 1).expect("a reference");
        let (params, _) =
            select_explicit(&mock_caps(), reference, &t_sparse(false)).expect("P_SPEC is allowed");
        assert_eq!(params, P_SPEC);
    }

    /// Every reference revision listed, with limits wide enough for Zephyr.
    fn wide_caps() -> Capabilities {
        let list: Vec<FamilySupport> = REFERENCE_REVISIONS
            .iter()
            .map(|reference| FamilySupport {
                family_id: reference.contract.family_id.to_owned(),
                revisions: vec![reference.contract.revision],
                params_digests: vec![],
            })
            .collect();
        Capabilities {
            max_degree: None,
            generated_families: list.clone(),
            accepted_families: list,
            ..mock_caps()
        }
    }

    fn gate_of(reference: &ReferenceRevision) -> GateTopology {
        GateTopology::load(reference.topology).expect("the reference topology loads")
    }

    #[test]
    fn every_reference_admits_on_its_own_topology() {
        let caps = wide_caps();
        for reference in REFERENCE_REVISIONS {
            let contract = reference.contract;
            let subject = format!("{}@{}", contract.family_id, contract.revision);
            let gate = gate_of(reference);
            let supported = select_supported(&caps, &subject, Some(reference), &gate.plain);
            assert!(supported.is_ok(), "{subject}: {:?}", supported.err());
            let explicit = select_explicit(&caps, reference, &gate.plain);
            assert!(explicit.is_ok(), "{subject}: {:?}", explicit.err());
        }
    }

    #[test]
    fn gate_hashes_follow_the_topology_files() {
        for family in ["vectors/uniform-field", "vectors/restricted-field"] {
            let reference = reference_revision(family, 1).expect("a reference revision");
            assert_eq!(reference.topology, T_SPARSE_TOPOLOGY, "{family}");
            assert_eq!(gate_of(reference).hash, FAMILY_TOPOLOGY_HASH, "{family}");
        }
        let mut seen: Vec<(&str, [u8; 32])> = Vec::new();
        for reference in REFERENCE_REVISIONS {
            let hash = gate_of(reference).hash;
            let file = reference.topology;
            if let Some((_, known)) = seen.iter().find(|(known_file, _)| *known_file == file) {
                assert_eq!(*known, hash, "{file}");
            } else {
                assert!(
                    seen.iter().all(|(_, known)| *known != hash),
                    "{file}: its hash is shared with another topology file"
                );
                seen.push((file, hash));
            }
        }
        assert_eq!(seen.len(), 4, "the distinct topology files");
    }

    #[test]
    fn a_lease_names_the_topology_hash_it_carries() {
        let order = *t_sparse(false).order_digest();
        let mut lease = FamilyLease::new("vectors/uniform-field", 1, &P_SPEC, &order);
        let generator = |lease: &FamilyLease| {
            lease
                .job("job")
                .generator
                .expect("a generator")
                .topology_hash
        };
        assert_eq!(generator(&lease), FAMILY_TOPOLOGY_HASH.to_vec());
        lease.topology_hash = [0x42; 32];
        assert_eq!(generator(&lease), vec![0x42; 32]);
    }

    #[test]
    fn the_hash_form_case_names_the_topology_of_its_gate() {
        let reference = reference_revision("quip/ice-glass", 1).expect("a reference revision");
        let gate = gate_of(reference);
        let cases = explicit_cases(&wide_caps(), reference, &gate);
        let (job, _) = plan_of(&cases, "explicit-topology-hash-form");
        assert_eq!(job.graph, Graph::TopologyHash(gate.hash.to_vec()));
        assert_eq!(job.h.len(), gate.plain.node_ids().len());
        assert_eq!(job.j.len(), gate.plain.edges().len());
    }
}
