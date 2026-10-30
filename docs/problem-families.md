# Problem families

A problem family is a named, versioned rule that builds one Ising instance from a nonce, a base topology, and parameter bytes.
This document is the normative contract for problem families.
The golden vectors (SC-2), the generator crate (SC-3), the protobuf extension (SC-4), and the conformance checks (SC-7) build on it.
The legacy draw, `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1`, does not change.
A lease or proof without a family descriptor uses only the legacy contract in [the solver contract](../SPEC.md).

Sections Identity through Family status and the chain are normative.
Each normative rule has a stable ID, `FAM-NN`, for the traceability table.
The Terms, Examples, Scenarios, and Traceability sections are informative.

## Terms

| Term | Meaning |
|---|---|
| family | A construction rule with a `family_id`, such as `quip/twisted-ice-dimer`. |
| revision | A `u32` that fixes every rule of one family version. Two revisions of one family share nothing but the ID. |
| descriptor | The triple of `family_id`, `revision`, and canonical parameter bytes. |
| parameter bytes | The canonical encoding of the generator inputs for one descriptor. |
| instance | The Ising model that one descriptor builds from one nonce and one topology, in canonical instance bytes. |
| status | The registry state of a family revision: `research`, `archived`, or `chain-authorized`. |
| constraint class | `soft` or `hard`. It states whether a witness must also meet feasibility constraints. |

## Examples

This section uses a documentation family, `example/uniform-field`, revision 1.
The `example` namespace is never valid in a lease, so this family cannot collide with a production family.
A lease that names `example/uniform-field` fails with code 3, `FamilyIdInvalid`.
The negative cases below apply the parameter checks directly, as an SC-2 vector does.

The schema has two fields.

| Tag | Name | Type | Scale | Bounds |
|---|---|---|---|---|
| 1 | `n_layers` | `u16` | 1 | 1 to 16 |
| 2 | `field_milli` | `i32` | 1000 | `-4000` to `4000` |

Bounds apply to the stored integer.
`field_milli = -1500` is a field of `-1.5` units.

### Positive cases

The parameter bytes for `n_layers = 2` and `field_milli = -1500` are 19 bytes:

```text
01                    format byte
01 00 02 00 00 00     tag 1, length 2
02 00                 n_layers = 2
02 00 04 00 00 00     tag 2, length 4
24 fa ff ff           field_milli = -1500
```

As one hex string: `01010002000000020002000400000024faffff`.

`params_digest` is `blake3::derive_key("quip family params v1", <those 19 bytes>)`.

The descriptor digest material is 58 bytes:

```text
15                                            len(family_id) = 21
65 78 61 6d 70 6c 65 2f 75 6e 69 66 6f 72 6d  "example/uniform-field"
2d 66 69 65 6c 64
01 00 00 00                                   revision = 1
<32 bytes>                                    params_digest
```

`descriptor_digest` is `blake3::derive_key("quip family descriptor v1", <those 58 bytes>)`.
This document does not list digest values.
The SC-2 vectors carry them.

### Negative cases

Each case changes one input of the preceding valid descriptor.

| Case | Input | Code |
|---|---|---|
| Tags out of order | `01` `0200 04000000 24faffff` `0100 02000000 0200` | 7 `ParamsMalformed` |
| Duplicate tag | `01` `0100 02000000 0200` `0100 02000000 0200` `0200 04000000 24faffff` | 7 `ParamsMalformed` |
| Unknown tag | the valid bytes, then `0300 01000000 00` | 7 `ParamsMalformed` |
| Missing tag | `01` `0100 02000000 0200` | 7 `ParamsMalformed` |
| Wrong length for a fixed-width type | `01` `0100 04000000 02000000` `0200 04000000 24faffff` | 7 `ParamsMalformed` |
| Value out of bounds | `01` `0100 02000000 0200` `0200 04000000 5ff0ffff`, that is `field_milli = -4001` | 9 `ParamsOutOfBounds` |
| Format byte `0x02` | `02` `0100 02000000 0200` `0200 04000000 24faffff` | 7 `ParamsMalformed` |
| Uppercase `family_id` | `family_id = "Quip/X"` | 3 `FamilyIdInvalid` |
| Revision zero | `revision = 0` | 4 `RevisionZero` |
| Length 4097 | the valid 19 bytes, then 4078 bytes `00` | 7 `ParamsMalformed` |

### Size bounds

This section does not define a construction for `example/uniform-field`.
The size bounds below are an illustrative contract fragment, not a complete contract.
They assume a construction that makes `n_layers` copies of the topology `T`.
Each node then gets one edge to its own copy in the next layer.
With `L = n_layers`, the FAM-38 size bounds are:

```text
N(p, T) = L * n_nodes(T)
E(p, T) = L * n_edges(T) + (L - 1) * n_nodes(T)
D(p, T) = max_degree(T) + min(L - 1, 2)
```

A node in a middle layer has its topology edges plus one edge to each neighbor layer, so `D` adds at most 2.
For `n_layers = 2` on a topology with 100 nodes, 150 edges, and at most 4 edges on one node, the bounds are `N = 200`, `E = 400`, and `D = 5`.
A solver that advertises `max_degree = 4` fails these bounds with code 13, `BoundsExceeded`, at admission (FAM-47), when an SC-2 vector runs the size-bound check directly.

## Identity

**FAM-01.** A `family_id` must be an ASCII string `<namespace>/<name>`.
The namespace must match `[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)*`.
The name must match `[a-z][a-z0-9-]*`.

**FAM-02.** A `family_id` must be 3 to 64 bytes long.
A `family_id` outside that length is code 3, `FamilyIdInvalid`.

**FAM-03.** Parties must compare `family_id` values byte for byte.
They must not fold case or normalize.

**FAM-04.** This repository reserves the `quip` namespace for the families that it registers.

**FAM-05.** The `example` namespace is only for documentation and tests.
A lease that names an `example` family must fail with code 3, `FamilyIdInvalid`.

**FAM-06.** A `revision` is a `u32` and must be at least 1.
Revision 0 fails with code 4, `RevisionZero`.

**FAM-07.** Revisions match exactly.
A party must not accept a revision range or a "latest" revision.
No revision is compatible with another revision.

**FAM-08.** A revision covers construction, random draws, numeric rules, ordering, metadata, and output semantics.
A change to any of these must get a new revision.

**FAM-09.** Status is a registry property of a family revision, not part of its identity.
The status values are `research`, `archived`, and `chain-authorized`.
A promotion changes the status and must not change the `family_id` or the revision.

## Parameter bytes

**FAM-10.** Parameter bytes must start with one format byte, `0x01`.
A sequence of fields follows.
Each field is `tag: u16 LE`, `length: u32 LE`, then `value: length bytes`.

**FAM-11.** Each tag must be greater than the preceding tag, so a duplicate tag fails with code 7.
Each tag must be a tag that the schema lists.
No field has a default: every field that the revision's schema lists must be present.

**FAM-12.** Parameter bytes must be at most 4096 bytes long.
Longer parameter bytes fail with code 7, `ParamsMalformed`.

**FAM-13.** A schema may use only these value types.

| Type | Length | Encoding |
|---|---|---|
| `u8`, `u16`, `u32`, `u64` | 1, 2, 4, 8 | Unsigned integer, little-endian. |
| `i32`, `i64` | 4, 8 | Two's complement integer, little-endian. |
| `bool` | 1 | `0x00` or `0x01`. |
| `bytes` | any | Raw bytes. |
| `ascii` | any | Bytes `0x20` to `0x7E`. |

For a fixed-width type, `length` must equal the width.
A `bool` byte other than `0x00` or `0x01` fails with code 7.
An `ascii` byte outside `0x20` to `0x7E` fails with code 7.

**FAM-14.** A schema must not use floating-point values.
A non-integer quantity uses an integer with a stated scale, written in the schema as `i32, scale 1000` (milli units).

**FAM-15.** Each family revision schema must list the tag, name, type, scale, and inclusive bounds of every field.
Bounds apply to the stored integer.
A `bool`, `bytes`, or `ascii` field has no scale.
The bounds of a `bool` field are 0 to 1.
For `bytes` and `ascii`, the bounds apply to the value length.

**FAM-16.** A layout fault (format byte, order, length, type, unknown tag, or missing tag) fails with code 7, `ParamsMalformed`.
A well-formed value outside its schema bounds fails with code 9, `ParamsOutOfBounds`.
A party checks the whole layout before any bounds, so code 7 wins over code 9.

**FAM-17.** Parameter bytes must hold generator inputs only.
They must not hold solver seeds, schedules, postprocess policy, read counts, or acceptance targets.

## Digests

**FAM-18.** Every digest is `blake3::derive_key(context, material)`, which is BLAKE3 in key-derivation mode.
This table gives the context string and material of each digest.

| Digest | Context string | Material |
|---|---|---|
| `params_digest` | `quip family params v1` | the canonical parameter bytes |
| `descriptor_digest` | `quip family descriptor v1` | `u8 len(family_id)` `family_id` `u32 LE revision` `params_digest` |
| `topology_order_digest` | `quip family topology order v1` | `u32 LE n_nodes`, each node id `u32 LE` in registration order, `u32 LE n_edges`, each edge as `u32 LE u`, `u32 LE v` native ids in registration order |
| stream seed | `quip family stream v1` | `nonce` (32) `descriptor_digest` (32) `topology_order_digest` (32) `u8 len(label)` `label` (ASCII) |
| `instance_digest` | `quip family instance v1` | the canonical instance bytes |

**FAM-19.** `Topology.hash` stays an opaque cache key and is not an order commitment.
`topology_order_digest` is the order commitment.
Two topologies with the same node and edge sets in a different order have different digests.
The material uses each edge as registered, so the edge `(v, u)` and the edge `(u, v)` give different digests.

## Random streams

The legacy stream does not change.
Salt index `i` overwrites bytes 0..7 of `base_salt` with `(salt_start + i).to_le_bytes()`.
The nonce is `BLAKE3(last_proof_block_hash || miner_account || salt)`.
`GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1 = 1` seeds ChaCha8 with the nonce.
It draws fields, then couplings, in chain registration order.
Each value is `allowed[next_u32 % len]`.
Its golden vectors do not change.

**FAM-20.** A family lease must derive the nonce with the preceding legacy nonce and salt rules.
No family revision changes the legacy stream or reinterprets a legacy draw.

**FAM-21.** Each substream is its own ChaCha8 instance.
Its 32-byte key is the stream seed from the digest table.
It starts at keystream word 0.
The cipher is the existing `ChaCha8Rng` (8 rounds, `crates/quip-protocol/src/chacha8.rs`).

**FAM-22.** A substream label must be ASCII and 1 to 32 bytes long.
Sample labels are `fields`, `perm`, and `match`.
A family revision must list every label that it uses in its contract.

**FAM-23.** `uniform(n)` takes `n` with `1 <= n <= 2^32 - 1` and computes in `u64`.

1. Set `limit = 2^32 - (2^32 mod n)`.
2. Draw `x = next_u32()`.
3. If `x >= limit`, go to step 2.
4. Return `x mod n`.

`uniform(n)` gives an unbiased result.
It reads at least one word, also for `n = 1`.
A contract must not call `uniform` with `n` outside that range.
The legacy stream keeps its biased `% len`.

**FAM-24.** `shuffle(a)` is this Fisher-Yates loop:

```text
for i in (1..len).rev() { j = uniform(i + 1); swap(a[i], a[j]) }
```

A list with fewer than two elements reads no words.

**FAM-25.** A revision that retries a construction step must state a fixed limit on the number of tries.
The redraw loop inside `uniform` is not a construction retry.

**FAM-26.** Retry exhaustion is deterministic for the same inputs and gives the outcome code 14, `NoInstance`.
A miner skips a `NoInstance` salt.
The salt counts in `salts_done`, and the miner sends no `Result` for it.
Every verifier must reject a `Result` for a `NoInstance` salt.

**FAM-27.** Generator arithmetic uses only integers and checks every operation for overflow.
An overflow never wraps.
An overflow fails with code 13, `BoundsExceeded`.
During generation, code 13 applies to one salt, with the same handling as code 14 in FAM-26 and FAM-52.
During generation, code 13 has only two causes.
One is an overflow.
The other is an instance beyond a FAM-37 bound or a bound in the Bounds item of the contract (FAM-38).
An instance beyond the size bounds `N`, `E`, or `D` is a generator defect, not a code 13 cause (FAM-38).
Every party applies the same causes, so every verifier can decide them.
The limits of one solver never cause code 13 during generation.
Every verifier must reject a `Result` for that salt.

**FAM-28.** These inputs must not affect output:

- floating point
- a clock
- a host RNG
- hash-map iteration order
- thread count
- a solver seed

## Canonical instances

**FAM-29.** Coefficients use the milli unit (scale 1000), the same unit as `energy_milli`.

**FAM-30.** Canonical instance bytes have this layout, all little-endian.

| Field | Type | Content |
|---|---|---|
| format | `u8` | `0x01` |
| `descriptor_digest` | 32 bytes | From the digest table. |
| `topology_order_digest` | 32 bytes | From the digest table. |
| `nonce` | 32 bytes | The salt nonce. An archived instance, with an empty nonce, has 32 zero bytes here. |
| `n_nodes` | `u32` | The number of nodes. |
| node ids | `u32` each | Native node ids in output order. |
| `n_edges` | `u32` | The number of edges. |
| edges | `u32 u`, `u32 v` each | Dense positions into the node list. |
| `h` | `i32` each | One field per node, in node output order. |
| `J` | `i32` each | One coupling per edge, in edge order. |
| `offset` | `i64` | The energy offset in milli. |
| `n_sections` | `u16` | The number of metadata sections. |
| sections | `u16 kind`, `u32 length`, payload each | Metadata sections in ascending kind order. |

**FAM-31.** The family contract defines the node output order.
The default is base topology registration order, filtered to the nodes that the instance uses.

**FAM-32.** Each edge must have `u < v`.
Edges must appear in ascending `(u, v)` order.
An instance must not contain a duplicate edge or a self-loop.

On an explicit job, an edge list that breaks FAM-32, or an edge position at or past `n_nodes` (FAM-30), gives code 15, `InstanceDigestMismatch` (FAM-65).
In a generator, such an instance is a generator defect.

**FAM-33.** The energy of spins `s` is `offset + sum(h_i s_i) + sum(J_uv s_u s_v)`, an exact `i64` in milli.

**FAM-34.** A metadata section is `u16 kind`, `u32 length`, then the payload.
Each kind must be greater than the preceding kind, so each kind occurs at most once.
This table lists the registered kinds.
A family contract defines the payload layout of each kind that it uses.
These metadata faults give code 16, `MetadataInvalid`:

- a kind that the table does not register
- a kind that the family contract does not list
- a kind that is not greater than the preceding kind
- a section length that runs past the end of the metadata region
- a metadata region shorter than the 2-byte `n_sections`
- bytes after the last declared section

| Kind | Name |
|---|---|
| 1 | `tiles` |
| 2 | `gauges` |
| 3 | `dimers` |
| 4 | `charges` |
| 5 | `incidence` |

**FAM-35.** Metadata is part of the instance bytes, so `instance_digest` covers it.

**FAM-36.** Before a solver uses metadata, it must validate the metadata against the instance.
A solver that regenerates the instance meets this rule.
A solver must never trust metadata as a hint.
Metadata that fails validation gives code 16, `MetadataInvalid`.
The FAM-34 layout faults are validation failures.

**FAM-37.** An instance must stay within these bounds.

| Quantity | At most |
|---|---|
| `n_nodes` | 2^24 |
| `n_edges` | 2^26 |
| each `\|h\|` and `\|J\|` | 2^30 milli |
| each `\|offset\|` | 2^56 milli |
| metadata total | 2^26 bytes |

The metadata total counts the whole metadata region.
That region is the `u16 n_sections` field plus all section headers and payloads.
A family revision may declare tighter bounds.
At admission, a lease fails with code 13, `BoundsExceeded`, if `N(p, T)` or `E(p, T)` (FAM-38) exceeds the `n_nodes` or `n_edges` bound of this table or of the Bounds item of the contract.
At admission, code 13 also applies if a size bound exceeds an advertised limit (FAM-47).
During generation, an instance that would exceed a bound of this table or of the Bounds item of the contract fails with code 13.
The miner then skips that salt as FAM-27 states.

## Family contracts

**FAM-38.** Every family revision must publish a contract with these items.

| Item | Content |
|---|---|
| ID | The `family_id`. |
| Revision | The `u32` revision. |
| Status | `research`, `archived`, or `chain-authorized`. |
| Parameter schema | A table with tag, name, type, scale, and inclusive bounds for each field. |
| Substream labels | Every label the revision reads, and what each one draws. |
| Construction steps | Each step, its draws in order, and its integer arithmetic. |
| Output order | The node output order, if it differs from the default. |
| Offset | How the construction computes `offset`. |
| Metadata | Each kind the revision emits, with its payload layout. |
| Constraint class | `soft` or `hard`. A `hard` contract also lists its feasibility constraints. |
| Bounds | Any bounds tighter than the instance bounds. The declared bounds must make arithmetic overflow impossible. |
| Size bounds | The formulas `N(p, T)`, `E(p, T)`, and `D(p, T)` that this rule defines. |
| Max retries | The fixed try limit of each retried step, or none. |

The size bounds are three integer upper-bound functions.
Each function takes the parameter values `p` and only these statistics of the base topology `T`:

- `n_nodes(T)`, the number of nodes.
- `n_edges(T)`, the number of edges.
- `max_degree(T)`, the most edges incident on one node, counted over the edges of `T` in registration order. A topology has no self-loops.

| Function | Upper bound on |
|---|---|
| `N(p, T)` | The number of instance nodes. |
| `E(p, T)` | The number of instance edges. |
| `D(p, T)` | The most edges incident on one instance node (max connectivity). |

The functions use integer arithmetic only.
The evaluating party computes each formula in checked `u64` arithmetic over `u64` values of the topology statistics.
The evaluating party checks each operation, and an overflow gives code 13, `BoundsExceeded`, at admission.
An evaluation draws no random word and runs in constant time or in time linear in the size of `T`.
Each bound must hold for every nonce.
If `N(p, T)` is at least 1, `D(p, T)` must be at most `N(p, T) - 1`.
Admission compares `N(p, T)` and `E(p, T)` with the FAM-37 bounds and the Bounds item of the contract.
SC-3 exposes the three functions as one function, `size_bounds(params, topology) -> { nodes, edges, degree }`.
That name is guidance for SC-3, and the size bounds never go on the wire.

An instance with more nodes than `N(p, T)`, more edges than `E(p, T)`, or a node with more incident edges than `D(p, T)` is a generator defect.
The revision fails SC-7 conformance, and every verifier must reject a `Result` for that salt.

**FAM-39.** Each contract must declare one constraint class, `soft` or `hard`.
A `soft` family keeps every source penalty as an energy term.
A `hard` contract lists feasibility constraints.
A witness that violates any of them is invalid at any energy.

**FAM-40.** In a `soft` family, a defect counter is a diagnostic only.
Adding a defect counter never turns a `soft` family into a `hard` one.

**FAM-41.** The adapted P-static, D-static, and F-static families are `hard`.
The original synthetic ice, twisted-ice, charged-ice, field-ice, and routing families are `soft`.
Each adapted family has its own `family_id`, separate from the `family_id` of every original family.

## Stable codes

**FAM-42.** Generator and admission codes have these numeric values forever.
SC-2 vectors and SC-7 checks use them.
The right column gives the wire mapping.

| Code | Name | Meaning | Wire `RejectReason` |
|---|---|---|---|
| 1 | `DescriptorMissing` | family generator algorithm without `family` | `MALFORMED` |
| 2 | `DescriptorUnexpected` | legacy algorithm with `family` set | `MALFORMED` |
| 3 | `FamilyIdInvalid` | `family_id` breaks the grammar or uses `example` | `MALFORMED` |
| 4 | `RevisionZero` | `revision == 0` | `MALFORMED` |
| 5 | `FamilyUnsupported` | `family_id` not supported by this party | `FAMILY_UNSUPPORTED` |
| 6 | `RevisionUnsupported` | known family, revision not supported | `FAMILY_UNSUPPORTED` |
| 7 | `ParamsMalformed` | a layout fault: format byte, tag order, length, type, an unknown tag, or a missing tag | `MALFORMED` |
| 8 | `ParamsDigestMismatch` | sent `params_digest` differs from recomputed | `MALFORMED` |
| 9 | `ParamsOutOfBounds` | a value outside schema bounds | `MALFORMED` |
| 10 | `ParamsRestricted` | params valid but outside this solver's advertised restriction | `FAMILY_UNSUPPORTED` |
| 11 | `TopologyOrderMismatch` | sent `topology_order_digest` differs from the cached topology | `TOPOLOGY_MISMATCH` |
| 12 | `TopologyIncompatible` | family cannot build on this topology | `FAMILY_UNSUPPORTED` |
| 13 | `BoundsExceeded` | at admission, a size bound exceeds a FAM-37 bound, a Bounds item bound, or an advertised limit, or its evaluation overflows. During generation, an overflow, or an instance beyond a FAM-37 bound or a Bounds item bound. On an explicit job, an instance beyond a FAM-37 bound (FAM-65). | `TOO_LARGE` at admission and on an explicit job. During generation, no reject, and the miner skips the salt. |
| 14 | `NoInstance` | deterministic retry exhaustion for one salt | no reject, and the miner skips the salt |
| 15 | `InstanceDigestMismatch` | explicit job: sent `instance_digest` differs, or the edge list breaks FAM-32 or an edge position is at or past `n_nodes` | `MALFORMED` |
| 16 | `MetadataInvalid` | metadata fails validation against the instance | `MALFORMED` |

**FAM-43.** This table gives the phase of each code.
Within each phase, the lowest failing code wins.

| Phase | Codes | When |
|---|---|---|
| Admission | 1 to 12, and 13 from the size bounds (FAM-38, FAM-47) | Before any draw. |
| Generation | 13 from an overflow or from a FAM-37 bound or a Bounds item bound, and 14 | During generation. Each skips one salt, and the lease continues. |
| Explicit-job validation | 7 to 10, 13 for an instance beyond a FAM-37 bound, 15, and 16 | With no draw, in the FAM-65 order. |

## Wire extension

The family extension adds enum values, messages, and tags to `proto/quip/v1/miner.proto`.
SC-4 makes this change.
This block is the normative text for SC-4.

```proto
enum GeneratorAlgorithm {
  GENERATOR_ALGORITHM_UNSPECIFIED = 0;
  GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1 = 1;   // unchanged
  GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1 = 2;   // new: family lease
}
enum RejectReason { ... existing 0..9 unchanged ...
  FAMILY_UNSUPPORTED = 10;                     // new
}
message FamilyDescriptor {                      // new
  string family_id = 1;
  uint32 revision = 2;
  bytes  params = 3;          // canonical parameter bytes
  bytes  params_digest = 4;   // 32 bytes
}
message IsingProblemGenerator { ... tags 1..7 unchanged ...
  FamilyDescriptor family = 8;           // required iff algorithm == FAMILY_CHACHA8_V1
  bytes topology_order_digest = 9;       // 32 bytes, required iff family is set
}
message FamilyBinding {                         // new: explicit family job
  FamilyDescriptor family = 1;
  bytes nonce = 2;                  // 32 bytes; empty for an archived instance
  bytes instance_digest = 3;        // 32 bytes
  bytes metadata = 4;               // the metadata region, from u16 n_sections (FAM-67)
  bytes topology_order_digest = 5;  // 32 bytes
  repeated uint32 node_ids = 6;     // native node ids in instance output order
  int64 offset_milli = 7;           // the instance offset
}
message IsingProblem { ... existing tags unchanged ...
  FamilyBinding family = 17;
}
message FamilySupport {                         // new
  string family_id = 1;
  repeated uint32 revisions = 2;        // exact revisions
  repeated bytes params_digests = 3;    // empty = every valid parameter set
}
message Capabilities { ... tags 3..16 unchanged ...
  string solver_id = 17;
  repeated FamilySupport generated_families = 18;
  repeated FamilySupport accepted_families = 19;
}
```

This table lists every value and tag of the changed enums and messages.
`unchanged` rows match `proto/quip/v1/miner.proto` today.

| Message | Field | Tag | Status |
|---|---|---|---|
| `GeneratorAlgorithm` | `GENERATOR_ALGORITHM_UNSPECIFIED` | 0 | unchanged |
| `GeneratorAlgorithm` | `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1` | 1 | unchanged |
| `GeneratorAlgorithm` | `GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1` | 2 | new |
| `RejectReason` | `REJECT_REASON_UNSPECIFIED` | 0 | unchanged |
| `RejectReason` | `UNSUPPORTED_KIND` | 1 | unchanged |
| `RejectReason` | `TOO_LARGE` | 2 | unchanged |
| `RejectReason` | `EXPIRED` | 3 | unchanged |
| `RejectReason` | `OVERLOADED` | 4 | unchanged |
| `RejectReason` | `SHUTTING_DOWN` | 5 | unchanged |
| `RejectReason` | `MALFORMED` | 6 | unchanged |
| `RejectReason` | `TOPOLOGY_MISMATCH` | 7 | unchanged |
| `RejectReason` | `TOPOLOGY_MISSING` | 8 | unchanged |
| `RejectReason` | `TARGET_MISSING` | 9 | unchanged |
| `RejectReason` | `FAMILY_UNSUPPORTED` | 10 | new |
| `FamilyDescriptor` | `family_id` | 1 | new |
| `FamilyDescriptor` | `revision` | 2 | new |
| `FamilyDescriptor` | `params` | 3 | new |
| `FamilyDescriptor` | `params_digest` | 4 | new |
| `IsingProblemGenerator` | `algorithm` | 1 | unchanged |
| `IsingProblemGenerator` | `topology_hash` | 2 | unchanged |
| `IsingProblemGenerator` | `last_proof_block_hash` | 3 | unchanged |
| `IsingProblemGenerator` | `miner_account` | 4 | unchanged |
| `IsingProblemGenerator` | `base_salt` | 5 | unchanged |
| `IsingProblemGenerator` | `salt_start` | 6 | unchanged |
| `IsingProblemGenerator` | `salt_count` | 7 | unchanged |
| `IsingProblemGenerator` | `family` | 8 | new |
| `IsingProblemGenerator` | `topology_order_digest` | 9 | new |
| `FamilyBinding` | `family` | 1 | new |
| `FamilyBinding` | `nonce` | 2 | new |
| `FamilyBinding` | `instance_digest` | 3 | new |
| `FamilyBinding` | `metadata` | 4 | new |
| `FamilyBinding` | `topology_order_digest` | 5 | new |
| `FamilyBinding` | `node_ids` | 6 | new |
| `FamilyBinding` | `offset_milli` | 7 | new |
| `IsingProblem` | `topology_hash` | 1 | unchanged |
| `IsingProblem` | `edges` | 2 | unchanged |
| `IsingProblem` | `h_milli_le32` | 3 | reserved |
| `IsingProblem` | `j_milli_le32` | 4 | reserved |
| `IsingProblem` | `num_reads` | 5 | unchanged |
| `IsingProblem` | `gates` | 6 | reserved |
| `IsingProblem` | `num_sweeps` | 7 | unchanged |
| `IsingProblem` | `anneal_time_us` | 8 | unchanged |
| `IsingProblem` | `initial_spins` | 9 | unchanged |
| `IsingProblem` | `start_beta_milli` | 10 | unchanged |
| `IsingProblem` | `reversal_s_milli` | 11 | unchanged |
| `IsingProblem` | `reversal_pause_us` | 12 | unchanged |
| `IsingProblem` | `encoding` | 13 | unchanged |
| `IsingProblem` | `scale` | 14 | unchanged |
| `IsingProblem` | `h` | 15 | unchanged |
| `IsingProblem` | `j` | 16 | unchanged |
| `IsingProblem` | `family` | 17 | new |
| `FamilySupport` | `family_id` | 1 | new |
| `FamilySupport` | `revisions` | 2 | new |
| `FamilySupport` | `params_digests` | 3 | new |
| `Capabilities` | none (former `backend` string) | 1 | reserved |
| `Capabilities` | none (former `algorithm` string) | 2 | reserved |
| `Capabilities` | `supported_kinds` | 3 | unchanged |
| `Capabilities` | `max_nodes` | 4 | unchanged |
| `Capabilities` | `max_edges` | 5 | unchanged |
| `Capabilities` | `features` | 6 | unchanged |
| `Capabilities` | `protocol_version` | 7 | unchanged |
| `Capabilities` | `stream_width` | 8 | unchanged |
| `Capabilities` | `native_topology_hash` | 9 | unchanged |
| `Capabilities` | `encodings` | 10 | unchanged |
| `Capabilities` | `generators` | 11 | unchanged |
| `Capabilities` | `backend` | 12 | unchanged |
| `Capabilities` | `algorithm` | 13 | unchanged |
| `Capabilities` | `max_degree` | 14 | unchanged |
| `Capabilities` | `max_local_field_milli` | 15 | unchanged |
| `Capabilities` | `exact_resolution_milli` | 16 | unchanged |
| `Capabilities` | `solver_id` | 17 | new |
| `Capabilities` | `generated_families` | 18 | new |
| `Capabilities` | `accepted_families` | 19 | new |

**FAM-44.** The extension adds enum values and tags only.
Existing tags and enum values must keep their numbers and meanings.
Reserved numbers and names must stay reserved.
The package stays `quip.v1`.

**FAM-45.** The session `protocol_version` stays 2 in `Welcome` and `Capabilities`.
A family revision, the session protocol version, and the chain runtime version are separate numbers.
A new family revision must not change the session protocol version.
A change to the session protocol version or the chain runtime version must not change any family revision.

**FAM-46.** Each digest field in this extension holds 32 bytes.
`FamilyBinding.nonce` holds 32 bytes, or no bytes for an archived instance.
A wrong length fails with the code of the check that reads the field:

| Field | Wrong length gives |
|---|---|
| `FamilyDescriptor.params_digest` | code 8, `ParamsDigestMismatch` |
| `IsingProblemGenerator.topology_order_digest` | code 1, `DescriptorMissing` |
| `FamilyBinding.nonce` with a length other than 0 or 32 | code 15, `InstanceDigestMismatch` |
| `FamilyBinding.instance_digest` | code 15, `InstanceDigestMismatch` |
| `FamilyBinding.topology_order_digest` | code 15, `InstanceDigestMismatch` |
| `FamilyBinding.node_ids`, with a count other than the node count of the plain model | code 15, `InstanceDigestMismatch` |

## Lease admission

A family lease is a `Job` of kind `ISING_GENERATE` whose `generator.algorithm` is `GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1`.
FAM-43 sets the phase and order of the family codes.
This section places those codes among the session checks that exist today.

**FAM-47.** A solver must run lease admission in this order.
The first failure wins, and no step draws a random word.

1. The session checks that run before `LeaseSpec::from_proto` today keep their place: `EXPIRED`, `TOPOLOGY_MISSING`, then `TARGET_MISSING`.
2. The lease field checks of today, unchanged, each with `MALFORMED`. These checks come before code 1.
   - `Job.generator` is absent (`crates/quip-solver-core/src/lease.rs:295`).
   - `last_proof_block_hash`, `miner_account`, or `base_salt` is not 32 bytes (`crates/quip-protocol/src/lease.rs:302`).
   - `salt_count` is zero (`crates/quip-protocol/src/lease.rs:81`).
   - `salt_start + salt_count` overflows `u64` (`crates/quip-protocol/src/lease.rs:84`).
   - The session maps each of these errors to `MALFORMED` (`crates/quip-solver-core/src/lease.rs:296`). [Generated jobs](../SPEC.md#generated-jobs) in the solver contract states the same rules.
3. Codes 1 to 10, in ascending order.
4. The `topology_hash` check that exists today. A hash that differs from the cached `Topology.hash` gives `TOPOLOGY_MISMATCH`.
5. Codes 11, 12, and the admission part of code 13, in ascending order.

The advertised limits of a family lease are exactly `max_nodes`, `max_edges`, and `max_degree`, `Capabilities` fields 4, 5, and 14.
They have the meanings in [Model limits](../SPEC.md#model-limits).
An unset field makes no claim, and a `max_degree` of `0` sets no bound.
A `max_nodes` of `0` and a `max_edges` of `0` make no claim, the same as `max_degree = 0`, because a proto3 unset field reads 0.
`max_local_field_milli` and `exact_resolution_milli` do not apply to leases.

The admission part of code 13 applies if any of these conditions is true:

- The evaluation of a size bound (FAM-38) overflows.
- `N(p, T)` or `E(p, T)` exceeds a FAM-37 bound or a Bounds item bound.
- `N(p, T)` exceeds `max_nodes`, `E(p, T)` exceeds `max_edges`, or `D(p, T)` exceeds `max_degree`, for each field that makes a claim.

The solver decides these conditions from the parameters, the cached topology, and its own `Capabilities`, before any draw.
The same `Capabilities` snapshot always gives the same result.

Generation then starts.
Generation can give code 13 or code 14 only.
Each of them skips one salt and does not end the lease (FAM-27, FAM-52).

**FAM-48.** A lease with `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1` must not set `family` or a non-empty `topology_order_digest`.
If it sets either field, the lease fails with code 2, `DescriptorUnexpected`.
A coordinator must never build a lease with the legacy algorithm and tag 8 or tag 9 set.
An old consumer ignores those tags and runs the legacy draw.

**FAM-49.** A lease with `GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1` must set `family` and a 32-byte `topology_order_digest`.
A lease without `family` fails with code 1, `DescriptorMissing`.
A `topology_order_digest` of another length also gives code 1.

**FAM-50.** The solver computes `topology_order_digest` from the nodes and edges of the cached `Topology` in registration order.
If the sent digest differs, the lease fails with code 11, `TopologyOrderMismatch`, and the wire reason `TOPOLOGY_MISMATCH`.
An equal `Topology.hash` does not replace this check.
The coordinator answers `TOPOLOGY_MISMATCH` the way it does today: it sends the `Topology` again.

**FAM-51.** A family lease derives each salt and nonce with the legacy rules in FAM-20.
`Result.salt` and `Result.nonce` carry them as for a legacy lease.

**FAM-52.** A salt skipped for code 14 (FAM-26) or for code 13 during generation (FAM-27) does not end the lease.
The miner continues with the next salt.
`LeaseDone.salts_done` counts the skipped salt.
`LeaseDone.best_energy_milli` ignores it.
For a family lease, `LeaseDone.best_energy_milli` is the lowest FAM-33 energy, `offset` included.

**FAM-53.** The `Result` message does not change.
It carries no descriptor and no instance bytes.
The coordinator checks the reported salt and nonce with the legacy rules.
It must then regenerate the instance from that nonce and the descriptor and topology that it sent in the lease.
The regenerated instance is authoritative.
The coordinator computes the energy of each `Solution` on that instance, `offset` included, and checks every feasibility constraint of a `hard` family.
The coordinator must not use family data that a miner sends in place of its own copy.
For a family lease, `Solution.energy_milli` is the FAM-33 energy, `offset` included.
`SetTarget.max_energy_milli` applies to that energy, with the existing strictness.
For a family lease, `Solution.spins` is bit-packed in instance node output order (FAM-31).
Node `i` is bit `i % 8` of byte `i / 8`, and bit 1 is spin +1.
An entry is exactly `ceil(n_nodes / 8)` bytes, where `n_nodes` is the node count of the instance, not of the topology.
Padding bits are 0.
A witness with a set padding bit is invalid, the same as a witness of the wrong length.
The diversity check for `min_diversity_milli` uses these instance spins.

## Old consumers

A v2 consumer that predates this document cannot generate a family instance.
It fails safe for three reasons.

First, it rejects any generator value other than 1 with `MALFORMED` before any draw.
`LeaseSpec::from_proto` returns `LeaseError::UnknownGenerator` for any algorithm other than 1 (`crates/quip-protocol/src/lease.rs:296`).
The session maps that error to `MALFORMED` (`crates/quip-solver-core/src/lease.rs:296`).
The test at `crates/quip-protocol/tests/proto_lease.rs:113` checks the rejection.
The session test at `crates/quip-solver-core/tests/lease_session.rs:449`, `malformed_leases_are_rejected_before_drawing`, sends a lease with `algorithm = 0`, and three leases with bad salt fields.
It checks that the first reply to each lease is a `MALFORMED` reject.

Second, a coordinator never sends a family lease to a session that did not ask for it in `generated_families` (FAM-72).
An old consumer sends no `generated_families`, so it gets no family lease.

Third, an old solver that gets an `IsingProblem` with `family` set ignores tag 17 as an unknown field.
It solves the plain model.
That is the generic tier (FAM-56).
Acceptance stays with the coordinator (FAM-55), so the plain solve is safe.

**FAM-54.** A solver must reject a lease whose `GeneratorAlgorithm` value it does not know with `MALFORMED`, before any draw.
A solver that follows this document knows value 2.
Such a solver that lists no family in `generated_families` rejects a family lease with code 5, `FamilyUnsupported`, after codes 1 to 4 pass.

**FAM-55.** When a solver solves an `IsingProblem` with `family` set as a plain model, the coordinator must decide acceptance.
The coordinator adds `offset` to `Solution.energy_milli` and checks every feasibility constraint of a `hard` family.

## Capabilities

**FAM-56.** A solver declares family support at one of three tiers.

| Tier | `Capabilities` content | The solver |
|---|---|---|
| Generic explicit Ising | `ISING_SAMPLE` in `supported_kinds`, plus the existing limits. No family. | Solves an `IsingProblem` with `family` set as a plain model. The coordinator decides acceptance. |
| Accepted family | `(family_id, revision)` in `accepted_families`. | Validates the `FamilyBinding` (FAM-65 to FAM-68) and may use its structure. |
| Generated family | `(family_id, revision)` in `generated_families`. | Expands family leases itself or through the default session path. |

A tier applies per `(family_id, revision)`.
A solver that does not list a revision in `accepted_families` is a generic solver for that revision.

**FAM-57.** A solver must list a revision in `generated_families` only after that revision passes SC-7 conformance on that solver.

**FAM-58.** A solver must list `GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1` in `generators` if and only if `generated_families` is not empty.

**FAM-59.** A revision with status `archived` may appear in `accepted_families`.
It must not appear in `generated_families`.
Its instances arrive only as explicit jobs from hash-checked archives, with an empty `nonce`.

**FAM-60.** `solver_id` is ASCII.
It must match `[a-z][a-z0-9-]*(/[a-z0-9][a-z0-9.-]*)*` and be 3 to 64 bytes long.
It must stay the same across releases of the same solver, as in `quip-miner-cpu/otsuka`.
An empty `solver_id` means that the session is not a pin target.
`solver_id` is separate from `backend` and `algorithm`.

**FAM-61.** Each `FamilySupport` entry must meet these rules.

- `family_id` follows FAM-01 and FAM-02 and does not use the `example` namespace.
- `revisions` lists at least one revision, each at least 1. Each revision is an exact match (FAM-07).
- Each entry in `params_digests` is 32 bytes.
- An empty `params_digests` claims every valid parameter set of each listed revision.
- A non-empty `params_digests` claims only the parameter sets with those digests.
- One list has at most one entry for each `family_id`.

The coordinator must treat an entry that breaks a rule as a claim of nothing.
If one list has two or more entries with the same `family_id`, that list claims nothing for that `family_id`.
The coordinator treats the family as unsupported by that solver for that list.

**FAM-62.** For a lease, the solver decides codes 5, 6, and 10 from its `generated_families`.

| Lease condition | Code |
|---|---|
| No `generated_families` entry has the `family_id`. | 5, `FamilyUnsupported` |
| An entry has the `family_id`, but no entry lists the revision. | 6, `RevisionUnsupported` |
| The entry for the revision has a non-empty `params_digests` without the lease `params_digest`. | 10, `ParamsRestricted` |

For an explicit job, code 10 uses the `accepted_families` entry instead (FAM-65).
The `generated_families` list of a solver must follow FAM-61.
A solver must not advertise an invalid list.

**FAM-63.** `--capabilities` prints the new fields with these JSON names.

| Field | JSON name |
|---|---|
| `solver_id` | `solverId` |
| `generated_families` | `generatedFamilies` |
| `accepted_families` | `acceptedFamilies` |

Each `FamilySupport` is an object with the keys `familyId`, `revisions`, and `paramsDigests`.
Each digest is standard base64, as protobuf JSON maps `bytes`.
The output omits an empty list and an empty `solverId`.
This is a sample output:

```json
{
  "solverId": "quip-miner-cpu/otsuka",
  "generators": [
    "GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1",
    "GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1"
  ],
  "generatedFamilies": [
    {"familyId": "quip/twisted-ice-dimer", "revisions": [1]}
  ],
  "acceptedFamilies": [
    {"familyId": "quip/twisted-ice-dimer", "revisions": [1]}
  ]
}
```

The example shows only the new fields and `generators`.
The fields in [the `--capabilities` mapping](../SPEC.md#8-the---capabilities-mapping) do not change.

## Explicit family jobs

An explicit family job is a `Job` of kind `ISING_SAMPLE` whose `IsingProblem.family` holds a `FamilyBinding`.

**FAM-64.** An `IsingProblem` with `family` set must use the `edges` graph form.
The plain model must match the canonical instance as this table shows.

| `IsingProblem` field | Content |
|---|---|
| `edges` | The canonical edges as dense positions, in canonical order. |
| `h` | One field per node, in node output order. |
| `j` | One coupling per edge, in canonical edge order. |

Each value must decode exactly to the canonical milli coefficient.
`FamilyBinding.offset_milli` carries the offset, because the plain model has no field for it.
`Solution.energy_milli` for an explicit job is the plain-model energy, without `offset`.
The coordinator adds the offset when it decides family acceptance (FAM-55).
An accepted-family solver that gets an `IsingProblem` with `family` set in the `topology_hash` graph form cannot rebuild the instance bytes.
It rejects the job with code 15, `InstanceDigestMismatch`, and the wire reason `MALFORMED`.
A generic solver solves such a job as a plain model (FAM-69).

**FAM-65.** A solver first looks up the exact `(family_id, revision)` bytes of `FamilyBinding.family` in its `accepted_families`.
If the solver does not list that pair, the solver is a generic solver for the job and solves the plain model (FAM-69).
Codes 3 and 4 cannot occur for an explicit job, because a valid entry never lists such a pair (FAM-61).
If the solver lists the pair, the solver is an accepted-family solver for the job.
It must check the job in this order.
The first failure wins.
This is the FAM-43 rule that the lowest failing code wins, applied across all four steps.

1. Descriptor checks on `FamilyBinding.family`: codes 7, 8, 9, and 10, in ascending order. Code 10 uses the `params_digests` of the matching `accepted_families` entry.
2. Code 13, `BoundsExceeded`, for an instance beyond a FAM-37 bound. This step checks every FAM-37 bound on the rebuilt instance.
   That covers the node count, the edge count, each `|h|` and `|J|`, `|offset|`, and the metadata size.
3. Code 15, `InstanceDigestMismatch`: first the field lengths in FAM-46. Then the edge faults of FAM-32 and FAM-30. Last, the digest comparison in FAM-68.
4. Code 16, `MetadataInvalid` (FAM-67).

Codes 5 and 6 never occur for an explicit job, for the same reason.

**FAM-66.** `FamilyBinding.nonce` must be empty or 32 bytes.
In the canonical instance bytes, an empty `nonce` is 32 zero bytes (FAM-30).
Only the coordinator checks archive status, because a solver cannot know it: the coordinator must send an empty `nonce` only for a revision with status `archived`.

**FAM-67.** `FamilyBinding.metadata` holds the metadata region of the canonical instance bytes: `u16 n_sections`, then the sections.
Before the solver uses metadata, it must validate the metadata against the plain model (FAM-36).
A failure gives code 16, `MetadataInvalid`.

**FAM-68.** `instance_digest` is the FAM-18 digest of the canonical instance bytes.
An accepted-family solver must rebuild the canonical instance bytes (FAM-30) from these sources.

| Instance field | Source |
|---|---|
| format | `0x01` |
| `descriptor_digest` | Computed from `FamilyBinding.family` (FAM-18). |
| `topology_order_digest` | `FamilyBinding.topology_order_digest` |
| `nonce` | `FamilyBinding.nonce`, or 32 zero bytes for an empty `nonce` |
| `n_nodes` and node ids | `FamilyBinding.node_ids`, in instance output order |
| `n_edges` and edges | `IsingProblem.edges` |
| `h` and `J` | `IsingProblem.h` and `IsingProblem.j`, decoded to milli |
| `offset` | `FamilyBinding.offset_milli` |
| `n_sections` and sections | `FamilyBinding.metadata` |

The solver computes the digest of the rebuilt bytes and compares it with `instance_digest` before it uses any family structure.
A mismatch gives code 15, `InstanceDigestMismatch`.
The coordinator also does this check before it sends an explicit job.
It loads an archived instance only from an archive whose instance bytes match `instance_digest`.

**FAM-69.** A generic solver must solve an `IsingProblem` with `family` set as a plain model.
It must not reject the job only for the `family` field.

## Dispatch and pins

**FAM-70.** A coordinator may send an `IsingProblem` with `family` set to an accepted-family solver for that revision or to a generic solver.
A generic solver solves it as a plain model (FAM-69).
The coordinator must expect use of the family structure only from an accepted-family solver for that revision.
The coordinator must send the job to a session that lists the revision in `accepted_families` only if the `params_digest` of the binding passes the `params_digests` of that entry.

**FAM-71.** A coordinator reads eligibility from the latest `Capabilities` of each session.
A `Capabilities` update changes eligibility for every later dispatch at once.

**FAM-72.** A coordinator must send a family lease only to a session that meets both conditions.

- Its latest `Capabilities` lists the exact `(family_id, revision)` in `generated_families`.
- The lease `params_digest` passes the `params_digests` restriction of that entry.

If no session qualifies, the coordinator does not send the lease.
It must not fall back to the legacy generator.

**FAM-73.** A family rejection must not stop Ising work that does not use that family.
The coordinator keeps the session after `FAMILY_UNSUPPORTED`, or after a `MALFORMED` for a family lease or family job.
A `Reject` has no family code, and the coordinator identifies the job from `Reject.job_id`.
Legacy leases and plain jobs continue to go to that session.

**FAM-74.** An operator configuration may pin a job to one `solver_id`.
The coordinator selects in two steps.

1. Filter all sessions by compatibility: family tier, `params_digests`, kind, limits, and topology. The topology check uses the cached `Topology` and `native_topology_hash`. For a family lease, the limits check evaluates `size_bounds` and compares the result with the advertised limits of the session (FAM-47) before the coordinator sends the lease.
2. Narrow the result to the sessions with the pinned `solver_id`.

A pin never skips a compatibility check.
A session with an empty `solver_id` never matches a pin.

**FAM-75.** If no session passes both steps, the job fails with an explicit pin failure reason.
The coordinator must not fall back to another solver.
These reasons are coordinator-side and do not go on the wire.

| Reason | Condition |
|---|---|
| `PinUnknownSolver` | No session has the pinned `solver_id`. |
| `PinFamilyUnsupported` | A session has the pinned `solver_id`, but none lists the revision at the needed tier or passes `params_digests`. |
| `PinLimitsExceeded` | A pinned session lists the revision, but the job exceeds that session's limits. For a family lease, a size bound exceeds an advertised limit (FAM-47). |

If more than one reason applies, the first row that applies wins.

## Family status and the chain

**FAM-76.** A family revision with status `research` may appear in explicit jobs and in non-PoW work such as offline campaigns and test leases.
It must not appear in a PoW lease, that is a job with `Provenance.is_pow` set.
Every new family has status `research` until the chain authorizes it.

**FAM-77.** The adapted P-static, D-static, and F-static families have status `research`.

**FAM-78.** A revision with status `archived` must appear only in explicit jobs.
A coordinator must not send it in a lease.

**FAM-79.** For a PoW lease, the coordinator must take the descriptor from chain challenge state (VAL-2), never from a miner.
A proof cannot name its own descriptor.
Until VAL-2 is complete, every family has status `research` or `archived`.
Every PoW lease today uses the legacy generator.

**FAM-80.** A lease or proof without `family` uses only the legacy contract.
Replay of a historical legacy `(nonce, topology)` must give the same problem, bit for bit.
No family revision reinterprets a legacy proof.

## Scenarios

Each row gives a case with its outcome and code.

| Scenario | Outcome | Code |
|---|---|---|
| Absent descriptor: a lease with `GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1` and no `family` | The solver rejects the lease before any draw. | 1 `DescriptorMissing`, wire `MALFORMED` |
| Unsupported revision: a lease for a known `family_id` with a revision that the solver does not list | The solver rejects the lease. A coordinator that follows FAM-72 never sends it. | 6 `RevisionUnsupported`, wire `FAMILY_UNSUPPORTED` |
| Parameter digest mismatch: `params_digest` differs from the digest of `params` | The solver rejects the lease. | 8 `ParamsDigestMismatch`, wire `MALFORMED` |
| Topology order change: same node and edge sets, different registration order | `Topology.hash` checks can pass, but `topology_order_digest` differs. The solver rejects the lease, and the coordinator sends the `Topology` again. | 11 `TopologyOrderMismatch`, wire `TOPOLOGY_MISMATCH` |
| Invalid pin, unknown solver: no session has the pinned `solver_id` | The job fails. No other solver gets it. | `PinUnknownSolver` |
| Invalid pin, incompatible solver: the pinned session does not list the revision | The job fails. No other solver gets it. | `PinFamilyUnsupported` |
| Old-production replay: a historical lease without `family` | The legacy generator gives the same problem, bit for bit. | none |
| Old v2 consumer offered a family lease | A coordinator does not send it, because the consumer lists no `generated_families`. If the consumer gets one, it rejects it before any draw. | wire `MALFORMED` |
| Archived family offered as a lease | A coordinator does not send it, because no solver lists an archived revision in `generated_families`. If a solver gets one, it checks `generated_families` (FAM-62). | 5 `FamilyUnsupported` if no `generated_families` entry has the `family_id`, else 6 `RevisionUnsupported`. Wire `FAMILY_UNSUPPORTED`. |

## Traceability

Each `FAM-NN` rule maps to at least one SC-2 vector case or SC-7 conformance check.
SC-2 and SC-7 create these cases and checks.
`SC-2+SC-7` marks a rule with both kinds of evidence.

| Requirement | Evidence |
|---|---|
| FAM-01 | `SC-2:family-id-grammar-namespace-and-name` |
| FAM-02 | `SC-2:family-id-length-3-and-64-and-65` |
| FAM-03 | `SC-2:family-id-uppercase-invalid` |
| FAM-04 | `SC-7:quip-namespace-registry-only` |
| FAM-05 | `SC-2:family-id-example-namespace-rejected` |
| FAM-06 | `SC-2:revision-zero-rejected` |
| FAM-07 | SC-2+SC-7: `SC-2:revision-neighbor-unsupported`, `SC-7:capabilities-exact-revisions-only` |
| FAM-08 | `SC-7:revision-vectors-frozen` |
| FAM-09 | `SC-7:status-change-keeps-descriptor-digest` |
| FAM-10 | `SC-2:params-format-byte-and-field-layout` |
| FAM-11 | `SC-2:params-tag-out-of-order-duplicate-unknown-missing` |
| FAM-12 | `SC-2:params-length-4096-and-4097` |
| FAM-13 | `SC-2:params-type-width-bool-ascii` |
| FAM-14 | `SC-7:schema-has-no-float-type` |
| FAM-15 | `SC-7:schema-lists-tag-name-type-scale-bounds` |
| FAM-16 | `SC-2:params-layout-fault-wins-over-bounds` |
| FAM-17 | `SC-7:schema-holds-generator-inputs-only` |
| FAM-18 | `SC-2:digest-derive-key-contexts` |
| FAM-19 | `SC-2:topology-order-digest-reordered-edges` |
| FAM-20 | `SC-2:family-lease-legacy-nonce-derivation` |
| FAM-21 | `SC-2:substream-seed-and-first-words` |
| FAM-22 | SC-2+SC-7: `SC-2:substream-label-length-limits`, `SC-7:contract-lists-substream-labels` |
| FAM-23 | `SC-2:uniform-rejection-redraw-and-n-one` |
| FAM-24 | `SC-2:shuffle-fisher-yates-order` |
| FAM-25 | `SC-7:contract-states-max-retries` |
| FAM-26 | SC-2+SC-7: `SC-2:no-instance-salt-skipped`, `SC-7:verifier-rejects-no-instance-result` |
| FAM-27 | SC-2+SC-7: `SC-2:generation-overflow-salt-skipped`, `SC-7:verifier-rejects-overflow-salt-result` |
| FAM-28 | `SC-7:generator-output-independent-of-host` |
| FAM-29 | `SC-2:coefficients-in-milli` |
| FAM-30 | `SC-2:instance-bytes-layout-and-archived-zero-nonce` |
| FAM-31 | `SC-2:node-output-order-default-filtered` |
| FAM-32 | `SC-2:instance-edges-sorted-no-duplicate-no-self-loop` |
| FAM-33 | `SC-2:instance-energy-with-offset` |
| FAM-34 | `SC-2:metadata-kind-and-section-length-faults` |
| FAM-35 | `SC-2:metadata-change-changes-instance-digest` |
| FAM-36 | `SC-2:metadata-invalid-against-instance` |
| FAM-37 | `SC-2:instance-bounds-exceeded-salt-skipped`, `SC-2:size-bounds-over-instance-bounds-at-admission` |
| FAM-38 | SC-2+SC-7: `SC-2:size-bounds-example-family`, `SC-7:contract-has-every-item`, `SC-7:instance-within-declared-size-bounds` |
| FAM-39 | `SC-7:contract-declares-constraint-class` |
| FAM-40 | `SC-7:soft-defect-counter-diagnostic-only` |
| FAM-41 | `SC-7:adapted-families-hard-separate-ids` |
| FAM-42 | `SC-2:stable-code-values-and-wire-mapping` |
| FAM-43 | `SC-2:code-phase-lowest-code-wins` |
| FAM-44 | `SC-7:proto-existing-tags-unchanged` |
| FAM-45 | `SC-7:protocol-version-stays-2-with-families` |
| FAM-46 | `SC-2:extension-field-wrong-length-codes` |
| FAM-47 | SC-2+SC-7: `SC-2:lease-admission-order-with-field-checks`, `SC-7:lease-admission-degree-bound-too-large` |
| FAM-48 | SC-2+SC-7: `SC-2:legacy-lease-with-family-rejected`, `SC-7:coordinator-never-sets-family-on-legacy-lease` |
| FAM-49 | `SC-2:family-lease-without-descriptor-rejected` |
| FAM-50 | `SC-2:topology-order-mismatch-same-hash` |
| FAM-51 | `SC-2:family-lease-result-salt-and-nonce` |
| FAM-52 | `SC-2:skipped-salt-lease-continues-best-energy-with-offset` |
| FAM-53 | SC-2+SC-7: `SC-2:family-result-spins-instance-order`, `SC-7:coordinator-regenerates-family-result` |
| FAM-54 | SC-2+SC-7: `SC-2:unknown-generator-value-malformed`, `SC-7:no-generated-families-rejects-family-lease` |
| FAM-55 | `SC-7:coordinator-accepts-plain-model-solve-with-offset` |
| FAM-56 | `SC-7:capability-tier-per-revision` |
| FAM-57 | `SC-7:generated-family-passes-conformance` |
| FAM-58 | `SC-7:generators-lists-family-algorithm-iff-generated` |
| FAM-59 | `SC-7:archived-revision-not-generated` |
| FAM-60 | `SC-2:solver-id-grammar-and-length` |
| FAM-61 | `SC-2:family-support-entry-validity-and-duplicate-id` |
| FAM-62 | `SC-2:lease-codes-5-6-10-from-generated-families` |
| FAM-63 | `SC-7:capabilities-json-family-fields` |
| FAM-64 | `SC-2:explicit-job-topology-hash-form-rejected` |
| FAM-65 | `SC-2:explicit-job-tier-lookup-and-validation-order` |
| FAM-66 | SC-2+SC-7: `SC-2:explicit-job-nonce-length-and-archived-empty`, `SC-7:coordinator-empty-nonce-only-archived` |
| FAM-67 | `SC-2:explicit-job-metadata-invalid` |
| FAM-68 | SC-2+SC-7: `SC-2:explicit-job-rebuild-instance-digest`, `SC-7:accepted-solver-checks-instance-digest-before-structure` |
| FAM-69 | `SC-7:generic-solver-ignores-family-binding` |
| FAM-70 | `SC-7:dispatch-binding-passes-accepted-params-digests` |
| FAM-71 | `SC-7:dispatch-follows-latest-capabilities` |
| FAM-72 | `SC-7:dispatch-family-lease-needs-generated-entry` |
| FAM-73 | `SC-7:family-job-reject-keeps-session-work` |
| FAM-74 | `SC-7:pin-filters-compatibility-and-topology-first`, `SC-7:dispatch-checks-size-bounds-against-limits` |
| FAM-75 | `SC-7:pin-failure-reasons-no-fallback`, `SC-7:pin-limits-exceeded-size-bounds` |
| FAM-76 | `SC-7:research-family-never-in-pow-lease` |
| FAM-77 | `SC-7:adapted-families-research-status` |
| FAM-78 | `SC-7:archived-revision-explicit-jobs-only` |
| FAM-79 | `SC-7:pow-descriptor-from-chain-state` |
| FAM-80 | `SC-2:legacy-replay-bit-for-bit` |
