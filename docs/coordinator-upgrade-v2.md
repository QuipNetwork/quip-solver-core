# Upgrade a coordinator to protocol version 2

Update the coordinator and its solvers together for `0.0.2-rc3`.
Protocol version 2 does not support version 1 peers.
The protobuf package remains `quip.v1`.
Use [the wire definition](../proto/quip/v1/miner.proto) and [the solver contract](../SPEC.md) for message details.

## Wire changes

The tables list every field and enum change from version 1.
An absent field has no old number.
Retired numbers stay reserved and must never carry another meaning.

| Message and field | Old number | New number or replacement | Change |
| --- | --- | --- | --- |
| `Hello.protocol_version` | 3 | `capabilities.protocol_version`, 11 → 7 | Remove and reserve name and number |
| `Hello.backend` | 4 | `capabilities.backend`, 11 → 12 | Remove and reserve name and number |
| `Hello.algorithm` | 5 | `capabilities.algorithm`, 11 → 13 | Remove and reserve name and number |
| `Hello.supported_kinds` | 6 | `capabilities.supported_kinds`, 11 → 3 | Remove and reserve name and number |
| `Hello.max_nodes` | 7 | `capabilities.max_nodes`, 11 → 4 | Remove and reserve name and number |
| `Hello.max_edges` | 8 | `capabilities.max_edges`, 11 → 5 | Remove and reserve name and number |
| `Hello.native_topology_hash` | 9 | `capabilities.native_topology_hash`, 11 → 9 | Remove and reserve name and number |
| `Hello.features` | 10 | `capabilities.features`, 11 → 6 | Remove and reserve name and number |
| `Hello.capabilities` | Absent | 11 | Add nested `Capabilities` |
| `Capabilities.backend` | 1 | 12 | Replace string with `Backend`, reserve number 1 |
| `Capabilities.algorithm` | 2 | 13 | Replace string with `Algorithm`, reserve number 2 |
| `Capabilities.encodings` | Absent | 10 | Add supported coefficient encodings |
| `Capabilities.generators` | Absent | 11 | Add supported generators |
| `Topology.allowed_j_milli` | Absent | 5 | Add allowed coupling values |
| `IsingProblem.h_milli_le32` | 3 | `h`, 15 | Remove and reserve old name and number |
| `IsingProblem.j_milli_le32` | 4 | `j`, 16 | Remove and reserve old name and number |
| `IsingProblem.gates` | Reserved 6 | Reserved 6 | Also reserve the name `gates` |
| `IsingProblem.encoding` | Absent | 13 | Add `CoefficientEncoding` |
| `IsingProblem.scale` | Absent | 14 | Add integer steps per unit |
| `IsingProblem.h` | Absent | 15 | Add encoded fields |
| `IsingProblem.j` | Absent | 16 | Add encoded couplings |
| `Job.generator` | Absent | 7 | Add `IsingProblemGenerator` |
| `IsingProblemGenerator.algorithm` | Absent | 1 | Add generator enum |
| `IsingProblemGenerator.topology_hash` | Absent | 2 | Add cached topology reference |
| `IsingProblemGenerator.last_proof_block_hash` | Absent | 3 | Add nonce input |
| `IsingProblemGenerator.miner_account` | Absent | 4 | Add nonce input |
| `IsingProblemGenerator.base_salt` | Absent | 5 | Add salt template |
| `IsingProblemGenerator.salt_start` | Absent | 6 | Add first counter |
| `IsingProblemGenerator.salt_count` | Absent | 7 | Add range length |
| `Solution.spins_bytes` | 1 | `spins`, 3 | Remove and reserve old name and number |
| `Solution.spins` | Absent | 3 | Add bit-packed spins |
| `Result.salt` | Absent | 4 | Add winning lease salt |
| `Result.nonce` | Absent | 5 | Add derived nonce |
| `LeaseDone.job_id` | Absent | 1 | Add lease identifier |
| `LeaseDone.salts_done` | Absent | 2 | Add completed salt count |
| `LeaseDone.best_energy_milli` | Absent | 3 | Add lowest observed energy |
| `MinerMsg.lease_done` | Absent | 9 | Add lease completion message |
| `SetTarget.max_proof_solutions` | Absent | 7 | Add chain proof limit |

`Capabilities.protocol_version` remains field 7, with value 2.
`Welcome.protocol_version` remains field 1, with value 2.
`Solution.energy_milli` remains field 2.
Other fields keep their numbers and meanings.
No existing enum value changes number or becomes reserved.
`GATE_CIRCUIT = 2` remains an unsupported job kind.

All values below are new. Each old value is absent.
The prefix column gives the exact protobuf prefix for every suffix in its row.

| Enum | Prefix | New suffix and number |
| --- | --- | --- |
| `Backend` | `BACKEND_` | `UNSPECIFIED = 0`, `CPU = 1`, `CUDA = 2`, `METAL = 3`, `ANE = 4`, `DWAVE_QPU = 5`, `EXEC = 6`, `MOCK = 7` |
| `Algorithm` | `ALGORITHM_` | `UNSPECIFIED = 0`, `SA = 1`, `GIBBS = 2`, `QUANTUM_ANNEAL = 3`, `FSA = 4`, `MSA = 5`, `FLATIRON = 6` |
| `Algorithm` | `ALGORITHM_` | `MPS = 7`, `MFA = 8`, `SB = 9`, `BSB = 10`, `GBSB = 11`, `GDSB = 12`, `GGDSB = 13` |
| `Algorithm` | `ALGORITHM_` | `HBSB = 14`, `HDSB = 15`, `SBQA = 16`, `TEDSB = 17`, `EXTERNAL = 18` |
| `CoefficientEncoding` | `COEFFICIENT_ENCODING_` | `UNSPECIFIED = 0`, `I32 = 1`, `I16 = 2`, `I8 = 3`, `F16 = 4`, `F32 = 5`, `F64 = 6` |
| `GeneratorAlgorithm` | `GENERATOR_ALGORITHM_` | `UNSPECIFIED = 0`, `BLAKE3_CHACHA8_V1 = 1` |
| `JobKind` | None | `ISING_GENERATE = 3` |
| `RejectReason` | None | `TARGET_MISSING = 9` |

## Handshake

Read capabilities from `Hello.capabilities`, field 11.
Require that message and `capabilities.protocol_version == 2` before accepting work.
Check `Hello.session_token`, then send `Welcome { protocol_version: 2 }` and `Configure`.
Wait for `Ready` and work credits before sending jobs.
Use `GetCapabilities` for a later capability query.

A v1 `Hello` decodes with no nested capabilities under the v2 schema.
A v1 coordinator reads the removed version field as zero in a v2 `Hello`.
These peers fail the capability or version check, rather than protobuf decoding.
A Rust v2 solver rejects `Welcome { protocol_version: 1 }` with `ConfigInvalid`, exit code 64.

The Rust session advertises `ISING_SAMPLE`, `ISING_GENERATE`, and `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1`.
Its encodings contain `I32` plus its coefficient type's encoding, without duplicates.
`Fixed<I4, S>` adds no encoding.
C solvers use the Rust session's default lease path.
The Python and TypeScript examples advertise only `ISING_SAMPLE` and `I32`.
Send leases only to a peer that advertises the job kind and generator.

## Topology and target

Send `Topology` with `allowed_h_milli` and `allowed_j_milli`.
Keep `nodes` and `edges` in chain registration order.
The generator draws fields in node order, then couplings in edge order.
The sorted order used for a topology hash is not the draw order.
Topology edge endpoints name native node identifiers, not dense positions.
`TopologyView::from_proto` maps these identifiers to positions without reordering.

Set `SetTarget.max_proof_solutions` to the runtime's `QuantumPowMaxSolutions`.
That value is 32 on 2026-09-23.
Do not infer this limit from `min_solutions` or the requested read count.
A lease needs a target with a nonzero proof limit.
The session returns `TARGET_MISSING` when that target is absent.

## Build a lease

Send `Job.kind = ISING_GENERATE` with `Job.generator` and a unique `job_id`.
Use `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1` and the current cached topology hash.
Set `last_proof_block_hash`, `miner_account`, and `base_salt` to exactly 32 bytes each.
Use a positive `salt_count` and make sure `salt_start + salt_count` fits in `u64`.
The session rejects invalid nonce inputs, unknown generators, empty ranges, and counter overflow with `MALFORMED`.
No cached topology produces `TOPOLOGY_MISSING`.
A hash different from the cached topology produces `TOPOLOGY_MISMATCH`.

Salt index `i` runs from zero through `salt_count - 1`.
Copy `base_salt`, then replace bytes 0 to 7 with `(salt_start + i).to_le_bytes()`.
Choose bytes 8 to 31 per miner to separate their ranges.
An all-zero `base_salt` with `salt_start = ctr` reproduces `salt_from_counter(ctr)` at index zero.
The accepted end counter can equal `u64::MAX`, but the last issued counter is then `u64::MAX - 1`.

The nonce is `BLAKE3(last_proof_block_hash || miner_account || salt)`.
ChaCha8 uses the nonce as its seed.
Each draw selects `allowed[next_u32 % allowed.len()]`.

Size `salt_count` and `deadline_ms` for a few seconds of work.
`deadline_ms` is an absolute Unix timestamp in milliseconds, with zero meaning no deadline.
`Cancel` addresses generations, not individual lease identifiers.
Renew work with another lease after completion.
For a comparison test, give two miners identical nonce inputs, salt ranges, and topology data.
They then draw identical problems, including when the miners use different sampling algorithms.

One accepted lease consumes one credit.
The default loop draws each problem and sends it through the sampler stream.
A session-wide bound limits generated salts in flight to `stream_width`.
Plain jobs can run while a lease is active and refund their own credits.

The lease keeps its admission topology snapshot.
Each completed salt uses the target current at scoring time.
A winner produces one `Result` with that salt, nonce, and proof set.
After the last result, the session sends one `LeaseDone` with a one-credit refund.
`salts_done` counts completed successful salt samples, including empty read sets.
Empty reads produce no result and leave `best_energy_milli` unchanged.
The lowest energy starts at `i64::MAX` and stays there if no reads finish.

## Choose a coefficient encoding

Send a narrow encoding only if the miner advertises it and every coefficient converts exactly.
Otherwise send `I32` at scale 1000.
`h` contains one element per node and `j` contains one per edge.
Both arrays use little-endian elements in graph order.

| Encoding | Bytes per element | Scale | Unit value |
| --- | --- | --- | --- |
| `I32` | 4 | Positive | Stored value divided by scale |
| `I16` | 2 | Positive | Stored value divided by scale |
| `I8` | 1 | Positive | Stored value divided by scale |
| `F16` | 2 | 0 | Stored floating-point value |
| `F32` | 4 | 0 | Stored floating-point value |
| `F64` | 8 | 0 | Stored floating-point value |

Every coefficient needs an exact `i32` milli value.
The decoder returns `MALFORMED` for these inputs:

- An unknown or unspecified encoding.
- An invalid scale.
- An incomplete element.
- A non-finite float.
- A fractional milli value.
- A value outside the `i32` milli range.

Matching the solver's type does not bypass these checks.

This example chooses `I8` at scale 1 only for whole units within its range:

```rust
use quip_proto::v1::{Capabilities, CoefficientEncoding, IsingProblem};

fn encode_problem(h: &[i32], j: &[i32], caps: &Capabilities) -> IsingProblem {
    let fits = |m: &i32| *m % 1000 == 0 && i8::try_from(*m / 1000).is_ok();
    let narrow = caps.encodings.contains(&(CoefficientEncoding::I8 as i32))
        && h.iter().chain(j).all(fits);
    let encode = |values: &[i32]| -> Vec<u8> {
        if narrow {
            values.iter().map(|m| (*m / 1000) as i8 as u8).collect()
        } else {
            values.iter().flat_map(|m| m.to_le_bytes()).collect()
        }
    };
    IsingProblem {
        encoding: if narrow { CoefficientEncoding::I8 } else { CoefficientEncoding::I32 } as i32,
        scale: if narrow { 1 } else { 1000 },
        h: encode(h),
        j: encode(j),
        ..Default::default()
    }
}
```

Fill the graph and sampling fields before sending the returned problem.
The example checks both arrays before narrowing either array.

## Decode and verify results

`Solution.spins` uses one bit per spin, with the lowest bit first.
Node `i` occupies bit `i % 8` of byte `i / 8`.
Bit 1 means +1 and bit 0 means `-1`.
Require exactly `ceil(num_nodes / 8)` bytes and zero padding bits.
Use `quip_protocol::wire::decode_spins_packed` to decode and check this layout.
The Python wheel's `encode_spins` and `decode_spins` instead use one byte per spin.
Use its `encode_spins_packed` and `decode_spins_packed` for v2 solutions.

Replace local target and lease verification copies with `quip-protocol` functions.
The default `session` feature enables the protobuf adapters.
`meets_target` chooses a proof from raw reads.
It keeps energy-valid reads, sorts stably by energy, and caps the set at `max_proof_solutions`.
It then checks energy, solution count, and integer diversity.
Energy must be below the ceiling. Diversity may equal the threshold.

Use `verify_lease_result` on the received proof without selecting another set.
It checks salt membership, the derived nonce, packed spins, host energies, and the submitted proof's target gates.
It does not check `Result.job_id` or select the topology for you.
Match the job identifier and use the lease's topology snapshot before calling it.

```rust
use quip_proto::v1::{IsingProblemGenerator, Result as WireResult, SetTarget, Topology};
use quip_protocol::lease::{verify_lease_result, TopologyView, Verified};
use quip_protocol::target::{meets_target, ProofSet, Target, TargetMiss};

fn choose(reads: &[(&[i8], i64)], wire_target: &SetTarget)
    -> Result<ProofSet, TargetMiss>
{
    meets_target(reads, &Target::from_proto(wire_target))
}

fn verify(
    lease: &IsingProblemGenerator,
    topology: &Topology,
    wire_target: &SetTarget,
    result: &WireResult,
) -> Result<Verified, String> {
    let topology = TopologyView::from_proto(topology).map_err(|e| format!("{e:?}"))?;
    verify_lease_result(lease, &topology, &Target::from_proto(wire_target), result)
        .map_err(|e| e.to_string())
}
```

`ProofSet.indices` identifies the chosen reads in proof order.
`Verified` holds the salt, nonce, and `ProofStats`, not another solution list.
Submit `Result.solutions` unchanged and in order, after decoding spins for the chain proof.
Reordering equal-energy reads can change diversity selection ties.

## Stop rules

| Stop | Default lease behavior |
| --- | --- |
| `Cancel { max_generation }` covers a nonzero generation | Stop drawing. Send `LeaseDone` and one credit refund at once with the salts finished so far. Drop later outcomes. Report `Status.abandoned_generation`. |
| `Shutdown { grace_ms }` | Stop drawing. Accept outcomes until the lease close deadline, `grace_ms - min(grace_ms / 4, 250 ms)`. Then send `LeaseDone` and one credit refund. Exit within `grace_ms`. |
| A nonzero deadline passes | Stop drawing. Send `LeaseDone` and one credit refund at once with the salts finished so far. Drop later outcomes, as for cancellation. |
| Stream closes or the session becomes fatal | Abandon the lease. Do not expect a completion summary. |

Generation zero has no cancellation watermark.
Neither repeated nor out-of-order cancellation changes that rule.
The miner commits `LeaseDone` and its credit refund to the outbound queue without an intervening await.
If the connection stays open, the coordinator receives both messages together.
A local sampler can push until the lease close deadline.
A local sampler can push the same salt index twice, but the second push sends and counts nothing.

For local generation, `LeaseSink::is_stopped()` covers cancellation, deadlines, shutdown, completion, and a closed writer.
On shutdown, stop starting salts immediately.
The sink accepts verified winners from work already running until the lease close deadline.
Cancellation and lease expiry reject later pushes and queued winners after the summary.
A session task closes cancelled local leases even if the sampler does not return.
A local fatal error or panic sends `Fatal` without a completion summary or credit refund for that lease.

Live local workers cannot exceed the advertised credit window.
The session removes finished workers before a new worker starts.
If stopped workers still fill that window, the session sends a device-fault `Fatal` stating that stopped lease workers did not retire.

## Plain and mempool jobs

Keep sending `ISING_SAMPLE` through `Job.ising`.
Replace only its coefficient fields and the returned `Solution` layout.
Credits, rejects, warm starts, provenance, and mempool cancellation rules remain the same.
Plain results leave `salt` and `nonce` empty and do not use `LeaseDone`.
A mempool job uses generation zero and retains its `order_id` attribution.

## Golden vectors

The fixtures come from a throwaway program with `quantum-validation` 0.3.1 at commit `2ca50ee`.
The generator is not a published crate or a runtime dependency.
It calls the validator's diversity and selection functions and reproduces the proof gates.

The initial set holds 69 diversity cases, 449 selection cases, and 125 proof cases.
Two later malformed-proof regressions bring the current proof total to 127.
Nine salts cover three leases:

| Lease case | Nodes | Edges | Salts | Salt layout |
| --- | --- | --- | --- | --- |
| `small` | 8 | 12 | 4 | Counter starts at 5, with `0x33` in bytes 8 to 31 |
| `advantage2_counts` | 4,577 | 41,515 | 3 | All-zero base salt, counter starts at zero |
| `wide_allowed_near_counter_limit` | 16 | 40 | 2 | Wide allowed values, counter starts at `u64::MAX - 2` |


Target cases cover selection ties, half-up diversity rounding, and more valid reads than the proof limit.
Energy equal to the ceiling fails. Diversity equal to the threshold passes.
Other cases set `min_solutions` or `min_diversity_milli` to zero.
The regression cases add ragged solutions and a zero spin.
Lease cases pin each salt, nonce, and BLAKE3 hash of the little-endian field and coupling arrays.

Read [the target fixtures](../crates/quip-solver-conformance/vectors/golden_target.json)
and [the lease fixtures](../crates/quip-solver-conformance/vectors/golden_lease.json) for the exact inputs and expected outputs.
