# Sample Quip solvers

Four mock solvers, one per language. Each one covers the whole solver
contract in SPEC.md and passes the conformance gate, so each can run as a
quip-miner. Each one consumes a published Quip artifact rather than a path into
this repository.

The sampler in every example is trivial on purpose. It returns `num_reads`
copies of the all-(+1) configuration. The value of these examples is the
plumbing around that sampler, not the sampler itself.

## Conformance status

Every solver below passes `quip-solver-drive`, which is the definition of
conformance in SPEC.md section 6.

CI runs each one against the built release artifact in a clean container, not
against this source tree. The Rust sample builds against the unpacked `.crate`
tarballs, C++ compiles against the released header and library, Python installs
the wheel, and TypeScript installs the packed npm tarball. A file missing from
a package therefore fails before that package reaches a registry.

| Language | Directory | Artifact it consumes | Session loop | Conformant |
| -- | -- | -- | -- | -- |
| Rust | `rust/` | `quip-solver-core` from crates.io | From the crate | Yes |
| C++ | `cpp/` | `libquip_solver_c` plus `quip_solver.h` | From the C library | Yes |
| Python | `python/` | `quip-solver-core` wheel | Written in the example | Yes |
| TypeScript | `typescript/` | `@quip.network/quip-solver-core` from npm | Written in the example | Yes |

## Two ways to build a solver

The preceding table splits the four examples into two groups.

**Rust and C++ reuse the Rust session loop.** A Rust solver supplies a `Sampler`
and calls `run`. A C++ solver registers one callback and calls
`quip_solver_run`. Neither one writes a state machine. SPEC.md section 9 asks
for this: the C binding wraps the one loop instead of copying it, so a C++
solver cannot drift from a Rust solver.

**Python and TypeScript write the state machine.** Neither published package
carries the session loop today, so `mock_miner.py` and `mock_miner.ts` handle
the handshake, credits, rejects, cancellation, and shutdown themselves. Both
still call the shipped consensus code for energy scoring and spin encoding.
Python calls it through PyO3, TypeScript through WebAssembly. Both run the same
compiled Rust that the crates.io release runs.

That second point matters more than it looks. The network recomputes the energy
of every solution and rejects a solution whose reported energy disagrees. A
second copy of the scorer is a liability, so no example contains one.

## Run the conformance gate

Build the driver once from the repository root.

```sh
cargo build -p quip-solver-conformance --bin quip-solver-drive --release
```

Then point it at any solver binary. Exit code 0 means conformant.

```sh
./target/release/quip-solver-drive <solver-binary> unix:///tmp/quip-check.sock
```

The driver spawns the binary and speaks the proto over the socket. It scores
the session and accepts an executable in any language.

## Build each example

### Rust

```sh
cd examples/rust && cargo build --release
./target/release/mock_miner --capabilities
```

### C++

```sh
cd examples/cpp && make lib && make mock_miner
./mock_miner --capabilities
```

`make lib` builds `libquip_solver_c` from this repository. A consumer outside
this repository instead points `QUIP_LIB_DIR` and `QUIP_INCLUDE_DIR` at the
released artifact.

### Python

```sh
pip install quip-solver-core
cd examples/python && python mock_miner.py --capabilities
```

### TypeScript

```sh
cd examples/typescript && npm install && npm run build
node dist/mock_miner.js --capabilities
```

## The four modes

Every example supports the four modes in SPEC.md section 2.

| Mode | What it does |
| -- | -- |
| `--capabilities` | Prints the capabilities JSON and exits. Must not open the device. |
| `--solve` | Reads one problem as JSON on stdin, writes solutions on stdout. |
| `--check` | Opens the device and exits. Exit 69 means the host cannot run it. |
| session | Streams jobs for the life of the process. |

Session mode takes `--quip-coordinator unix://<path>` and `--miner-id <id>`. The
solver dials that socket. The solver must not listen on it.

## Two traps these examples record

Both cost real debugging time. Both apply to any solver that writes its own
session loop.

**Set the gRPC authority explicitly.** A Unix socket has no hostname, so gRPC
derives an `:authority` header from the socket path. Tonic rejects that header
and answers `RST_STREAM(PROTOCOL_ERROR)` before the handshake completes. The
Python and TypeScript examples both set the authority to `localhost`.

**Flush the outbound stream before you exit.** On `Shutdown`, half-close the
send side and let the queued messages drain. A solver that returns immediately
drops whatever it has already queued. The coordinator reads that as a solver
that ignored those jobs, and conformance fails with no error message anywhere.
