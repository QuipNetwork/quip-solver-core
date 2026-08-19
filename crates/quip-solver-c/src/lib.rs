//! C ABI over the Quip solver session loop.
//!
//! A C or C++ solver registers one sampling callback and calls
//! [`quip_solver_run`]. Everything else — the four command-line modes, the
//! handshake, credits, cancellation, and the exit codes — is the Rust loop in
//! `quip-solver-core`. Following SPEC section 9, this wraps that loop rather
//! than reimplementing it, so a C solver cannot drift from a Rust one.

use std::ffi::{c_char, c_int, CStr};

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

/// One Ising problem handed to the sampling callback.
///
/// `edges` is a flat array of `2 * num_edges` vertex indices: `u0, v0, u1, v1,
/// ...`. `h` holds `num_nodes` fields and `j` holds `num_edges` couplings.
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
    /// Flat vertex index pairs, `2 * num_edges` entries.
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
/// may reuse or free `spins` immediately afterwards.
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
/// `backend` and `algorithm` are NUL-terminated strings. `features` is an array
/// of NUL-terminated strings with `num_features` entries, and may be NULL when
/// `num_features` is 0. All strings are copied during
/// [`quip_solver_run`], so the caller may free them once it returns.
#[repr(C)]
pub struct QuipBackendIdentity {
    /// Backend name, for example `"cpu"`.
    pub backend: *const c_char,
    /// Algorithm name, for example `"sa"`.
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

/// Ising energy in milli-units, using the golden-pinned consensus scorer.
///
/// A C solver scores its own solutions with this rather than writing the loop
/// itself. The network rejects a solution whose reported energy disagrees, so a
/// second implementation is a liability, not a convenience.
///
/// `edges` holds `2 * num_edges` flat vertex index pairs. Returns 0 when any
/// pointer is NULL or the lengths disagree.
///
/// # Safety
///
/// Each pointer must address the number of elements its length argument states.
#[no_mangle]
pub unsafe extern "C" fn quip_energy_milli(
    h: *const f64,
    num_nodes: usize,
    j: *const f64,
    num_edges: usize,
    edges: *const u32,
    spins: *const i8,
    num_spins: usize,
) -> i64 {
    if h.is_null() || j.is_null() || edges.is_null() || spins.is_null() {
        return 0;
    }
    if num_spins != num_nodes {
        return 0;
    }
    // SAFETY: non-null and sized per the caller's contract, checked above.
    let (h, j, edges, spins) = unsafe {
        (
            std::slice::from_raw_parts(h, num_nodes),
            std::slice::from_raw_parts(j, num_edges),
            std::slice::from_raw_parts(edges, num_edges * 2),
            std::slice::from_raw_parts(spins, num_spins),
        )
    };
    let pairs: Vec<(usize, usize)> = edges
        .chunks_exact(2)
        .map(|p| (p[0] as usize, p[1] as usize))
        .collect();
    quip_solver_core::quip_protocol::scoring::energy_milli(spins, h, j, &pairs)
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
/// readable bytes. Both hold for the duration of one `sample` call.
unsafe extern "C" fn emit_solution(
    sink: *mut core::ffi::c_void,
    spins: *const i8,
    num_spins: usize,
    energy_milli: i64,
) {
    if sink.is_null() || (spins.is_null() && num_spins != 0) {
        return;
    }
    // SAFETY: `sink` is the `&mut Sink` this call passed in, and `spins`
    // addresses `num_spins` readable bytes per the header contract.
    let sink = unsafe { &mut *sink.cast::<Sink>() };
    let spins = unsafe { std::slice::from_raw_parts(spins, num_spins) };
    sink.results.push(SamplerResult {
        spins: spins.to_vec(),
        energy_milli,
    });
}

impl Sampler for CSampler {
    fn sample(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let edges: Vec<u32> = graph
            .edges
            .iter()
            .flat_map(|&(u, v)| {
                [
                    u32::try_from(u).unwrap_or(u32::MAX),
                    u32::try_from(v).unwrap_or(u32::MAX),
                ]
            })
            .collect();
        let c_graph = QuipIsingGraph {
            h: graph.h.as_ptr(),
            num_nodes: graph.h.len(),
            j: graph.j.as_ptr(),
            num_edges: graph.j.len(),
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

        // SAFETY: both structs live until this call returns, `emit_solution`
        // matches QuipEmitFn, and `sink` outlives the callback.
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

/// Copies a C string, or returns `None` when it is NULL or not UTF-8.
///
/// # Safety
///
/// `s` must be NULL or point to a NUL-terminated string.
unsafe fn owned_str(s: *const c_char) -> Option<&'static str> {
    if s.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees NUL termination.
    let text = unsafe { CStr::from_ptr(s) }.to_str().ok()?;
    // BackendIdentity holds &'static str and the process runs until the session
    // ends, so leaking one small string per field is the honest lifetime.
    Some(Box::leak(text.to_owned().into_boxed_str()))
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

    let argc_usize = match usize::try_from(argc) {
        Ok(n) => n,
        Err(_) => return ExitCode::ConfigInvalid as i32,
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

    // SAFETY: the caller guarantees these are NUL-terminated or NULL.
    let backend = unsafe { owned_str(id.backend) }.unwrap_or("c");
    let algorithm = unsafe { owned_str(id.algorithm) }.unwrap_or("custom");

    let mut features: Vec<&'static str> = Vec::with_capacity(id.num_features);
    if !id.features.is_null() {
        // SAFETY: non-null with `num_features` entries per the contract.
        let raw = unsafe { std::slice::from_raw_parts(id.features, id.num_features) };
        for &f in raw {
            // SAFETY: each entry is NUL-terminated per the contract.
            if let Some(s) = unsafe { owned_str(f) } {
                features.push(s);
            }
        }
    }

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
