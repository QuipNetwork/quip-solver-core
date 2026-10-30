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

use ising_generators::Code;
use quip_proto::v1::RejectReason;

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
