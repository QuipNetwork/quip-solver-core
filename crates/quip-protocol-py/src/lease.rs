//! Lease salts, nonces, and the legacy draw for `quip_solver_core._core.lease`.
//!
//! Every call goes into `quip_protocol::lease`, so the salts, nonces, and
//! milli arrays a Python miner computes are the ones the Rust session and the
//! chain compute. Faults raise [`LeaseError`] with a stable `name` attribute.

use ising_generators::random::derive_nonce as derive;
use prost::Message;
use pyo3::exceptions::{PyIndexError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::{create_exception, wrap_pyfunction};
use quip_proto::v1::{EdgeList, IsingProblemGenerator, Topology};
use quip_protocol::chacha8::DrawError;
use quip_protocol::lease::{LeaseSpec, TopologyError, TopologyView};

create_exception!(
    quip_solver_core,
    LeaseError,
    PyValueError,
    "A lease or legacy draw fault. `name` is its stable name."
);

fn lease_error(py: Python<'_>, name: &str, message: &str) -> PyErr {
    let err = LeaseError::new_err(format!("{name}: {message}"));
    if let Err(e) = err.value(py).setattr("name", name) {
        return e;
    }
    err
}

/// The stable name of a lease specification fault.
const fn spec_fault_name(fault: quip_protocol::lease::LeaseError) -> &'static str {
    use quip_protocol::lease::LeaseError as E;
    match fault {
        E::ZeroCount => "zero_count",
        E::CounterOverflow => "counter_overflow",
        E::BadLength { .. } => "bad_length",
        E::UnknownGenerator(_) => "unknown_generator",
        E::DescriptorUnexpected => "descriptor_unexpected",
        E::DescriptorMissing => "descriptor_missing",
    }
}

/// A 32-byte input, or a `ValueError` naming `field`.
fn bytes32(bytes: &[u8], field: &str) -> PyResult<[u8; 32]> {
    <[u8; 32]>::try_from(bytes)
        .map_err(|_| PyValueError::new_err(format!("{field} must be exactly 32 bytes")))
}

/// A validated salt lease from a serialized `IsingProblemGenerator`.
///
/// Pass `generator.SerializeToString()`. The checks are the Rust session's:
/// field lengths, the counter range, and the FAM-48 descriptor rules.
#[pyclass(frozen, module = "quip_solver_core")]
struct Lease {
    spec: LeaseSpec,
}

#[pymethods]
impl Lease {
    /// # Errors
    ///
    /// `LeaseError` named `malformed` when the bytes do not decode, or the
    /// name of the specification fault.
    #[new]
    fn new(py: Python<'_>, generator: &[u8]) -> PyResult<Self> {
        let message = IsingProblemGenerator::decode(generator)
            .map_err(|e| lease_error(py, "malformed", &e.to_string()))?;
        LeaseSpec::from_proto(&message)
            .map(|spec| Self { spec })
            .map_err(|fault| lease_error(py, spec_fault_name(fault), &fault.to_string()))
    }

    #[getter]
    const fn salt_count(&self) -> u64 {
        self.spec.salt_count
    }

    /// The salt at `index`.
    ///
    /// # Errors
    ///
    /// `IndexError` when `index` is outside `0..salt_count`.
    fn salt<'py>(&self, py: Python<'py>, index: u64) -> PyResult<Bound<'py, PyBytes>> {
        let salt = self.spec.salt(index).ok_or_else(|| outside(index))?;
        Ok(PyBytes::new(py, &salt))
    }

    /// The nonce of the salt at `index`.
    ///
    /// # Errors
    ///
    /// `IndexError` when `index` is outside `0..salt_count`.
    fn nonce<'py>(&self, py: Python<'py>, index: u64) -> PyResult<Bound<'py, PyBytes>> {
        let nonce = self.spec.nonce(index).ok_or_else(|| outside(index))?;
        Ok(PyBytes::new(py, &nonce))
    }

    /// The index of `salt`, or `None` when the lease does not issue it.
    ///
    /// # Errors
    ///
    /// `ValueError` when `salt` is not 32 bytes.
    fn index_of(&self, salt: &[u8]) -> PyResult<Option<u64>> {
        Ok(self.spec.index_of(&bytes32(salt, "salt")?))
    }
}

fn outside(index: u64) -> PyErr {
    PyIndexError::new_err(format!("salt index {index} is outside the lease"))
}

/// `BLAKE3(last_proof || miner || salt)`, the nonce of one salt.
///
/// # Errors
///
/// `ValueError` when an input is not 32 bytes.
#[pyfunction]
fn derive_nonce<'py>(
    py: Python<'py>,
    last_proof: &[u8],
    miner: &[u8],
    salt: &[u8],
) -> PyResult<Bound<'py, PyBytes>> {
    let nonce = derive(
        bytes32(last_proof, "last_proof")?,
        bytes32(miner, "miner")?,
        bytes32(salt, "salt")?,
    );
    Ok(PyBytes::new(py, &nonce))
}

/// The `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1` draw of `nonce`.
///
/// `nodes` and `edges` are the cached `Topology` in registration order, with
/// native node ids. Returns `(h_milli, j_milli)`: one field per node, then one
/// coupling per edge, in that order.
///
/// # Errors
///
/// `ValueError` when `nonce` is not 32 bytes. `LeaseError` named
/// `duplicate_node`, `unknown_node`, or `empty_allowed_values`.
#[pyfunction]
fn draw(
    py: Python<'_>,
    nonce: &[u8],
    nodes: Vec<u32>,
    edges: Vec<(u32, u32)>,
    allowed_h_milli: Vec<i32>,
    allowed_j_milli: Vec<i32>,
) -> PyResult<(Vec<i32>, Vec<i32>)> {
    let nonce = bytes32(nonce, "nonce")?;
    let (u, v) = edges.into_iter().unzip();
    let topology = Topology {
        hash: Vec::new(),
        nodes,
        edges: Some(EdgeList { u, v }),
        allowed_h_milli,
        allowed_j_milli,
    };
    let view = TopologyView::from_proto(&topology).map_err(|fault| match fault {
        TopologyError::DuplicateNode(id) => {
            lease_error(py, "duplicate_node", &format!("node {id} occurs twice"))
        }
        TopologyError::UnknownNode(id) => {
            lease_error(py, "unknown_node", &format!("edge names unknown node {id}"))
        }
        // The pairs above unzip into lists of one length.
        TopologyError::UnequalEdgeLists => {
            lease_error(py, "unequal_edge_lists", "edge lists differ in length")
        }
    })?;
    view.draw(nonce).map_err(|fault| match fault {
        DrawError::EmptyAllowedValues => {
            lease_error(py, "empty_allowed_values", &fault.to_string())
        }
    })
}

/// Register the `lease` submodule and `LeaseError`.
pub(crate) fn register(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    let lease = PyModule::new(py, "lease")?;
    lease.add_class::<Lease>()?;
    lease.add_function(wrap_pyfunction!(derive_nonce, &lease)?)?;
    lease.add_function(wrap_pyfunction!(draw, &lease)?)?;
    m.add_submodule(&lease)?;

    let error = py.get_type::<LeaseError>();
    error.setattr("name", "")?;
    m.add("LeaseError", error)?;
    Ok(())
}
