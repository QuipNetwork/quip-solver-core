# Migrating to quip-solver-core 0.0.2-rc3

Update the coordinator and solver together.
This release requires protocol version 2 and rejects version 1 handshakes.
Follow the [coordinator upgrade guide](docs/coordinator-upgrade-v2.md) for wire changes and verification examples.

## Identity names

Replace strings in `BackendIdentity.backend` and `.algorithm` with `quip_proto::v1::Backend` and `quip_proto::v1::Algorithm` values.
`quip_solver_core::quip_proto` re-exports these enums.
The command-line output keeps lowercase names through the fixed map in `quip_protocol::session`.

| Command-line or C name | Rust value |
| --- | --- |
| `unspecified` | `Backend::Unspecified` |
| `cpu` | `Backend::Cpu` |
| `cuda` | `Backend::Cuda` |
| `metal` | `Backend::Metal` |
| `ane` | `Backend::Ane` |
| `dwave-qpu` | `Backend::DwaveQpu` |
| `exec` | `Backend::Exec` |
| `mock` | `Backend::Mock` |
| `unspecified` | `Algorithm::Unspecified` |
| `sa` | `Algorithm::Sa` |
| `gibbs` | `Algorithm::Gibbs` |
| `quantum-anneal` | `Algorithm::QuantumAnneal` |
| `fsa` | `Algorithm::Fsa` |
| `msa` | `Algorithm::Msa` |
| `flatiron` | `Algorithm::Flatiron` |
| `mps` | `Algorithm::Mps` |
| `mfa` | `Algorithm::Mfa` |
| `sb` | `Algorithm::Sb` |
| `bsb` | `Algorithm::Bsb` |
| `gbsb` | `Algorithm::Gbsb` |
| `gdsb` | `Algorithm::Gdsb` |
| `ggdsb` | `Algorithm::Ggdsb` |
| `hbsb` | `Algorithm::Hbsb` |
| `hdsb` | `Algorithm::Hdsb` |
| `sbqa` | `Algorithm::Sbqa` |
| `tedsb` | `Algorithm::Tedsb` |
| `external` | `Algorithm::External` |

Replace old `backend` labels `test` and `narrow` with `Backend::Mock`.
Split `cuda-sa` into `Backend::Cuda` and `Algorithm::Sa`.
Those old labels are not accepted string aliases.
Use `Algorithm::External` for an algorithm with no dedicated enum value.

C identity fields remain strings.
A `NULL` `backend` becomes `BACKEND_UNSPECIFIED` and a `NULL` algorithm becomes `ALGORITHM_EXTERNAL`.
An unknown non-NULL string ends the run with `ConfigInvalid`, exit code 64, and lists the accepted names.

## Sampling and leases

Keep `sample` and the existing streaming methods for the default lease path.
These methods need no changes.
The session generates salt problems and sends them through the sampler stream.
It sends one result per winning salt, one `LeaseDone` per completed lease, then one credit.

For device-side generation, return `true` from `generates_locally()` and override this method:

```rust
fn sample_lease(
    &self,
    lease: &Lease,
    topology: &TopologyView,
    params: &SampleParams,
    out: &LeaseSink,
) -> Result<(), SampleError>;
```

Import these types from `quip_solver_core`.
The method runs on a separate blocking thread per lease, concurrently with the sampler stream.
Use `lease.salt_count()`, `lease.salt(i)`, and `lease.nonce(i)` to iterate the range.
Call `out.push(i, reads)` once per finished salt.
Poll `out.is_stopped()` to stop for cancellation, deadlines, or shutdown.
The method takes no `CancelToken` parameter.
The host redraws and rescores candidate winners. A mismatch or panic ends the run as a device fault.
A local fatal error sends no `LeaseDone` or credit refund for that lease.
On shutdown, `out.is_stopped()` becomes true immediately, so start no new salts.
The sink accepts verified winners from work already running until the grace deadline.
The lease closes and sends `LeaseDone` when its worker returns or grace expires, whichever comes first.
Cancellation and lease expiry reject later pushes and queued winners.
Live local workers cannot exceed the advertised credit window.
The session removes finished workers before starting another worker.
If stopped workers still fill that window, the session ends with a device fault.

Report the exact energy of the model your sampler receives.
The session now checks coefficient conversion per problem and skips rescoring when that conversion is exact.
A narrow coefficient type no longer implies that the session always replaces your energies.

## Wire and binding changes

Read identity, protocol version, and capabilities from `Hello.capabilities`.
Send coefficients through `encoding`, `scale`, `h`, and `j`.
The canonical form is `I32` at scale 1000.
The schema removes the old `h_milli_le32` and `j_milli_le32` fields.

Replace `Solution.spins_bytes` with bit-packed `Solution.spins`, field 3.
Node `i` uses bit `i % 8` of byte `i / 8`, lowest bit first.
Bit 1 means +1. Bit 0 means `-1`.
Require `ceil(num_nodes / 8)` bytes and zero padding bits.
Use `encode_spins_packed` and `decode_spins_packed` for this field.
The Python wheel's `encode_spins` and `decode_spins` still use one byte per spin.
The C sampling callback still emits one `int8_t` per spin, and the Rust session packs the wire output.

The Rust session advertises `I32` plus its coefficient type's encoding and the default lease generator.
Python and TypeScript example miners support v2 plain jobs, but do not advertise `ISING_GENERATE`.

# Migrating to quip-solver-core 0.0.2-rc1

A solver built with 0.0.1 compiles and runs unchanged. The new `Sampler`
methods have defaults, and `SampleParams`, `StreamJob`, and `IsingGraph` do
not change. Two changes can stop a build:

- A struct literal of the generated `IsingProblem` must name the four new
  fields, or end with `..Default::default()`.
- An exhaustive `match` on `quip_protocol::wire::WireError` must handle
  `BadPackedLength` and `NonZeroPadding`.

To use warm starts, return `true` from `accepts_warm_start` and override
`sample_warm`. A solver that keeps more than one model in flight
overrides `sample_stream_warm` instead. The session then advertises `initial-spins`.
Do not add the feature to `BackendIdentity.features` by hand.

# Migrating to quip-solver-core 0.0.1

This note is for maintainers of `quip-miner-cpu` and `quip-miner-cuda`, and
for C and C++ solver authors building with `libquip_solver_c`. Two public
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

## C energy scoring

These C binding changes apply to solvers that link `libquip_solver_c`.

`quip_energy_milli` used to return the energy and report every failure as
`0`. Zero is a legal Ising energy.
A caller with a `NULL` pointer or mismatched lengths received a wrong score and reported it to the network.
The energy now leaves through an out-parameter, and the
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

The status identifies success, a missing pointer, or mismatched lengths:

```text
QUIP_ENERGY_OK = 0
QUIP_ENERGY_NULL_POINTER = 1
QUIP_ENERGY_LENGTH_MISMATCH = 2
```

On failure, the function leaves the out-parameter untouched.

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

Any array pointer may now be `NULL` when its length is 0. An empty problem
scores 0 with status `QUIP_ENERGY_OK`.

## C identity strings

The old C binding replaced invalid UTF-8 in identity fields with defaults.
`quip_solver_run` now returns `ConfigInvalid`, exit code 64.
It logs the failed field.

In 0.0.1, a `NULL` `backend` or `algorithm` still means `"c"` and `"custom"`.
The 0.0.2-rc3 rules replace these defaults.
The function drops a `NULL` entry inside `features` and logs its index.
A `NULL` `features` array with a nonzero `num_features`
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

The release tarball still carries `include/quip_solver.h`
beside `libquip_solver_c.so` and `libquip_solver_c.a`, so a consumer that
builds with the tarball needs no change.

A failure to generate the header now stops the build.
The old build printed a warning and kept the earlier header.
A C caller could then use a struct layout that no longer matched.
