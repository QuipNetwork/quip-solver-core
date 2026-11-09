//! Witness bytes for square-link families: packing, labeled constraints,
//! witness checks, and canonical bytes under request-preserving symmetries.
//!
//! A witness holds one spin per link in link order. Link `i` is bit `i % 8` of
//! byte `i / 8`, a set bit is spin +1, padding bits are 0, and the length is
//! exactly `ceil(links / 8)`. Nothing here reads Ising coefficients or
//! penalties.

use alloc::vec::Vec;
use core::fmt;

use crate::square::{
    charges, check_request_shape, symmetries, Request, RequestError, SquareLattice, MAX_LINKS,
};

/// Witness bytes or spins are malformed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WitnessError {
    /// The byte length is not `ceil(links / 8)`.
    Length,
    /// A padding bit is set.
    Padding,
    /// A spin is not +1 or -1.
    Value,
    /// The request has the wrong shape. No work sized by the request ran.
    Request(RequestError),
}

impl fmt::Display for WitnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Length => "the witness length is wrong",
            Self::Padding => "a witness padding bit is set",
            Self::Value => "a witness spin is not +1 or -1",
            Self::Request(error) => return write!(f, "the request is malformed: {error}"),
        })
    }
}

impl core::error::Error for WitnessError {}

/// Packs spins into witness bytes.
///
/// # Errors
///
/// [`WitnessError::Value`] when a spin is not +1 or -1.
pub fn pack(spins: &[i8]) -> Result<Vec<u8>, WitnessError> {
    let mut bytes = Vec::new();
    pack_into(spins, &mut bytes)?;
    Ok(bytes)
}

/// Packs spins into `bytes`, reusing its buffer.
fn pack_into(spins: &[i8], bytes: &mut Vec<u8>) -> Result<(), WitnessError> {
    bytes.clear();
    bytes.resize(spins.len().div_ceil(8), 0);
    for (i, &spin) in spins.iter().enumerate() {
        match spin {
            1 => {
                if let Some(byte) = bytes.get_mut(i / 8) {
                    *byte |= 1 << (i % 8);
                }
            }
            -1 => {}
            _ => return Err(WitnessError::Value),
        }
    }
    Ok(())
}

/// Unpacks witness bytes into one spin per link.
///
/// The length and padding checks run first. A link count above
/// [`MAX_LINKS`] is a length error, so the result never exceeds the largest
/// lattice, whatever the caller passes.
///
/// # Errors
///
/// [`WitnessError::Length`] when the length is not `ceil(links / 8)` or the
/// link count is above [`MAX_LINKS`], and [`WitnessError::Padding`] when a
/// padding bit is set.
pub fn unpack(bytes: &[u8], links: u32) -> Result<Vec<i8>, WitnessError> {
    let count = links as usize;
    if bytes.len() != count.div_ceil(8) {
        return Err(WitnessError::Length);
    }
    let used = count % 8;
    if used != 0 && bytes.last().is_some_and(|&b| b >> used != 0) {
        return Err(WitnessError::Padding);
    }
    if links > MAX_LINKS {
        return Err(WitnessError::Length);
    }
    Ok((0..count)
        .map(|i| {
            let set = bytes.get(i / 8).is_some_and(|b| (b >> (i % 8)) & 1 == 1);
            if set {
                1
            } else {
                -1
            }
        })
        .collect())
}

/// One labeled requirement of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Constraint {
    /// The spin of `link` is `spin`.
    Fixed {
        /// The link.
        link: u32,
        /// The required spin.
        spin: i8,
    },
    /// The charge of `vertex` is `charge`.
    Charge {
        /// The vertex.
        vertex: u32,
        /// The required charge.
        charge: i8,
    },
}

/// The constraints of a request: fixed spins by link, then charge labels by
/// vertex. The label of a constraint is its position.
///
/// # Errors
///
/// The shape error of [`check_request_shape`], before anything is allocated.
pub fn constraints(
    lattice: &SquareLattice,
    request: &Request,
) -> Result<Vec<Constraint>, RequestError> {
    check_request_shape(lattice, request)?;
    let fixed = request
        .fixed
        .iter()
        .map(|&(link, spin)| Constraint::Fixed { link, spin });
    let labeled = request.charges.iter().enumerate().filter_map(|(v, label)| {
        let vertex = u32::try_from(v).ok()?;
        label.map(|charge| Constraint::Charge { vertex, charge })
    });
    Ok(fixed.chain(labeled).collect())
}

/// The labels of the constraints that the witness fails, ascending.
///
/// The check reads the spins and the request only. It is independent of any
/// penalty energy.
///
/// # Errors
///
/// [`WitnessError::Request`] when the request has the wrong shape, then a
/// [`WitnessError`] when the bytes are malformed. Both come before any
/// constraint check. A well shaped request that no witness can meet is not an
/// error here. It gives failed labels.
pub fn check_witness(
    lattice: &SquareLattice,
    request: &Request,
    bytes: &[u8],
) -> Result<Vec<u32>, WitnessError> {
    check_request_shape(lattice, request).map_err(WitnessError::Request)?;
    let spins = unpack(bytes, lattice.link_count())?;
    let q = charges(lattice, &spins).ok_or(WitnessError::Value)?;
    let mut failed = Vec::new();
    for (label, constraint) in constraints(lattice, request)
        .map_err(WitnessError::Request)?
        .into_iter()
        .enumerate()
    {
        let holds = match constraint {
            Constraint::Fixed { link, spin } => spins.get(link as usize) == Some(&spin),
            Constraint::Charge { vertex, charge } => q.get(vertex as usize) == Some(&charge),
        };
        if !holds {
            failed.push(u32::try_from(label).map_err(|_| WitnessError::Length)?);
        }
    }
    Ok(failed)
}

/// The canonical bytes of a witness: the least packed image over the
/// symmetries that preserve the request, compared as bytes.
///
/// The identity always passes for a well shaped request, so the result is
/// never longer than the input.
///
/// # Errors
///
/// [`WitnessError::Request`] when the request has the wrong shape, then a
/// [`WitnessError`] when the bytes are malformed.
pub fn canonical(
    lattice: &SquareLattice,
    request: &Request,
    bytes: &[u8],
) -> Result<Vec<u8>, WitnessError> {
    let group = symmetries(lattice, request).map_err(WitnessError::Request)?;
    let spins = unpack(bytes, lattice.link_count())?;
    let mut least = bytes.to_vec();
    let mut image = Vec::new();
    let mut packed = Vec::new();
    for symmetry in group {
        symmetry
            .apply_into(lattice, &spins, &mut image)
            .ok_or(WitnessError::Value)?;
        pack_into(&image, &mut packed)?;
        if packed < least {
            least.clone_from(&packed);
        }
    }
    Ok(least)
}
