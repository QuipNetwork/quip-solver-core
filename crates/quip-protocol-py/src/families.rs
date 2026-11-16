//! Family generation and checks for `quip_solver_core._core.families`.
//!
//! Every function is a thin call into `ising-generators`, so the instance
//! bytes a Python miner builds are the bytes Rust builds. Family faults raise
//! a subclass of [`FamilyError`] with `code` and `name` attributes (FAM-42).
//! Compact codec faults raise [`CodecError`] with a stable `name` and no FAM code.
//! A wrong Python argument type stays a `TypeError`.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};

use ising_generators::descriptor::{self, AdvertisedLimits, SizeBounds};
use ising_generators::family::{self, BaseTopology, Defect, SaltFault};
use ising_generators::instance::{self, InstanceLimits, InstanceParts};
use ising_generators::registry::CONTRACTS;
use ising_generators::Code;
use pyo3::exceptions::{PyMemoryError, PyOverflowError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedBytes;
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

/// Little-endian `int64` bytes of `values`, written straight into the bytes
/// object.
fn le_i64(
    py: Python<'_>,
    len: usize,
    values: impl Iterator<Item = i64>,
) -> PyResult<Bound<'_, PyBytes>> {
    let size = len
        .checked_mul(8)
        .ok_or_else(|| PyOverflowError::new_err("the buffer size overflows"))?;
    PyBytes::new_with(py, size, |buf| {
        for (chunk, value) in buf.chunks_exact_mut(8).zip(values) {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        Ok(())
    })
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

    /// `node_ids` as little-endian `int64`, 8 bytes per node.
    #[getter]
    fn node_ids_i64<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let ids = self.inner.node_ids();
        le_i64(py, ids.len(), ids.iter().map(|&id| i64::from(id)))
    }

    /// `edges` as little-endian `int64`, flat: `u0, v0, u1, v1, ...`.
    #[getter]
    fn edges_i64<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let edges = self.inner.edges();
        let len = edges
            .len()
            .checked_mul(2)
            .ok_or_else(|| PyOverflowError::new_err("the buffer size overflows"))?;
        let flat = edges
            .iter()
            .flat_map(|&(u, v)| [i64::from(u), i64::from(v)]);
        le_i64(py, len, flat)
    }

    /// `h` as little-endian `int64`, 8 bytes per node.
    #[getter]
    fn h_i64<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let h = self.inner.h();
        le_i64(py, h.len(), h.iter().map(|&v| i64::from(v)))
    }

    /// `j` as little-endian `int64`, 8 bytes per edge.
    #[getter]
    fn j_i64<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let j = self.inner.j();
        le_i64(py, j.len(), j.iter().map(|&v| i64::from(v)))
    }

    #[getter]
    fn offset_milli(&self) -> i64 {
        self.inner.offset_milli()
    }

    #[getter]
    fn metadata<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.metadata())
    }

    /// The instance digest (FAM-18). Computed on each read, with the GIL
    /// released.
    #[getter]
    fn digest<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        let inner = &self.inner;
        let digest = py.detach(|| inner.digest());
        digest_bytes(py, &digest)
    }

    /// The canonical instance bytes (FAM-30). Encoded on each call, with the
    /// GIL released.
    fn canonical_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let inner = &self.inner;
        let bytes = py
            .detach(|| inner.canonical_bytes())
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

/// Why generation gave no instance, before it becomes a Python exception.
/// Generation runs with the GIL released, so it cannot build one itself.
enum Failure {
    /// `AdmissionError` with this code and message.
    Admission(Code, String),
    /// `SaltSkipped` or `GeneratorDefect`.
    Salt(SaltFault),
}

impl Failure {
    fn into_py(self, py: Python<'_>) -> PyErr {
        match self {
            Self::Admission(code, message) => coded::<AdmissionError>(py, code, &message),
            Self::Salt(fault) => salt_error(py, fault),
        }
    }
}

fn salt_error(py: Python<'_>, salt: SaltFault) -> PyErr {
    match salt {
        SaltFault::Skip(code) => coded::<SaltSkipped>(py, code, "the salt gives no instance"),
        SaltFault::Defect(defect) => {
            fault::<GeneratorDefect>(py, 0, defect_name(defect), "construction defect")
        }
    }
}

/// The base topology and the admitted size bounds of a descriptor. This is
/// the FAM-47 topology admission that `generate` and `Generator` share.
fn admit_base(
    admitted: &descriptor::Admitted<'_>,
    nodes: Vec<u32>,
    edges: &[(u32, u32)],
    advertised: &AdvertisedLimits,
) -> Result<(BaseTopology, SizeBounds), Failure> {
    let topology = BaseTopology::new(nodes, edges)
        .map_err(|fault| Failure::Admission(Code::TopologyIncompatible, format!("{fault:?}")))?;
    let order_digest = *topology.order_digest();
    let bounds = family::admit_topology(admitted, &topology, &order_digest, advertised)
        .map_err(|code| Failure::Admission(code, "topology admission failed".to_owned()))?;
    Ok((topology, bounds))
}

fn nonce32(nonce: &[u8]) -> PyResult<[u8; 32]> {
    array32(nonce).ok_or_else(|| PyValueError::new_err("nonce must be exactly 32 bytes"))
}

/// Build the canonical instance of `nonce` on the topology `nodes` and `edges`.
///
/// `edges` holds native node ids in registration order. A limit of 0 makes
/// no advertised claim. The sent order digest is the topology's own, so code
/// 11 cannot occur here. The topology work and the construction run with the
/// GIL released.
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
    let advertised = AdvertisedLimits {
        max_nodes,
        max_edges,
        max_degree,
    };
    let (topology, bounds) = py
        .detach(|| admit_base(&admitted, nodes, &edges, &advertised))
        .map_err(|failure| failure.into_py(py))?;
    // The nonce check stays after topology admission, as before.
    let nonce = nonce32(nonce)?;
    py.detach(|| family::generate_instance(&admitted, &topology, &bounds, nonce))
        .map(|inner| Instance { inner })
        .map_err(|fault| salt_error(py, fault))
}

/// The nonce-independent state of one descriptor on one topology.
struct Prepared {
    family_id: String,
    revision: u32,
    params: Vec<u8>,
    params_digest: [u8; 32],
    topology: BaseTopology,
    bounds: SizeBounds,
}

impl Prepared {
    /// The canonical instance of `nonce`. Runs without the GIL.
    fn generate(&self, nonce: [u8; 32]) -> Result<instance::Instance, Failure> {
        // The same inputs passed admission when the generator was built.
        let admitted = descriptor::admit(
            CONTRACTS,
            self.family_id.as_bytes(),
            self.revision,
            &self.params,
            &self.params_digest,
        )
        .map_err(|code| Failure::Admission(code, "descriptor admission failed".to_owned()))?;
        family::generate_instance(&admitted, &self.topology, &self.bounds, nonce)
            .map_err(Failure::Salt)
    }

    /// The instance of every nonce, in order, on up to `threads` threads.
    fn generate_all(
        &self,
        nonces: &[[u8; 32]],
        threads: NonZeroUsize,
    ) -> Result<Vec<Result<instance::Instance, Failure>>, &'static str> {
        let workers = threads.get().min(nonces.len());
        if workers <= 1 {
            return Ok(nonces.iter().map(|&n| self.generate(n)).collect());
        }
        // Each worker takes the next unclaimed index, so a slow salt does not
        // hold back a fixed share of the batch.
        let next = AtomicUsize::new(0);
        let work = || {
            let mut done = Vec::new();
            loop {
                let at = next.fetch_add(1, Ordering::Relaxed);
                let Some(&nonce) = nonces.get(at) else {
                    return done;
                };
                done.push((at, self.generate(nonce)));
            }
        };
        let parts = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers).map(|_| scope.spawn(work)).collect();
            handles
                .into_iter()
                .map(std::thread::ScopedJoinHandle::join)
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| "a generation thread panicked")?;
        let mut slots: Vec<Option<Result<instance::Instance, Failure>>> =
            std::iter::repeat_with(|| None).take(nonces.len()).collect();
        for (at, result) in parts.into_iter().flatten() {
            if let Some(slot) = slots.get_mut(at) {
                *slot = Some(result);
            }
        }
        slots
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or("a nonce was not generated")
    }
}

/// A descriptor admitted on one topology, ready to generate any nonce.
///
/// The constructor runs descriptor and topology admission once. `generate`
/// then takes only the nonce and builds the same instance as the
/// module-level `generate` with the same arguments. Both methods release the
/// GIL while they build.
#[pyclass(frozen, module = "quip_solver_core")]
struct Generator {
    prepared: Prepared,
}

#[pymethods]
impl Generator {
    /// Admit the descriptor and the topology.
    ///
    /// # Errors
    ///
    /// `AdmissionError` (3 to 9, 12, 13), as `generate` raises it.
    #[new]
    #[pyo3(signature = (
        family_id, revision, params, params_digest, nodes, edges,
        *, max_nodes = 0, max_edges = 0, max_degree = 0
    ))]
    #[expect(
        clippy::too_many_arguments,
        clippy::needless_pass_by_value,
        reason = "the Python signature is the API, and PyO3 extracts owned lists"
    )]
    fn new(
        py: Python<'_>,
        family_id: &str,
        revision: u32,
        params: &[u8],
        params_digest: &[u8],
        nodes: Vec<u32>,
        edges: Vec<(u32, u32)>,
        max_nodes: u64,
        max_edges: u64,
        max_degree: u64,
    ) -> PyResult<Self> {
        let admitted = admit(py, family_id, revision, params, params_digest)?;
        let sent_digest = array32(params_digest).ok_or_else(|| {
            coded::<AdmissionError>(
                py,
                Code::ParamsDigestMismatch,
                "descriptor admission failed",
            )
        })?;
        let advertised = AdvertisedLimits {
            max_nodes,
            max_edges,
            max_degree,
        };
        let (topology, bounds) = py
            .detach(|| admit_base(&admitted, nodes, &edges, &advertised))
            .map_err(|failure| failure.into_py(py))?;
        Ok(Self {
            prepared: Prepared {
                family_id: family_id.to_owned(),
                revision,
                params: params.to_vec(),
                params_digest: sent_digest,
                topology,
                bounds,
            },
        })
    }

    /// The canonical instance of `nonce`.
    ///
    /// # Errors
    ///
    /// `SaltSkipped` (13 or 14) or `GeneratorDefect`. A nonce that is not 32
    /// bytes is a `ValueError`.
    fn generate(&self, py: Python<'_>, nonce: &[u8]) -> PyResult<Instance> {
        let nonce = nonce32(nonce)?;
        let prepared = &self.prepared;
        py.detach(|| prepared.generate(nonce))
            .map(|inner| Instance { inner })
            .map_err(|failure| failure.into_py(py))
    }

    /// The instance of each nonce, in order, built on `threads` threads.
    ///
    /// `threads = 0` uses the available parallelism. Element `i` of the
    /// result is the `Instance` of `nonces[i]`, or a `SaltSkipped` exception
    /// object (not raised) when that salt gives no instance. A defect raises
    /// `GeneratorDefect` for the first defective nonce in order, after every
    /// nonce is built.
    ///
    /// # Errors
    ///
    /// `GeneratorDefect`, or `ValueError` when a nonce is not 32 bytes. That
    /// check runs before any generation.
    #[pyo3(signature = (nonces, *, threads = 0))]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "PyO3 extracts an owned list of nonces"
    )]
    fn generate_batch(
        &self,
        py: Python<'_>,
        nonces: Vec<PyBackedBytes>,
        threads: usize,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let mut fixed = Vec::with_capacity(nonces.len());
        for (at, nonce) in nonces.iter().enumerate() {
            fixed.push(array32(nonce).ok_or_else(|| {
                PyValueError::new_err(format!("nonce {at} must be exactly 32 bytes"))
            })?);
        }
        let threads = NonZeroUsize::new(threads)
            .or_else(|| std::thread::available_parallelism().ok())
            .unwrap_or(NonZeroUsize::MIN);
        let prepared = &self.prepared;
        let results = py
            .detach(|| prepared.generate_all(&fixed, threads))
            .map_err(PyRuntimeError::new_err)?;
        let mut out = Vec::with_capacity(results.len());
        for result in results {
            match result {
                Ok(inner) => out.push(Py::new(py, Instance { inner })?.into_any()),
                Err(Failure::Salt(SaltFault::Skip(code))) => {
                    let err = coded::<SaltSkipped>(py, code, "the salt gives no instance");
                    out.push(err.into_value(py).into_any());
                }
                Err(failure) => return Err(failure.into_py(py)),
            }
        }
        Ok(out)
    }
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
    families.add_class::<Generator>()?;
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
