# Problem families

A problem family is a named, versioned rule that builds one Ising instance from a nonce, a base topology, and parameter bytes.
This document is the normative contract for problem families.
The golden vectors (SC-2), the generator crate (SC-3), the protobuf extension (SC-4), and the conformance checks (SC-7) build on it.
The legacy draw, `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1`, does not change.
A lease or proof without a family descriptor uses only the legacy contract in [the solver contract](../SPEC.md).

Sections Identity through Stable codes are normative.
Each normative rule has a stable ID, `FAM-NN`, for the traceability table.
The Terms and Examples sections are informative.

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

## Identity

**FAM-01.** A `family_id` must be an ASCII string `<namespace>/<name>`.
The namespace must match `[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)*`.
The name must match `[a-z][a-z0-9-]*`.

**FAM-02.** A `family_id` must be 3 to 64 bytes long.

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
An overflow is a generator error and never wraps.

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
| `nonce` | 32 bytes | The salt nonce. |
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

**FAM-33.** The energy of spins `s` is `offset + sum(h_i s_i) + sum(J_uv s_u s_v)`, an exact `i64` in milli.

**FAM-34.** A metadata section is `u16 kind`, `u32 length`, then the payload.
Each kind must be greater than the preceding kind, so each kind occurs at most once.
This table lists the registered kinds.
A family contract defines the payload layout of each kind that it uses.

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

**FAM-37.** An instance must stay within these bounds.

| Quantity | Limit |
|---|---|
| `n_nodes` | 2^24 |
| `n_edges` | 2^26 |
| each `\|h\|` and `\|J\|` | 2^30 milli |
| metadata total | 2^26 bytes |

A family revision may declare tighter bounds.
An instance that would exceed the bounds or the advertised limits fails with code 13, `BoundsExceeded`.

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
| Bounds | Any bounds tighter than the instance bounds. |
| Max retries | The fixed try limit of each retried step, or none. |

**FAM-39.** Each contract must declare one constraint class, `soft` or `hard`.
A `soft` family keeps every source penalty as an energy term.
A `hard` contract lists feasibility constraints.
A witness that violates any of them is invalid at any energy.

**FAM-40.** In a `soft` family, a defect counter is a diagnostic only.
Adding a defect counter never turns a `soft` family into a `hard` one.

**FAM-41.** The adapted P-static, D-static, and F-static families are `hard`.
The original synthetic ice, twisted-ice, charged-ice, field-ice, and routing families are `soft`.

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
| 7 | `ParamsMalformed` | bad format byte, order, length, type, unknown or missing tag | `MALFORMED` |
| 8 | `ParamsDigestMismatch` | sent `params_digest` differs from recomputed | `MALFORMED` |
| 9 | `ParamsOutOfBounds` | a value outside schema bounds | `MALFORMED` |
| 10 | `ParamsRestricted` | params valid but outside this solver's advertised restriction | `FAMILY_UNSUPPORTED` |
| 11 | `TopologyOrderMismatch` | sent `topology_order_digest` differs from the cached topology | `TOPOLOGY_MISMATCH` |
| 12 | `TopologyIncompatible` | family cannot build on this topology | `FAMILY_UNSUPPORTED` |
| 13 | `BoundsExceeded` | instance would exceed bounds or advertised limits | `TOO_LARGE` |
| 14 | `NoInstance` | deterministic retry exhaustion for one salt | no reject; salt skipped |
| 15 | `InstanceDigestMismatch` | explicit job: sent `instance_digest` differs | `MALFORMED` |
| 16 | `MetadataInvalid` | metadata fails validation against the instance | `MALFORMED` |

**FAM-43.** Admission checks run in ascending code order, and the first failure wins.
Every check runs before any draw.

## Traceability

Task 2 completes this table.

| Requirement | Evidence |
|---|---|
