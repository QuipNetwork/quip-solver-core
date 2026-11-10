//! Helpers shared by the family tests: a lattice topology, a nonce, admission
//! through a contract, and the size bounds of an admitted descriptor.
//!
//! A family test file declares this module with
//! `#[path = "common/family.rs"] mod family;`.

use ising_generators::descriptor::{
    admit, params_digest, Admitted, AdvertisedLimits, Contract, SizeBounds,
};
use ising_generators::family::{admit_topology, BaseTopology};
use ising_generators::ice::zephyr_lattice;
use ising_generators::Code;

/// The z(`m`, `t`) Zephyr lattice as a base topology.
///
/// # Panics
///
/// When the shape is not a valid lattice.
pub(crate) fn lattice_topology(m: u16, t: u8) -> BaseTopology {
    let (nodes, edges) = zephyr_lattice(m, t).expect("lattice");
    BaseTopology::new(nodes, &edges).expect("topology")
}

/// A nonce whose 32 bytes all equal `i`.
pub(crate) fn nonce_of(i: u8) -> [u8; 32] {
    [i; 32]
}

/// Admit `bytes` as the parameters of `contract` at its own revision, with the
/// matching parameter digest.
///
/// # Errors
///
/// The admission code.
pub(crate) fn admitted<'a>(contract: &'a Contract, bytes: &'a [u8]) -> Result<Admitted<'a>, Code> {
    admit(
        std::slice::from_ref(contract),
        contract.family_id.as_bytes(),
        contract.revision,
        bytes,
        &params_digest(bytes),
    )
}

/// The size bounds of an admitted descriptor on `topology`, with the
/// topology's own order digest and default advertised limits.
///
/// # Errors
///
/// The admission code.
pub(crate) fn bounds(admitted: &Admitted<'_>, topology: &BaseTopology) -> Result<SizeBounds, Code> {
    admit_topology(
        admitted,
        topology,
        topology.order_digest(),
        &AdvertisedLimits::default(),
    )
}
