# quip-solver-core

This repository is the shared Quip solver contract. A solver binary accepts
Ising problems and returns spin configurations with their energies.

The contract has two bindings. One is a Rust crate workspace. The other is a
Python wheel named `quip_proto`. The wheel carries the consensus primitives
and the generated gRPC stubs, not the session loop, so a Python solver
written today writes its own state machine. A Rust solver conforms today by
supplying a `Sampler` and calling `run`.

The full contract is in [SPEC.md](SPEC.md).
[CHANGELOG.md](CHANGELOG.md) lists the 0.0.0-rc1 changes.
[MIGRATING.md](MIGRATING.md) covers the move from `quip-miner-core`.

## Crates

| Crate | What it holds |
| -- | -- |
| `quip-solver-core` | The `Sampler` trait, Ising and CSR types, beta ladder, adaptive budget, session loop |
| `quip-proto` | Generated tonic and prost stubs for `proto/quip/v1/miner.proto` |
| `quip-protocol` | Wire codec, energy and diversity scoring, handshake, ChaCha8 draw, nonce derivation |
| `quip-solver-conformance` | Golden vectors and the scripted session driver |
| `quip-protocol-py` | PyO3 bindings exposing `quip-protocol` scoring and wire primitives to Python |

## Add the Rust dependency

```toml
[dependencies]
quip-solver-core = { git = "https://gitlab.com/quip.network/quip-solver-core.git", tag = "v0.0.0-rc1" }
quip-proto = { git = "https://gitlab.com/quip.network/quip-solver-core.git", tag = "v0.0.0-rc1" }

[dev-dependencies]
quip-solver-conformance = { git = "https://gitlab.com/quip.network/quip-solver-core.git", tag = "v0.0.0-rc1" }
```
