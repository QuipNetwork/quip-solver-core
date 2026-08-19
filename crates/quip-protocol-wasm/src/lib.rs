//! WebAssembly bindings for the consensus primitives in `quip-protocol`.
//!
//! Exposes the same surface as the `PyO3` bindings in `quip-protocol-py`:
//! `energy_milli`, `set_diversity`, the four wire codecs, and the `ExitCode`
//! constants. Both bindings call the identical Rust functions, so JavaScript,
//! Python, and Rust cannot disagree about consensus-critical math.

use quip_protocol::{scoring, wire};
use wasm_bindgen::prelude::*;

/// Ising energy in milli-units for the given spins, fields, and edges.
///
/// `edges` is a flat array of vertex index pairs: `[u0, v0, u1, v1, ...]`.
/// JavaScript has no tuple type, so the pairing is positional.
///
/// # Errors
///
/// Returns an error when `edges` has an odd length, or when its length does not
/// match `j`.
#[wasm_bindgen(js_name = energyMilli)]
pub fn energy_milli(spins: &[i8], h: &[f64], j: &[f64], edges: &[u32]) -> Result<i64, JsError> {
    if !edges.len().is_multiple_of(2) {
        return Err(JsError::new(
            "edges must hold vertex index pairs, so its length must be even",
        ));
    }
    let pairs: Vec<(usize, usize)> = edges
        .chunks_exact(2)
        .map(|pair| (pair[0] as usize, pair[1] as usize))
        .collect();
    if pairs.len() != j.len() {
        return Err(JsError::new(&format!(
            "edges holds {} pairs but j holds {} couplings; they must match",
            pairs.len(),
            j.len()
        )));
    }
    Ok(scoring::energy_milli(spins, h, j, &pairs))
}

/// Pairwise solution-set diversity in \[0, 1\].
///
/// `solutions` is a flat concatenation of equal-length spin vectors, split
/// every `width` entries, because `wasm-bindgen` cannot pass a jagged array.
///
/// # Errors
///
/// Returns an error when `width` is zero, or when `solutions` is not a whole
/// multiple of `width`.
#[wasm_bindgen(js_name = setDiversity)]
pub fn set_diversity(solutions: &[i8], width: usize) -> Result<f64, JsError> {
    if width == 0 {
        return Err(JsError::new("width must be greater than zero"));
    }
    if !solutions.len().is_multiple_of(width) {
        return Err(JsError::new(&format!(
            "solutions holds {} spins, which is not a whole multiple of width {width}",
            solutions.len()
        )));
    }
    let split: Vec<Vec<i8>> = solutions.chunks_exact(width).map(<[i8]>::to_vec).collect();
    Ok(scoring::set_diversity(&split))
}

/// Encode `i32` values as little-endian bytes.
#[wasm_bindgen(js_name = encodeI32Le)]
#[must_use]
pub fn encode_i32_le(values: &[i32]) -> Vec<u8> {
    wire::encode_i32_le(values)
}

/// Decode little-endian `i32` bytes.
///
/// # Errors
///
/// Returns an error when the byte length is not a multiple of 4.
#[wasm_bindgen(js_name = decodeI32Le)]
pub fn decode_i32_le(bytes: &[u8]) -> Result<Vec<i32>, JsError> {
    wire::decode_i32_le(bytes).map_err(|e| JsError::new(&e.to_string()))
}

/// Encode spins as packed bytes.
#[wasm_bindgen(js_name = encodeSpins)]
#[must_use]
pub fn encode_spins(spins: &[i8]) -> Vec<u8> {
    wire::encode_spins(spins)
}

/// Decode packed spin bytes.
///
/// # Errors
///
/// Returns an error when the payload is malformed.
#[wasm_bindgen(js_name = decodeSpins)]
pub fn decode_spins(bytes: &[u8]) -> Result<Vec<i8>, JsError> {
    wire::decode_spins(bytes).map_err(|e| JsError::new(&e.to_string()))
}

/// Process exit codes a solver reports back to the coordinator.
///
/// Mirrors `quip_solver_core._core.ExitCode` in the Python bindings.
#[wasm_bindgen]
pub struct ExitCode;

#[wasm_bindgen]
impl ExitCode {
    /// Normal termination.
    #[wasm_bindgen(getter = CLEAN)]
    #[must_use]
    pub fn clean() -> u8 {
        0
    }

    /// The supplied configuration was not usable.
    #[wasm_bindgen(getter = CONFIG_INVALID)]
    #[must_use]
    pub fn config_invalid() -> u8 {
        64
    }

    /// The host environment cannot run this solver.
    #[wasm_bindgen(getter = ENV_INCOMPATIBLE)]
    #[must_use]
    pub fn env_incompatible() -> u8 {
        69
    }

    /// An unrecoverable internal fault.
    #[wasm_bindgen(getter = INTERNAL_FATAL)]
    #[must_use]
    pub fn internal_fatal() -> u8 {
        70
    }

    /// The coordinator rejected the presented token.
    #[wasm_bindgen(getter = TOKEN_REJECTED)]
    #[must_use]
    pub fn token_rejected() -> u8 {
        77
    }
}
