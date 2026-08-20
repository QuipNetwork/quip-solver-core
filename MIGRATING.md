# Migrating to quip-solver-core 0.0.1-rc1

This note is for maintainers of `quip-miner-cpu` and `quip-miner-cuda`, and
for C and C++ solver authors building against `libquip_solver_c`. Two public
Rust types and one C entry point change in this release. A build that still
names the old types fails.

## Dependency

The crate moved. Change the git source and the package name.

Before:

```toml
[dependencies]
quip-miner-core = { git = "https://gitlab.com/quip.network/quip-protocol.git", tag = "<tag>" }
```

After:

```toml
[dependencies]
quip-solver-core = { git = "https://gitlab.com/quip.network/quip-solver-core.git", tag = "<tag>" }
```

Replace `<tag>` with the tag chosen at publication.

## `Sampler::sample` error type

`sample` no longer returns a wire `RejectReason`. It returns a
`SampleError`. The session maps `SampleError` to the wire at the boundary.
A `Sampler` reports a device condition. It does not name wire enum values.

Before:

```rust
fn sample(
    &self,
    graph: &IsingGraph,
    params: &SampleParams,
) -> Result<Vec<SamplerResult>, RejectReason>;
```

After:

```rust
fn sample(
    &self,
    graph: &IsingGraph,
    params: &SampleParams,
) -> Result<Vec<SamplerResult>, SampleError>;
```

Map a size bound to `Capacity`. Map transient load to `DeviceBusy`. Map a
state that needs a restart to `DeviceFault`.

## Cancellation

This release removes `CancelGuard` and `StreamJob.generation`. Use
`CancelToken` and `StreamJob.watermark: Option<u64>`. A `Cancel` never
abandons a job with no watermark.

Before:

```rust
pub struct StreamJob {
    pub job_id: Vec<u8>,
    pub graph: IsingGraph,
    pub params: SampleParams,
    pub generation: u64,
}

fn sample_stream(
    &self,
    jobs: tokio::sync::mpsc::Receiver<StreamJob>,
    out: tokio::sync::mpsc::Sender<StreamResult>,
    cancel: CancelGuard,
);

impl CancelGuard {
    pub fn is_cancelled(&self, generation: u64) -> bool;
}
```

After:

```rust
pub struct StreamJob {
    pub job_id: Vec<u8>,
    pub graph: IsingGraph,
    pub params: SampleParams,
    pub watermark: Option<u64>,
}

fn sample_stream(
    &self,
    jobs: tokio::sync::mpsc::Receiver<StreamJob>,
    out: tokio::sync::mpsc::Sender<StreamResult>,
    cancel: CancelToken,
);

impl CancelToken {
    pub fn is_cancelled(&self, watermark: Option<u64>) -> bool;
}
```

Generation `0` used to mean a mempool job that a reseed never cancels.
That rule now lives in `prepare_job`. The token itself has no zero case.
Pass `watermark: None` for a job that `Cancel` must not abandon.

This note does not claim that any consumer repository has migrated.

## The C ABI: `quip_energy_milli`

The three sections below are for authors of a C or C++ solver that links
`libquip_solver_c`. The Rust API is unaffected.

`quip_energy_milli` used to return the energy and report every failure as
`0`. Zero is a legal Ising energy, so a caller that passed a NULL pointer
or mismatched lengths received a confident wrong score and reported it to
the network. The energy now leaves through an out-parameter, and the
return value is a status.

Before:

```c
int64_t quip_energy_milli(const double *h,
                          uintptr_t num_nodes,
                          const double *j,
                          uintptr_t num_edges,
                          const uint32_t *edges,
                          const int8_t *spins,
                          uintptr_t num_spins);
```

After:

```c
int32_t quip_energy_milli(const double *h,
                          uintptr_t num_nodes,
                          const double *j,
                          uintptr_t num_edges,
                          const uint32_t *edges,
                          const int8_t *spins,
                          uintptr_t num_spins,
                          int64_t *out_energy_milli);
```

The status is `QUIP_ENERGY_OK` (0), `QUIP_ENERGY_NULL_POINTER` (1), or
`QUIP_ENERGY_LENGTH_MISMATCH` (2). On any status other than
`QUIP_ENERGY_OK`, the out-parameter is left untouched.

Update a call site like this:

```c
std::int64_t energy = 0;
const std::int32_t scored = quip_energy_milli(graph->h, graph->num_nodes,
                                              graph->j, graph->num_edges,
                                              graph->edges,
                                              spins.data(), spins.size(),
                                              &energy);
if (scored != QUIP_ENERGY_OK) {
    return QUIP_SAMPLE_DEVICE_FAULT;
}
```

`examples/cpp/mock_miner.cpp` shows the complete callback.

Any array pointer may now be NULL when its length is 0. An empty problem
scores 0 with status `QUIP_ENERGY_OK`.

## The C ABI: identity strings that are not UTF-8

`QuipBackendIdentity.backend`, `.algorithm`, and each entry of `.features`
used to fall back to a default when the bytes were not UTF-8. That
advertised an identity the operator never wrote. `quip_solver_run` now
returns exit code 64 (config invalid) and logs which field failed.

A NULL `backend` or `algorithm` is unchanged: it still means `"c"` and
`"custom"`. A NULL entry inside `features` is dropped, with a warning
naming its index. A NULL `features` array with a nonzero `num_features`
now returns exit code 64.

## The generated C header moves

`cargo build -p quip-solver-c` used to write `quip_solver.h` into
`crates/quip-solver-c/include/`. A build that writes into the source tree
fails in a read-only or vendored checkout. The header is now generated
beside the library it describes.

Before:

```sh
cc -I crates/quip-solver-c/include ...
```

After:

```sh
cc -I target/release ...
```

The release tarball is unchanged. It still carries `include/quip_solver.h`
beside `libquip_solver_c.so` and `libquip_solver_c.a`, so a consumer that
builds against the tarball needs no change.

A failure to generate the header now stops the build. The previous
behaviour printed a warning and left the earlier header in place, which
could compile a C caller against a struct layout that no longer matched.
