# Changelog

This file documents changes to the Quip solver contract.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.0.2-rc3

### Added

- `ISING_GENERATE` salt leases, `IsingProblemGenerator`, and `LeaseDone`.
  The default Rust and C session generates problems and reports winning salts.
- Optional Rust `generates_locally`, `sample_lease`, `Lease`, and `LeaseSink` APIs.
  The host verifies candidate winners from local generation.
  The writer checks local winners for cancellation and lease expiry before transmission.
  Shutdown stops new salts but accepts verified winners from running work during grace.
  The local lease closes when its worker returns or grace expires, whichever comes first.
  Local fatal errors send no lease summary or credit refund.
  The advertised credit window bounds live local workers.
  Stopped workers that prevent replacement work cause a device fault.
- `CoefficientEncoding` and integer scales, with `I32`, `I16`, `I8`, `F16`, `F32`, and `F64` wire forms.
- `Topology.allowed_j_milli`, `SetTarget.max_proof_solutions`, and the `TARGET_MISSING` reject reason.
- Shared `meets_target` and `verify_lease_result` checks, with validator-derived golden vectors.
- The [coordinator upgrade guide](docs/coordinator-upgrade-v2.md).

### Changed

- Protocol version 2 requires a coordinated upgrade of solvers and coordinators.
  `Hello.capabilities` carries the capability message.
- `Backend` and `Algorithm` identities use enums. Command-line identity names remain lowercase.
- `Solution.spins` uses bit-packed spins. Lease results add `salt` and `nonce`.
- The session checks coefficient exactness per problem and rescores only lossy conversions.
  Matching encoding and scale permit direct decoding without a retained milli copy.
- Python and TypeScript stubs and example miners use v2 plain jobs.
- Example version output reports protocol 2.
  C identity comments describe enum defaults and invalid names.
  Python codec comments distinguish one byte per spin from bit-packed spins.

### Removed

- The duplicated v1 `Hello` fields.
  These cover the version and identity, supported kinds, limits, topology hash, and features.
- The v1 string fields at `Capabilities` numbers 1 and 2.
- `IsingProblem.h_milli_le32`, `IsingProblem.j_milli_le32`, and `Solution.spins_bytes`.
  Their old names and numbers remain reserved.
  `IsingProblem.gates` also has a reserved name alongside its already reserved number 6.

## 0.0.2-rc2

### Added

- `IsingGraph<C = f64>` and `Sampler<C = f64>` let a Rust sampler select
  its coefficient representation. The fields keep the names `h` and `j`.
- The sealed `Coefficient` trait supports `f64`, `f32`, `half::f16`, and
  `Fixed<T, SCALE>` for `i32`, `i16`, `i8`, and `I4` storage.
  `Milli` is `Fixed<i32, 1000>`. Fixed-point conversions round halfway
  values away from zero and saturate to the storage range.
- Lossy samplers receive converted coefficients. The session and JSON
  driver re-score their results from the original coefficients.
- `quip_protocol::scoring::energy_from_milli` scores `i32` coefficients
  directly. The Rust example uses `Sampler<Milli>` and this scorer.

The default coefficient type is `f64`. C, Python, WebAssembly, JSON, and
wire interfaces keep their existing forms.

### Changed

- `run`, `run_code`, and `driver::solve` take a second type parameter for
  the coefficient type. A call that names the sampler type explicitly, such
  as `solve::<S>(...)`, now writes `solve::<S, _>(...)` or lets the compiler
  infer both. Calls that infer the sampler type compile unchanged.
- A struct literal `IsingGraph { h: vec![], j: vec![], edges: vec![] }` whose
  type nothing else fixes no longer infers `f64`. Write
  `IsingGraph::<f64> { .. }`, give the binding the type `IsingGraph`, or call
  `IsingGraph::new`.

## 0.0.2-rc1

Optional warm-start states on `IsingProblem`, for quantum processing unit reverse anneal and
seeded SA.

### Added

- `IsingProblem` field 9, `initial_spins`, carries bit-packed start states.
  Fields 10 to 12 (`start_beta_milli`, `reversal_s_milli`,
  `reversal_pause_us`) set where a seeded anneal starts. A solver that uses
  them advertises the `initial-spins` feature. `SPEC.md` section 3 describes the
  encoding and the solver behavior.
- `Sampler::accepts_warm_start`, `Sampler::sample_warm`, and
  `Sampler::sample_stream_warm`, all defaulted, plus the `WarmStart` and
  `WarmStreamJob` types and the `INITIAL_SPINS_FEATURE` constant.
- `quip_protocol::wire::encode_spins_packed` and `decode_spins_packed`, with
  the `WireError::BadPackedLength` and `WireError::NonZeroPadding` variants.
- The conformance driver sends a seeded job to every solver. It grades a
  solver that advertises `initial-spins` on a malformed state and on a
  seeded 4096-spin ring. `DriverReport::advertises_initial_spins()` and
  `DriverReport::warm_start_conformant()` are public.

### Changed

- The session rejects a start state with the wrong length or a padding bit set with `MALFORMED`.
  It also rejects `reversal_s_milli` values of 1000 or more. This applies to every solver, including one that does not use the
  states.

## 0.0.1

Packaging, conformance, and release-pipeline fixes. This section consolidates
the two release candidates below. Nothing shipped in 0.0.1 that was not in
0.0.1-rc2.

### Fixed

- The sdist now carries the generated `quip` gRPC stub package, so an install
  that falls back to the sdist works. 0.0.0 marked it wheel-only, and a
  from-sdist install failed at first import.
- The conformance driver expects the doubled sweep echo from a `gibbs`
  solver, so a gibbs solver can pass `is_conformant()`.
- The release-tag rule requires a full `v<major>.<minor>.<patch>` with an
  optional prerelease. The old rule matched any tag that starts with `v` and
  a digit.
- `release:validate` waits for the lint job.

### Added

- PyPI wheels for macOS arm64 and Linux aarch64.
- `CONFIGURED_SWEEPS`, `GIBBS_SWEEP_MULTIPLIER`,
  `DriverReport::expected_meta_sweeps()`, and `SampleError::is_fatal()` are
  public.
- `Sampler::declared_stream_width()` accepts `0` for a device-dependent width
  that is unknown until the device opens.
- `release:tag-guard`, which fails the pipeline of any tag outside the
  release pattern instead of skipping every release job on a green pipeline.

## 0.0.1-rc2

Release-pipeline fixes from the v0.0.1-rc1 recon: an anchored tag rule, a
guard job for malformed tags, and a lint gate on releases.

### Fixed

- The release-tag rule matched any tag that starts with `v` and a digit. A
  malformed tag such as `v0.0.1-typo` started the whole publish sequence and
  failed only at the version check. The rule now requires a full
  `v<major>.<minor>.<patch>` with an optional prerelease.
- `release:validate` now waits for the lint job. A comment excused the gap:
  four unformatted files predate the job.
  The 0.0.0 release already includes those format fixes.

### Added

- `release:tag-guard`, which fails the pipeline of any tag outside the release
  pattern. Before this job, such a tag skipped every release job while
  `release:validate` ran and passed: a green pipeline that published nothing.

## 0.0.1-rc1

This release fixes packaging and conformance in 0.0.0.
The sdist installs correctly. PyPI covers macOS arm64 and Linux aarch64.
A Gibbs solver can pass the conformance driver.

### Fixed

- The sdist now carries the generated `quip` gRPC stub package. 0.0.0 marked
  it wheel-only, so an install that fell back to the sdist built successfully
  and then failed at first import with `ModuleNotFoundError: No module named
  'quip'`. `make check-python-dist` now fails when the stubs are missing from
  the tarball.
- The conformance driver expects the doubled sweep echo from a `gibbs`
  solver. `SPEC.md` pins a sweep budget. A Gibbs solver runs and reports twice that budget.
  `sweeps_honoured()` compared every solver with the raw budget, so no Gibbs solver could pass `is_conformant()`.

### Added

- PyPI wheels for macOS arm64 and Linux aarch64, built on the group's Apple
  Silicon and arm64 docker runners. 0.0.0 shipped only the linux/amd64 wheel,
  which sent every other platform to the broken sdist.
- `CONFIGURED_SWEEPS`, `GIBBS_SWEEP_MULTIPLIER`, and
  `DriverReport::expected_meta_sweeps()` are public, so a solver repository's
  tests can state sweep expectations without mirroring the literals.
- `SampleError::is_fatal()` is public, so a solver with its own
  `sample_stream` pump reuses the session's fatality rule instead of
  matching variants.
- `Sampler::declared_stream_width()` accepts `0`: a device-dependent width,
  unknown until the device opens. `--capabilities` keeps the `0`, the
  in-session `Capabilities` reply carries the live width, and the session
  logs no misdeclaration for a width it could never state.

## 0.0.0

The first release. Use 0.0.0 rather than any release candidate: the rc tags
only exercised the deployment pipeline.

### Fixed

- The floating-point energy scorer, which disagreed with the integer-milli
  consensus rule near a rounding boundary. Scoring now recovers the exact
  milli values and sums them in 128-bit integers.
- The C binary interface read out of bounds when `j` and `edges` had different lengths.
  It also returned the legal energy 0 for invalid calls.
  It passed `NULL` to `slice::from_raw_parts` for empty solutions.
  It could leave the sampler thread running after `quip_solver_run` returned.
- Cancelled in-flight jobs no longer emit a `Result`. The session checks the
  cancel watermark again after sampling, and the writer drops stale results.
- A panic in the outbound writer now exits 70 rather than 0. An unexpected
  stream close exits 70 after `Welcome` and 77 before it, in all three example
  programs.
- The release pipeline. The publish jobs now wait for conformance and the smoke tests.
  PyPI and npm promote the exact artifacts the smoke tests ran.
- `Hello.features` was always empty, and `--capabilities` hardcoded the
  stream width. Both answers now come from one function.

### Added

- A conformance driver that checks message causality, scores returned spins
  again, exercises `GetCapabilities`, `Ping`, live cancellation, and
  `backend_toml`, and keeps a credit ledger.
- Byte-level wire fixtures in `golden_wire.json`, which the Rust tests check.
- A lint job for shell and Python, with shellcheck, shfmt, and ruff.
  Run `make lint` locally or in CI.
- Tests for the C binding crate and exit codes 64, 69, 70, and 77.

### Changed

- The TypeScript stubs encode 64-bit integers as `bigint` rather than
  `number`, so watermarks greater than 2^53 survive the round trip.
- `max_nodes = 0` and `max_edges = 0` mean unlimited everywhere. The parser
  read 0 as a zero cap while `Hello` advertised unlimited.
- A tag now moves the npm `latest` dist-tag to the version it publishes.
  The job used `rc`, which left `latest` on the first version.
  OpenID Connect permits only `npm publish` and `npm stage publish` here.
  A later `npm dist-tag add` needs a stored token, which the pipeline does not hold.

## 0.0.0-rc6

Use 0.0.0-rc6, the first version present on all three registries. 0.0.0-rc1 and
0.0.0-rc2 reached crates.io only. 0.0.0-rc3 reached PyPI only, as a wheel with
no source distribution. 0.0.0-rc4 published nowhere. 0.0.0-rc5 reached npm only.

### Fixed

- The source distribution declared `License-File: LICENSE` and `License-File: NOTICE`, but contained neither file.
  Both files sit at the repository root.
  Maturin named them in the metadata without packing them.
  PyPI rejected the source upload with 400 after accepting the wheel.

### Added

- `scripts/check-sdist-license-files.sh`, which fails when a source
  distribution names a license file it does not contain. `twine check` accepts
  that combination, because the metadata itself is well formed, so nothing
  before the upload caught it.

### Changed

- The registries publish in order of recovery cost, starting with npm, then PyPI, then crates.io.
  An operator can unpublish an npm version within 72 hours.
  An operator can delete a PyPI release, but cannot reuse its filenames.
  An operator can only yank a crates.io version.
  The first failure stops the remaining jobs.

### Fixed

- The npm registry rejected the publish with 422.
  It accepts provenance attestations only from GitLab-hosted runners, and every runner here is self-hosted. Trusted publishing turns provenance on by itself, so the
  job now disables it. Published packages carry no provenance attestation until
  this job moves to a hosted runner.
- The PyPI upload, which reported a bare `400 Bad Request` and discarded the
  response body. The upload now runs with `--verbose`, which prints the reason.

## 0.0.0-rc3

0.0.0-rc3 reached PyPI only, and only as a wheel.

### Changed

- crates.io publishes last and waits for PyPI and npm to succeed.
  It cannot accept a second upload of a version.
  Publishing it first spent a version number when a later job failed.
  Publishing it last keeps that version free after an earlier failure.
  The operator can then fix and push the tag again.

### Fixed

- The npm publish, which failed on provenance after authenticating. The
  job drops the explicit `--provenance` flag.
  Trusted publishing generates provenance itself.
  The flag turned a missing attestation token into a failed publish. `SIGSTORE_ID_TOKEN` is now declared for the
  automatic path.
- The order of the PyPI job, which ran `twine check` before installing twine.

## 0.0.0-rc2

0.0.0-rc2 reached crates.io only.

### Fixed

- The PyPI release job, which never ran a line of its script. The maturin image
  sets an entrypoint, so each command in the job arrived as an argument to
  `maturin` instead of running in a shell.
- The npm release job, which reported a missing login when the cause was an
  incomplete OpenID Connect exchange. The job now checks the Node and npm versions and
  the presence of the identity token, and reports which one is missing.

## 0.0.0-rc1

### Added

- Published releases. The Rust crates go to crates.io, the `quip-solver-core`
  wheel to PyPI, and `@quip.network/quip-solver-core` to npm. The C library ships as
  a release artifact. A tag builds and publishes all four.
- `quip-solver-c`, a C binary interface over the session loop. A C or C++ solver registers
  one sampling callback and calls `quip_solver_run`. It also exports
  `quip_energy_milli` so a C solver scores with the shipped scorer.
- `@quip.network/quip-solver-core`, the npm package. It carries the consensus
  primitives as WebAssembly and the generated gRPC stubs.
- `run_code`, which returns the `ExitCode` enum. `run` stays as a thin wrapper
  for a Rust `main`. A foreign function interface needs the numeric code, and
  `std::process::ExitCode` has no numeric accessor.
- `quip-solver-core` re-exports `quip_proto` and `quip_protocol`. A solver now
  names one dependency instead of two that must move in lockstep.
- Example solvers in Rust, C++, Python, and TypeScript under `examples/`. Each
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

- **Breaking:** the licence changes from `AGPL-3.0-or-later` to Apache-2.0.
  Apache-2.0 adds an express patent grant and stays compatible with `AGPLv3`.
  A developer can combine this code into an `AGPLv3` work.
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
