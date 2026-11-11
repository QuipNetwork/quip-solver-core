# `quip-solver-core`

This repository is the shared Quip solver contract. A solver binary accepts
Ising problems and returns spin configurations with their energies.

Rust solvers supply a `Sampler` and call `run`.
C and C++ solvers register sampling callbacks with the C library.
The Python wheel and npm package carry consensus primitives and generated gRPC stubs.
Python and TypeScript solvers provide their own session loops.

The full contract is in [SPEC.md](SPEC.md).
[CHANGELOG.md](CHANGELOG.md) lists the 0.0.11 changes.
[MIGRATING.md](MIGRATING.md) covers solver API changes.
Releases from `0.0.2-rc3` require protocol version 2.
Update the coordinator and solvers together using the
[coordinator upgrade guide](docs/coordinator-upgrade-v2.md).
The [family release](docs/family-release.md) lists the families, the package versions, and the upgrade order of the release.
The [miner hardware guide](docs/miner-side-generation.md) describes how a GPU or other device draws lease problems.
The [topology file format](docs/topology-format.md) defines the `.bin.gz` files that hold the family vectors topologies.

## Crates

| Crate | What it holds |
| -- | -- |
| `quip-solver-core` | The `Sampler` trait, Ising and compressed sparse row types, inverse-temperature ladder, adaptive budget, session loop |
| `quip-proto` | Generated tonic and prost stubs for `proto/quip/v1/miner.proto` |
| `quip-protocol` | Wire codec, energy and diversity scoring, handshake, ChaCha8 draw |
| `ising-generators` | Family descriptors, canonical instances, ChaCha8 streams, and nonce derivation, `no_std` |
| `quip-solver-conformance` | Golden vectors and the scripted session driver |
| `quip-protocol-py` | PyO3 bindings exposing `quip-protocol` scoring and wire primitives and the family generator to Python |
| `quip-protocol-wasm` | WebAssembly bindings behind the npm package |
| `quip-solver-c` | C binary interface over the session loop, shipped as a release artifact |

## Install

Every binding ships from its own registry. The C library ships as a release
artifact, because a C consumer wants the compiled object and the header.

### Rust

```toml
[dependencies]
quip-solver-core = "0.0.11"

[dev-dependencies]
quip-solver-conformance = "0.0.11"
```

One dependency is enough. `quip-solver-core` re-exports `quip_proto` and
`quip_protocol`. Default `Sampler` implementations use `IsingGraph<f64>`
and `quip_solver_core::quip_protocol::scoring::energy_milli`.

A solver can select `Sampler<Milli>` to receive exact wire coefficients.
Import `Milli` from `quip_solver_core::coefficient`. Its stored integer is
available through `.0`. Score those integers with
`quip_solver_core::quip_protocol::scoring::energy_from_milli`.
The Rust example shows this choice.

### Python

```sh
pip install quip-solver-core
```

The wheel exposes `scoring`, `wire`, and `families`. The `families` module
builds the canonical instance of a family lease with `families.generate`. It
takes the descriptor, the topology as node and edge lists, and the nonce.

### Node packages

```sh
npm install @quip.network/quip-solver-core
```

### Native libraries

Download `quip-solver-clib-<tag>-linux-amd64.tar.gz` from the release page. It
holds `libquip_solver_c.so`, `libquip_solver_c.a`, and `include/quip_solver.h`.

## Samples

`examples/` holds a conformant mock solver in four languages. Rust, C++,
Python, and TypeScript each consume the published artifact and pass
`quip-solver-drive`. Start there rather than from this README.

## Conformance

A solver is conformant when `quip-solver-drive` reports a conformant session
and a conformant `--solve` run.
The `--family` gate applies to a solver that lists generated families.

```sh
cargo build -p quip-solver-conformance --bin quip-solver-drive --release
./target/release/quip-solver-drive <solver-binary> unix:///tmp/quip-check.sock
./target/release/quip-solver-drive --solve <solver-binary>
./target/release/quip-solver-drive --family <solver-binary> unix:///tmp/quip-family.sock
```

Exit code 0 means conformant. With `--family`, a revision that the solver
lists in `generated_families` but the driver cannot drive fails the gate. The
driver spawns any executable, so this works for a solver in any language.

## Build toolchain image

CI jobs on linux/amd64 run on a prebuilt image, so no job installs system
packages or toolchains at run time. `.gitlab/ci-toolchain.Dockerfile` defines
it. The image holds the Rust toolchain from `rust-toolchain.toml` with clippy,
rustfmt, and the `wasm32-unknown-unknown` target, plus protoc, Node.js 24, npm,
wasm-pack, shellcheck, shfmt, and a Python virtual environment in `/opt/venv`.
The environment holds maturin, twine, grpcio-tools, and ruff. The Dockerfile
carries every version pin and checksum. Repository source, npm packages, and the
cargo registry stay out of the image.

The `build-toolchain-image` job builds the image with kaniko. It runs
automatically on a branch or merge request pipeline that changes the
Dockerfile. It never runs on a tag pipeline.

Jobs reference the image by digest in `TOOLCHAIN_IMAGE` in `.gitlab-ci.yml`. A
rebuild does not change what jobs run until you update that digest. To change
the toolchain:

1. Edit the Dockerfile and push. The job builds the image.
2. Copy the digest from the end of the job log into `TOOLCHAIN_IMAGE`.

Run the job by hand when a pinned tool changes without a Dockerfile edit. Run it
to pick up a security update to the Debian packages, or to
rebuild after the registry deletes the image.

## Licence

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
