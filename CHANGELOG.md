# Changelog

This file documents changes to the Quip solver contract.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.0

### Added

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

- `CancelGuard`.
- `StreamJob.generation`.
- `RejectReason` as the `Sampler::sample` error type.
