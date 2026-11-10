# Family release 0.0.9

## What the release contains

Release 0.0.9 of `quip-solver-core` carries the canonical problem families.
It adds a family generator, a family registry, family vectors, and the Python `families` module, and it leaves protocol version 2 unchanged.
The new wire fields are additive.

Pin the same version in every package that you use.

| Package | Where | Version | What it carries |
|---|---|---|---|
| `ising-generators` | crates.io | `0.0.9` | The family generators and `ising_generators::registry::CONTRACTS` |
| `quip-proto` | crates.io | `0.0.9` | The protobuf bindings for `quip.v1` |
| `quip-protocol` | crates.io | `0.0.9` | The protocol rules and the lease logic |
| `quip-solver-core` | crates.io | `0.0.9` | The Rust solver runtime and the family session |
| `quip-solver-conformance` | crates.io | `0.0.9` | The conformance gate and the family vectors |
| `quip-solver-core` | PyPI | `0.0.9` | The wheel with the `families` module |
| `@quip.network/quip-solver-core` | npm | `0.0.9` | The TypeScript and WebAssembly package |

The release is a set of packages that match.
Use `=0.0.9` for the crates and `==0.0.9` for the wheel.
A vector file and a generator from different versions can disagree, and the pin check fails when they do.

The check scripts `scripts/check-crate-contents.sh` and `scripts/check-wheel-families.sh` run in `make check-release`.
They fail when a crate lacks a generator source file or a registered vector file, or when the wheel lacks the `families` module or lists a different set of families than the registry.

## The families in this release

Every entry of `crates/quip-solver-conformance/vectors/families/registry.json` is in the release.
The registry holds ten entries.
Eight are families that a solver can serve.
Two, `vectors/restricted-field@1` and `vectors/uniform-field@1`, are test constructions for the conformance gate.
A solver does not serve them, and `families.supported()` does not list them.

Every entry has the status `research`.
A `research` family is for experiments.
No family is active on chain.
Activation is a separate decision and this release does not make it.

The pin is the `vectors_blake3` value.
It is the BLAKE3 hash of the canonical JSON of the cases of that revision in the vector file.
Any change to a case changes the pin.

| Family | Rev | Status | Contract | Vector file | Pin (`vectors_blake3`) |
|---|---|---|---|---|---|
| `quip/charged-ice` | 1 | `research` | [contract](families/quip-charged-ice-1.md) | `ice_charge.json` | `1096452a74b91006f432c1411892cb67ff09ad5a4355ef25c1bddcf118f777ce` |
| `quip/defects-static` | 1 | `research` | [contract](families/quip-defects-static-1.md) | `defects.json` | `ebce2efd433f37f5dd9e2f57111c6220a190af6230fa38ed3186b8db5f1212ba` |
| `quip/flow-static` | 1 | `research` | [contract](families/quip-flow-static-1.md) | `flow.json` | `fd38e689865e0e5055d18cbc82a0781a361b59cb1b94b29c5619156c35ac401c` |
| `quip/ice-fields` | 1 | `research` | [contract](families/quip-ice-fields-1.md) | `ice_fields.json` | `72bcc8c5187d2b54250ca989782f0911ce33af5a8aecdff84d123c7fafdcaaaf` |
| `quip/ice-glass` | 1 | `research` | [contract](families/quip-ice-glass-1.md) | `ice_structure.json` | `2d864176c13dd035d0c6de837a1e81fd0d532ffc47d62006446aadbd19b004c9` |
| `quip/polariton-static` | 1 | `research` | [contract](families/quip-polariton-static-1.md) | `polariton.json` | `6a7aff05df982ba8e9691c08d46900f92ae523cdb0b73d7f43832f60e7d6dc87` |
| `quip/twisted-ice-dimer` | 1 | `research` | [contract](families/quip-twisted-ice-dimer-1.md) | `twisted_dimer.json` | `b7f96cd53c0826854b2f176c0d278d3467d8e19fa30e907176e588eb8c1aeed8` |
| `quip/two-commodity-routing` | 1 | `research` | [contract](families/quip-two-commodity-routing-1.md) | `two_commodity.json` | `0cad2295c069d3b5df0723f376e2b9c2bcf9319b1e4bf6d4963d32af84712a4e` |
| `vectors/restricted-field` | 1 | `research` | [contract](families/vectors-restricted-field-1.md) | `layered.json` | `3f7adf7f3de08ef65a09cd0b583df2d518620d646c312c31b4009b3e8e5aca2a` |
| `vectors/uniform-field` | 1 | `research` | [contract](families/vectors-uniform-field-1.md) | `layered.json` | `fe0205ba0ed2c88eb71d1ddac6388228235751c60fd728c2fead935c8431d45a` |

The vector files are in `crates/quip-solver-conformance/vectors/families/`.
[The family vectors guide](family-vectors.md) explains how they are made and how the pins work.
The registry is the source of truth, and this table copies it.

### How the pins are checked

`cargo test -p quip-solver-conformance --test family_vectors registered_vectors_are_frozen` recomputes the pin of every entry from its vector file and fails on a mismatch.
`changed_registered_case_fails_the_pin` in the same file proves that a changed case fails.
`cargo test -p quip-solver-conformance --test family_registry` checks that each contract document has the status of its registry entry.
`python -m pytest conformance/test_family_vectors_drift.py` checks that each vector file is still the output of the oracle.

### Which test covers each family

The `family_conformance` test is the acceptance gate for the SC-7 checker.
Its test `every_released_family_passes_its_vector_cases` reads the registry and keeps every entry that is not archived.
It then runs `check_case` on every case of that revision, using the vector file and the contract of the entry.
The test fails when an entry has no contract or no cases, so a family added to the registry later is covered without a code change.
This covers every family in the table above.

Two more kinds of check cover the eight families that a solver can serve.

- A test file of the same name in `crates/ising-generators/tests/` builds instances and checks their structure and energies.
- The Rust recompute in `crates/quip-solver-conformance/tests/family_vectors.rs` compares each vector file with code that does not share the generator.

| Family | Generator test | Vector recompute |
|---|---|---|
| `quip/charged-ice` | `ising-generators` `tests/ice_charge.rs` | `family_vectors`, section checks for `ice_charge.json` |
| `quip/defects-static` | `ising-generators` `tests/defects.rs` | `family_vectors`, section checks for `defects.json` |
| `quip/flow-static` | `ising-generators` `tests/flow.rs` | `family_vectors`, `tests/family_vectors/flow.rs` |
| `quip/ice-fields` | `ising-generators` `tests/ice_fields.rs` | `family_vectors`, section checks for `ice_fields.json` |
| `quip/ice-glass` | `ising-generators` `tests/ice_glass.rs` and `tests/ice_structure.rs` | `family_vectors`, the `cases` of `ice_structure.json` |
| `quip/polariton-static` | `ising-generators` `tests/polariton.rs` | `family_vectors`, section checks for `polariton.json` |
| `quip/twisted-ice-dimer` | `ising-generators` `tests/twisted_dimer.rs` | `family_vectors`, every section of `twisted_dimer.json` |
| `quip/two-commodity-routing` | `ising-generators` `tests/two_commodity.rs` | `family_vectors`, section checks for `two_commodity.json` |
| `vectors/restricted-field`, `vectors/uniform-field` | The layered tests in `family_conformance` | `family_vectors`, `layered.json` |

The Python wheel has its own check.
`conformance/test_family_generation.py` runs `test_supported_matches_the_registry`, which requires that `families.supported()` equals the eight families that a solver can serve.

## Upgrade order

Upgrade in this order.
Each step needs the step before it.

1. `ising-generators` and `quip-proto`.
2. `quip-protocol` and `quip-solver-core`.
3. Solvers, and the Python wheel and the npm package that they use.
4. The coordinator.

The coordinator goes last because it decides what a solver receives.
A coordinator that issues family leases only to solvers that declare the family needs solvers that already declare it.
FAM-56 defines the declaration tiers.
FAM-72 says that a coordinator never sends a family lease to a session that did not list the family in `generated_families`.

## Version 2 negotiation

The release is additive on protocol version 2.
`protocol_version` stays 2.
[The coordinator upgrade guide](coordinator-upgrade-v2.md) covers version 2 itself.
This release adds three things to the wire.
The `Capabilities` message gains `generated_families` and `accepted_families`.
The `IsingProblem` message gains an optional `family` binding.
The generator algorithm enum gains the family value.

A solver that lists no family keeps working, because a coordinator sends it only the leases and jobs that it handled before.
If an old solver receives a job with a `family` binding, it ignores the unknown field and solves the plain model.
A solver of this release that lists no family rejects a family lease with code 5, `FamilyUnsupported`.

Old production fixtures still pass.
These tests prove it.

| Claim | Test |
|---|---|
| Old encodings of `Capabilities`, `Hello`, `Job`, `Lease`, `Reject`, and `Result` keep their meaning | `crates/quip-proto/tests/family_wire.rs`, the `old_*_bytes_keep_their_meaning` tests |
| A lease with the new tags keeps its legacy fields | `crates/quip-proto/tests/family_wire.rs`, `a_lease_with_tags_8_and_9_keeps_its_legacy_fields` |
| The protocol version is still 2 | `crates/quip-solver-conformance/tests/golden_wire.rs`, `protocol_version_stays_two` |
| The wire encodings and enum numbers match the production fixture | `golden_wire.rs`, `canonical_encodings_match_the_fixture` and `enum_numbers_match_the_fixture` |
| Legacy leases and legacy rejects give the pinned results in a live session | `crates/quip-solver-core/tests/family_session.rs`, `legacy_lease_results_are_pinned` and `legacy_reject_reasons_are_pinned` |
| A solver that lists no family declares nothing | `crates/quip-solver-core/tests/family_capabilities.rs`, `the_generic_declaration_lists_nothing` |
| A local lease still reports unique verified winners | `crates/quip-solver-core/tests/lease_session.rs`, `a_local_lease_reports_unique_verified_winners_and_counts_them` |

New family fixtures are checked by `family_wire_vectors_cross_the_wire` in `golden_wire.rs`, and by `family_leases_use_the_vector_salts_and_nonces` and `lease_admission_vectors_hold_in_the_session` in `family_session.rs`.

## How to reproduce the release checks

Run these from a checkout of the release.

```sh
make check-crate-publish check-python-dist
cargo test -p quip-solver-conformance --test family_conformance
cargo test -p quip-solver-conformance --test family_registry --test family_vectors
```

None of these commands writes to a registry.
