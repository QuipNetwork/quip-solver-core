# quip-solver-core

This repository is the shared Quip solver contract. A solver binary accepts
Ising problems and returns spin configurations with their energies.

The contract has two bindings. One is a Rust crate workspace. The other is a
Python wheel named `quip_proto`. A solver written in either language can
conform.

The full contract is in [SPEC.md](SPEC.md).
[CHANGELOG.md](CHANGELOG.md) lists the 0.3.0 changes.
[MIGRATING.md](MIGRATING.md) covers the move from `quip-miner-core`.

## Crates

| Crate | What it holds |
| -- | -- |
| `quip-solver-core` | The `Sampler` trait, Ising and CSR types, beta ladder, adaptive budget, session loop |
| `quip-proto` | Generated tonic and prost stubs for `proto/quip/v1/miner.proto` |
| `quip-protocol` | Wire codec, energy and diversity scoring, handshake, ChaCha8 draw, nonce derivation |
| `quip-solver-conformance` | Golden vectors and the scripted session driver |

## Add the Rust dependency

```toml
[dependencies]
quip-solver-core = { git = "https://gitlab.com/quip.network/quip-solver-core.git", tag = "v0.3.0" }
quip-proto = { git = "https://gitlab.com/quip.network/quip-solver-core.git", tag = "v0.3.0" }

[dev-dependencies]
quip-solver-conformance = { git = "https://gitlab.com/quip.network/quip-solver-core.git", tag = "v0.3.0" }
```
