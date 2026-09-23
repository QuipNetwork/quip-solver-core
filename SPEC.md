# Quip solver contract

This document is the Quip solver contract. A solver in any language
follows this document.

Sections 2, 3, 5, and 6 are normative for every language.
Section 4 binds that contract to Rust.

## 1. What a Quip solver is

A Quip solver is a binary that accepts Ising problems and returns spin
configurations with their energies. Each configuration is a vector of spins
in {-1, +1} together with the energy of that assignment.

## 2. The four modes

A solver binary supports four modes.

### `--capabilities`

This mode prints what the solver supports, then exits. It must not open the
device.

### `--solve`

This mode reads one problem as JSON on stdin. It writes the solutions as JSON
on stdout, then exits.

`XQSA` and IsingMark use `--solve`.

### `--check`

This mode opens the device and exits. Exit 0 means the device is runnable.
Exit 69 means the host cannot run this solver. It does not start a session.

### Session mode

This mode streams jobs for the life of the process. quipminer uses session
mode.

The session-mode command line is normative. A solver must accept these two
flags:

- `--quip-coordinator <unix://path>`
- `--miner-id <id>`

The solver dials that Unix socket. The solver must not listen on the socket.

`quip-solver-drive` launches a solver that accepts those two arguments and
speaks the proto.

### Exit codes

These codes apply to all four modes. They are the same values that
`Fatal.exit_code` carries.

| Code | Name | Meaning |
|---|---|---|
| 0 | Clean | Clean exit. |
| 64 | ConfigInvalid | Missing or invalid command-line flags or config. Examples: no `--quip-coordinator`, a bad Welcome. |
| 69 | EnvIncompatible | The host cannot run this solver. `--check` failed. |
| 70 | InternalFatal | Unexpected internal failure. |
| 77 | TokenRejected | `QUIP_SESSION_TOKEN` is missing, empty, or rejected. |

## 3. The wire contract

`proto/quip/v1/miner.proto` is the normative message definition.

The coordinator serves a bidirectional gRPC stream on a Unix-domain socket.
The solver dials that socket.

The coordinator passes the session token in the `QUIP_SESSION_TOKEN`
environment variable. The solver echoes the token in `Hello`. The solver
must not put the token on the command line.

Messages flow in this order:

1. The solver sends `Hello`.
2. The coordinator sends `Welcome`.
3. The coordinator sends `Configure`.
4. The solver sends `Ready`.
5. The coordinator may send `Topology` and `SetTarget` before or after
   `Ready`. A round that never sends them is valid.
6. The solver and the coordinator exchange `JobRequest` and `Job` until
   `Shutdown`.

`Hello.capabilities` holds the protocol version, identity enums, limits, and supported features.
Require `Hello.capabilities.protocol_version == 2` and send `Welcome { protocol_version: 2 }`.
A Rust solver rejects any other welcome version with `ConfigInvalid`, exit code 64.
A v1 peer cannot complete this handshake.
The package name remains `quip.v1`.
See the [coordinator upgrade guide](docs/coordinator-upgrade-v2.md) for every changed field number.

`SetTarget.num_sweeps` can pin the sweep budget.
A `gibbs` solver runs and reports twice this count.
Other algorithms use the pinned count directly.

M is the solver. C is the coordinator.

| Message | Direction | Purpose |
|---|---|---|
| `Hello` | M→C | Identify and advertise capabilities. |
| `Welcome` | C→M | Accept the session. State the protocol version. |
| `Configure` | C→M | Session knobs and this solver's verbatim config. |
| `Topology` | C→M | Current problem graph. |
| `SetTarget` | C→M | Difficulty target and optional parameter pins. |
| `Ready` | M→C | Configured and ready to accept jobs. |
| `JobRequest` | M→C | Grant credits. |
| `Job` | C→M | One Ising problem or a salt lease. |
| `Result` | M→C | Plain job solutions or one winning lease salt. |
| `LeaseDone` | M→C | Finish a lease and report its counts and lowest energy. |
| `Reject` | M→C | Decline a job, with a reason. |
| `Cancel` | C→M | Abandon stale generations. |
| `Ping` | C→M | Liveness probe. |
| `Status` | M→C | Health, load, and liveness state. |
| `Shutdown` | C→M | End the session within a grace window. |
| `Fatal` | M→C | Report a fatal reason before exit. |
| `GetCapabilities` | C→M | Ask for the `Capabilities` message. |
| `Capabilities` | M→C | Reply with what this solver supports. |

### Coefficients and solutions

`IsingProblem.encoding` names the element type in the little-endian `h` and `j` arrays.
Each node has one field value. Each edge has one coupling value.
Integer encodings `I32`, `I16`, and `I8` require a positive `scale`.
Their unit value is `stored / scale`.
`I32` at scale 1000 is the canonical milli form.
Float encodings `F16`, `F32`, and `F64` require scale zero and store unit values.

Each coefficient needs an exact `i32` milli value.
The decoder returns `MALFORMED` for these inputs:

- An unknown or unspecified encoding.
- An invalid scale.
- An incomplete element.
- A non-finite float.
- A fractional milli value.
- A value outside the `i32` milli range.

Send a narrow encoding only when every value converts exactly and the solver advertises that encoding.
Otherwise send `I32` at scale 1000.

`Solution.spins` is bit-packed in the same layout as `initial_spins` below.
Its field number is 3. The schema reserves the old `spins_bytes` name and field 1.
`energy_milli` remains field 2.
Plain and mempool jobs keep their credits, rejects, provenance, and warm-start behavior.

### Generated jobs

`ISING_GENERATE` is job kind 3.
Its `Job.generator` carries a topology hash, nonce inputs, generator algorithm, and salt range.
Use `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1`.
`last_proof_block_hash`, `miner_account`, and `base_salt` each contain 32 bytes.
`salt_count` must be positive, and `salt_start + salt_count` must fit in `u64`.
Invalid fields produce `MALFORMED` before any draw.

Salt index `i` replaces bytes 0 to 7 of `base_salt` with `(salt_start + i).to_le_bytes()`.
Bytes 8 to 31 stay unchanged.
The nonce is `BLAKE3(last_proof_block_hash || miner_account || salt)`.
ChaCha8 draws fields before couplings, in chain registration order.
Each value is `allowed[next_u32 % allowed.len()]`.

`Topology` carries both `allowed_h_milli` and `allowed_j_milli`.
Its nodes and edges keep chain registration order, with native node identifiers as edge endpoints.
An absent topology produces `TOPOLOGY_MISSING`. A different cached hash produces `TOPOLOGY_MISMATCH`.
An absent target or zero `SetTarget.max_proof_solutions` produces `TARGET_MISSING`.
That limit must equal the runtime's `QuantumPowMaxSolutions`, which is 32 on 2026-09-23.

One lease consumes one credit.
The default session draws salt problems and sends them through the sampler stream.
A session-wide bound keeps at most `stream_width` generated salts in flight.
Plain jobs can interleave with leases and refund their own credits.
Each lease keeps its topology snapshot, but each salt uses the target current at scoring time.

For each winning salt, the session sends one `Result` with 32-byte `salt` and `nonce` fields.
`Result.solutions` holds the selected proof set in proof order.
The coordinator verifies and submits that set unchanged and in order.
`quip_protocol::target::meets_target` selects the proof set.
`quip_protocol::lease::verify_lease_result` verifies the complete wire result with a lease, topology, and target.

After its last result, a completed lease sends one `LeaseDone`, then one credit.
`salts_done` counts successfully completed salt samples, including empty read sets.
Empty reads send no result and do not change the lowest energy.
`best_energy_milli` is the lowest energy observed, or `i64::MAX` if no reads finish.
The stop rules in section 5 bound this completion sequence.

### Warm starts

An `IsingProblem` may carry start states in `initial_spins`, field 9. A
solver that uses them lists the feature string `initial-spins` in
`Hello.capabilities.features` and `Capabilities.features`. A solver that does not list
the feature ignores fields 9 to 12 and runs a cold start.

Each `initial_spins` entry is one state, bit-packed in topology node order.
Node `i` is bit `i % 8` of byte `i / 8`, lowest bit first. Bit
`1` is spin +1 and bit `0` is spin –1. An entry is exactly
`ceil(num_nodes / 8)` bytes, and its padding bits are `0`. The entries come in
order of energy, lowest first.

Three fields set where a seeded anneal starts. The value `0` in any of
them means the solver picks.

| Field | Used by | Meaning |
|---|---|---|
| `start_beta_milli`, 10 | Seeded SA | Inverse temperature the anneal starts from, in milli-units. |
| `reversal_s_milli`, 11 | Seeded quantum processing unit | Anneal fraction `s` that the reverse anneal backs off to, in milli-units, from 1 to 999. |
| `reversal_pause_us`, 12 | Seeded quantum processing unit | Pause at the reversal point, in microseconds. |

The solver applies these rules:

| Condition | Solver behavior |
|---|---|
| `initial_spins` is empty. | Run a cold start and ignore fields 10 to 12. |
| The job has fewer states than `num_reads`. | Seed one read per state. Start the remaining reads cold. |
| The job has more states than `num_reads`. | Use the first `num_reads` states. |
| The solver takes one state per job, as a quantum processing unit does. | Use the first state. |
| A state has the wrong length or a padding bit set. | Send `Reject` with `MALFORMED`. |
| `reversal_s_milli` is 1000 or more. | Send `Reject` with `MALFORMED`. |

The Rust session applies the two `MALFORMED` rules to every job, whether or
not the solver lists the feature.

## 4. The sampler contract

This section is the Rust binding of the language-neutral contract. This
section is not the contract itself.

Sections 2, 3, 5, and 6 are normative for every language.
This section is normative only for Rust solvers.

A Rust solver supplies a `Sampler` and calls `run`.

`sample` is the one required method. It takes an `IsingGraph` and
`SampleParams`. It returns `Result<Vec<SamplerResult>, SampleError>`.
`sample` must not panic.

`IsingGraph<C = f64>` stores coefficients in `h` and `j`.
`Sampler<C = f64>`, `StreamJob<C = f64>`, and `WarmStreamJob<C = f64>`
carry the same coefficient type. The default is `f64`.
The session converts each wire coefficient once when it decodes a job.

The sealed `Coefficient` trait supports `f64`, `f32`, `half::f16`, and
`Fixed<T, SCALE>` for `i32`, `i16`, `i8`, and `I4` storage.
`SCALE` is positive. Fixed-point conversion rounds halfway values away
from zero and saturates. `I4` stores values from `-8` through `7` in one byte.
`Milli` is `Fixed<i32, 1000>` and preserves each wire coefficient.

Report the exact energy of the model your solver receives.
The session checks exactness per problem.

A matching encoding and scale permit direct decoding.
The session still checks the bytes.
It keeps no milli copy for that path.

| Solver coefficient type | Direct encoding | Scale |
| --- | --- | --- |
| `Fixed<i32, S>` | `I32` | `S` |
| `Fixed<i16, S>` | `I16` | `S` |
| `Fixed<i8, S>` | `I8` | `S` |
| `half::f16` | `F16` | 0 |
| `f32` | `F32` | 0 |
| `f64` | `F64` | 0 |

Other legal inputs first decode to exact milli values, then convert through `C::from_milli`.
For types without `Coefficient::EXACT`, the session checks whether `to_unit() * 1000` equals each original milli value.
It retains the milli arrays and replaces reported energies only when that check fails.
`f64` and `Milli` are exact types and keep reported energies.
Generated problems use the same conversion check.
The JSON driver applies it to wire-representable inputs and preserves its existing handling of other JSON values.
Use `quip_protocol::scoring::energy_from_milli` to score stored milli integers.

`BackendIdentity.backend` is `quip_proto::v1::Backend`.
`BackendIdentity.algorithm` is `quip_proto::v1::Algorithm`.
Use `Backend::Cpu` and `Algorithm::Sa` for a CPU simulated annealer.
The [migration name table](MIGRATING.md#identity-names) lists all supported names.
The fixed map in `quip_protocol::session` keeps command-line identity strings lowercase.

The twelve defaulted methods are:

| Method | Default |
|---|---|
| `sample_stream` | Serial loop over `sample`. Polls the cancel token at dequeue. |
| `accepts_warm_start` | `false`. `true` adds `initial-spins` to the advertised features. |
| `sample_warm` | Ignores the `WarmStart` and calls `sample`. |
| `sample_stream_warm` | Serial loop over `sample_warm` for a seeded job and `sample` for a cold one. |
| `stream_width` | `1` |
| `declared_stream_width` | `1`. `0` declares a device-dependent width, resolved when the device opens. |
| `utilization` | `0.0` |
| `should_throttle` | `false` |
| `max_reads` | `u32::MAX` |
| `apply_config` | No action. |
| `generates_locally` | `false`. The session draws lease problems. |
| `sample_lease` | Returns `DeviceFault` if called without an override. |

### Optional local generation

A solver opts in by returning `true` from `generates_locally()` and overriding this method:

```rust
fn sample_lease(
    &self,
    lease: &Lease,
    topology: &TopologyView,
    params: &SampleParams,
    out: &LeaseSink,
) -> Result<(), SampleError>;
```

The session runs it on a dedicated blocking `quip-lease` thread for each lease.
These calls run concurrently with the sampler stream and can overlap other lease calls.
The session joins them within its shutdown grace window.

Use indices in `0..lease.salt_count()` with `lease.salt(i)` and `lease.nonce(i)`.
Those two methods return zero arrays for an out-of-range index.
`TopologyView` holds the node count, dense edges, and allowed values in draw order.
`TopologyView::draw(nonce)` provides the host draw.

Call `LeaseSink::push(salt_index, reads)` once for each finished salt, including an empty read set.
It takes `Vec<SamplerResult>` and returns `Result<(), LeaseStopped>`.
Poll `LeaseSink::is_stopped()` before more work.
It covers cancellation, deadlines, shutdown, completion, and a closed writer.
This method has no `CancelToken` parameter.

The session first tests reported energies with the current target.
For a candidate winner, it redraws the problem on the host and rescores every submitted read.
It checks each spin vector and compares every reported energy with the host energy.
It checks the current target again before sending the winner.
A mismatch or a sampler panic is a device fault and ends the run.
C solvers use the default lease path and have no local-generation callback.

### Sampling errors

`SampleError` has three variants. Plain jobs use these wire responses:

| Variant | Meaning | Plain-job response |
|---|---|---|
| `Capacity` | The job exceeds a device bound. An identical job fails again. | `RejectReason::TooLarge` |
| `DeviceBusy` | The device is busy now. An identical job may succeed later. | `RejectReason::Overloaded` |
| `DeviceFault(String)` | The device does not recover without a restart. | `RejectReason::Overloaded`, then `Fatal` |

For a plain job, `DeviceFault` sends a reject and `Fatal` with `exit_code = 70`.
For a lease, a device fault ends the session with `Fatal` without a per-salt reject.
Nonfatal lease sampling errors do not produce winning results.

The session calls `sample_stream_warm` instead of `sample_stream` only when
`accepts_warm_start` returns `true`. The session then passes each job as a
`WarmStreamJob`. Its `warm_start` holds the decoded states, cut to
`num_reads`, and the start point. A solver that uses warm starts overrides
`accepts_warm_start` and `sample_warm`. A solver that keeps more than one
model in flight overrides `sample_stream_warm` instead of `sample_warm`.

## 5. Cancellation

These wire rules apply to every language. The Rust binding follows.

The coordinator sends `Cancel { max_generation }`. The solver abandons jobs whose nonzero generation is at or below `max_generation`.
It stops work on those jobs and sends no `Result` for them.
It reports the abandoned generation in `Status.abandoned_generation`.

A job carries its own cancellation watermark, and that watermark is
optional. A job with no watermark is never cancelled by this mechanism.
Generation `0` means no watermark.
Mempool jobs use this value and survive every `Cancel`. The watermark
is opaque and monotonic, so repeated or out-of-order `Cancel` messages are
idempotent.

A solver that owns its own sweep loop must check for cancellation at its
checkpoints, not only when it takes a job off the queue. A long sweep that
only checks at dequeue keeps burning device time on work nobody is waiting
for.

In the Rust binding, the watermark reaches the solver as
`StreamJob.watermark: Option<u64>`, and the solver checks it by calling
`CancelToken::is_cancelled(watermark)`. The `CancelToken` itself is never
optional. The `None` belongs to the job's watermark.

### Lease stops

| Event | Default session behavior |
| --- | --- |
| `Cancel` covers a nonzero lease generation | Stop before the next salt. Drop later reads. Send `LeaseDone` after outstanding outcomes drain, then one credit. |
| `Shutdown { grace_ms }` | Stop drawing. Drain outstanding reads and winners within the grace window. Send `LeaseDone` if the drain completes, then exit. |
| A nonzero `deadline_ms` passes | Stop drawing and drop later reads, as for cancellation. |
| Stream closes or the session becomes fatal | Abandon leases without a guaranteed `LeaseDone`. |

`Status.abandoned_generation` reports the cancellation watermark.
A default sampler must return outstanding salt outcomes before its lease summary can complete.
A shutdown timeout can prevent that summary from reaching the coordinator.

For local generation, shutdown closes `LeaseSink` immediately.
New pushes fail with `LeaseStopped`, while already queued messages may drain.
A session task closes cancelled local leases even when the sampler does not return.
A local sampler panic can emit `LeaseDone` before `Fatal`.
A fatal run remains failed regardless of a preceding summary.

`Cancel` remains generation-only. It cannot address one lease by job identifier.
Size the count and absolute Unix deadline for short leases, then renew work.
A zero deadline means no deadline.

## 6. Conformance

A solver is conformant when both of these hold:

- `quip-solver-drive <solver-binary> unix://<socket>` reports a conformant
  session.
- The adaptive parameters of the solver match the `golden_adapt.json` cases.

The driver spawns any executable. The driver speaks the proto over the
socket. This definition holds for a solver written in any language.

Every session sends one job that carries `initial_spins`, and every solver
must return a `Result` for it. A solver that lists `initial-spins` gets two
more jobs. It must reject a state of the wrong length with `MALFORMED`. It
must also return the planted ground-state energy of a 4096-spin ring when
that ground state is its seed. A cold anneal of the same sweep budget leaves
domain walls in the ring and does not reach that energy.

A Rust solver repository may use this convenience form:

```rust
use quip_solver_conformance::driver::drive_miner;

#[tokio::test]
async fn solver_is_conformant() {
    let report = drive_miner(bin, "unix:///tmp/s.sock").await;
    assert!(report.is_conformant());
}
```

## 7. Versioning

Release `0.0.2-rc3` uses protocol version 2 and breaks the v1 wire contract.
Update coordinators and solvers together. The protobuf package remains `quip.v1`.
The retired field numbers remain reserved.

Pin a tag. Never a branch, and never a bare revision.

A change to any of these is a major version:

- A field removed or renumbered in `proto/quip/v1/miner.proto`.
- A method added to, or removed from, the `Sampler` trait without a default.
- A change to the type or meaning of `SampleError`, `CancelToken`,
  `StreamJob`, `StreamResult`, `IsingGraph`, `SampleParams`, or
  `SamplerResult`.
- A change to any golden vector.

A change to any of these is a minor version:

- A message, field, or remote procedure call added to the proto.
- A defaulted method added to the `Sampler` trait.
- A golden vector case added.

The contract is close to frozen by design. A solver repository that pins a
tag and never moves it keeps working.

## 8. The `--capabilities` mapping

The `--capabilities` output maps the `Capabilities` message to JSON.
Field names are lowerCamelCase.
Identity enums use the fixed lowercase names, rather than protobuf enum names.
The fields are:

- `backend`
- `algorithm`
- `supportedKinds`
- `maxNodes`
- `maxEdges`
- `features`
- `protocolVersion`
- `streamWidth`
- `encodings`
- `generators`
- `nativeTopologyHash`. Omit it when unset. The value is standard base64
  because protobuf JSON maps `bytes` that way.

`backend` and `algorithm` keep names such as `"cpu"`, `"sa"`, and `"dwave-qpu"`.
Job kinds, encodings, and generators use their protobuf enum names.
For an `f64` CPU sampler, the output has this shape:

```json
{
  "backend": "cpu",
  "algorithm": "sa",
  "supportedKinds": ["ISING_SAMPLE", "ISING_GENERATE"],
  "maxNodes": 100000,
  "maxEdges": 1000000,
  "features": [],
  "protocolVersion": 2,
  "streamWidth": 1,
  "encodings": ["COEFFICIENT_ENCODING_I32", "COEFFICIENT_ENCODING_F64"],
  "generators": ["GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1"]
}
```

The Rust session advertises `I32` plus the solver's own encoding, without duplicates.
`Fixed<I4, S>` advertises only `I32`.
Rust and C solvers advertise the default generator even without local generation.
The Python and TypeScript examples advertise only `ISING_SAMPLE` and `I32`, with no generators.

`streamWidth: 0` declares a width that is a property of the opened device,
such as a lane count derived from a GPU's multiprocessor count.
`--capabilities` answers with the device closed, so `0` is the honest static
answer. The in-session `Capabilities` reply comes from a session that holds
the device open, and carries the live width instead. This is the one field
where the two answers can differ.

## 9. Adding a solver in another language

The normative wire contract is `proto/quip/v1/miner.proto`. Section 2 states the session-mode command line.

`quip-miner-dwave` is the working example of a solver built this way today.

### Now

These pieces exist now:

- The proto.
- The `quip-solver-core` wheel: protocol primitives, scoring, wire encoding, and
  exit codes.
- `quip-solver-drive` as the conformance gate for a solver in any language.
- C and C++ through a C binary interface with a registered callback. `quip_solver_run`
  carries the session loop and calls the sampler callback the solver
  registers. The artifact is `libquip_solver_c` plus `quip_solver.h`, attached
  to each release. `examples/cpp` is a conformant solver built on it.

The `quip-solver-core` wheel carries protocol primitives and handshake helpers. It
does not carry the session loop. A Python solver written today must write
the state machine itself.

The release pipeline builds the C library on `rust:1.97.1`, which links
glibc 2.39. The artifact requires a host with glibc 2.39 or later. A solver on an older distribution must build
the crate itself rather than use the attached artifact. The smoke test
compiles with the released tarball on that same image, so it proves the
header matches the object but not the floor. A smoke image on an older glibc
is future work.

### Next

Next work adds the remaining first-class bindings that carry the session loop.
A solver in that language then supplies only a sampler.

- Python through PyO3.
- Node through a napi-rs addon.

Both wrap the one Rust loop. They do not reimplement the loop.

### Not planned

Browser WebAssembly is not planned. A browser cannot open a Unix or raw TCP socket.

A browser would need a new transport on both the solver and the coordinator.
A new transport is a protocol change.
