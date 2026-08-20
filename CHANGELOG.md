# Changelog

This file documents changes to the Quip solver contract.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.0.0-rc4

Use 0.0.0-rc4, the first version present on all three registries. 0.0.0-rc1 and
0.0.0-rc2 reached crates.io only. 0.0.0-rc3 reached PyPI only, as a wheel with
no source distribution.

### Changed

- The registries now publish in order of how hard each one is to undo: npm,
  then PyPI, then crates.io. An unwanted npm version can be unpublished within
  72 hours, a PyPI release can be deleted but never reuses its filenames, and a
  crates.io version can only be yanked. The first failure now stops everything
  after it, so a broken release strands as little as possible.

### Fixed

- The npm publish, which the registry rejected with 422 because npm accepts a
  provenance attestation only from a GitLab-hosted runner, and every runner
  here is self-hosted. Trusted publishing turns provenance on by itself, so the
  job now disables it. Published packages carry no provenance attestation until
  this job moves to a hosted runner.
- The PyPI upload, which reported a bare `400 Bad Request` and discarded the
  response body. The upload now runs with `--verbose`, which prints the reason.

## 0.0.0-rc3

0.0.0-rc3 reached PyPI only, and only as a wheel.

### Changed

- crates.io now publishes last, in its own stage, and waits for PyPI and npm to
  succeed. crates.io is the only one of the three that can never accept a
  second upload of a version, so publishing it first spent a version number
  every time a later job failed. Publishing it last leaves the version
  untouched when anything else fails, so the same tag can be fixed and pushed
  again.

### Fixed

- The npm publish, which failed on provenance after authenticating. The
  explicit `--provenance` flag is gone, because npm generates provenance on its
  own through trusted publishing, and the flag turned a missing attestation
  token into a failed publish. `SIGSTORE_ID_TOKEN` is now declared for the
  automatic path.
- The order of the PyPI job, which ran `twine check` before installing twine.

## 0.0.0-rc2

0.0.0-rc2 reached crates.io only.

### Fixed

- The PyPI release job, which never ran a line of its script. The maturin image
  sets an entrypoint, so each command in the job arrived as an argument to
  `maturin` instead of running in a shell.
- The npm release job, which reported a missing login when the cause was an
  incomplete OIDC exchange. The job now checks the Node and npm versions and
  the presence of the identity token, and reports which one is missing.

## 0.0.0-rc1

### Added

- Published releases. The Rust crates go to crates.io, the `quip-solver-core`
  wheel to PyPI, and `@quip.network/quip-solver-core` to npm. The C library ships as
  a release artifact. A tag builds and publishes all four.
- `quip-solver-c`, a C ABI over the session loop. A C or C++ solver registers
  one sampling callback and calls `quip_solver_run`. It also exports
  `quip_energy_milli` so a C solver scores with the shipped scorer.
- `@quip.network/quip-solver-core`, the npm package. It carries the consensus
  primitives as WebAssembly and the generated gRPC stubs.
- `run_code`, which returns the `ExitCode` enum. `run` stays as a thin wrapper
  for a Rust `main`. A foreign function interface needs the numeric code, and
  `std::process::ExitCode` cannot be read back into a number.
- `quip-solver-core` re-exports `quip_proto` and `quip_protocol`. A solver now
  names one dependency instead of two that must move in lockstep.
- Sample solvers in Rust, C++, Python, and TypeScript under `examples/`. Each
  one passes the conformance gate.
- The `quip-solver-conformance` crate. It holds the golden vectors and the
  scripted session driver.
- `Capabilities` and `GetCapabilities` messages on the session stream.
  `--capabilities` prints that same message.
- `--solve` mode. The binary reads one JSON problem on stdin and writes JSON
  solutions on stdout.
- `CancelToken::abandoned`. `Status.abandoned_generation` now reports the
  current abandoned watermark.
- Workspace `license` and `repository` metadata on every crate.

### Changed

- **Breaking:** The licence changes from AGPL-3.0-or-later to Apache-2.0.
  Apache-2.0 adds an express patent grant and stays compatible with AGPLv3, so
  this code can still be combined into an AGPLv3 work.
- `quip-proto` ships its generated stubs instead of running `tonic-build` from
  a build script. A consumer no longer needs `protoc` on the build host. A test
  regenerates the stubs and fails when the checked-in copy is stale.
- `quip-protocol` gates its `session` module behind a default feature. Turning
  the feature off drops `tonic` and `prost`, which is what lets the consensus
  core build for `wasm32`.
- **Breaking:** `Sampler::sample` now returns
  `Result<Vec<SamplerResult>, SampleError>`. The previous error type was
  `RejectReason`. `SampleError` has three variants: `Capacity`,
  `DeviceBusy`, and `DeviceFault`. The crate maps them to the wire
  `RejectReason` at the boundary. A `Sampler` no longer names wire enum
  values.
- **Breaking:** `CancelToken` replaces `CancelGuard`.
  `StreamJob.watermark: Option<u64>` replaces `StreamJob.generation`.
  A job with no watermark is never cancelled.
- This repository renames `quip-miner-core` to `quip-solver-core`. The
  public surface is otherwise unchanged.
- `--capabilities` JSON uses the protobuf JSON mapping. Field names are
  lowerCamelCase.
- A closed stdout pipe from `--solve` or `--capabilities` exits 0.
- `native_topology_hash` serializes as standard base64.

### Removed

- The `tonic`, `prost`, and `tokio` dependencies of `quip-protocol`. No source
  file used them.
- The `quip-proto` build script.
- `CancelGuard`.
- `StreamJob.generation`.
- `RejectReason` as the `Sampler::sample` error type.
