# Changelog

This file documents changes to the Quip solver contract.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.0.1-rc2

Release-pipeline fixes from the v0.0.1-rc1 recon: an anchored tag rule, a
guard job for malformed tags, and a lint gate on releases.

### Fixed

- The release-tag rule matched any tag that starts with `v` and a digit. A
  malformed tag such as `v0.0.1-typo` started the whole publish sequence and
  failed only at the version check. The rule now requires a full
  `v<major>.<minor>.<patch>` with an optional prerelease.
- `release:validate` now waits for the lint job. A comment excused the gap:
  four unformatted files predate the job. Those files were reformatted before
  0.0.0 shipped, so the exemption had outlived its reason.

### Added

- `release:tag-guard`, which fails the pipeline of any tag outside the release
  pattern. Before this job, such a tag skipped every release job while
  `release:validate` ran and passed: a green pipeline that published nothing.

## 0.0.1-rc1

Packaging and conformance fixes for the 0.0.0 release: the sdist installs
correctly, PyPI covers macOS arm64 and Linux aarch64, and a gibbs solver can
pass the conformance driver.

### Fixed

- The sdist now carries the generated `quip` gRPC stub package. 0.0.0 marked
  it wheel-only, so an install that fell back to the sdist built successfully
  and then failed at first import with `ModuleNotFoundError: No module named
  'quip'`. `make check-python-dist` now fails when the stubs are missing from
  the tarball.
- The conformance driver expects the doubled sweep echo from a `gibbs`
  solver. SPEC.md pins a sweep budget and a Gibbs solver runs — and reports —
  twice it, but `sweeps_honoured()` compared every solver against the raw
  budget, so no gibbs backend could pass `is_conformant()`.

### Added

- PyPI wheels for macOS arm64 and Linux aarch64, built on the group's Apple
  Silicon and arm64 docker runners. 0.0.0 shipped only the linux/amd64 wheel,
  which sent every other platform to the broken sdist.
- `CONFIGURED_SWEEPS`, `GIBBS_SWEEP_MULTIPLIER`, and
  `DriverReport::expected_meta_sweeps()` are public, so a solver repository's
  tests can state sweep expectations without mirroring the literals.
- `SampleError::is_fatal()` is public, so a backend with its own
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
- The C ABI. It read out of bounds when `j` and `edges` disagreed in length,
  returned 0 (a legal energy) for every invalid call, passed NULL to
  `slice::from_raw_parts` for empty solutions, and could leave the sampler
  thread running after `quip_solver_run` returned.
- Cancelled in-flight jobs no longer emit a `Result`. The session checks the
  cancel watermark again after sampling, and the writer drops stale results.
- A panic in the outbound writer now exits 70 rather than 0. An unexpected
  stream close exits 70 after `Welcome` and 77 before it, in all three sample
  implementations.
- The release pipeline. The publish jobs now wait for conformance and the
  smoke tests, and PyPI and npm promote the exact artifacts the smoke tests
  ran rather than rebuilding them.
- `Hello.features` was always empty, and `--capabilities` hardcoded the
  stream width. Both answers now come from one function.

### Added

- A conformance driver that checks message causality, scores returned spins
  again, exercises `GetCapabilities`, `Ping`, live cancellation, and
  `backend_toml`, and keeps a credit ledger.
- Byte-level wire fixtures (`golden_wire.json`), asserted from the Rust tests.
- A lint job for shell and Python (shellcheck, shfmt, ruff), run by `make
  lint` locally and in CI.
- Tests for the previously untested C ABI crate, and end-to-end tests for
  exit codes 64, 69, 70, and 77.

### Changed

- The TypeScript stubs encode 64-bit integers as `bigint` rather than
  `number`, so watermarks above 2^53 survive the round trip.
- `max_nodes = 0` and `max_edges = 0` mean unlimited everywhere. The parser
  previously read 0 as a zero cap while `Hello` advertised unlimited.
- A tag now moves the npm `latest` dist-tag to the version it publishes. The
  job previously published under `rc`, which left `latest` on whatever was
  published first. Moving the tag after the fact is not possible here: OIDC
  authorises `npm publish` and `npm stage publish` and nothing else, so
  `npm dist-tag add` would need a stored npm token, and this pipeline holds no
  registry secrets.

## 0.0.0-rc6

Use 0.0.0-rc6, the first version present on all three registries. 0.0.0-rc1 and
0.0.0-rc2 reached crates.io only. 0.0.0-rc3 reached PyPI only, as a wheel with
no source distribution. 0.0.0-rc4 published nowhere. 0.0.0-rc5 reached npm only.

### Fixed

- The source distribution, which declared `License-File: LICENSE` and
  `License-File: NOTICE` and contained neither. Both files sit at the
  repository root rather than inside the crate, so maturin named them in the
  metadata and packed neither, and PyPI rejected the upload with 400 after
  accepting the wheel of the same version.

### Added

- `scripts/check-sdist-license-files.sh`, which fails when a source
  distribution names a license file it does not contain. `twine check` accepts
  that combination, because the metadata itself is well formed, so nothing
  before the upload caught it.

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
