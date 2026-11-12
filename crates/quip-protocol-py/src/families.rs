//! Family generation and checks for `quip_solver_core._core.families`.
//!
//! Every function is a thin call into `ising-generators`, so the instance
//! bytes a Python miner builds are the bytes Rust builds. Family faults raise
//! a subclass of [`FamilyError`] with `code` and `name` attributes (FAM-42).
//! Compact codec faults raise [`CodecError`] with a stable `name` and no FAM code.
//! A wrong Python argument type stays a `TypeError`.

use ising_generators::descriptor::{self, AdvertisedLimits};
use ising_generators::family::{self, BaseTopology, Defect, SaltFault};
use ising_generators::instance::{self, InstanceLimits, InstanceParts};
use ising_generators::registry::CONTRACTS;
use ising_generators::Code;
use pyo3::exceptions::{PyMemoryError, PyOverflowError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3::{create_exception, wrap_pyfunction, PyTypeInfo};

create_exception!(
    quip_solver_core,
    FamilyError,
    PyValueError,
    "A family fault. `code` is the FAM-42 code and `name` its stable name."
);
create_exception!(
    quip_solver_core,
    AdmissionError,
    FamilyError,
    "The descriptor, parameters, or topology fail admission (codes 3 to 13)."
);
create_exception!(
    quip_solver_core,
    SaltSkipped,
    FamilyError,
    "The salt gives no instance (code 13 or 14). Skip it and continue."
);
create_exception!(
    quip_solver_core,
    InstanceInvalid,
    FamilyError,
    "An explicit instance is invalid (codes 13, 15, and 16)."
);
create_exception!(
    quip_solver_core,
    GeneratorDefect,
    FamilyError,
    "The construction broke its contract. `code` is 0 and `name` is the defect."
);

/// Build `E` with a message and the `code` and `name` attributes.
fn fault<E: PyTypeInfo>(py: Python<'_>, code: u8, name: &str, message: &str) -> PyErr {
    let err = PyErr::new::<E, _>(format!("{name} ({code}): {message}"));
    let value = err.value(py);
    if let Err(e) = value
        .setattr("code", code)
        .and_then(|()| value.setattr("name", name))
    {
        return e;
    }
    err
}

fn coded<E: PyTypeInfo>(py: Python<'_>, code: Code, message: &str) -> PyErr {
    fault::<E>(py, code.value(), &code.to_string(), message)
}

/// The stable name of a defect.
const fn defect_name(defect: Defect) -> &'static str {
    match defect {
        Defect::UnlistedLabel => "unlisted_label",
        Defect::ConstructionCode(_) => "construction_code",
        Defect::InstanceInvalid(_) => "instance_invalid",
        Defect::SizeBound => "size_bound",
        Defect::MetadataRejected => "metadata_rejected",
        Defect::NoConstruction => "no_construction",
        _ => "unknown",
    }
}

fn digest_bytes<'py>(py: Python<'py>, digest: &[u8; 32]) -> Bound<'py, PyBytes> {
    PyBytes::new(py, digest)
}

/// A digest or nonce of exactly 32 bytes, or `None`.
fn array32(bytes: &[u8]) -> Option<[u8; 32]> {
    <[u8; 32]>::try_from(bytes).ok()
}

/// An immutable canonical instance. Coefficients keep the integer milli scale.
#[pyclass(frozen, module = "quip_solver_core")]
struct Instance {
    inner: instance::Instance,
}

#[pymethods]
impl Instance {
    #[getter]
    fn descriptor_digest<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        digest_bytes(py, self.inner.descriptor_digest())
    }

    #[getter]
    fn topology_order_digest<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        digest_bytes(py, self.inner.topology_order_digest())
    }

    /// The nonce, or `None` for an archived instance.
    #[getter]
    fn nonce<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.inner.nonce().map(|n| digest_bytes(py, n))
    }

    #[getter]
    fn node_ids(&self) -> Vec<u32> {
        self.inner.node_ids().to_vec()
    }

    #[getter]
    fn edges(&self) -> Vec<(u32, u32)> {
        self.inner.edges().to_vec()
    }

    #[getter]
    fn h(&self) -> Vec<i32> {
        self.inner.h().to_vec()
    }

    #[getter]
    fn j(&self) -> Vec<i32> {
        self.inner.j().to_vec()
    }

    #[getter]
    fn offset_milli(&self) -> i64 {
        self.inner.offset_milli()
    }

    #[getter]
    fn metadata<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.metadata())
    }

    /// The instance digest (FAM-18).
    #[getter]
    fn digest<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        digest_bytes(py, &self.inner.digest())
    }

    /// The canonical instance bytes (FAM-30).
    fn canonical_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = self
            .inner
            .canonical_bytes()
            .map_err(|e| PyMemoryError::new_err(e.to_string()))?;
        Ok(PyBytes::new(py, &bytes))
    }
}

/// Admit the descriptor against the registry, raising `AdmissionError`.
fn admit<'a>(
    py: Python<'_>,
    family_id: &'a str,
    revision: u32,
    params: &'a [u8],
    params_digest: &[u8],
) -> PyResult<descriptor::Admitted<'a>> {
    descriptor::admit(
        CONTRACTS,
        family_id.as_bytes(),
        revision,
        params,
        params_digest,
    )
    .map_err(|code| coded::<AdmissionError>(py, code, "descriptor admission failed"))
}

/// The `(family_id, revision)` pairs this build implements, in registry order.
#[pyfunction]
fn supported() -> Vec<(&'static str, u32)> {
    CONTRACTS
        .iter()
        .map(|c| (c.family_id, c.revision))
        .collect()
}

/// The BLAKE3 digest of the parameter bytes (FAM-13).
#[pyfunction]
fn params_digest<'py>(py: Python<'py>, params: &[u8]) -> Bound<'py, PyBytes> {
    digest_bytes(py, &descriptor::params_digest(params))
}

/// The BLAKE3 topology order digest of `nodes` and `edges` in the given order
/// (FAM-19).
///
/// # Errors
///
/// `OverflowError` when a count is above `u32::MAX`.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "the Python signature is the API, and PyO3 extracts owned lists"
)]
fn topology_order_digest(
    py: Python<'_>,
    nodes: Vec<u32>,
    edges: Vec<(u32, u32)>,
) -> PyResult<Bound<'_, PyBytes>> {
    let digest = descriptor::topology_order_digest(&nodes, &edges)
        .ok_or_else(|| PyOverflowError::new_err("a count is above u32::MAX"))?;
    Ok(digest_bytes(py, &digest))
}

/// Check a descriptor.
///
/// # Errors
///
/// `AdmissionError` with a code from 3 to 9.
#[pyfunction]
#[pyo3(name = "admit")]
fn admit_descriptor(
    py: Python<'_>,
    family_id: &str,
    revision: u32,
    params: &[u8],
    params_digest: &[u8],
) -> PyResult<()> {
    admit(py, family_id, revision, params, params_digest).map(|_| ())
}

/// The descriptor digest of an admitted descriptor (FAM-18).
///
/// An explicit family job carries no descriptor digest, so the solver computes
/// it from `FamilyBinding.family` (FAM-68) and passes it to `validate_instance`.
///
/// # Errors
///
/// `AdmissionError` with a code from 3 to 9.
#[pyfunction]
fn descriptor_digest<'py>(
    py: Python<'py>,
    family_id: &str,
    revision: u32,
    params: &[u8],
    params_digest: &[u8],
) -> PyResult<Bound<'py, PyBytes>> {
    let admitted = admit(py, family_id, revision, params, params_digest)?;
    Ok(digest_bytes(py, &admitted.descriptor().digest()))
}

/// Build the canonical instance of `nonce` on the topology `nodes` and `edges`.
///
/// `edges` holds native node ids in registration order. A limit of 0 makes
/// no advertised claim. The sent order digest is the topology's own, so code
/// 11 cannot occur here.
///
/// # Errors
///
/// `AdmissionError` (3 to 9, 12, 13), `SaltSkipped` (13 or 14), or
/// `GeneratorDefect`. A nonce that is not 32 bytes is a `ValueError`.
#[pyfunction]
#[pyo3(signature = (
    family_id, revision, params, params_digest, nodes, edges, nonce,
    *, max_nodes = 0, max_edges = 0, max_degree = 0
))]
#[expect(
    clippy::too_many_arguments,
    clippy::needless_pass_by_value,
    reason = "the Python signature is the API, and PyO3 extracts owned lists"
)]
fn generate(
    py: Python<'_>,
    family_id: &str,
    revision: u32,
    params: &[u8],
    params_digest: &[u8],
    nodes: Vec<u32>,
    edges: Vec<(u32, u32)>,
    nonce: &[u8],
    max_nodes: u64,
    max_edges: u64,
    max_degree: u64,
) -> PyResult<Instance> {
    let admitted = admit(py, family_id, revision, params, params_digest)?;
    let topology = BaseTopology::new(nodes, &edges).map_err(|fault| {
        coded::<AdmissionError>(py, Code::TopologyIncompatible, &format!("{fault:?}"))
    })?;
    let advertised = AdvertisedLimits {
        max_nodes,
        max_edges,
        max_degree,
    };
    let order_digest = *topology.order_digest();
    let bounds = family::admit_topology(&admitted, &topology, &order_digest, &advertised)
        .map_err(|code| coded::<AdmissionError>(py, code, "topology admission failed"))?;
    let nonce =
        array32(nonce).ok_or_else(|| PyValueError::new_err("nonce must be exactly 32 bytes"))?;
    family::generate_instance(&admitted, &topology, &bounds, nonce)
        .map(|inner| Instance { inner })
        .map_err(|err| match err {
            SaltFault::Skip(code) => coded::<SaltSkipped>(py, code, "the salt gives no instance"),
            SaltFault::Defect(defect) => {
                fault::<GeneratorDefect>(py, 0, defect_name(defect), "construction defect")
            }
        })
}

/// Rebuild and check an explicit instance in the FAM-65 order.
///
/// `nonce` is `None` or `b""` for an archived instance, as on the wire.
///
/// # Errors
///
/// `AdmissionError` (3 to 9) from the descriptor, then `InstanceInvalid` with
/// code 13, 15, or 16.
#[pyfunction]
#[pyo3(signature = (
    family_id, revision, params, params_digest, descriptor_digest,
    topology_order_digest, nonce, node_ids, edges, h, j, offset_milli,
    metadata, instance_digest
))]
#[expect(clippy::too_many_arguments, reason = "the Python signature is the API")]
fn validate_instance(
    py: Python<'_>,
    family_id: &str,
    revision: u32,
    params: &[u8],
    params_digest: &[u8],
    descriptor_digest: &[u8],
    topology_order_digest: &[u8],
    nonce: Option<&[u8]>,
    node_ids: Vec<u32>,
    edges: Vec<(u32, u32)>,
    h: Vec<i32>,
    j: Vec<i32>,
    offset_milli: i64,
    metadata: Vec<u8>,
    instance_digest: &[u8],
) -> PyResult<Instance> {
    let admitted = admit(py, family_id, revision, params, params_digest)?;
    let mismatch =
        |message: &str| coded::<InstanceInvalid>(py, Code::InstanceDigestMismatch, message);
    let own = admitted.descriptor().digest();
    // An empty nonce is an archived instance, as on the wire.
    let nonce = nonce.filter(|bytes| !bytes.is_empty());
    let sent_nonce = nonce.map(array32);
    let order = array32(topology_order_digest);
    let sent = array32(instance_digest);
    // The FAM-37 bounds come first. A field of the wrong length reads as zeros
    // here and fails with code 15 below (FAM-65 step 2 before step 3).
    let parts = InstanceParts {
        descriptor_digest: own,
        topology_order_digest: order.unwrap_or([0; 32]),
        nonce: sent_nonce.map(|n| n.unwrap_or([0; 32])),
        node_ids,
        edges,
        h,
        j,
        offset_milli,
        metadata,
    };
    parts
        .check_bounds(&InstanceLimits::FAM_37)
        .map_err(|code| coded::<InstanceInvalid>(py, code, "the instance is beyond the bounds"))?;
    if array32(descriptor_digest) != Some(own) {
        return Err(mismatch("descriptor_digest differs from the descriptor"));
    }
    if order.is_none() {
        return Err(mismatch("topology_order_digest must be 32 bytes"));
    }
    if sent_nonce == Some(None) {
        return Err(mismatch("nonce must be empty or 32 bytes"));
    }
    let sent = sent.ok_or_else(|| mismatch("instance_digest must be 32 bytes"))?;
    let contract = admitted.contract();
    let inner = instance::Instance::new(
        parts,
        &InstanceLimits::FAM_37,
        contract.metadata_kinds,
        Some(&sent),
    )
    .map_err(|code| coded::<InstanceInvalid>(py, code, "the instance is invalid"))?;
    let accepted = contract
        .construction
        .is_some_and(|c| (c.check_metadata)(admitted.params(), &inner));
    if !accepted {
        return Err(coded::<InstanceInvalid>(
            py,
            Code::MetadataInvalid,
            "the metadata does not match the instance",
        ));
    }
    Ok(Instance { inner })
}

/// Whether `spins` is a feasible witness of `instance`.
///
/// # Errors
///
/// `AdmissionError` (3 to 9) from the descriptor, and `InstanceInvalid` 15 when
/// the instance was built for another descriptor.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts an owned Vec of spins from Python"
)]
fn feasible(
    py: Python<'_>,
    family_id: &str,
    revision: u32,
    params: &[u8],
    params_digest: &[u8],
    instance: &Instance,
    spins: Vec<i8>,
) -> PyResult<bool> {
    let admitted = admit(py, family_id, revision, params, params_digest)?;
    if instance.inner.descriptor_digest() != &admitted.descriptor().digest() {
        return Err(coded::<InstanceInvalid>(
            py,
            Code::InstanceDigestMismatch,
            "the instance belongs to another descriptor",
        ));
    }
    Ok(family::feasible(&admitted, &instance.inner, &spins))
}

create_exception!(
    quip_solver_core,
    CodecError,
    PyValueError,
    "A compact codec failure with a stable `name` attribute."
);

fn codec_error(py: Python<'_>, error: ising_generators::codec::CodecError) -> PyErr {
    if error == ising_generators::codec::CodecError::Allocation {
        return PyMemoryError::new_err(error.to_string());
    }
    let err = CodecError::new_err(error.to_string());
    if let Err(error) = err.value(py).setattr("name", error.name()) {
        return error;
    }
    err
}

/// Encode a structurally valid instance with admitted descriptor context.
#[pyfunction]
fn encode_instance<'py>(
    py: Python<'py>,
    family_id: &str,
    revision: u32,
    params: &[u8],
    params_digest: &[u8],
    instance: &Instance,
) -> PyResult<Bound<'py, PyBytes>> {
    let admitted = admit(py, family_id, revision, params, params_digest)?;
    let bytes = ising_generators::registry::encode_instance(&admitted, &instance.inner)
        .map_err(|error| codec_error(py, error))?;
    Ok(PyBytes::new(py, &bytes))
}

/// Decode compact bytes. Family validation remains a separate operation.
#[pyfunction]
fn decode_instance(
    py: Python<'_>,
    family_id: &str,
    revision: u32,
    params: &[u8],
    params_digest: &[u8],
    data: &[u8],
) -> PyResult<Instance> {
    let admitted = admit(py, family_id, revision, params, params_digest)?;
    ising_generators::registry::decode_instance(&admitted, data)
        .map(|inner| Instance { inner })
        .map_err(|error| codec_error(py, error))
}

/// Encode exact instance-order spins using a prepared family codec.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts an owned list of spins"
)]
fn encode_witness<'py>(
    py: Python<'py>,
    family_id: &str,
    revision: u32,
    params: &[u8],
    params_digest: &[u8],
    instance: &Instance,
    spins: Vec<i8>,
) -> PyResult<Bound<'py, PyBytes>> {
    let admitted = admit(py, family_id, revision, params, params_digest)?;
    let codec = ising_generators::registry::prepare_witness_codec(&admitted, &instance.inner)
        .map_err(|error| codec_error(py, error))?;
    let bytes = codec
        .encode(&spins)
        .map_err(|error| codec_error(py, error))?;
    Ok(PyBytes::new(py, &bytes))
}

/// Borrow the witness payload until the core length and padding checks finish.
#[pyfunction]
fn decode_witness(
    py: Python<'_>,
    family_id: &str,
    revision: u32,
    params: &[u8],
    params_digest: &[u8],
    instance: &Instance,
    data: &[u8],
) -> PyResult<Vec<i8>> {
    let admitted = admit(py, family_id, revision, params, params_digest)?;
    let codec = ising_generators::registry::prepare_witness_codec(&admitted, &instance.inner)
        .map_err(|error| codec_error(py, error))?;
    codec.decode(data).map_err(|error| codec_error(py, error))
}

/// Register the `families` submodule and the exception classes.
pub(crate) fn register(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    let families = PyModule::new(py, "families")?;
    families.add_function(wrap_pyfunction!(supported, &families)?)?;
    families.add_function(wrap_pyfunction!(params_digest, &families)?)?;
    families.add_function(wrap_pyfunction!(topology_order_digest, &families)?)?;
    families.add_function(wrap_pyfunction!(admit_descriptor, &families)?)?;
    families.add_function(wrap_pyfunction!(descriptor_digest, &families)?)?;
    families.add_function(wrap_pyfunction!(generate, &families)?)?;
    families.add_function(wrap_pyfunction!(validate_instance, &families)?)?;
    families.add_function(wrap_pyfunction!(feasible, &families)?)?;
    families.add_function(wrap_pyfunction!(encode_instance, &families)?)?;
    families.add_function(wrap_pyfunction!(decode_instance, &families)?)?;
    families.add_function(wrap_pyfunction!(encode_witness, &families)?)?;
    families.add_function(wrap_pyfunction!(decode_witness, &families)?)?;
    families.add_class::<Instance>()?;
    m.add("CodecError", py.get_type::<CodecError>())?;
    m.add_submodule(&families)?;

    let base = py.get_type::<FamilyError>();
    base.setattr("code", 0u8)?;
    base.setattr("name", "")?;
    m.add("FamilyError", base)?;
    m.add("AdmissionError", py.get_type::<AdmissionError>())?;
    m.add("SaltSkipped", py.get_type::<SaltSkipped>())?;
    m.add("InstanceInvalid", py.get_type::<InstanceInvalid>())?;
    m.add("GeneratorDefect", py.get_type::<GeneratorDefect>())?;
    Ok(())
}
