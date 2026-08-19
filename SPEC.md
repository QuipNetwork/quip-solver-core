# Quip solver contract

This document is the Quip solver contract. A solver in any language
follows this document.

Sections 2 (The four modes), 3 (The wire contract), 5 (Cancellation), and
6 (Conformance) are normative for every language. Section 4 (The Rust
contract) is the Rust binding of that contract. Section 4 is not the
contract itself.

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

XQSA and IsingMark use `--solve`.

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

A `Welcome` whose protocol version is not `1` ends the session.

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
| `Job` | C→M | One Ising problem to solve. |
| `Result` | M→C | Solutions plus sampler metadata. |
| `Reject` | M→C | Decline a job, with a reason. |
| `Cancel` | C→M | Abandon stale generations. |
| `Ping` | C→M | Liveness probe. |
| `Status` | M→C | Health, load, and liveness state. |
| `Shutdown` | C→M | End the session within a grace window. |
| `Fatal` | M→C | Report a fatal reason before exit. |
| `GetCapabilities` | C→M | Ask for the `Capabilities` message. |
| `Capabilities` | M→C | Reply with what this solver supports. |

## 4. The Rust contract

This section is the Rust binding of the language-neutral contract. This
section is not the contract itself.

Sections 2 (The four modes), 3 (The wire contract), 5 (Cancellation), and
6 (Conformance) are normative for every language. This section is
normative only for Rust solvers.

A Rust solver supplies a `Sampler` and calls `run`.

`sample` is the one required method. It takes an `IsingGraph` and
`SampleParams`. It returns `Result<Vec<SamplerResult>, SampleError>`.
`sample` must not panic.

The six defaulted methods are:

| Method | Default |
|---|---|
| `sample_stream` | Serial loop over `sample`. Polls the cancel token at dequeue. |
| `stream_width` | `1` |
| `utilization` | `0.0` |
| `should_throttle` | `false` |
| `max_reads` | `u32::MAX` |
| `apply_config` | No action. |

`SampleError` has three variants:

| Variant | Meaning | Maps to |
|---|---|---|
| `Capacity` | The job exceeds a device bound. An identical job fails again. | `RejectReason::TooLarge` |
| `DeviceBusy` | The device is busy now. An identical job may succeed later. | `RejectReason::Overloaded` |
| `DeviceFault(String)` | The device does not recover without a restart. | `RejectReason::Overloaded`, then `Fatal` |

On `DeviceFault`, the session rejects the job, sends `Fatal` with
`exit_code = 70`, and ends.

## 5. Cancellation

This section is normative for every language, so it is stated on the wire
first. The Rust binding follows.

The coordinator sends `Cancel { max_generation }`. Every job whose own
generation is at or below `max_generation` is abandoned. The solver stops
work on those jobs, sends no `Result` for them, and reports the abandoned
generation in `Status.abandoned_generation` rather than in a `Result`.

A job carries its own cancellation watermark, and that watermark is
optional. A job with no watermark is never cancelled by this mechanism.
Generation `0` means exactly that: mempool jobs carry no watermark, so a
`Cancel` never abandons them, whatever `max_generation` says. The watermark
is opaque and monotonic, so repeated or out-of-order `Cancel` messages are
idempotent.

A solver that owns its own sweep loop must check for cancellation at its
checkpoints, not only when it takes a job off the queue. A long sweep that
only checks at dequeue keeps burning device time on work nobody is waiting
for.

In the Rust binding, the watermark reaches the solver as
`StreamJob.watermark: Option<u64>`, and the solver checks it by calling
`CancelToken::is_cancelled(watermark)`. The `CancelToken` itself is never
optional; the `None` belongs to the job's watermark.

## 6. Conformance

A solver is conformant when both of these hold:

- `quip-solver-drive <solver-binary> unix://<socket>` reports a conformant
  session.
- The adaptive parameters of the solver match the `golden_adapt.json` cases.

The driver spawns any executable. The driver speaks the proto over the
socket. This definition holds for a solver written in any language.

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

Pin a tag. Never a branch, and never a bare revision.

A change to any of these is a major version:

- A field removed or renumbered in `proto/quip/v1/miner.proto`.
- A method added to, or removed from, the `Sampler` trait without a default.
- A change to the type or meaning of `SampleError`, `CancelToken`,
  `StreamJob`, `StreamResult`, `IsingGraph`, `SampleParams`, or
  `SamplerResult`.
- A change to any golden vector.

A change to any of these is a minor version:

- A message, field, or RPC added to the proto.
- A defaulted method added to the `Sampler` trait.
- A golden vector case added.

The contract is close to frozen by design. A solver repository that pins a
tag and never moves it keeps working.

## 8. The `--capabilities` JSON mapping

The `--capabilities` output is the protobuf JSON mapping of the
`Capabilities` message. Field names are lowerCamelCase:

<!-- vale Microsoft.Avoid = NO -->
- `backend`
- `algorithm`
<!-- vale Microsoft.Avoid = YES -->
- `supportedKinds`
- `maxNodes`
- `maxEdges`
- `features`
- `protocolVersion`
- `streamWidth`
- `nativeTopologyHash` (omitted when unset). The value is standard base64
  because protobuf JSON maps `bytes` that way.

This changed in 0.0.0-rc1. The previous hand-written output used
`supported_kinds`, `max_nodes`, and `max_edges`.
<!-- vale Microsoft.Avoid = NO -->
`backend` and `algorithm` keep the same spelling.
<!-- vale Microsoft.Avoid = YES -->

`--capabilities` and the `Capabilities` message on the session stream are
the same message. One message must not have two spellings.

## 9. Adding a solver in another language

The normative wire contract is `proto/quip/v1/miner.proto`. Section 2 (The
four modes) states the session-mode command line.

`quip-miner-dwave` is the working example of a solver built this way today.

### Now

These pieces exist now:

- The proto.
- The `quip-solver-core` wheel: protocol primitives, scoring, wire encoding, and
  exit codes.
- `quip-solver-drive` as the conformance gate for a solver in any language.

The `quip-solver-core` wheel carries protocol primitives and handshake helpers. It
does not carry the session loop. A Python solver written today must write
the state machine itself.

### Next

Next work adds first-class bindings that carry the session loop. A solver in
another language then supplies only a sampler.

- Python through PyO3.
- C and C++ through a C ABI with a registered callback.
- Node through a napi-rs addon.

All three wrap the one Rust loop. They do not reimplement the loop.

### Not planned

Browser WASM is not planned. A browser cannot open a Unix or raw TCP socket.

A browser would need a new transport on both the solver and the coordinator.
A new transport is a protocol change.
