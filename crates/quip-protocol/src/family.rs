//! FAM-42 mapping from family codes to wire reject reasons.
//!
//! | Code | Admission | Explicit job | Generation |
//! |---|---|---|---|
//! | 1 to 4 | `MALFORMED` | none | none |
//! | 5, 6 | `FAMILY_UNSUPPORTED` | none | none |
//! | 7 to 9 | `MALFORMED` | `MALFORMED` | none |
//! | 10 | `FAMILY_UNSUPPORTED` | `FAMILY_UNSUPPORTED` | none |
//! | 11 | `TOPOLOGY_MISMATCH` | none | none |
//! | 12 | `FAMILY_UNSUPPORTED` | none | none |
//! | 13 | `TOO_LARGE` | `TOO_LARGE` | none |
//! | 14 | none | none | none |
//! | 15, 16 | none | `MALFORMED` | none |
//!
//! `None` means no `Reject` is sent. The session decides which phase applies;
//! this module only maps a code in a phase.
//!
//! The module also holds the pure wire checks for the support lists in
//! `Capabilities`: [`solver_id_class`] (FAM-60), [`entry_claims`] (FAM-61)
//! and [`support_code`] (FAM-62). An entry that breaks FAM-61 claims nothing,
//! so an invalid list behaves like an empty one.

use ising_generators::descriptor::{check_family_id, IdContext};
use ising_generators::Code;
use quip_proto::v1::{FamilySupport, RejectReason};

/// The FAM-43 phase in which a family code is reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodePhase {
    /// A lease is admitted.
    Admission,
    /// An explicit job is validated.
    ExplicitJob,
    /// Salts are generated.
    Generation,
}

/// FAM-42 wire reason for `code` in `phase`. `None` means no `Reject` is sent:
/// the code cannot occur in that phase, or (generation) the miner skips the salt.
#[must_use]
pub const fn reject_reason(code: Code, phase: CodePhase) -> Option<RejectReason> {
    use CodePhase::{Admission, ExplicitJob};
    match (code, phase) {
        (
            Code::DescriptorMissing
            | Code::DescriptorUnexpected
            | Code::FamilyIdInvalid
            | Code::RevisionZero,
            Admission,
        )
        | (
            Code::ParamsMalformed | Code::ParamsDigestMismatch | Code::ParamsOutOfBounds,
            Admission | ExplicitJob,
        )
        | (Code::InstanceDigestMismatch | Code::MetadataInvalid, ExplicitJob) => {
            Some(RejectReason::Malformed)
        }
        (
            Code::FamilyUnsupported | Code::RevisionUnsupported | Code::TopologyIncompatible,
            Admission,
        )
        | (Code::ParamsRestricted, Admission | ExplicitJob) => {
            Some(RejectReason::FamilyUnsupported)
        }
        (Code::TopologyOrderMismatch, Admission) => Some(RejectReason::TopologyMismatch),
        (Code::BoundsExceeded, Admission | ExplicitJob) => Some(RejectReason::TooLarge),
        // Generation and every other pair have no wire reason. `Code` is non-exhaustive, so a
        // new code needs a row above; the test over `Code::ALL` catches a miss.
        _ => None,
    }
}

/// FAM-60 classification of a `solver_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolverIdClass {
    /// Empty: the session is not a pin target.
    Empty,
    /// Matches `[a-z][a-z0-9-]*(/[a-z0-9][a-z0-9.-]*)*` and is 3 to 64 bytes.
    Valid,
    /// Not empty and not valid.
    Invalid,
}

/// `[a-z][a-z0-9-]*`
fn is_name_segment(segment: &[u8]) -> bool {
    match segment.split_first() {
        Some((first, rest)) => {
            first.is_ascii_lowercase()
                && rest
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        }
        None => false,
    }
}

/// `[a-z0-9][a-z0-9.-]*`
fn is_version_segment(segment: &[u8]) -> bool {
    match segment.split_first() {
        Some((first, rest)) => {
            (first.is_ascii_lowercase() || first.is_ascii_digit())
                && rest.iter().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'.'
                })
        }
        None => false,
    }
}

/// Classify `id` by FAM-60.
#[must_use]
pub fn solver_id_class(id: &[u8]) -> SolverIdClass {
    if id.is_empty() {
        return SolverIdClass::Empty;
    }
    if !(3..=64).contains(&id.len()) {
        return SolverIdClass::Invalid;
    }
    let mut segments = id.split(|b| *b == b'/');
    let first_ok = segments.next().is_some_and(is_name_segment);
    if first_ok && segments.all(is_version_segment) {
        SolverIdClass::Valid
    } else {
        SolverIdClass::Invalid
    }
}

/// FAM-61: whether each entry of one list claims anything. An entry claims
/// nothing if its `family_id` breaks FAM-01 or FAM-02 or uses `example`,
/// its `revisions` is empty or holds 0, a digest is not 32 bytes, or another
/// entry in the list has the same `family_id`. Repeated revisions are allowed.
#[must_use]
pub fn entry_claims(list: &[FamilySupport]) -> Vec<bool> {
    list.iter()
        .map(|entry| {
            let unique = list
                .iter()
                .filter(|other| other.family_id == entry.family_id)
                .count()
                == 1;
            let revisions_ok = !entry.revisions.is_empty() && !entry.revisions.contains(&0);
            let digests_ok = entry.params_digests.iter().all(|d| d.len() == 32);
            let grammar_ok = check_family_id(entry.family_id.as_bytes(), IdContext::Lease).is_ok();
            unique && revisions_ok && digests_ok && grammar_ok
        })
        .collect()
}

/// FAM-62 over one list (and FAM-65 code 10 for `accepted_families`):
/// `Some(FamilyUnsupported)` if no claiming entry has `family_id`,
/// `Some(RevisionUnsupported)` if none lists `revision`,
/// `Some(ParamsRestricted)` if every listing entry restricts and none has
/// `params_digest`, else `None`. Only entries that claim (see
/// [`entry_claims`]) count, so an empty or invalid list claims nothing.
///
/// For a lease checked against `generated_families`, each code is a reject.
/// For an explicit job checked against `accepted_families`, codes 5 and 6
/// mean that the solver is a generic solver for the job and solves the plain
/// model (FAM-65). Only code 10 is a reject there.
#[must_use]
pub fn support_code(
    list: &[FamilySupport],
    family_id: &str,
    revision: u32,
    params_digest: &[u8],
) -> Option<Code> {
    let claiming: Vec<&FamilySupport> = list
        .iter()
        .zip(entry_claims(list))
        .filter(|(entry, claims)| *claims && entry.family_id == family_id)
        .map(|(entry, _)| entry)
        .collect();
    if claiming.is_empty() {
        return Some(Code::FamilyUnsupported);
    }
    let listing: Vec<&&FamilySupport> = claiming
        .iter()
        .filter(|entry| entry.revisions.contains(&revision))
        .collect();
    if listing.is_empty() {
        return Some(Code::RevisionUnsupported);
    }
    let admitted = listing.iter().any(|entry| {
        entry.params_digests.is_empty() || entry.params_digests.iter().any(|d| d == params_digest)
    });
    (!admitted).then_some(Code::ParamsRestricted)
}
