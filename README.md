# quip-solver-core

This repository is the shared Quip solver contract. A solver binary accepts
Ising problems and returns spin configurations with their energies.

The contract has two bindings. One is a Rust crate workspace. The other is a
Python wheel named `quip-solver-core`. The wheel carries the consensus primitives
and the generated gRPC stubs, not the session loop, so a Python solver
written today writes its own state machine. A Rust solver conforms today by
supplying a `Sampler` and calling `run`.

The full contract is in [SPEC.md](SPEC.md).
[CHANGELOG.md](CHANGELOG.md) lists the 0.1.0-rc1 changes.
[MIGRATING.md](MIGRATING.md) covers the move from `quip-miner-core`.

## Crates

| Crate | What it holds |
| -- | -- |
| `quip-solver-core` | The `Sampler` trait, Ising and CSR types, beta ladder, adaptive budget, session loop |
| `quip-proto` | Generated tonic and prost stubs for `proto/quip/v1/miner.proto` |
| `quip-protocol` | Wire codec, energy and diversity scoring, handshake, ChaCha8 draw, nonce derivation |
| `quip-solver-conformance` | Golden vectors and the scripted session driver |
| `quip-protocol-py` | PyO3 bindings exposing `quip-protocol` scoring and wire primitives to Python |
| `quip-protocol-wasm` | WebAssembly bindings behind the npm package |
| `quip-solver-c` | C ABI over the session loop, shipped as a release artifact |

## Install

Every binding ships from its own registry. The C library ships as a release
artifact, because a C consumer wants the compiled object and the header.

### Rust

```toml
[dependencies]
quip-solver-core = "0.1.0-rc1"

[dev-dependencies]
quip-solver-conformance = "0.1.0-rc1"
```

One dependency is enough. `quip-solver-core` re-exports `quip_proto` and
`quip_protocol`, so the consensus scorer is at
`quip_solver_core::quip_protocol::scoring::energy_from_milli`.

### Python

```sh
pip install quip-solver-core
```

### JavaScript and TypeScript

```sh
npm install @quip.network/quip-solver-core@rc
```

### C and C++

Download `quip-solver-clib-<tag>-linux-amd64.tar.gz` from the release page. It
holds `libquip_solver_c.so`, `libquip_solver_c.a`, and `include/quip_solver.h`.

## Samples

`examples/` holds a conformant mock solver in four languages. Rust, C++,
Python, and TypeScript each consume the published artifact and pass
`quip-solver-drive`. Start there rather than from this README.

## Conformance

A solver is conformant when `quip-solver-drive` reports a conformant session.

```sh
cargo build -p quip-solver-conformance --bin quip-solver-drive --release
./target/release/quip-solver-drive <solver-binary> unix:///tmp/quip-check.sock
```

Exit code 0 means conformant. The driver spawns any executable, so this works
for a solver in any language.

## Licence

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
