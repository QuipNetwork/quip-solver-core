//! Family descriptors, canonical instances, and `ChaCha8` streams for the Quip
//! solver protocol.
//!
//! The crate is `#![no_std]` and uses `alloc`. The `std` feature (on by
//! default) only enables runtime SIMD detection in `blake3`. It changes no API.
//!
//! Modules:
//! - [`rng`]: the `ChaCha8` stream and per-substream seeding.
//! - [`legacy`]: the legacy nonce derivation.
//! - [`descriptor`]: family ids, parameter schemas, descriptors, and admission.
//! - [`instance`]: canonical instance bytes, limits, and metadata validation.

#![no_std]

extern crate alloc;

mod digest;

pub mod descriptor;
pub mod instance;
pub mod legacy;
pub mod rng;

use core::fmt;

/// FAM-42 stable codes. The discriminant is the code value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[repr(u8)]
pub enum Code {
    /// A lease needs a descriptor and has none.
    DescriptorMissing = 1,
    /// A lease carries a descriptor it must not have.
    DescriptorUnexpected = 2,
    /// The family id is outside the grammar or the length bounds.
    FamilyIdInvalid = 3,
    /// The revision is zero.
    RevisionZero = 4,
    /// No contract matches the family id.
    FamilyUnsupported = 5,
    /// No contract matches the revision.
    RevisionUnsupported = 6,
    /// The parameter bytes have a layout fault.
    ParamsMalformed = 7,
    /// The sent parameter digest differs from the computed digest.
    ParamsDigestMismatch = 8,
    /// A parameter value is outside its bounds.
    ParamsOutOfBounds = 9,
    /// The contract restricts the parameter values.
    ParamsRestricted = 10,
    /// The topology order digest differs.
    TopologyOrderMismatch = 11,
    /// The topology does not fit the family.
    TopologyIncompatible = 12,
    /// A size or instance bound is exceeded.
    BoundsExceeded = 13,
    /// The job has no instance.
    NoInstance = 14,
    /// The instance digest differs, or the instance fields are faulty.
    InstanceDigestMismatch = 15,
    /// The metadata region is invalid.
    MetadataInvalid = 16,
}

impl Code {
    /// Every code, in ascending order of value.
    pub const ALL: [Self; 16] = [
        Self::DescriptorMissing,
        Self::DescriptorUnexpected,
        Self::FamilyIdInvalid,
        Self::RevisionZero,
        Self::FamilyUnsupported,
        Self::RevisionUnsupported,
        Self::ParamsMalformed,
        Self::ParamsDigestMismatch,
        Self::ParamsOutOfBounds,
        Self::ParamsRestricted,
        Self::TopologyOrderMismatch,
        Self::TopologyIncompatible,
        Self::BoundsExceeded,
        Self::NoInstance,
        Self::InstanceDigestMismatch,
        Self::MetadataInvalid,
    ];

    /// The numeric code value.
    #[must_use]
    pub const fn value(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::DescriptorMissing => "DescriptorMissing",
            Self::DescriptorUnexpected => "DescriptorUnexpected",
            Self::FamilyIdInvalid => "FamilyIdInvalid",
            Self::RevisionZero => "RevisionZero",
            Self::FamilyUnsupported => "FamilyUnsupported",
            Self::RevisionUnsupported => "RevisionUnsupported",
            Self::ParamsMalformed => "ParamsMalformed",
            Self::ParamsDigestMismatch => "ParamsDigestMismatch",
            Self::ParamsOutOfBounds => "ParamsOutOfBounds",
            Self::ParamsRestricted => "ParamsRestricted",
            Self::TopologyOrderMismatch => "TopologyOrderMismatch",
            Self::TopologyIncompatible => "TopologyIncompatible",
            Self::BoundsExceeded => "BoundsExceeded",
            Self::NoInstance => "NoInstance",
            Self::InstanceDigestMismatch => "InstanceDigestMismatch",
            Self::MetadataInvalid => "MetadataInvalid",
        };
        f.write_str(name)
    }
}

impl core::error::Error for Code {}
