//! The report and the graders of the `--family` gate. The graders are pure
//! functions over the messages a miner sent, so unit tests feed them
//! hand-built transcripts and need no binary.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::slice::from_ref;

use ising_generators::descriptor::{admit, AdvertisedLimits, SizeBounds};
use ising_generators::family::{admit_topology, BaseTopology, SaltFault};
use ising_generators::instance::Instance;
use ising_generators::Code;
use quip_proto::v1::{miner_msg, Capabilities, GeneratorAlgorithm};
use quip_protocol::family::support_code;
use quip_protocol::lease::LeaseSpec;
use quip_protocol::scoring::energy_from_milli;
use quip_protocol::wire::decode_spins_packed;

use super::{reference_revision, ReferenceRevision};
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the family script uses it in the next commit")
)]
pub(crate) struct ExpectedLease {
    pub(crate) spec: LeaseSpec,
    pub(crate) salts: Vec<Result<Instance, SaltFault>>,
}

/// `family_id@revision` for each advertised pair, in `Hello` order, without
/// repeats. The reference is `None` when the gate cannot drive the pair.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the family script uses it in the next commit")
)]
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the family script uses it in the next commit")
)]
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the family script uses it in the next commit")
)]
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the family script uses it in the next commit")
)]
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the family script uses it in the next commit")
)]
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the family script uses it in the next commit")
)]
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
    fn an_accepted_only_pair_has_no_subject() {
        let mut caps = mock_caps();
        caps.generated_families = vec![];
        assert!(plan_subjects(&caps).is_empty());
    }
}
