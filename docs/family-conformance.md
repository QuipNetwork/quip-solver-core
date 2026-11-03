# Family conformance

## Purpose

This document defines how a solver passes SC-7 conformance for a generated problem family.
It names the gate that FAM-57 requires and shows how to run the gate.
It also lists the cases that the gate grades and every SC-7 check id of [the problem families contract](problem-families.md#traceability) with its coverage in this repository.
`docs/problem-families.md` is the normative source.
This document never overrides it.

## The gate (FAM-57)

A revision passes SC-7 conformance on a solver when both of these hold:

1. `quip-solver-drive --family <solver-binary> unix://<socket>` reports no failed case, and the `supported-revision` case of that `family_id@revision` passed.
   Run it once for each generation path that the binary uses.
   The default session path and a local path are separate runs.
2. The instances of the generator that the solver runs equal the checked-in vectors of the revision.
   Use `families::check_case` for a Rust `Contract`, or `families::compare` per salt for any other generator.
   Third-party optimized implementations use the same vectors.

A `Skipped` case is not a pass.
A revision without a reference contract in `quip-solver-conformance` cannot pass yet, so a solver must not list it (FAM-57).

## Running the gate

Build the driver, then run it against the solver binary.

```sh
cargo build -p quip-solver-conformance --bin quip-solver-drive --release
./target/release/quip-solver-drive --family <solver-binary> unix:///tmp/quip-family.sock
```

Exit status 0 means no case failed.
The driver prints the header `--family conformance report for <solver-binary>:` and one line per case on standard output.
It prints the miner log on standard error if the miner wrote one.
If a case failed, it also prints `conformance failed for <solver-binary>` on standard error and exits with status 1.
The driver starts the solver binary with no extra arguments.
A solver whose local path needs a flag must start in that path from a wrapper script.
A Rust test may call `families::drive_miner_family(bin, args, socket)` and pass the flags in `args`.

For condition 2, a Rust solver calls `families::check_case` with its `Contract` and each `VectorCase` that `families::parse_vector_cases` reads from `FAMILY_LAYERED`.
A solver in another language computes its instances for each salt of a vector case and calls `families::compare` on each one.

## Cases of the family gate

The gate runs against a mock coordinator.
It reads the `Hello` capabilities first, then walks the cases in the order below.
A case reports `pass`, `fail`, or `skip`.
A skip carries its reason.
Per-revision cases carry the subject `family_id@revision`.

### Session cases

| Case | Rule | What the case checks |
|---|---|---|
| `hello-protocol-version-2` | FAM-45 | `Hello` capabilities carry `protocol_version` 2 |
| `generators-list-family-iff-generated` | FAM-58 | `generators` lists `FAMILY_CHACHA8_V1` if and only if `generated_families` is not empty |
| `session-setup` | none | After `Welcome`, configuration, the topology, and the target, the binary sends `Ready` and requests credit. It then answers the setup `Cancel` with a status. A failure names the phase that did not complete. |
| `no-generated-families-rejects-family-lease` | FAM-54 | A binary that lists no generated family rejects a family lease with `FAMILY_UNSUPPORTED`. The gate skips the case when the binary lists generated families. |
| `unknown-family` | FAM-47 | A binary that generates rejects a lease for `vectors/none` with `FAMILY_UNSUPPORTED` (code 5). The gate skips the case when the binary lists no generated family or lists `vectors/none`. |

### Cases of each revision

The gate runs these ten cases for each advertised `(family_id, revision)` that has a reference revision.
At present the reference revisions are `vectors/uniform-field@1` and `vectors/restricted-field@1`.
A case needs a reference params set that gives the admission code the case tests.
If no set gives that code, the gate skips the case and states the reason.

| Case | What the case sends | Pass when |
|---|---|---|
| `supported-revision` | A valid lease for the revision | The binary returns a `Result` for every generated salt, with the energy of the reference instance, then one `LeaseDone` and one credit refund |
| `revision-plus-one` | The next revision number | One `Reject` with the revision-unsupported reason, no `Result`, and one refund (FAM-07) |
| `params-malformed` | Params bytes that do not parse | One `Reject` for malformed params |
| `params-digest-mismatch` | A params digest of 32 zero bytes | One `Reject` for a digest mismatch |
| `params-out-of-bounds` | Params outside the declared bounds | One `Reject` for params out of bounds |
| `params-restricted` | Params that the solver restricts | One `Reject` for restricted params |
| `topology-order-mismatch` | A topology order digest of a reordered topology | One `Reject` for a topology order mismatch |
| `degree-bound-too-large` | Params whose degree exceeds the advertised bound | One `Reject` with the bounds-exceeded code (FAM-47) |
| `stale-lease` | A generation at or below the `Cancel` watermark | The binary sends nothing for the lease except an optional `LeaseDone` with zero salts done, then refunds one credit |
| `deadline-expired` | A deadline already past | One `Reject` with the expired reason |

### Closing cases

| Case | What the case checks |
|---|---|
| `credit-ledger-balanced` | The binary refunded exactly one credit for each job that the gate dispatched |
| `clean-exit` | After `Shutdown`, the binary exits with code 0 and closes the stream |

## Generated-instance vectors and the checker

`crates/quip-solver-conformance/vectors/families/layered.json` holds the generated instances of the layered test construction for fixed lease nonces.
Its provenance is `test-construction`.
`make family-vectors` rewrites it, and no test writes it.
[Family golden vectors](family-vectors.md) describes the fixture files.

The checker lives in `quip_solver_conformance::families`.

| Item | Use |
|---|---|
| `parse_vector_cases` | Reads the `layered` section of a vector file into `VectorCase` values |
| `export_instances` | Generates the canonical instance of each nonce with a `Contract`, without sampling |
| `check_case` | Exports every salt of a case and compares each salt in index order. It returns the first `CaseMismatch`. |
| `compare` | Compares one generated salt with its `ExpectedSalt` by outcome, then bytes, then digest |
| `first_diff` | Walks both canonical byte strings in FAM-30 order and names the first differing `Component` |

The test `family_conformance.rs` runs the checker against faulty generators.
Two faults change the coupling order and the metadata region.
The third fault exceeds the declared size bounds.
Each fault must fail at the component it changes.
The first two faults keep the all-up energy of the honest instance, so a reported energy alone cannot expose them.

## Coverage

Each row is one SC-7 check id from the Traceability section of the problem families contract.
Status is `covered` or `not covered`.
Coverage is `host-only` for a covered row and `none` for a row that is not covered.
Evidence for a covered row lists tests as `path::test`.
Evidence for a row that is not covered starts with `needs` and names the missing piece.
Tests in `family_conformance.rs` keep this table equal to the contract.

| Check id | Rule | Status | Coverage | Evidence |
|---|---|---|---|---|
| `quip-namespace-registry-only` | FAM-04 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `capabilities-exact-revisions-only` | FAM-07 | covered | host-only | `crates/quip-solver-core/tests/family_drive.rs::the_default_path_passes_the_family_gate`, `crates/quip-solver-core/tests/family_drive.rs::the_local_path_passes_the_family_gate` (case `revision-plus-one`) |
| `revision-vectors-frozen` | FAM-08 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `status-change-keeps-descriptor-digest` | FAM-09 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `schema-has-no-float-type` | FAM-14 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `schema-lists-tag-name-type-scale-bounds` | FAM-15 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `schema-holds-generator-inputs-only` | FAM-17 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `contract-lists-substream-labels` | FAM-22 | covered | host-only | `crates/quip-solver-conformance/tests/family_conformance.rs::no_vector_nonce_opens_an_unlisted_label` |
| `contract-states-max-retries` | FAM-25 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `verifier-rejects-no-instance-result` | FAM-26 | not covered | none | needs CO-2 and VAL-1 |
| `verifier-rejects-overflow-salt-result` | FAM-27 | not covered | none | needs CO-2 and VAL-1 |
| `generator-output-independent-of-host` | FAM-28 | covered | host-only | `crates/quip-solver-conformance/tests/family_conformance.rs::exported_instances_do_not_depend_on_thread_count` (thread count on one host) |
| `contract-has-every-item` | FAM-38 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `instance-within-declared-size-bounds` | FAM-38 | covered | host-only | `crates/quip-solver-conformance/tests/family_conformance.rs::generated_instances_stay_within_size_bounds`, `crates/quip-solver-conformance/tests/family_conformance.rs::faulty_oversize_fails_at_the_outcome` |
| `contract-declares-constraint-class` | FAM-39 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `soft-defect-counter-diagnostic-only` | FAM-40 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `adapted-families-hard-separate-ids` | FAM-41 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `proto-existing-tags-unchanged` | FAM-44 | covered | host-only | `crates/quip-solver-conformance/tests/golden_wire.rs::canonical_encodings_match_the_fixture`, `crates/quip-solver-conformance/tests/golden_wire.rs::enum_numbers_match_the_fixture` |
| `protocol-version-stays-2-with-families` | FAM-45 | covered | host-only | `crates/quip-solver-conformance/tests/golden_wire.rs::protocol_version_stays_two`, `crates/quip-solver-core/tests/family_drive.rs::the_default_path_passes_the_family_gate` (case `hello-protocol-version-2`) |
| `lease-admission-degree-bound-too-large` | FAM-47 | covered | host-only | `crates/quip-solver-core/tests/family_drive.rs::the_default_path_passes_the_family_gate` (case `degree-bound-too-large`) |
| `coordinator-never-sets-family-on-legacy-lease` | FAM-48 | not covered | none | needs CO-2 |
| `coordinator-regenerates-family-result` | FAM-53 | not covered | none | needs CO-2 |
| `no-generated-families-rejects-family-lease` | FAM-54 | covered | host-only | `crates/quip-solver-core/tests/family_drive.rs::a_generic_binary_is_not_asked_to_generate` |
| `coordinator-accepts-plain-model-solve-with-offset` | FAM-55 | not covered | none | needs CO-2 |
| `capability-tier-per-revision` | FAM-56 | covered | host-only | `crates/quip-solver-core/tests/family_session.rs::a_tier_applies_per_revision` |
| `generated-family-passes-conformance` | FAM-57 | covered | host-only | `crates/quip-solver-conformance/tests/family_conformance.rs::reference_layered_passes_every_vector_case`, `crates/quip-solver-core/tests/family_drive.rs::the_default_path_passes_the_family_gate`, `crates/quip-solver-core/tests/family_drive.rs::the_local_path_passes_the_family_gate` |
| `generators-lists-family-algorithm-iff-generated` | FAM-58 | covered | host-only | `crates/quip-solver-core/tests/family_capabilities.rs::generic_sampler_advertises_no_family`, `crates/quip-solver-core/tests/family_capabilities.rs::restricted_family_sampler_advertises_its_declaration`, `crates/quip-solver-core/tests/family_drive.rs::the_default_path_passes_the_family_gate` |
| `archived-revision-not-generated` | FAM-59 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `capabilities-json-family-fields` | FAM-63 | covered | host-only | `crates/quip-solver-core/tests/family_capabilities.rs::cli_prints_the_family_fields`, `crates/quip-solver-core/tests/family_capabilities.rs::cli_and_session_answers_agree` |
| `coordinator-empty-nonce-only-archived` | FAM-66 | not covered | none | needs CO-2 |
| `accepted-solver-checks-instance-digest-before-structure` | FAM-68 | covered | host-only | `crates/quip-solver-core/tests/family_session.rs::explicit_job_vectors_hold_in_the_session`, `crates/quip-solver-core/tests/family_session.rs::binding_field_lengths_follow_the_vectors` |
| `generic-solver-ignores-family-binding` | FAM-69 | covered | host-only | `crates/quip-solver-core/tests/family_session.rs::a_generic_solver_ignores_the_binding` |
| `dispatch-binding-passes-accepted-params-digests` | FAM-70 | not covered | none | needs CO-2 |
| `dispatch-follows-latest-capabilities` | FAM-71 | not covered | none | needs CO-2 |
| `dispatch-family-lease-needs-generated-entry` | FAM-72 | not covered | none | needs CO-2 |
| `family-job-reject-keeps-session-work` | FAM-73 | not covered | none | needs CO-2. `crates/quip-solver-core/tests/family_session.rs::a_rejected_family_lease_keeps_the_session_and_its_credit` tests the solver side. |
| `pin-filters-compatibility-and-topology-first` | FAM-74 | not covered | none | needs CO-2 |
| `dispatch-checks-size-bounds-against-limits` | FAM-74 | not covered | none | needs CO-2 |
| `pin-failure-reasons-no-fallback` | FAM-75 | not covered | none | needs CO-2 |
| `pin-limits-exceeded-size-bounds` | FAM-75 | not covered | none | needs CO-2 |
| `research-family-never-in-pow-lease` | FAM-76 | not covered | none | needs CO-2 |
| `adapted-families-research-status` | FAM-77 | not covered | none | needs the family registry and contracts (QUI-2019 MR3) |
| `archived-revision-explicit-jobs-only` | FAM-78 | not covered | none | needs CO-2 |
| `pow-descriptor-from-chain-state` | FAM-79 | not covered | none | needs VAL-2 |

## Limits

Every case here is host-only.
This repository has no device generator.
The gate sees the wire only.
It cannot see the instance a local generator built, so condition 2 checks bytes through the library.
`exported_instances_do_not_depend_on_thread_count` varies the thread count on one host only.
