//! Compact, canonical family witness and instance codecs.

use alloc::vec::Vec;
use core::fmt;

pub(crate) mod bits;
// These shared modules are frozen while family adapters land independently.
#[expect(
    clippy::allow_attributes,
    reason = "the staged scaffold needs an allowance that remains valid when adapters use all helpers"
)]
#[allow(
    dead_code,
    reason = "shared block helpers are consumed incrementally by family adapters"
)]
pub(crate) mod blocks;
#[expect(
    clippy::allow_attributes,
    reason = "the staged scaffold needs an allowance that remains valid when adapters use all helpers"
)]
#[allow(
    dead_code,
    reason = "shared instance helpers are consumed incrementally by family adapters"
)]
pub(crate) mod instance;

/// A compact codec failure, independent of the protocol FAM codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// No codec exists for this exact family revision.
    UnsupportedFamily,
    /// The instance belongs to a different descriptor.
    DescriptorMismatch,
    /// The instance structure or metadata is invalid.
    InvalidInstance,
    /// A count, size, or coefficient exceeds its bound.
    BoundsExceeded,
    /// The payload or spin count has the wrong length.
    Length,
    /// High padding bits are nonzero.
    Padding,
    /// A spin is neither minus one nor plus one.
    SpinValue,
    /// A rank or symbol is outside its alphabet.
    InvalidSymbol,
    /// A hard constraint is unsatisfied.
    Infeasible,
    /// An alternate representation of the same value was supplied.
    NonCanonical,
    /// A fallible reservation failed.
    Allocation,
    /// The instance lies outside the compact structural schema.
    UnencodableInstance,
}
impl CodecError {
    /// Stable snake-case error name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::UnsupportedFamily => "unsupported_family",
            Self::DescriptorMismatch => "descriptor_mismatch",
            Self::InvalidInstance => "invalid_instance",
            Self::BoundsExceeded => "bounds_exceeded",
            Self::Length => "length",
            Self::Padding => "padding",
            Self::SpinValue => "spin_value",
            Self::InvalidSymbol => "invalid_symbol",
            Self::Infeasible => "infeasible",
            Self::NonCanonical => "non_canonical",
            Self::Allocation => "allocation",
            Self::UnencodableInstance => "unencodable_instance",
        }
    }
}
impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
impl core::error::Error for CodecError {}

/// Owned state for linear witness processing after family metadata validation.
pub struct WitnessCodec {
    state: State,
}
enum State {
    Forest(crate::native::codec::ForestCodec),
    Blocks(blocks::BlocksCodec),
}
impl From<crate::native::codec::ForestCodec> for WitnessCodec {
    fn from(value: crate::native::codec::ForestCodec) -> Self {
        Self {
            state: State::Forest(value),
        }
    }
}
impl From<blocks::BlocksCodec> for WitnessCodec {
    fn from(value: blocks::BlocksCodec) -> Self {
        Self {
            state: State::Blocks(value),
        }
    }
}
impl WitnessCodec {
    /// Encode exact instance-order spins.
    ///
    /// # Errors
    /// Returns length, spin-value, feasibility, bounds, or allocation errors.
    pub fn encode(&self, spins: &[i8]) -> Result<Vec<u8>, CodecError> {
        match &self.state {
            State::Forest(c) => c.encode(spins),
            State::Blocks(c) => c.encode(spins),
        }
    }
    /// Decode a canonical payload in linear time on prepared state.
    ///
    /// # Errors
    /// Rejects malformed lengths, padding, ranks, infeasibility, and alternate encodings.
    pub fn decode(&self, bytes: &[u8]) -> Result<Vec<i8>, CodecError> {
        match &self.state {
            State::Forest(c) => c.decode(bytes),
            State::Blocks(c) => c.decode(bytes),
        }
    }
    /// Maximum valid payload length for this instance.
    #[must_use]
    pub fn max_encoded_len(&self) -> usize {
        match &self.state {
            State::Forest(c) => c.max_encoded_len(),
            State::Blocks(c) => c.max_encoded_len(),
        }
    }
    /// Number of spins in this instance.
    #[must_use]
    pub fn spin_count(&self) -> usize {
        match &self.state {
            State::Forest(c) => c.spin_count(),
            State::Blocks(c) => c.spin_count(),
        }
    }
}

pub(crate) fn filled<T: Clone>(len: usize, value: T) -> Result<Vec<T>, CodecError> {
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| CodecError::Allocation)?;
    out.resize(len, value);
    Ok(out)
}
pub(crate) fn copied<T: Clone>(values: &[T]) -> Result<Vec<T>, CodecError> {
    let mut out = Vec::new();
    out.try_reserve_exact(values.len())
        .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(values);
    Ok(out)
}
pub(crate) fn check_spins(spins: &[i8], count: usize) -> Result<(), CodecError> {
    if spins.len() != count {
        return Err(CodecError::Length);
    }
    if spins.iter().any(|&s| s != -1 && s != 1) {
        return Err(CodecError::SpinValue);
    }
    Ok(())
}
