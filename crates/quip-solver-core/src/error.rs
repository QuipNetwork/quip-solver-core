//! Why a solver could not complete a job.
//!
//! Protocol-free by design. A solver reports a device condition, and the
//! session layer maps it to a wire reason. The `RejectReason` this replaces is
//! a nine-variant wire enum, of which only two could ever originate in a
//! backend; `job.rs` validation produces the other seven.

use quip_proto::v1::RejectReason;

/// A device condition that stopped a job.
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
}

impl SampleError {
    /// Map to the wire reason the coordinator receives for this job.
    #[must_use]
    pub(crate) fn to_reject_reason(&self) -> RejectReason {
        match self {
            Self::Capacity => RejectReason::TooLarge,
            Self::DeviceBusy | Self::DeviceFault(_) => RejectReason::Overloaded,
        }
    }

    /// True when the session must end rather than request more work.
    #[must_use]
    pub(crate) fn is_fatal(&self) -> bool {
        matches!(self, Self::DeviceFault(_))
    }
}

impl std::fmt::Display for SampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Capacity => write!(f, "capacity"),
            Self::DeviceBusy => write!(f, "device busy"),
            Self::DeviceFault(detail) => write!(f, "device fault: {detail}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SampleError;
    use quip_proto::v1::RejectReason;

    #[test]
    fn capacity_maps_to_too_large() {
        assert_eq!(
            SampleError::Capacity.to_reject_reason(),
            RejectReason::TooLarge
        );
    }

    #[test]
    fn busy_and_fault_both_map_to_overloaded() {
        assert_eq!(
            SampleError::DeviceBusy.to_reject_reason(),
            RejectReason::Overloaded
        );
        assert_eq!(
            SampleError::DeviceFault("nvml: GPU 0 lost".to_owned()).to_reject_reason(),
            RejectReason::Overloaded
        );
    }

    #[test]
    fn only_a_fault_ends_the_session() {
        assert!(!SampleError::Capacity.is_fatal());
        assert!(!SampleError::DeviceBusy.is_fatal());
        assert!(SampleError::DeviceFault(String::new()).is_fatal());
    }
}
