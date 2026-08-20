/* Quip solver C ABI. Generated from src/lib.rs by cbindgen; do not edit. */

#ifndef QUIP_SOLVER_H
#define QUIP_SOLVER_H

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * The sampling callback ran to completion.
 */
#define QUIP_SAMPLE_OK 0

/**
 * The job exceeds a device bound. An identical job fails again.
 */
#define QUIP_SAMPLE_CAPACITY 1

/**
 * The device is busy now. An identical job may succeed later.
 */
#define QUIP_SAMPLE_DEVICE_BUSY 2

/**
 * The device needs a restart. The session sends `Fatal` and ends.
 */
#define QUIP_SAMPLE_DEVICE_FAULT 3

/**
 * What the solver advertises in `--capabilities` and `Hello`.
 *
 * `backend` and `algorithm` are NUL-terminated strings. `features` is an array
 * of NUL-terminated strings with `num_features` entries, and may be NULL when
 * `num_features` is 0. All strings are copied during
 * [`quip_solver_run`], so the caller may free them once it returns.
 */
typedef struct {
    /**
     * Backend name, for example `"cpu"`.
     */
    const char *backend;
    /**
     * Algorithm name, for example `"sa"`.
     */
    const char *algorithm;
    /**
     * Largest node count this solver accepts.
     */
    uint32_t max_nodes;
    /**
     * Largest edge count this solver accepts.
     */
    uint32_t max_edges;
    /**
     * Optional feature strings.
     */
    const char *const *features;
    /**
     * Number of entries in `features`.
     */
    uintptr_t num_features;
    /**
     * Lower bound on adaptive sweeps.
     */
    uint32_t min_sweeps;
    /**
     * Upper bound on adaptive sweeps.
     */
    uint32_t max_sweeps;
    /**
     * Lower bound on adaptive reads.
     */
    uint32_t min_reads;
    /**
     * Upper bound on adaptive reads.
     */
    uint32_t max_reads;
    /**
     * Reads-per-solution lower factor.
     */
    uint32_t reads_solution_min_factor;
    /**
     * Reads-per-solution upper factor.
     */
    uint32_t reads_solution_max_factor;
    /**
     * Reads-per-solution floor factor.
     */
    uint32_t reads_solution_floor_factor;
} QuipBackendIdentity;

/**
 * One Ising problem handed to the sampling callback.
 *
 * `edges` is a flat array of `2 * num_edges` vertex indices: `u0, v0, u1, v1,
 * ...`. `h` holds `num_nodes` fields and `j` holds `num_edges` couplings.
 */
typedef struct {
    /**
     * Local fields, one per node.
     */
    const double *h;
    /**
     * Number of nodes, and the length of `h`.
     */
    uintptr_t num_nodes;
    /**
     * Couplings, one per edge.
     */
    const double *j;
    /**
     * Number of edges, and the length of `j`.
     */
    uintptr_t num_edges;
    /**
     * Flat vertex index pairs, `2 * num_edges` entries.
     */
    const uint32_t *edges;
} QuipIsingGraph;

/**
 * Resolved sampling knobs for one job.
 */
typedef struct {
    /**
     * How many solutions to return.
     */
    uintptr_t num_reads;
    /**
     * Sweeps in the anneal.
     */
    uintptr_t num_sweeps;
    /**
     * Sweeps per beta rung.
     */
    uintptr_t sweeps_per_beta;
    /**
     * Nonzero when `beta_min`/`beta_max` carry a pinned range.
     */
    int32_t has_beta_range;
    /**
     * Start of the beta ladder, valid when `has_beta_range` is nonzero.
     */
    double beta_min;
    /**
     * End of the beta ladder, valid when `has_beta_range` is nonzero.
     */
    double beta_max;
    /**
     * Deterministic seed for this job.
     */
    uint64_t seed;
} QuipSampleParams;

/**
 * Emits one solution from inside the sampling callback.
 *
 * Call once per read. The bytes are copied before this returns, so the caller
 * may reuse or free `spins` immediately afterwards.
 */
typedef void (*QuipEmitFn)(void *sink,
                           const int8_t *spins,
                           uintptr_t num_spins,
                           int64_t energy_milli);

/**
 * The sampling callback a C solver registers.
 *
 * Return [`QUIP_SAMPLE_OK`] after emitting the solutions, or one of the
 * `QUIP_SAMPLE_*` error codes. Must not unwind into Rust.
 */
typedef int32_t (*QuipSampleFn)(void *user_data,
                                const QuipIsingGraph *graph,
                                const QuipSampleParams *params,
                                QuipEmitFn emit,
                                void *sink);

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * Ising energy in milli-units, using the golden-pinned consensus scorer.
 *
 * A C solver scores its own solutions with this rather than writing the loop
 * itself. The network rejects a solution whose reported energy disagrees, so a
 * second implementation is a liability, not a convenience.
 *
 * `edges` holds `2 * num_edges` flat vertex index pairs. Returns 0 when any
 * pointer is NULL or the lengths disagree.
 *
 * # Safety
 *
 * Each pointer must address the number of elements its length argument states.
 */
int64_t quip_energy_milli(const double *h,
                          uintptr_t num_nodes,
                          const double *j,
                          uintptr_t num_edges,
                          const uint32_t *edges,
                          const int8_t *spins,
                          uintptr_t num_spins);

/**
 * Runs a Quip solver: parses the command line, dispatches the mode, and drives
 * the session, calling `sample` for every job.
 *
 * Returns the process exit code from SPEC section 2: 0 clean, 64 config
 * invalid, 69 environment incompatible, 70 internal fatal, 77 token rejected.
 *
 * # Safety
 *
 * `id` must point to a valid [`QuipBackendIdentity`]. `argv` must hold `argc`
 * NUL-terminated strings. `sample` must not unwind.
 */
int32_t quip_solver_run(const QuipBackendIdentity *id,
                        int argc,
                        const char *const *argv,
                        QuipSampleFn sample,
                        void *user_data);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* QUIP_SOLVER_H */
