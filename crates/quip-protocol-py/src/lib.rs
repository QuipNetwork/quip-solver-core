//! `PyO3` bindings for the consensus primitives in `quip-protocol`.
//!
//! Exposes `quip_solver_core._core.scoring`, `quip_solver_core._core.wire`,
//! and `quip_solver_core._core.ExitCode`. The package's `__init__` re-exports
//! these, so `from quip_solver_core import scoring, wire` works. Because the
//! math is the Rust source, the Python side cannot drift from it.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::wrap_pyfunction;

/// Ising energy in milli-units for the given spins, fields, and edges.
///
/// `PyO3` extracts owned `Vec`s from Python; the body only borrows them.
///
/// # Errors
///
/// Returns a Python `ValueError` when `edges` and `j` differ in length, matching
/// the WASM binding. Silently scoring a mismatched graph would let a solver
/// submit a confidently wrong energy.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts owned Vec arguments from Python objects"
)]
fn energy_milli(
    spins: Vec<i8>,
    h: Vec<f64>,
    j: Vec<f64>,
    edges: Vec<(usize, usize)>,
) -> PyResult<i64> {
    if edges.len() != j.len() {
        return Err(PyValueError::new_err(format!(
            "edges holds {} pairs but j holds {} couplings; they must match",
            edges.len(),
            j.len()
        )));
    }
    Ok(quip_protocol::scoring::energy_milli(&spins, &h, &j, &edges))
}

/// Pairwise solution-set diversity in \[0, 1\].
///
/// `PyO3` extracts an owned `Vec` of spin vectors from Python. Each inner
/// vector is one solution; their common length is the spin width.
///
/// # Errors
///
/// Returns a Python `ValueError` when the width is zero, or when the inner
/// vectors do not share a single width. Those are the same packing invariants
/// the WASM binding enforces on a flat buffer plus `width`.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts owned Vec arguments from Python objects"
)]
fn set_diversity(solutions: Vec<Vec<i8>>) -> PyResult<f64> {
    if let Some(first) = solutions.first() {
        let width = first.len();
        if width == 0 {
            return Err(PyValueError::new_err("width must be greater than zero"));
        }
        if solutions.iter().any(|row| row.len() != width) {
            return Err(PyValueError::new_err(format!(
                "solutions rows must share a single width; first row has {width}"
            )));
        }
    }
    Ok(quip_protocol::scoring::set_diversity(&solutions))
}

/// Encode `i32` values as little-endian bytes.
///
/// `PyO3` extracts an owned `Vec` from Python.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts owned Vec arguments from Python objects"
)]
fn encode_i32_le(values: Vec<i32>) -> Vec<u8> {
    quip_protocol::wire::encode_i32_le(&values)
}

/// Decode little-endian `i32` bytes.
///
/// # Errors
///
/// Returns a Python `ValueError` when the byte length is not a multiple of 4.
///
/// `PyO3` extracts an owned `Vec` from Python.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts owned Vec arguments from Python objects"
)]
fn decode_i32_le(b: Vec<u8>) -> PyResult<Vec<i32>> {
    quip_protocol::wire::decode_i32_le(&b).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Encode spins as packed bytes (`+1`/`-1` → bit representation).
///
/// `PyO3` extracts an owned `Vec` from Python.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts owned Vec arguments from Python objects"
)]
fn encode_spins(spins: Vec<i8>) -> Vec<u8> {
    quip_protocol::wire::encode_spins(&spins)
}

/// Decode packed spin bytes.
///
/// # Errors
///
/// Returns a Python `ValueError` when the payload is malformed.
///
/// `PyO3` extracts an owned `Vec` from Python.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts owned Vec arguments from Python objects"
)]
fn decode_spins(b: Vec<u8>) -> PyResult<Vec<i8>> {
    quip_protocol::wire::decode_spins(&b).map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Encode `{-1,+1}` spins to the bit-packed form `Solution.spins` carries.
///
/// `PyO3` extracts an owned `Vec` from Python.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts owned Vec arguments from Python objects"
)]
fn encode_spins_packed(spins: Vec<i8>) -> Vec<u8> {
    quip_protocol::wire::encode_spins_packed(&spins)
}

/// Decode `num_spins` bit-packed spins.
///
/// # Errors
///
/// Returns a Python `ValueError` when the byte length is not
/// `ceil(num_spins / 8)`, or when a padding bit is set.
///
/// `PyO3` extracts an owned `Vec` from Python.
#[pyfunction]
#[expect(
    clippy::needless_pass_by_value,
    reason = "PyO3 extracts owned Vec arguments from Python objects"
)]
fn decode_spins_packed(b: Vec<u8>, num_spins: usize) -> PyResult<Vec<i8>> {
    quip_protocol::wire::decode_spins_packed(&b, num_spins)
        .map_err(|e| PyValueError::new_err(e.to_string()))
}

/// Python module `quip_solver_core._core`: scoring, wire, and `ExitCode`.
#[pymodule]
fn _core(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    let scoring = PyModule::new(py, "scoring")?;
    scoring.add_function(wrap_pyfunction!(energy_milli, &scoring)?)?;
    scoring.add_function(wrap_pyfunction!(set_diversity, &scoring)?)?;
    m.add_submodule(&scoring)?;

    let wire = PyModule::new(py, "wire")?;
    wire.add_function(wrap_pyfunction!(encode_i32_le, &wire)?)?;
    wire.add_function(wrap_pyfunction!(decode_i32_le, &wire)?)?;
    wire.add_function(wrap_pyfunction!(encode_spins, &wire)?)?;
    wire.add_function(wrap_pyfunction!(decode_spins, &wire)?)?;
    wire.add_function(wrap_pyfunction!(encode_spins_packed, &wire)?)?;
    wire.add_function(wrap_pyfunction!(decode_spins_packed, &wire)?)?;
    m.add_submodule(&wire)?;

    let exit_code = PyModule::new(py, "ExitCode")?;
    exit_code.add("CLEAN", 0u8)?;
    exit_code.add("CONFIG_INVALID", 64u8)?;
    exit_code.add("ENV_INCOMPATIBLE", 69u8)?;
    exit_code.add("INTERNAL_FATAL", 70u8)?;
    exit_code.add("TOKEN_REJECTED", 77u8)?;
    m.add_submodule(&exit_code)?;

    Ok(())
}
