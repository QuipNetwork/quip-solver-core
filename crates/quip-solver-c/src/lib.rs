//! C ABI over the Quip solver session loop.
//!
//! A C or C++ solver registers one sampling callback and calls
//! [`quip_solver_run`]. Everything else — the four command-line modes, the
//! handshake, credits, cancellation, and the exit codes — is the Rust loop in
//! `quip-solver-core`. Following SPEC section 9, this wraps that loop rather
//! than reimplementing it, so a C solver cannot drift from a Rust one.
#![expect(
    unsafe_code,
    reason = "this crate is the C ABI boundary. Every extern fn validates its \
              pointers and hands off to quip-solver-core, which keeps the \
              workspace `unsafe-code = deny`."
)]

use std::ffi::{c_char, c_int, CStr};
use std::str::Utf8Error;

use clap::Parser;
use quip_solver_core::adapt::AdaptBounds;
use quip_solver_core::{
    run_code, BackendIdentity, CommonArgs, ExitCode, IsingGraph, OpenError, SampleError,
    SampleParams, Sampler, SamplerResult,
};

/// The sampling callback ran to completion.
pub const QUIP_SAMPLE_OK: i32 = 0;
/// The job exceeds a device bound. An identical job fails again.
pub const QUIP_SAMPLE_CAPACITY: i32 = 1;
/// The device is busy now. An identical job may succeed later.
pub const QUIP_SAMPLE_DEVICE_BUSY: i32 = 2;
/// The device needs a restart. The session sends `Fatal` and ends.
pub const QUIP_SAMPLE_DEVICE_FAULT: i32 = 3;

/// [`quip_energy_milli`] wrote the energy to its out-parameter.
pub const QUIP_ENERGY_OK: i32 = 0;
/// [`quip_energy_milli`] found a NULL pointer where its length argument
/// promised elements. The out-parameter is left untouched.
pub const QUIP_ENERGY_NULL_POINTER: i32 = 1;
/// [`quip_energy_milli`] found lengths that cannot describe one problem:
/// `num_spins` differs from `num_nodes`, or `2 * num_edges` overflows. The
/// out-parameter is left untouched.
pub const QUIP_ENERGY_LENGTH_MISMATCH: i32 = 2;

/// One Ising problem handed to the sampling callback.
///
/// `edges` is a flat array of `2 * num_edges` vertex indices: `u0, v0, u1, v1,
/// ...`. `h` holds `num_nodes` fields and `j` holds `num_edges` couplings. The
/// three lengths always agree: `num_edges` is derived from the edge list that
/// `edges` and `j` both describe, so reading `num_edges` couplings and
/// `2 * num_edges` indices is always in bounds.
#[repr(C)]
pub struct QuipIsingGraph {
    /// Local fields, one per node.
    pub h: *const f64,
    /// Number of nodes, and the length of `h`.
    pub num_nodes: usize,
    /// Couplings, one per edge.
    pub j: *const f64,
    /// Number of edges, and the length of `j`.
    pub num_edges: usize,
    /// Flat vertex index pairs, `2 * num_edges` entries. Every index is less
    /// than `num_nodes`.
    pub edges: *const u32,
}

/// Resolved sampling knobs for one job.
#[repr(C)]
pub struct QuipSampleParams {
    /// How many solutions to return.
    pub num_reads: usize,
    /// Sweeps in the anneal.
    pub num_sweeps: usize,
    /// Sweeps per beta rung.
    pub sweeps_per_beta: usize,
    /// Nonzero when `beta_min`/`beta_max` carry a pinned range.
    pub has_beta_range: i32,
    /// Start of the beta ladder, valid when `has_beta_range` is nonzero.
    pub beta_min: f64,
    /// End of the beta ladder, valid when `has_beta_range` is nonzero.
    pub beta_max: f64,
    /// Deterministic seed for this job.
    pub seed: u64,
}

/// Emits one solution from inside the sampling callback.
///
/// Call once per read. The bytes are copied before this returns, so the caller
/// may reuse or free `spins` immediately afterwards. `spins` may be NULL when
/// `num_spins` is 0.
pub type QuipEmitFn = unsafe extern "C" fn(
    sink: *mut core::ffi::c_void,
    spins: *const i8,
    num_spins: usize,
    energy_milli: i64,
);

/// The sampling callback a C solver registers.
///
/// Return [`QUIP_SAMPLE_OK`] after emitting the solutions, or one of the
/// `QUIP_SAMPLE_*` error codes. Must not unwind into Rust.
pub type QuipSampleFn = unsafe extern "C" fn(
    user_data: *mut core::ffi::c_void,
    graph: *const QuipIsingGraph,
    params: *const QuipSampleParams,
    emit: QuipEmitFn,
    sink: *mut core::ffi::c_void,
) -> i32;

/// What the solver advertises in `--capabilities` and `Hello`.
///
/// `backend` and `algorithm` are NUL-terminated strings, and may be NULL to
/// take the default. `features` is an array of NUL-terminated strings with
/// `num_features` entries, and may be NULL only when `num_features` is 0. Bytes
/// that are not UTF-8 end the run with the config-invalid exit code rather than
/// falling back to a default. All strings are copied during [`quip_solver_run`],
/// so the caller may free them once it returns.
#[repr(C)]
pub struct QuipBackendIdentity {
    /// Backend name, for example `"cpu"`. NULL means `"c"`.
    pub backend: *const c_char,
    /// Algorithm name, for example `"sa"`. NULL means `"custom"`.
    pub algorithm: *const c_char,
    /// Largest node count this solver accepts.
    pub max_nodes: u32,
    /// Largest edge count this solver accepts.
    pub max_edges: u32,
    /// Optional feature strings.
    pub features: *const *const c_char,
    /// Number of entries in `features`.
    pub num_features: usize,
    /// Lower bound on adaptive sweeps.
    pub min_sweeps: u32,
    /// Upper bound on adaptive sweeps.
    pub max_sweeps: u32,
    /// Lower bound on adaptive reads.
    pub min_reads: u32,
    /// Upper bound on adaptive reads.
    pub max_reads: u32,
    /// Reads-per-solution lower factor.
    pub reads_solution_min_factor: u32,
    /// Reads-per-solution upper factor.
    pub reads_solution_max_factor: u32,
    /// Reads-per-solution floor factor.
    pub reads_solution_floor_factor: u32,
}

/// Borrows `len` elements from `ptr`, and the empty slice when `len` is 0.
///
/// `slice::from_raw_parts` demands a non-null, aligned pointer even for a
/// zero-length slice, so passing NULL straight through is undefined behaviour
/// however harmless it looks. Every C-facing array in this crate goes through
/// here so that "nothing here" can be spelled NULL, which is what a C caller
/// does.
///
/// # Safety
///
/// When `len` is nonzero, `ptr` must address `len` initialised, aligned `T`
/// values that stay valid for the returned borrow.
unsafe fn borrow_or_empty<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
    if len == 0 {
        return &[];
    }
    // SAFETY: `len` is nonzero, so the caller's contract on `ptr` applies.
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

/// Reads a flat `2 * n` index array as `n` vertex pairs.
fn vertex_pairs(flat: &[u32]) -> Vec<(usize, usize)> {
    flat.chunks_exact(2)
        .filter_map(|pair| match pair {
            [u, v] => Some((*u as usize, *v as usize)),
            // `chunks_exact(2)` never yields another shape; matching rather
            // than indexing is what keeps this free of a panicking path.
            _ => None,
        })
        .collect()
}

/// Ising energy in milli-units, using the golden-pinned consensus scorer.
///
/// A C solver scores its own solutions with this rather than writing the loop
/// itself. The network rejects a solution whose reported energy disagrees, so a
/// second implementation is a liability, not a convenience.
///
/// `edges` holds `2 * num_edges` flat vertex index pairs. On
/// [`QUIP_ENERGY_OK`] the energy is written to `out_energy_milli`; on any other
/// status `out_energy_milli` is left untouched. The status is the only error
/// channel: 0 is a legal energy, so a caller that reads the energy without
/// checking the status cannot tell a scored problem from a rejected one.
///
/// Any array pointer may be NULL when its length is 0.
///
/// Returns [`QUIP_ENERGY_OK`], [`QUIP_ENERGY_NULL_POINTER`], or
/// [`QUIP_ENERGY_LENGTH_MISMATCH`].
///
/// # Safety
///
/// Each pointer must address the number of elements its length argument states,
/// and `out_energy_milli` must address one writable `int64_t`.
#[no_mangle]
pub unsafe extern "C" fn quip_energy_milli(
    h: *const f64,
    num_nodes: usize,
    j: *const f64,
    num_edges: usize,
    edges: *const u32,
    spins: *const i8,
    num_spins: usize,
    out_energy_milli: *mut i64,
) -> i32 {
    if out_energy_milli.is_null()
        || (num_nodes != 0 && h.is_null())
        || (num_edges != 0 && (j.is_null() || edges.is_null()))
        || (num_spins != 0 && spins.is_null())
    {
        return QUIP_ENERGY_NULL_POINTER;
    }
    if num_spins != num_nodes {
        return QUIP_ENERGY_LENGTH_MISMATCH;
    }
    // The `edges` buffer is twice as long as `num_edges`. On a wrapping
    // multiply that length would shrink, and `from_raw_parts` would hand the
    // scorer a slice reaching far past the allocation.
    let Some(flat_len) = num_edges.checked_mul(2) else {
        return QUIP_ENERGY_LENGTH_MISMATCH;
    };

    // SAFETY: every pointer is non-null wherever its length is nonzero, and the
    // caller's contract sizes each one; `flat_len` is the checked `2 *
    // num_edges` the header documents for `edges`.
    let (h, j, edges, spins) = unsafe {
        (
            borrow_or_empty(h, num_nodes),
            borrow_or_empty(j, num_edges),
            borrow_or_empty(edges, flat_len),
            borrow_or_empty(spins, num_spins),
        )
    };
    let pairs = vertex_pairs(edges);
    let energy = quip_solver_core::quip_protocol::scoring::energy_milli(spins, h, j, &pairs);

    // SAFETY: checked non-null above; the caller guarantees it addresses one
    // writable `i64`.
    unsafe { *out_energy_milli = energy };
    QUIP_ENERGY_OK
}

/// Clap wrapper so the C entry point can parse the normative command line
/// without the caller reimplementing it.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
}

/// Bridges the registered C callback into the Rust [`Sampler`] trait.
struct CSampler {
    sample: QuipSampleFn,
    user_data: *mut core::ffi::c_void,
}

// SAFETY: the session loop calls `sample` from a blocking worker thread and
// never concurrently for one `CSampler`. `user_data` is opaque to Rust; the C
// side owns it for the whole run and is responsible for its own interior
// synchronisation, which the header states as a contract.
unsafe impl Send for CSampler {}
// SAFETY: as for `Send`. `CSampler` holds no interior mutability of its own.
unsafe impl Sync for CSampler {}

/// Receives solutions emitted by the C callback.
struct Sink {
    results: Vec<SamplerResult>,
}

/// Appends one emitted solution. Installed as the [`QuipEmitFn`] for every call.
///
/// # Safety
///
/// `sink` must point to a live `Sink`, and `spins` must address `num_spins`
/// readable bytes unless `num_spins` is 0. Both hold for the duration of one
/// `sample` call.
unsafe extern "C" fn emit_solution(
    sink: *mut core::ffi::c_void,
    spins: *const i8,
    num_spins: usize,
    energy_milli: i64,
) {
    if sink.is_null() || (num_spins != 0 && spins.is_null()) {
        return;
    }
    // SAFETY: `sink` is the `&mut Sink` this call passed in.
    let sink = unsafe { &mut *sink.cast::<Sink>() };
    // SAFETY: `spins` addresses `num_spins` readable bytes per the header
    // contract; a zero length never touches the pointer.
    let spins = unsafe { borrow_or_empty(spins, num_spins) };
    sink.results.push(SamplerResult {
        spins: spins.to_vec(),
        energy_milli,
    });
}

impl CSampler {
    /// Flattens `graph.edges` into the `2 * num_edges` `u32` array the C ABI
    /// carries, or explains why the graph cannot cross the boundary.
    ///
    /// The session already rejects a job whose `j` and `edges` disagree, so a
    /// rejection here means a Rust-side bug. Catching it costs two comparisons
    /// and is the difference between a rejected job and a C solver reading past
    /// the end of an array it was told the length of.
    fn flatten_edges(graph: &IsingGraph) -> Result<Vec<u32>, String> {
        if graph.j.len() != graph.edges.len() {
            return Err(format!(
                "graph carries {} couplings for {} edges; the C ABI describes both with one \
                 num_edges",
                graph.j.len(),
                graph.edges.len()
            ));
        }
        let Some(flat_len) = graph.edges.len().checked_mul(2) else {
            return Err(format!(
                "{} edges overflow the 2 * num_edges index array the C ABI carries",
                graph.edges.len()
            ));
        };

        let num_nodes = graph.h.len();
        let mut flat = Vec::with_capacity(flat_len);
        for &(u, v) in &graph.edges {
            for endpoint in [u, v] {
                if endpoint >= num_nodes {
                    return Err(format!(
                        "edge endpoint {endpoint} is out of range for {num_nodes} nodes"
                    ));
                }
                match u32::try_from(endpoint) {
                    Ok(index) => flat.push(index),
                    Err(_) => {
                        return Err(format!(
                            "edge endpoint {endpoint} does not fit the u32 the C ABI carries"
                        ))
                    }
                }
            }
        }
        Ok(flat)
    }
}

impl Sampler for CSampler {
    fn sample(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let edges = match Self::flatten_edges(graph) {
            Ok(edges) => edges,
            Err(why) => {
                tracing::error!("refusing to hand the C sampler a malformed graph: {why}");
                // Capacity, not DeviceFault: this job can never succeed, but the
                // device is healthy and the session must survive to take the
                // next one.
                return Err(SampleError::Capacity);
            }
        };

        // Every length below comes from an array that is actually passed, so
        // the `2 * num_edges` and `num_edges` reads the header promises the C
        // callback are both in bounds.
        let c_graph = QuipIsingGraph {
            h: graph.h.as_ptr(),
            num_nodes: graph.h.len(),
            j: graph.j.as_ptr(),
            num_edges: graph.edges.len(),
            edges: edges.as_ptr(),
        };
        let (has_beta_range, beta_min, beta_max) = match params.beta_range {
            Some((lo, hi)) => (1, lo, hi),
            None => (0, 0.0, 0.0),
        };
        let c_params = QuipSampleParams {
            num_reads: params.num_reads,
            num_sweeps: params.num_sweeps,
            sweeps_per_beta: params.sweeps_per_beta,
            has_beta_range,
            beta_min,
            beta_max,
            seed: params.seed,
        };
        let mut sink = Sink {
            results: Vec::with_capacity(params.num_reads),
        };

        // SAFETY: both structs and the `edges` buffer they point into live
        // until this call returns, `emit_solution` matches QuipEmitFn, and
        // `sink` outlives the callback.
        let code = unsafe {
            (self.sample)(
                self.user_data,
                &raw const c_graph,
                &raw const c_params,
                emit_solution,
                (&raw mut sink).cast::<core::ffi::c_void>(),
            )
        };

        match code {
            QUIP_SAMPLE_OK => Ok(sink.results),
            QUIP_SAMPLE_CAPACITY => Err(SampleError::Capacity),
            QUIP_SAMPLE_DEVICE_BUSY => Err(SampleError::DeviceBusy),
            _ => Err(SampleError::DeviceFault(format!(
                "C sampler returned {code}"
            ))),
        }
    }
}

/// Copies a C string into a leaked `&'static str`.
///
/// `Ok(None)` is a NULL pointer, which every caller reads as "take the
/// default". `Err` is a string that is present but not UTF-8, which is a
/// configuration error: silently substituting a default there would advertise
/// an identity the operator never wrote.
///
/// # Safety
///
/// `s` must be NULL or point to a NUL-terminated string.
unsafe fn owned_str(s: *const c_char) -> Result<Option<&'static str>, Utf8Error> {
    if s.is_null() {
        return Ok(None);
    }
    // SAFETY: the caller guarantees NUL termination.
    let text = unsafe { CStr::from_ptr(s) }.to_str()?;
    // BackendIdentity holds &'static str and the process runs until the session
    // ends, so leaking one small string per field is the honest lifetime.
    Ok(Some(Box::leak(text.to_owned().into_boxed_str())))
}

/// Reads one identity string, or reports that the run must stop.
///
/// # Safety
///
/// `s` must be NULL or point to a NUL-terminated string.
unsafe fn identity_str(
    s: *const c_char,
    field: &str,
    default: &'static str,
) -> Result<&'static str, ExitCode> {
    // SAFETY: forwarded from this function's own contract.
    match unsafe { owned_str(s) } {
        Ok(Some(text)) => Ok(text),
        Ok(None) => Ok(default),
        Err(e) => {
            tracing::error!("identity {field} is not valid UTF-8: {e}");
            Err(ExitCode::ConfigInvalid)
        }
    }
}

/// Read a registered backend name. NULL selects the unspecified backend.
///
/// # Safety
/// The pointer must be NULL or name a NUL-terminated string.
unsafe fn identity_backend(
    s: *const c_char,
) -> Result<quip_solver_core::quip_proto::v1::Backend, ExitCode> {
    use quip_solver_core::quip_proto::v1::Backend;
    use quip_solver_core::quip_protocol::session::{backend_from_name, backend_name};
    // SAFETY: forwarded from this function's contract.
    let name = unsafe { identity_str(s, "backend", "unspecified") }?;
    backend_from_name(name).ok_or_else(|| {
        let accepted: Vec<_> = (0..=7)
            .filter_map(|v| Backend::try_from(v).ok())
            .map(backend_name)
            .collect();
        tracing::error!(
            "unknown backend {name:?}; accepted names: {}",
            accepted.join(", ")
        );
        ExitCode::ConfigInvalid
    })
}

/// Read a registered algorithm name. NULL selects the external algorithm.
///
/// # Safety
/// The pointer must be NULL or name a NUL-terminated string.
unsafe fn identity_algorithm(
    s: *const c_char,
) -> Result<quip_solver_core::quip_proto::v1::Algorithm, ExitCode> {
    use quip_solver_core::quip_proto::v1::Algorithm;
    use quip_solver_core::quip_protocol::session::{algorithm_from_name, algorithm_name};
    // SAFETY: forwarded from this function's contract.
    let name = unsafe { identity_str(s, "algorithm", "external") }?;
    algorithm_from_name(name).ok_or_else(|| {
        let accepted: Vec<_> = (0..=18)
            .filter_map(|v| Algorithm::try_from(v).ok())
            .map(algorithm_name)
            .collect();
        tracing::error!(
            "unknown algorithm {name:?}; accepted names: {}",
            accepted.join(", ")
        );
        ExitCode::ConfigInvalid
    })
}

/// Collects the advertised feature strings.
///
/// # Safety
///
/// When `id.num_features` is nonzero, `id.features` must address that many
/// NUL-terminated strings.
unsafe fn identity_features(id: &QuipBackendIdentity) -> Result<Vec<&'static str>, ExitCode> {
    if id.num_features == 0 {
        return Ok(Vec::new());
    }
    if id.features.is_null() {
        tracing::error!(
            "identity declares {} features but the features array is NULL",
            id.num_features
        );
        return Err(ExitCode::ConfigInvalid);
    }
    // SAFETY: non-null with `num_features` entries per the contract.
    let raw = unsafe { std::slice::from_raw_parts(id.features, id.num_features) };

    let mut features = Vec::with_capacity(raw.len());
    for (index, &feature) in raw.iter().enumerate() {
        // SAFETY: each entry is NUL-terminated per the contract.
        match unsafe { owned_str(feature) } {
            Ok(Some(text)) => features.push(text),
            // A hole in the array is the one case worth surviving: the identity
            // is still describable without it. It is never silent.
            Ok(None) => tracing::warn!("identity feature {index} is NULL and was dropped"),
            Err(e) => {
                tracing::error!("identity feature {index} is not valid UTF-8: {e}");
                return Err(ExitCode::ConfigInvalid);
            }
        }
    }
    Ok(features)
}

/// Runs a Quip solver: parses the command line, dispatches the mode, and drives
/// the session, calling `sample` for every job.
///
/// Returns the process exit code from SPEC section 2: 0 clean, 64 config
/// invalid, 69 environment incompatible, 70 internal fatal, 77 token rejected.
///
/// # Safety
///
/// `id` must point to a valid [`QuipBackendIdentity`]. `argv` must hold `argc`
/// NUL-terminated strings. `sample` must not unwind.
#[no_mangle]
pub unsafe extern "C" fn quip_solver_run(
    id: *const QuipBackendIdentity,
    argc: c_int,
    argv: *const *const c_char,
    sample: QuipSampleFn,
    user_data: *mut core::ffi::c_void,
) -> i32 {
    if id.is_null() || argv.is_null() || argc <= 0 {
        return ExitCode::ConfigInvalid as i32;
    }
    // SAFETY: checked non-null above; the caller guarantees the layout.
    let id = unsafe { &*id };

    let Ok(argc_usize) = usize::try_from(argc) else {
        return ExitCode::ConfigInvalid as i32;
    };
    // SAFETY: the caller guarantees `argv` holds `argc` NUL-terminated strings.
    let raw_args = unsafe { std::slice::from_raw_parts(argv, argc_usize) };
    let mut args: Vec<String> = Vec::with_capacity(argc_usize);
    for &arg in raw_args {
        if arg.is_null() {
            return ExitCode::ConfigInvalid as i32;
        }
        // SAFETY: non-null and NUL-terminated per the contract.
        match unsafe { CStr::from_ptr(arg) }.to_str() {
            Ok(s) => args.push(s.to_owned()),
            Err(_) => return ExitCode::ConfigInvalid as i32,
        }
    }

    let cli = match Cli::try_parse_from(&args) {
        Ok(c) => c,
        Err(e) => {
            // clap renders --help and --version through the same error path.
            let _ = e.print();
            return if e.use_stderr() {
                ExitCode::ConfigInvalid as i32
            } else {
                ExitCode::Clean as i32
            };
        }
    };

    // Install the log subscriber before reading the identity: a malformed
    // feature string is reported through `tracing`, and `run_code` does not
    // install one until after that point. A failure here needs no handling —
    // `run_code` runs the same check and reports it with the backend name
    // attached — and an install inside `run_code` keeps whichever subscriber
    // is already in place.
    let _ = quip_solver_core::logging::init(&cli.common.log_level);

    // SAFETY: the caller guarantees these are NUL-terminated or NULL.
    let backend = match unsafe { identity_backend(id.backend) } {
        Ok(text) => text,
        Err(code) => return code as i32,
    };
    // SAFETY: as above.
    let algorithm = match unsafe { identity_algorithm(id.algorithm) } {
        Ok(text) => text,
        Err(code) => return code as i32,
    };
    // SAFETY: `features` holds `num_features` NUL-terminated strings per the
    // contract, and the NULL case is handled inside.
    let features = match unsafe { identity_features(id) } {
        Ok(features) => features,
        Err(code) => return code as i32,
    };

    let identity = BackendIdentity {
        backend,
        algorithm,
        max_nodes: id.max_nodes,
        max_edges: id.max_edges,
        features: Box::leak(features.into_boxed_slice()),
        adapt: AdaptBounds {
            min_sweeps: id.min_sweeps,
            max_sweeps: id.max_sweeps,
            min_reads: id.min_reads,
            max_reads: id.max_reads,
            reads_solution_min_factor: id.reads_solution_min_factor,
            reads_solution_max_factor: id.reads_solution_max_factor,
            reads_solution_floor_factor: id.reads_solution_floor_factor,
        },
    };

    let exit = run_code(identity, &cli.common, || {
        Ok::<CSampler, OpenError>(CSampler { sample, user_data })
    });
    exit as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// A value no scorer produces, so a guard that returns without writing is
    /// distinguishable from one that wrote a real zero.
    const UNTOUCHED: i64 = i64::MIN;

    /// A two-node problem with one coupling.
    fn problem() -> (Vec<f64>, Vec<f64>, Vec<u32>, Vec<i8>) {
        (vec![1.0, -1.0], vec![0.5], vec![0, 1], vec![1, -1])
    }

    #[test]
    fn energy_matches_the_consensus_scorer() {
        let (h, j, edges, spins) = problem();
        let mut out = UNTOUCHED;
        // SAFETY: every pointer addresses the length passed beside it, and
        // `out` is one writable i64.
        let status = unsafe {
            quip_energy_milli(
                h.as_ptr(),
                h.len(),
                j.as_ptr(),
                j.len(),
                edges.as_ptr(),
                spins.as_ptr(),
                spins.len(),
                &raw mut out,
            )
        };
        assert_eq!(status, QUIP_ENERGY_OK);
        let expected =
            quip_solver_core::quip_protocol::scoring::energy_milli(&spins, &h, &j, &[(0, 1)]);
        assert_eq!(out, expected);
    }

    /// The reason the status code exists: 0 is a legal energy.
    #[test]
    fn a_genuine_zero_energy_reports_ok() {
        let h = [0.0, 0.0];
        let j = [0.0];
        let edges = [0_u32, 1];
        let spins = [1_i8, 1];
        let mut out = UNTOUCHED;
        // SAFETY: as above.
        let status = unsafe {
            quip_energy_milli(
                h.as_ptr(),
                h.len(),
                j.as_ptr(),
                j.len(),
                edges.as_ptr(),
                spins.as_ptr(),
                spins.len(),
                &raw mut out,
            )
        };
        assert_eq!(status, QUIP_ENERGY_OK);
        assert_eq!(out, 0, "a flat problem really does score zero");
    }

    #[test]
    fn a_null_field_array_is_reported_rather_than_scored() {
        let (h, j, edges, spins) = problem();
        let mut out = UNTOUCHED;
        // SAFETY: `h` is NULL, which the guard rejects before any read.
        let status = unsafe {
            quip_energy_milli(
                ptr::null(),
                h.len(),
                j.as_ptr(),
                j.len(),
                edges.as_ptr(),
                spins.as_ptr(),
                spins.len(),
                &raw mut out,
            )
        };
        assert_eq!(status, QUIP_ENERGY_NULL_POINTER);
        assert_eq!(out, UNTOUCHED);
    }

    #[test]
    fn a_null_spin_array_is_reported_rather_than_scored() {
        let (h, j, edges, spins) = problem();
        let mut out = UNTOUCHED;
        // SAFETY: `spins` is NULL, which the guard rejects before any read.
        let status = unsafe {
            quip_energy_milli(
                h.as_ptr(),
                h.len(),
                j.as_ptr(),
                j.len(),
                edges.as_ptr(),
                ptr::null(),
                spins.len(),
                &raw mut out,
            )
        };
        assert_eq!(status, QUIP_ENERGY_NULL_POINTER);
        assert_eq!(out, UNTOUCHED);
    }

    #[test]
    fn a_null_out_parameter_is_reported_rather_than_written() {
        let (h, j, edges, spins) = problem();
        // SAFETY: the out-parameter is NULL, which the guard rejects first.
        let status = unsafe {
            quip_energy_milli(
                h.as_ptr(),
                h.len(),
                j.as_ptr(),
                j.len(),
                edges.as_ptr(),
                spins.as_ptr(),
                spins.len(),
                ptr::null_mut(),
            )
        };
        assert_eq!(status, QUIP_ENERGY_NULL_POINTER);
    }

    #[test]
    fn spins_that_do_not_cover_the_nodes_are_reported() {
        let (h, j, edges, _) = problem();
        let spins = [1_i8, -1, 1];
        let mut out = UNTOUCHED;
        // SAFETY: every pointer addresses the length passed beside it; only the
        // node/spin counts disagree.
        let status = unsafe {
            quip_energy_milli(
                h.as_ptr(),
                h.len(),
                j.as_ptr(),
                j.len(),
                edges.as_ptr(),
                spins.as_ptr(),
                spins.len(),
                &raw mut out,
            )
        };
        assert_eq!(status, QUIP_ENERGY_LENGTH_MISMATCH);
        assert_eq!(out, UNTOUCHED);
    }

    #[test]
    fn an_edge_count_that_overflows_the_flat_length_is_reported() {
        let (h, j, edges, spins) = problem();
        let mut out = UNTOUCHED;
        // SAFETY: `num_edges` is rejected by the checked multiply before any
        // pointer is read, so the oversized length never reaches a slice.
        let status = unsafe {
            quip_energy_milli(
                h.as_ptr(),
                h.len(),
                j.as_ptr(),
                usize::MAX,
                edges.as_ptr(),
                spins.as_ptr(),
                spins.len(),
                &raw mut out,
            )
        };
        assert_eq!(status, QUIP_ENERGY_LENGTH_MISMATCH);
        assert_eq!(out, UNTOUCHED);
    }

    #[test]
    fn an_empty_problem_scores_zero_through_null_pointers() {
        let mut out = UNTOUCHED;
        // SAFETY: every length is 0, so no pointer is dereferenced; `out` is
        // one writable i64.
        let status = unsafe {
            quip_energy_milli(
                ptr::null(),
                0,
                ptr::null(),
                0,
                ptr::null(),
                ptr::null(),
                0,
                &raw mut out,
            )
        };
        assert_eq!(status, QUIP_ENERGY_OK);
        assert_eq!(out, 0);
    }

    #[test]
    fn an_empty_solution_is_recorded_without_dereferencing_null() {
        let mut sink = Sink {
            results: Vec::new(),
        };
        // SAFETY: `sink` is a live `Sink`; `spins` is NULL with a zero length,
        // which the ABI allows and `borrow_or_empty` never dereferences.
        unsafe {
            emit_solution(
                (&raw mut sink).cast::<core::ffi::c_void>(),
                ptr::null(),
                0,
                -7,
            );
        }
        assert_eq!(sink.results.len(), 1);
        let first = sink.results.first().expect("one result");
        assert_eq!(first.energy_milli, -7);
        assert!(first.spins.is_empty());
    }

    /// Set by [`refusing_sampler`], which must never run.
    static REACHED_C_SIDE: AtomicBool = AtomicBool::new(false);

    /// Records that the C side was entered at all.
    ///
    /// # Safety
    ///
    /// Matches [`QuipSampleFn`]; it touches none of its arguments.
    unsafe extern "C" fn refusing_sampler(
        _user_data: *mut core::ffi::c_void,
        _graph: *const QuipIsingGraph,
        _params: *const QuipSampleParams,
        _emit: QuipEmitFn,
        _sink: *mut core::ffi::c_void,
    ) -> i32 {
        REACHED_C_SIDE.store(true, Ordering::SeqCst);
        QUIP_SAMPLE_OK
    }

    #[test]
    fn a_graph_whose_couplings_and_edges_disagree_never_reaches_the_c_side() {
        let sampler = CSampler {
            sample: refusing_sampler,
            user_data: ptr::null_mut(),
        };
        // Two couplings, one edge: the old code advertised num_edges = 2 while
        // the flat array held 2 entries, so a callback reading 2 * num_edges
        // ran four entries off the end.
        let graph = IsingGraph::new(vec![0.0, 0.0], vec![0.5, 0.25], vec![(0, 1)]);
        let err = sampler
            .sample(&graph, &SampleParams::default())
            .expect_err("a mismatched graph must be rejected");
        assert_eq!(err, SampleError::Capacity);
        assert!(
            !REACHED_C_SIDE.load(Ordering::SeqCst),
            "the malformed graph must not cross the ABI at all"
        );
    }

    #[test]
    fn an_out_of_range_endpoint_is_rejected() {
        let graph = IsingGraph::new(vec![0.0, 0.0], vec![0.5], vec![(0, 7)]);
        let why = CSampler::flatten_edges(&graph).expect_err("endpoint 7 has no node");
        assert!(why.contains("out of range"), "{why}");
    }

    /// What [`inspecting_sampler`] read back out of the FFI struct.
    static SEEN_NUM_EDGES: AtomicUsize = AtomicUsize::new(usize::MAX);
    /// Sum of the `2 * num_edges` indices the callback was entitled to read.
    static SEEN_INDEX_SUM: AtomicUsize = AtomicUsize::new(usize::MAX);

    /// Reads exactly what the header entitles a C callback to read.
    ///
    /// # Safety
    ///
    /// Matches [`QuipSampleFn`]: `graph` points at a live [`QuipIsingGraph`]
    /// whose `edges` holds `2 * num_edges` entries.
    unsafe extern "C" fn inspecting_sampler(
        _user_data: *mut core::ffi::c_void,
        graph: *const QuipIsingGraph,
        _params: *const QuipSampleParams,
        _emit: QuipEmitFn,
        _sink: *mut core::ffi::c_void,
    ) -> i32 {
        // SAFETY: the caller passes a live struct for the duration of the call.
        let graph = unsafe { &*graph };
        SEEN_NUM_EDGES.store(graph.num_edges, Ordering::SeqCst);
        let flat_len = graph.num_edges * 2;
        // SAFETY: the header promises `2 * num_edges` readable entries, which
        // is exactly the claim this test exists to check.
        let flat = unsafe { borrow_or_empty(graph.edges, flat_len) };
        SEEN_INDEX_SUM.store(flat.iter().map(|&i| i as usize).sum(), Ordering::SeqCst);
        QUIP_SAMPLE_OK
    }

    #[test]
    fn num_edges_describes_the_buffer_the_callback_may_read() {
        let sampler = CSampler {
            sample: inspecting_sampler,
            user_data: ptr::null_mut(),
        };
        let graph = IsingGraph::new(vec![0.0, 0.0, 0.0], vec![0.5, 0.25], vec![(0, 1), (1, 2)]);
        let results = sampler
            .sample(&graph, &SampleParams::default())
            .expect("a well-formed graph is accepted");
        assert!(results.is_empty(), "this sampler emits nothing");
        assert_eq!(SEEN_NUM_EDGES.load(Ordering::SeqCst), 2);
        assert_eq!(
            SEEN_INDEX_SUM.load(Ordering::SeqCst),
            4,
            "the flat array reads back as 0, 1, 1, 2"
        );
    }

    #[test]
    fn an_edgeless_graph_crosses_the_boundary_as_zero_edges() {
        let graph = IsingGraph::new(vec![1.0, -1.0], Vec::new(), Vec::new());
        let flat = CSampler::flatten_edges(&graph).expect("no edges is well formed");
        assert!(flat.is_empty());
    }

    #[test]
    fn a_null_identity_string_takes_the_default() {
        // SAFETY: a NULL pointer is the documented "take the default" case.
        let backend = unsafe { identity_backend(ptr::null()) }.expect("NULL is legal");
        assert_eq!(
            backend,
            quip_solver_core::quip_proto::v1::Backend::Unspecified
        );
    }

    #[test]
    fn unknown_quantum_algorithm_is_rejected() {
        // SAFETY: the literal is a NUL-terminated string.
        assert_eq!(
            unsafe { identity_algorithm(c"quantum".as_ptr()) },
            Err(ExitCode::ConfigInvalid)
        );
    }

    #[test]
    fn an_identity_string_that_is_not_utf8_ends_the_run() {
        // 0xFF is never valid UTF-8; the trailing 0 terminates the string.
        let bytes: [c_char; 2] = [-1, 0];
        // SAFETY: `bytes` is NUL-terminated and outlives the call.
        let err = unsafe { identity_str(bytes.as_ptr(), "backend", "c") }
            .expect_err("invalid UTF-8 is not a default");
        assert_eq!(err as i32, ExitCode::ConfigInvalid as i32);
    }

    #[test]
    fn a_features_array_that_is_null_but_counted_ends_the_run() {
        let id = identity_with_features(ptr::null(), 3);
        // SAFETY: `num_features` is nonzero with a NULL array, which the guard
        // rejects before any read.
        let err = unsafe { identity_features(&id) }.expect_err("a counted NULL array is invalid");
        assert_eq!(err as i32, ExitCode::ConfigInvalid as i32);
    }

    #[test]
    fn a_null_feature_entry_is_dropped_and_the_rest_survive() {
        let good = c"fast";
        let entries: [*const c_char; 2] = [ptr::null(), good.as_ptr()];
        let id = identity_with_features(entries.as_ptr(), entries.len());
        // SAFETY: `entries` holds two pointers, one NULL and one
        // NUL-terminated, and outlives the call.
        let features = unsafe { identity_features(&id) }.expect("a hole is survivable");
        assert_eq!(features, vec!["fast"]);
    }

    #[test]
    fn a_feature_entry_that_is_not_utf8_ends_the_run() {
        let bad: [c_char; 2] = [-1, 0];
        let entries: [*const c_char; 1] = [bad.as_ptr()];
        let id = identity_with_features(entries.as_ptr(), entries.len());
        // SAFETY: `entries` holds one NUL-terminated pointer that outlives the
        // call.
        let err = unsafe { identity_features(&id) }.expect_err("invalid UTF-8 is not a default");
        assert_eq!(err as i32, ExitCode::ConfigInvalid as i32);
    }

    /// A zeroed identity carrying only the feature array under test.
    fn identity_with_features(
        features: *const *const c_char,
        num_features: usize,
    ) -> QuipBackendIdentity {
        QuipBackendIdentity {
            backend: ptr::null(),
            algorithm: ptr::null(),
            max_nodes: 0,
            max_edges: 0,
            features,
            num_features,
            min_sweeps: 0,
            max_sweeps: 0,
            min_reads: 0,
            max_reads: 0,
            reads_solution_min_factor: 0,
            reads_solution_max_factor: 0,
            reads_solution_floor_factor: 0,
        }
    }
}
