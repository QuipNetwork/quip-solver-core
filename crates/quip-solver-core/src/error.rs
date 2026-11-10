//! Why a solver could not complete a job.
//!
//! Protocol-free by design. A solver reports a device condition, and the
//! session layer maps it to a wire reason. The `RejectReason` this replaces is
//! a nine-variant wire enum, of which only two could ever originate in a
//! backend; `job.rs` validation produces the other seven.

use quip_proto::v1::RejectReason;

/// A device or solver condition that stopped a job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SampleError {
    /// The job exceeds a device bound: `num_reads` above the backend cap, or a
    /// graph larger than device memory. An identical job fails again.
    Capacity,
    /// The device is busy right now. An identical job may succeed later.
    DeviceBusy,
    /// The device is in a bad state and will not recover without a restart.
    /// Carries an operator-facing detail for the log line and the `Fatal`
    /// message.
    ///
    /// Before this variant existed, a wedged GPU reported `Overloaded`
    /// forever and the miner kept accepting jobs it could not serve.
    DeviceFault(String),
    /// The solver ran and declines this instance: it found no solution it can
    /// report, for example an UNSAT verdict. Carries an operator-facing detail
    /// for the log line. The session survives, and an identical job fails again.
    Declined(String),
}

impl SampleError {
    /// Map to the wire reason the coordinator receives for this job.
    #[must_use]
    pub(crate) fn to_reject_reason(&self) -> RejectReason {
        match self {
            Self::Capacity => RejectReason::TooLarge,
            Self::DeviceBusy | Self::DeviceFault(_) => RejectReason::Overloaded,
            Self::Declined(_) => RejectReason::NoSolution,
        }
    }

    /// True when the session must end rather than request more work.
    ///
    /// Public because a backend that implements its own `sample_stream` pump
    /// re-derives this classification; matching on variants there drifts
    /// silently when one is added.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::DeviceFault(_))
    }
}

impl std::fmt::Display for SampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Capacity => write!(f, "capacity"),
            Self::DeviceBusy => write!(f, "device busy"),
            Self::DeviceFault(detail) => write!(f, "device fault: {detail}"),
            Self::Declined(detail) => write!(f, "declined: {detail}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SampleError;
    use quip_proto::v1::RejectReason;

    #[test]
    fn sample_errors_map_to_reasons_and_fatality() {
        let rows = [
            (
                "capacity",
                SampleError::Capacity,
                RejectReason::TooLarge,
                false,
            ),
            (
                "busy",
                SampleError::DeviceBusy,
                RejectReason::Overloaded,
                false,
            ),
            (
                "fault",
                SampleError::DeviceFault("nvml: GPU 0 lost".to_owned()),
                RejectReason::Overloaded,
                true,
            ),
            (
                "fault with an empty message",
                SampleError::DeviceFault(String::new()),
                RejectReason::Overloaded,
                true,
            ),
            (
                "declined",
                SampleError::Declined("UNSAT".to_owned()),
                RejectReason::NoSolution,
                false,
            ),
        ];
        for (label, error, reason, fatal) in rows {
            assert_eq!(error.to_reject_reason(), reason, "{label}: reason");
            assert_eq!(error.is_fatal(), fatal, "{label}: fatal");
        }
    }
}
