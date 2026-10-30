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
| Rust recompute | `cargo test -p quip-solver-conformance --test family_vectors` | Rust code agrees with the fixture |
| Drift | `python -m pytest conformance/test_family_vectors_drift.py` | The fixture is the oracle output, and the sources have not changed |
| Oracle | `python -m pytest conformance/test_family_oracle.py` | The oracle matches the existing goldens and the examples in the contract |

## Files and provenance

All files are under `crates/quip-solver-conformance/vectors/families/`.

| File | `provenance` | Content |
|---|---|---|
| `schema.json` | none | JSON Schema for every fixture file |
| `core.json` | `specified` | Every vector that SC-2 provides |
| `legacy.json` | `legacy-production` | References to the production legacy draw vectors, by case hash |

The `provenance` field has three values.

| Value | Meaning |
|---|---|
| `specified` | The vector follows from `docs/problem-families.md` |
| `legacy-production` | The vector pins a production legacy draw |
| `archived-research` | Reserved for later files that hold archived research instances |

No file uses `archived-research` yet.

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
| `provenance` | One of the three preceding values |
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
| `code-phase-lowest-code-wins`, `lease-admission-order-with-field-checks`, `legacy-lease-with-family-rejected`, `family-lease-without-descriptor-rejected`, `unknown-generator-value-malformed` | `lease_admission` | provided with abstract fault sets. Concrete proto leases need SC-4 tags 8 and 9. |
| `explicit-job-tier-lookup-and-validation-order`, `explicit-job-topology-hash-form-rejected` | `explicit_job` | provided with abstract fault sets |
| `extension-field-wrong-length-codes` | `field_lengths` | provided |
| `family-support-entry-validity-and-duplicate-id` | `family_support` | provided |
| `solver-id-grammar-and-length` | `solver_ids` | provided |
| `legacy-replay-bit-for-bit` | `legacy.json` | provided |
| `instance-bounds-exceeded-salt-skipped` | `instance_bounds` | partial. The vectors check the bound. SC-5 (lease execution) owns the salt skip. |
| `no-instance-salt-skipped` | none | deferred to SC-8, SC-12, and SC-13. Needs a construction with a retried step (FAM-25). |
| `generation-overflow-salt-skipped` | none | deferred to SC-8, SC-12, and SC-13. Needs a construction whose arithmetic can overflow. |
| `metadata-invalid-against-instance` | none | deferred to SC-8, SC-12, and SC-13. Only a family contract defines payload layouts (FAM-34, FAM-36). |
| `skipped-salt-lease-continues-best-energy-with-offset` | none | deferred to SC-5 with SC-8, SC-12, and SC-13. Needs a salt that a real construction skips. |

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
| F7 | `src/vectors.rs` exports `FAMILY_SCHEMA`, `FAMILY_CORE`, and `FAMILY_LEGACY` | Later family crates read the same bytes |
| F8 | ChaCha8 checkpoints stop at word 1002, and "large counters" means salt counters near `u64::MAX` | `ChaCha8Rng` has no seek, and a seek would change the scalar reference in `ising-generators` |
| F9 | CI runs the drift test in the `conformance` job, with no `.gitlab-ci.yml` change | That job already installs `blake3` |

## Regenerating

Run `make family-vectors` to rewrite `core.json` and `legacy.json`.
The target needs the `blake3` package, which the `dev` extra installs.
Run `cd conformance && python -m family_oracle check` to compare the files with the oracle output without writing.

A regeneration commit must say why every changed expected value changed.
Tests never regenerate the fixture.
CI runs the drift test in the `conformance` job.
`cargo test` does not run Python, so it cannot detect drift.

## Adding a family

A later family ticket adds one file per family revision in this format.
The file sets `provenance` to `specified` or `archived-research`.
It holds real constructions and real witnesses, not abstract fault sets.
Add the matching oracle code, then regenerate and commit the file with the reason for each value.

## Checks not covered

- The FAM-37 limits on node count, edge count, and metadata size are not built as instances. The `size_bounds` statistics cover the counts.
- ChaCha8 checkpoints stop at word 1002.
- The four deferred evidence ids in the coverage table have no vectors.
