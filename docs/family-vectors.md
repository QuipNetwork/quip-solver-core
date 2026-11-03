# Family golden vectors

## Purpose

The family golden vectors pin the bytes and the outcomes that [the problem families contract](problem-families.md) defines.
They cover identity grammars, parameter bytes, digests, random streams, canonical instances, stable codes, and admission order.
`docs/problem-families.md` is the normative source.
A vector never overrides it.
If a vector and the contract disagree, the contract wins and the vector is wrong.

An independent Python oracle writes the vectors.
A Rust test recomputes every expected value with its own code and compares the result with the fixture.
A pytest drift test checks that the fixture is still the exact output of the oracle.

| Check | Command | What it proves |
|---|---|---|
| Rust recompute | `cargo test -p quip-solver-conformance --test family_vectors` | Rust code in `tests/family_vectors.rs` agrees with `core.json`, `legacy.json`, `layered.json`, and the `cases` of `ice_structure.json`, and checks `ice_structure.json` against the schema |
| Ice structure | `cargo test -p ising-generators --test ice_structure` | The ice code agrees with `ice_structure.json`, the live topology, and the archived tiles |
| Drift | `python -m pytest conformance/test_family_vectors_drift.py` | The fixture is the oracle output, and the sources have not changed |
| Oracle | `python -m pytest conformance/test_family_oracle.py` | The oracle matches the existing goldens and the examples in the contract |

## Files and provenance

All files are under `crates/quip-solver-conformance/vectors/families/`.

| File | `provenance` | Content |
|---|---|---|
| `schema.json` | none | JSON Schema for every fixture file |
| `core.json` | `specified` | Every vector that SC-2 provides |
| `legacy.json` | `legacy-production` | References to the production legacy draw vectors, by case hash |
| `layered.json` | `test-construction` | Generated instances of the layered test construction for fixed lease nonces |
| `ice_structure.json` | `specified` | The ice tile structure (Zephyr lattice, candidate tiles, spines, sides, and the order-driven selection) on small topologies and on the live topology. Its `cases` section holds the registered vectors of `quip/ice-glass@1`, in the case format of `layered.json` |
| `ice/archived-tiles.bin` and `ice/archived-tiles.json` | `archived-research` | Tiles of two archived research models, and their sidecar |
| `registry.json` | `specified` | The family registry, maintained by hand |

The topologies that the vectors use are not inline.
Each one is a file under `crates/quip-solver-conformance/vectors/topologies/`, in the format of [docs/topology-format.md](topology-format.md).

| File | Content |
|---|---|
| `topologies/index.json` | One entry per topology file: name, file, kind, node and edge counts, order digest, BLAKE3 of the payload, provenance, and a description |
| `topologies/<name>.bin.gz` | One topology. Each distinct topology has one file. Two topologies are the same when the node list and the edge list match in the same order |
| `topologies/zephyr-z12t4-live.bin.gz` | `archived-research`. The live z(12, 4) topology as the lattice minus its removed nodes and removed edges (kind 1) |

A topology field in a vectors file holds a string path that starts at the directory of that file. One path is `../topologies/t-sparse.bin.gz`.
A file takes the name of the first case that uses it, in the order `core.json`, `layered.json`, `ice_structure.json`.
The T_SPARSE file is `t-sparse.bin.gz`.
A path starts at `vectors/families/`, the only directory with topology references today.
Each file in `topologies/` has an index entry. A vectors file refers to it.
The index entry of the live topology also holds `sources`, with the SHA-256 of the external specification file that the rebuild matches.
The oracle writes every topology file except the live one, and it writes `index.json`.
The live file holds archived research data, and the oracle reads it.
The Rust function `quip_solver_conformance::topology` decodes a reference.

The `provenance` field has four values.

| Value | Meaning |
|---|---|
| `specified` | The vector follows from `docs/problem-families.md` |
| `legacy-production` | The vector pins a production legacy draw |
| `test-construction` | The vector follows a test construction that only the tests and the conformance crate use |
| `archived-research` | Archived research data: the live topology file and the archived tiles |

Only the live topology file and the files under `ice/` use `archived-research`.
The files under `ice/` are fixtures and have no `sources` pins or schema.

The live topology file records the removed nodes and removed edges of the research topology.
The rebuild takes the z(12, 4) lattice without them.
It has 4575 nodes and 41474 edges.
Its index entry pins the SHA-256 of the external specification file that the rebuild matches.

The archived tiles come from the research generator, which used NumPy PCG64 permutations and SciPy matching.
They are not output of `quip/ice-glass`.
The sidecar names the two models and gives the byte range of each in the binary file.
The binary file has 1785 tiles per model.
A tile is four `u16` little-endian dense positions in the live topology.
The tiles are evidence that real research tiles are valid candidates.
They give no parity, because no code in this repository can regenerate them.

`legacy.json` holds 8 references into `golden_vectors.json` and `golden_lease.json`.
Each reference stores a BLAKE3 hash of the canonical JSON of the referenced case.
A change to a referenced case fails the test.
A new section in the production files does not.
The legacy draw, `GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1`, does not change, so these references never change.

## Fixture format

`schema.json` is the authority for the shape of every section.
This document does not restate each field.
The Rust test checks each file against a subset of JSON Schema and rejects any other keyword.

Encoding rules:

- A byte string is lowercase hex with an even length and no `0x` prefix. A 32-byte value is 64 characters.
- Every `u64` and `i64` value is a decimal string. JSON parsers that use doubles lose precision past 2^53.
- Every `u8`, `u16`, `u32`, `i32`, and small count is a JSON integer.
- The absent value is `null` in backticks here, and the JSON `null` in the files.
- The renderer is `json.dumps(doc, indent=2, sort_keys=True, ensure_ascii=True)` followed by a newline.

Every file has these top-level keys:

| Key | Content |
|---|---|
| `schema_revision` | The constant `1` |
| `provenance` | One of the four preceding values |
| `about` | Array of strings, at least one |
| `sources` | Array of `{path, blake3}` for the contract and each oracle file |
| other keys | Optional sections, listed in `schema.json` |

`core.json` has these sections:

| Group | Sections |
|---|---|
| Identity and parameters | `schemas`, `family_ids`, `revisions`, `params`, `labels` |
| Digests and streams | `descriptors`, `topologies`, `streams`, `uniform`, `shuffle`, `leases` |
| Instances | `instances`, `instance_defects`, `instance_bounds`, `metadata`, `size_bounds` |
| Codes and order | `codes`, `wire_reasons`, `lease_admission_order`, `lease_admission`, `explicit_job_order`, `explicit_job`, `field_lengths` |
| Capabilities | `family_support`, `lease_codes`, `solver_ids` |
| Traceability | `evidence` |

Every case in a section has a `name` and an `evidence` array of `SC-2:` ids.

## Coverage

The traceability table of the contract lists 51 `SC-2:` ids.
The vectors cover 46 ids, cover one in part, and leave four for later tickets.
The `evidence` section of `core.json` records the same split, and the drift test checks it.

| Evidence id (without `SC-2:`) | Section | Status |
|---|---|---|
| `family-id-grammar-namespace-and-name`, `family-id-length-3-and-64-and-65`, `family-id-uppercase-invalid`, `family-id-example-namespace-rejected` | `family_ids` | provided |
| `revision-zero-rejected` | `revisions` | provided |
| `revision-neighbor-unsupported`, `lease-codes-5-6-10-from-generated-families` | `lease_codes` | provided |
| `params-format-byte-and-field-layout`, `params-tag-out-of-order-duplicate-unknown-missing`, `params-length-4096-and-4097`, `params-type-width-bool-ascii`, `params-layout-fault-wins-over-bounds` | `params`, `schemas` | provided |
| `digest-derive-key-contexts` | `descriptors`, `topologies`, `streams`, `instances` | provided |
| `topology-order-digest-reordered-edges`, `topology-order-mismatch-same-hash` | `topologies`, `lease_admission` | provided |
| `family-lease-legacy-nonce-derivation`, `family-lease-result-salt-and-nonce` | `leases` | provided |
| `substream-seed-and-first-words` | `streams` | provided |
| `substream-label-length-limits` | `labels` | provided |
| `uniform-rejection-redraw-and-n-one` | `uniform` | provided |
| `shuffle-fisher-yates-order` | `shuffle` | provided |
| `coefficients-in-milli`, `instance-bytes-layout-and-archived-zero-nonce`, `node-output-order-default-filtered`, `instance-energy-with-offset`, `metadata-change-changes-instance-digest`, `explicit-job-rebuild-instance-digest`, `explicit-job-nonce-length-and-archived-empty`, `family-result-spins-instance-order` | `instances`, `field_lengths` | provided |
| `instance-edges-sorted-no-duplicate-no-self-loop` | `instance_defects` | provided |
| `metadata-kind-and-section-length-faults`, `explicit-job-metadata-invalid` | `metadata` | provided |
| `size-bounds-example-family`, `size-bounds-over-instance-bounds-at-admission` | `size_bounds` | provided |
| `stable-code-values-and-wire-mapping` | `codes`, `wire_reasons` | provided |
| `code-phase-lowest-code-wins`, `lease-admission-order-with-field-checks`, `legacy-lease-with-family-rejected`, `family-lease-without-descriptor-rejected`, `unknown-generator-value-malformed` | `lease_admission` | provided with abstract fault sets. The tags exist. The solver session tests in `crates/quip-solver-core/tests/family_session.rs` cover concrete proto leases and session admission. |
| `explicit-job-tier-lookup-and-validation-order`, `explicit-job-topology-hash-form-rejected` | `explicit_job` | provided with abstract fault sets |
| `extension-field-wrong-length-codes` | `field_lengths` | provided |
| `family-support-entry-validity-and-duplicate-id` | `family_support` | provided |
| `solver-id-grammar-and-length` | `solver_ids` | provided |
| `legacy-replay-bit-for-bit` | `legacy.json` | provided |
| `instance-bounds-exceeded-salt-skipped` | `instance_bounds` | partial. The vectors check the bound. The solver session tests in `crates/quip-solver-core/tests/family_session.rs` cover the salt skip. A vector of the skip waits for SC-12 and SC-13. |
| `no-instance-salt-skipped` | none | deferred to SC-12 and SC-13. Needs a construction with a retried step (FAM-25). The `quip/ice-glass` revision 1 has no retry, so it cannot supply this case. |
| `generation-overflow-salt-skipped` | none | deferred to SC-12 and SC-13. Needs a construction whose arithmetic can overflow. The `quip/ice-glass` revision 1 cannot reach an overflow, so it cannot supply this case. |
| `metadata-invalid-against-instance` | none | partial. The `quip/ice-glass` revision 1 defines a payload layout, and `crates/ising-generators/tests/ice_glass.rs` rejects each metadata fault. No vector holds a rejected instance. The vectors of SC-12 and SC-13 wait for their families (FAM-34, FAM-36). |
| `skipped-salt-lease-continues-best-energy-with-offset` | none | deferred to SC-12 and SC-13. The solver session tests in `crates/quip-solver-core/tests/family_session.rs` cover the skip and the offset. |

## Rulings

The contract left some places open when SC-2 began.
The ticket settled these gaps in `docs/problem-families.md`.
Each row names the rule that now states the answer.

| Gap | Rule | Settled behavior |
|---|---|---|
| S1 | FAM-02, FAM-42 | A `family_id` outside 3 to 64 bytes is code 3, `FamilyIdInvalid` |
| S2 | FAM-15 | A `bool`, `bytes`, or `ascii` field has no scale. The bounds of a `bool` are 0 to 1. |
| S4 | FAM-32, FAM-30, FAM-65 | On an explicit job, a bad edge list or an edge position at or past `n_nodes` is code 15 |
| S5 | FAM-65, FAM-37 | An explicit job checks every FAM-37 bound, including `\|h\|` and `\|J\|`, with code 13 |
| S6 | FAM-34 | A metadata region shorter than `n_sections`, or bytes after the last section, is code 16 |
| S7 | FAM-42 | Code 13 maps to `TOO_LARGE` at admission and on an explicit job |
| S8 | FAM-38 | Size formulas use checked `u64` arithmetic over `u64` topology statistics |
| S9 | FAM-47 | A `max_nodes` or `max_edges` of `0` makes no claim, the same as `max_degree = 0` |
| S10 | FAM-62 | A solver must keep its `generated_families` list valid under FAM-61 |
| S12 | FAM-53 | A witness with a set padding bit is invalid |
| S13 | FAM-19 | A flipped edge `(v, u)` gives a different topology digest |

Three gaps stay as rulings of the vectors only.

| Gap | Ruling |
|---|---|
| S3 | A label has `valid` and no code, because a label is a contract constant. ASCII means `0x00` to `0x7F`. Vectors test lengths 0, 1, 32, and 33, and a `0x80` byte. |
| S11 | The unknown-generator check sits among the step 2 field checks. All field checks give `MALFORMED` with no code, so vectors assert only the wire reason. |
| S14 | Vectors carry milli integers only. Wire decoding belongs to SC-4 and SC-7. |

The fixture format makes these decisions.

| # | Decision | Reason |
|---|---|---|
| F1 | `schema.json` is a JSON Schema checked by a subset checker in the Rust test, with no new crate | Other languages can check the same files |
| F2 | `u64` and `i64` values are decimal strings | Doubles lose precision past 2^53 |
| F3 | `example/all-types` revision 1 is a test-only schema in `core.json` | `example/uniform-field` has no `bytes`, `ascii`, `bool`, or 64-bit field |
| F4 | `sources` holds BLAKE3 hashes of the contract and each oracle `.py` file, and only the drift test checks them | A contract or oracle edit needs an explicit, reviewed regeneration commit |
| F5 | `legacy.json` hashes the canonical JSON of each referenced case | A case change fails, but a new section does not |
| F6 | The Rust test re-derives every expected code with its own decision functions | A Python bug in a code cannot ship unchecked |
| F7 | `src/vectors.rs` exports `FAMILY_SCHEMA`, `FAMILY_CORE`, `FAMILY_LEGACY`, and `FAMILY_LAYERED` | Later family crates read the same bytes |
| F8 | ChaCha8 checkpoints stop at word 1002, and "large counters" means salt counters near `u64::MAX` | `ChaCha8Rng` has no seek, and a seek would change the scalar reference in `ising-generators` |
| F9 | CI runs the drift test in the `conformance` job, with no `.gitlab-ci.yml` change | That job already installs `blake3` |
| F10 | `layered.json` cites `SC-7:` ids and has its own section | The SC-2 evidence model reads `core.json` and `legacy.json` only |
| F11 | People edit `registry.json` by hand, and it has no `sources` | Any change to a case fails the pin. A change to the generated bytes needs a new revision (FAM-08). A change to `name` or `evidence` needs a reviewed re-pin |

## Regenerating

Run `make family-vectors` to rewrite `core.json`, `legacy.json`, `layered.json`, and `ice_structure.json`.
The `sources` pins of every file hash the oracle modules, so a new oracle module changes the pins of every file.
The target needs the `blake3` package, which the `dev` extra installs.
Run `cd conformance && python -m family_oracle check` to compare the files with the oracle output without writing.

A regeneration commit must say why every changed expected value changed.
Tests never regenerate the fixture.
CI runs the drift test in the `conformance` job.
`cargo test` does not run Python, so it cannot detect drift.

## Adding a family

A family ticket adds its vectors file in this format and writes its contract. It also registers each revision.
[Family conformance](family-conformance.md#registering-a-family) states the steps.
The vectors file sets `provenance` to `specified` or `archived-research`.
It holds real constructions and real witnesses, not abstract fault sets.
Add the matching oracle code, then regenerate and commit the file with the reason for each value.

## Checks not covered

- The FAM-37 limits on node count, edge count, and metadata size are not built as instances. The `size_bounds` statistics cover the counts.
- ChaCha8 checkpoints stop at word 1002.
- The four deferred evidence ids in the coverage table have no vectors.
