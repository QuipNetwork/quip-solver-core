//! Admission through a contract, for tests that declare it with
//! `#[path = "common/admit.rs"] mod admit;`.

use ising_generators::descriptor::{admit, params_digest, Admitted, Contract};
use ising_generators::Code;

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
