//! Typed family declarations and their validation (FAM-56 to FAM-62).

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use common::example_bin;
use quip_proto::v1::FamilySupport;
use quip_proto::v1::GeneratorAlgorithm;
use quip_protocol::family::support_code;
use quip_solver_conformance::driver::drive_miner;
use quip_solver_core::families::{
    Declaration, DeclarationError, FamilyClaim, FamilyList, FamilyTable,
};
use quip_solver_core::ising_generators::descriptor::{
    params_digest, Contract, FieldSpec, Params, ValueType,
};
use quip_solver_core::ising_generators::family::{
    BaseTopology, Construction, InstanceBody, Streams,
};
use quip_solver_core::ising_generators::instance::Instance;
use quip_solver_core::ising_generators::instance::InstanceLimits;
use quip_solver_core::ising_generators::Code;
use quip_solver_core::{
    capabilities_for, BackendIdentity, IsingGraph, SampleError, SampleParams, Sampler,
    SamplerResult,
};
use serde_json::Value;
use std::process::Command;

use quip_solver_conformance::families::layered::{self, example_size_bounds, UNIFORM_FIELD};

const ZERO_FIELD_SCHEMA: &[FieldSpec] = &[
    FieldSpec {
        tag: 1,
        kind: ValueType::U16,
        min: 1,
        max: 16,
    },
    FieldSpec {
        tag: 2,
        kind: ValueType::I32,
        min: 0,
        max: 0,
    },
];

const ZERO_FIELD_PARAMS: [u8; 19] = [0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0];
const FIELD_1000_PARAMS: [u8; 19] = [
    0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0xE8, 0x03, 0, 0,
];
const FIELD_5000_PARAMS: [u8; 19] = [
    0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0x88, 0x13, 0, 0,
];
const MALFORMED_PARAMS: [u8; 1] = [0x02];

const fn contract(
    family_id: &'static str,
    revision: u32,
    schema: &'static [FieldSpec],
) -> Contract {
    Contract {
        family_id,
        revision,
        schema,
        metadata_kinds: &[],
        limits: InstanceLimits::FAM_37,
        size_bounds: example_size_bounds,
        construction: Some(&layered::LAYERED),
    }
}

const fn without_construction(contract: Contract) -> Contract {
    Contract {
        construction: None,
        ..contract
    }
}

const fn with_construction(contract: Contract, construction: &'static Construction) -> Contract {
    Contract {
        construction: Some(construction),
        ..contract
    }
}

const UNIFORM_V1: Contract = contract("vectors/uniform-field", 1, UNIFORM_FIELD);
const ZERO_FIELD_V2: Contract = contract("vectors/uniform-field", 2, ZERO_FIELD_SCHEMA);
const OTHER_V1: Contract = contract("vectors/other", 1, UNIFORM_FIELD);
const EXAMPLE_V1: Contract = contract("example/uniform-field", 1, UNIFORM_FIELD);

const fn claim(
    revisions: &'static [Contract],
    only_params: &'static [&'static [u8]],
) -> FamilyClaim {
    FamilyClaim {
        revisions,
        only_params,
    }
}

const fn declaration(
    generated: &'static [FamilyClaim],
    accepted: &'static [FamilyClaim],
) -> Declaration {
    Declaration {
        solver_id: "quip-mock/families",
        generated,
        accepted,
    }
}

fn entry(family_id: &str, revisions: &[u32], digests: &[[u8; 32]]) -> FamilySupport {
    FamilySupport {
        family_id: family_id.to_owned(),
        revisions: revisions.to_vec(),
        params_digests: digests.iter().map(|d| d.to_vec()).collect(),
    }
}

fn table_of(decl: &Declaration) -> Result<FamilyTable, String> {
    decl.validate().map_err(|e| format!("{e:?}"))
}

fn accept_topology(_: &Params<'_>, _: &BaseTopology) -> bool {
    true
}

fn accept_metadata(_: &Params<'_>, _: &Instance) -> bool {
    true
}

fn no_instance(
    _: &Params<'_>,
    _: &BaseTopology,
    _: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    Err(Code::NoInstance)
}

const fn labelled(labels: &'static [&'static str]) -> Construction {
    Construction {
        labels,
        check_topology: accept_topology,
        generate: no_instance,
        check_metadata: accept_metadata,
        feasible: None,
    }
}

static EMPTY_LABEL: Construction = labelled(&[""]);
static LONG_LABEL: Construction = labelled(&["abcdefghijklmnopqrstuvwxyzabcdefg"]);
static NON_ASCII_LABEL: Construction = labelled(&["caf\u{e9}"]);
static REPEATED_LABEL: Construction = labelled(&["salt", "salt"]);
static LONGEST_LABEL: Construction = labelled(&["abcdefghijklmnopqrstuvwxyzabcdef", "a"]);

#[test]
fn the_generic_declaration_lists_nothing() -> Result<(), String> {
    let table = table_of(&Declaration::GENERIC)?;
    if !table.solver_id().is_empty()
        || !table.generated().is_empty()
        || !table.accepted().is_empty()
    {
        return Err(format!("generic table is not empty: {table:?}"));
    }
    Ok(())
}

#[test]
fn a_restricted_declaration_becomes_wire_entries() -> Result<(), String> {
    const GENERATED: &[FamilyClaim] = &[claim(&[UNIFORM_V1], &[&ZERO_FIELD_PARAMS])];
    const ACCEPTED: &[FamilyClaim] = &[claim(&[UNIFORM_V1], &[])];
    let table = table_of(&declaration(GENERATED, ACCEPTED))?;
    let digest = params_digest(&ZERO_FIELD_PARAMS);
    if table.solver_id() != "quip-mock/families"
        || table.generated() != [entry("vectors/uniform-field", &[1], &[digest])]
        || table.accepted() != [entry("vectors/uniform-field", &[1], &[])]
    {
        return Err(format!("wire entries differ: {table:?}"));
    }
    Ok(())
}

#[test]
fn solver_ids_follow_the_vectors() -> Result<(), String> {
    let doc: Value =
        serde_json::from_str(quip_solver_conformance::FAMILY_CORE).map_err(|e| e.to_string())?;
    let cases = doc
        .get("solver_ids")
        .and_then(Value::as_array)
        .ok_or("solver_ids is missing")?;
    for case in cases {
        let name = case.get("name").and_then(Value::as_str).ok_or("no name")?;
        let hex = case
            .get("solver_id_hex")
            .and_then(Value::as_str)
            .ok_or("no solver_id_hex")?;
        let expected = case
            .get("expected")
            .and_then(Value::as_str)
            .ok_or("no expected")?;
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| {
                hex.get(i..i + 2)
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                    .ok_or_else(|| format!("{name}: bad hex"))
            })
            .collect::<Result<Vec<u8>, String>>()?;
        let id: &'static str = Box::leak(
            String::from_utf8(bytes)
                .map_err(|e| e.to_string())?
                .into_boxed_str(),
        );
        let decl = Declaration {
            solver_id: id,
            generated: &[],
            accepted: &[],
        };
        let refused = matches!(decl.validate(), Err(DeclarationError::InvalidSolverId(_)));
        if refused != (expected == "invalid") {
            return Err(format!("{name}: expected {expected}, refused {refused}"));
        }
    }
    Ok(())
}

/// One row per invalid declaration: a label for the failure message, the
/// generated and accepted claims, and the error `validate` must return.
type InvalidRow = (
    &'static str,
    &'static [FamilyClaim],
    &'static [FamilyClaim],
    DeclarationError,
);

const NO_CONSTRUCTION: &[Contract] = &[without_construction(UNIFORM_V1)];
const EMPTY_LABELS: &[Contract] = &[with_construction(UNIFORM_V1, &EMPTY_LABEL)];
const LONG_LABELS: &[Contract] = &[with_construction(UNIFORM_V1, &LONG_LABEL)];
const NON_ASCII_LABELS: &[Contract] = &[with_construction(UNIFORM_V1, &NON_ASCII_LABEL)];
const REPEATED_LABELS: &[Contract] = &[with_construction(UNIFORM_V1, &REPEATED_LABEL)];

const fn invalid_label(list: FamilyList) -> DeclarationError {
    DeclarationError::InvalidLabel {
        list,
        index: 0,
        revision: 1,
    }
}

const fn example_namespace(list: FamilyList) -> DeclarationError {
    DeclarationError::InvalidEntry {
        list,
        index: 0,
        family_id: "example/uniform-field",
    }
}

const fn no_construction(list: FamilyList) -> DeclarationError {
    DeclarationError::NoConstruction {
        list,
        index: 0,
        revision: 1,
    }
}

const INVALID_DECLARATIONS: &[InvalidRow] = &[
    (
        "a claim with no revision",
        &[claim(&[], &[])],
        &[],
        DeclarationError::NoRevision {
            list: FamilyList::Generated,
            index: 0,
        },
    ),
    (
        "a claim that mixes families",
        &[],
        &[claim(&[UNIFORM_V1, OTHER_V1], &[])],
        DeclarationError::MixedFamilies {
            list: FamilyList::Accepted,
            index: 0,
        },
    ),
    (
        "a repeated revision",
        &[claim(&[UNIFORM_V1, UNIFORM_V1], &[])],
        &[],
        DeclarationError::RepeatedRevision {
            list: FamilyList::Generated,
            index: 0,
            revision: 1,
        },
    ),
    (
        "two claims for one family",
        &[claim(&[UNIFORM_V1], &[]), claim(&[ZERO_FIELD_V2], &[])],
        &[],
        DeclarationError::InvalidEntry {
            list: FamilyList::Generated,
            index: 0,
            family_id: "vectors/uniform-field",
        },
    ),
    (
        "the example namespace in the generated list",
        &[claim(&[EXAMPLE_V1], &[])],
        &[],
        example_namespace(FamilyList::Generated),
    ),
    (
        "the example namespace in the accepted list",
        &[],
        &[claim(&[EXAMPLE_V1], &[])],
        example_namespace(FamilyList::Accepted),
    ),
    (
        "a parameter set out of bounds",
        &[claim(&[UNIFORM_V1], &[&FIELD_5000_PARAMS])],
        &[],
        DeclarationError::ParamsRejected {
            list: FamilyList::Generated,
            index: 0,
            param: 0,
            revision: 1,
            code: Code::ParamsOutOfBounds,
        },
    ),
    (
        "a malformed parameter set",
        &[],
        &[claim(&[UNIFORM_V1], &[&MALFORMED_PARAMS])],
        DeclarationError::ParamsRejected {
            list: FamilyList::Accepted,
            index: 0,
            param: 0,
            revision: 1,
            code: Code::ParamsMalformed,
        },
    ),
    (
        "a parameter set too wide for the second revision",
        &[claim(&[UNIFORM_V1, ZERO_FIELD_V2], &[&FIELD_1000_PARAMS])],
        &[],
        DeclarationError::ParamsRejected {
            list: FamilyList::Generated,
            index: 0,
            param: 0,
            revision: 2,
            code: Code::ParamsOutOfBounds,
        },
    ),
    (
        "a generated contract with no construction",
        &[claim(NO_CONSTRUCTION, &[])],
        &[],
        no_construction(FamilyList::Generated),
    ),
    (
        "an accepted contract with no construction",
        &[],
        &[claim(NO_CONSTRUCTION, &[])],
        no_construction(FamilyList::Accepted),
    ),
    (
        "an empty construction label",
        &[claim(EMPTY_LABELS, &[])],
        &[],
        invalid_label(FamilyList::Generated),
    ),
    (
        "a construction label that is too long",
        &[claim(LONG_LABELS, &[])],
        &[],
        invalid_label(FamilyList::Generated),
    ),
    (
        "a non-ASCII construction label",
        &[claim(NON_ASCII_LABELS, &[])],
        &[],
        invalid_label(FamilyList::Generated),
    ),
    (
        "a repeated construction label",
        &[claim(REPEATED_LABELS, &[])],
        &[],
        invalid_label(FamilyList::Generated),
    ),
];

#[test]
fn invalid_declarations_are_refused() {
    for (label, generated, accepted, expected) in INVALID_DECLARATIONS {
        assert_eq!(
            declaration(generated, accepted).validate().as_ref(),
            Err(expected),
            "{label}"
        );
    }
}

/// The success halves of the parameter-fit and label checks: a restriction
/// that fits every listed revision, and a label at the length limit.
#[test]
fn valid_declarations_are_accepted() {
    const ZERO: &[FamilyClaim] = &[claim(&[UNIFORM_V1, ZERO_FIELD_V2], &[&ZERO_FIELD_PARAMS])];
    const LONGEST: &[Contract] = &[with_construction(UNIFORM_V1, &LONGEST_LABEL)];
    const GOOD: &[FamilyClaim] = &[claim(LONGEST, &[])];
    let table = declaration(ZERO, &[])
        .validate()
        .expect("zero-field restriction fits both revisions");
    assert_eq!(
        table.generated(),
        [entry(
            "vectors/uniform-field",
            &[1, 2],
            &[params_digest(&ZERO_FIELD_PARAMS)]
        )]
    );
    assert!(declaration(GOOD, &[]).validate().is_ok());
}

#[test]
fn an_empty_restriction_claims_every_parameter_set() -> Result<(), String> {
    const GENERATED: &[FamilyClaim] = &[claim(&[UNIFORM_V1], &[])];
    let table = table_of(&declaration(GENERATED, &[]))?;
    let listed = table.generated();
    if listed.iter().any(|e| !e.params_digests.is_empty()) {
        return Err("digests are not empty".to_owned());
    }
    let digest = params_digest(&FIELD_1000_PARAMS);
    if support_code(listed, "vectors/uniform-field", 1, &digest).is_some() {
        return Err("an unrestricted claim refused a parameter set".to_owned());
    }
    Ok(())
}

fn unique_socket(tag: &str) -> Result<String, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok(format!(
        "/tmp/quip-core-{tag}-{}-{nanos}.sock",
        std::process::id()
    ))
}

fn identity() -> BackendIdentity {
    BackendIdentity {
        backend: quip_proto::v1::Backend::Mock,
        algorithm: quip_proto::v1::Algorithm::Sa,
        max_nodes: 100,
        max_edges: 100,
        features: &[],
        adapt: quip_solver_core::adapt::AdaptBounds {
            min_sweeps: 1,
            max_sweeps: 1,
            min_reads: 1,
            max_reads: 1,
            reads_solution_min_factor: 1,
            reads_solution_max_factor: 1,
            reads_solution_floor_factor: 0,
        },
        limits: quip_solver_core::ModelLimits::UNSTATED,
    }
}

const GENERATED: &[FamilyClaim] = &[claim(&[UNIFORM_V1], &[&ZERO_FIELD_PARAMS])];
const ACCEPTED: &[FamilyClaim] = &[claim(&[UNIFORM_V1], &[])];

struct Generic;
impl Sampler for Generic {
    fn sample(&self, _: &IsingGraph, _: &SampleParams) -> Result<Vec<SamplerResult>, SampleError> {
        Ok(vec![])
    }
}

struct Restricted;
impl Sampler for Restricted {
    fn sample(&self, _: &IsingGraph, _: &SampleParams) -> Result<Vec<SamplerResult>, SampleError> {
        Ok(vec![])
    }
    fn declaration() -> Declaration {
        declaration(GENERATED, ACCEPTED)
    }
}

struct Local;
impl Sampler for Local {
    fn sample(&self, _: &IsingGraph, _: &SampleParams) -> Result<Vec<SamplerResult>, SampleError> {
        Ok(vec![])
    }
    fn generates_locally() -> bool {
        true
    }
    fn declaration() -> Declaration {
        declaration(GENERATED, ACCEPTED)
    }
}

struct BadId;
impl Sampler for BadId {
    fn sample(&self, _: &IsingGraph, _: &SampleParams) -> Result<Vec<SamplerResult>, SampleError> {
        Ok(vec![])
    }
    fn declaration() -> Declaration {
        Declaration {
            solver_id: "Bad",
            generated: &[],
            accepted: &[],
        }
    }
}

const LEGACY: i32 = GeneratorAlgorithm::Blake3Chacha8V1 as i32;
const FAMILY: i32 = GeneratorAlgorithm::FamilyChacha8V1 as i32;

#[test]
fn capabilities_follow_the_declaration() -> Result<(), String> {
    let id = identity();
    let generic = capabilities_for::<Generic, f64>(&id).map_err(|e| e.to_string())?;
    if !generic.solver_id.is_empty()
        || !generic.generated_families.is_empty()
        || !generic.accepted_families.is_empty()
        || generic.generators != [LEGACY]
    {
        return Err(format!("generic advertises families: {generic:?}"));
    }
    let table = table_of(&Restricted::declaration())?;
    let restricted = capabilities_for::<Restricted, f64>(&id).map_err(|e| e.to_string())?;
    if restricted.solver_id != table.solver_id()
        || restricted.generated_families != table.generated()
        || restricted.accepted_families != table.accepted()
        || restricted.generators != [LEGACY, FAMILY]
    {
        return Err(format!(
            "capabilities differ from the table: {restricted:?}"
        ));
    }
    let local = capabilities_for::<Local, f64>(&id).map_err(|e| e.to_string())?;
    if local.solver_id != restricted.solver_id
        || local.generators != restricted.generators
        || local.generated_families != restricted.generated_families
        || local.accepted_families != restricted.accepted_families
    {
        return Err(format!(
            "local {local:?} differs from default {restricted:?}"
        ));
    }
    match capabilities_for::<BadId, f64>(&id) {
        Err(DeclarationError::InvalidSolverId("Bad")) => Ok(()),
        other => Err(format!("expected InvalidSolverId, got {other:?}")),
    }
}

/// The `--capabilities` JSON of `example`, without the backend identity.
fn capabilities_without_backend(example: &str) -> Result<Value, String> {
    let out = Command::new(example_bin(example))
        .arg("--capabilities")
        .output()
        .map_err(|e| format!("{example}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{example}: --capabilities failed"));
    }
    let mut value: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("{example}: {e}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| format!("{example}: capabilities are not an object"))?;
    let _ = object.remove("backend");
    let _ = object.remove("algorithm");
    Ok(value)
}

#[test]
fn both_lease_binaries_advertise_the_same_capabilities() -> Result<(), String> {
    let local = capabilities_without_backend("mock_sampler_lease_local")?;
    let default = capabilities_without_backend("mock_sampler_lease")?;
    if local != default {
        return Err(format!("local {local} differs from default {default}"));
    }
    Ok(())
}

fn cli_json() -> Result<Value, String> {
    let out = Command::new(example_bin("mock_sampler_families"))
        .arg("--capabilities")
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("--capabilities failed".to_owned());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

fn support_json(entries: &[FamilySupport]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|e| {
                let mut o = serde_json::Map::new();
                let _ = o.insert("familyId".to_owned(), Value::from(e.family_id.clone()));
                if !e.revisions.is_empty() {
                    let _ = o.insert("revisions".to_owned(), Value::from(e.revisions.clone()));
                }
                if !e.params_digests.is_empty() {
                    let digests: Vec<String> = e
                        .params_digests
                        .iter()
                        .map(|d| STANDARD.encode(d))
                        .collect();
                    let _ = o.insert("paramsDigests".to_owned(), Value::from(digests));
                }
                Value::Object(o)
            })
            .collect(),
    )
}

#[tokio::test]
async fn cli_and_session_capabilities_agree() -> Result<(), String> {
    let bin = example_bin("mock_sampler_families")
        .to_string_lossy()
        .into_owned();
    let socket = unique_socket("families")?;
    let report = drive_miner(&bin, &format!("unix://{socket}")).await;
    if !report.is_conformant() {
        return Err(report.summary());
    }
    let hello = report
        .hello
        .as_ref()
        .and_then(|h| h.capabilities.as_ref())
        .ok_or("no Hello capabilities")?;
    let reply = report
        .capabilities_received
        .as_ref()
        .ok_or("no Capabilities reply")?;
    if hello.solver_id != reply.solver_id
        || hello.generated_families != reply.generated_families
        || hello.accepted_families != reply.accepted_families
        || hello.generators != reply.generators
    {
        return Err(format!("Hello {hello:?} differs from reply {reply:?}"));
    }
    let v = cli_json()?;
    let digest = STANDARD.encode(params_digest(&ZERO_FIELD_PARAMS));
    let expected = serde_json::json!({
        "generators": [
            "GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1",
            "GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1"
        ],
        "solverId": "quip-mock/families",
        "generatedFamilies": [{
            "familyId": "vectors/uniform-field",
            "revisions": [1],
            "paramsDigests": [digest]
        }],
        "acceptedFamilies": [{"familyId": "vectors/uniform-field", "revisions": [1]}]
    });
    for (key, want) in expected.as_object().ok_or("expected is not an object")? {
        if v.get(key) != Some(want) {
            return Err(format!("{key}: got {:?}, want {want}", v.get(key)));
        }
    }
    if v.get("solverId") != Some(&Value::from(reply.solver_id.clone()))
        || v.get("generatedFamilies") != Some(&support_json(&reply.generated_families))
        || v.get("acceptedFamilies") != Some(&support_json(&reply.accepted_families))
    {
        return Err(format!(
            "CLI JSON {v} differs from the session reply {reply:?}"
        ));
    }
    Ok(())
}

#[test]
fn the_table_finds_each_declared_contract() -> Result<(), String> {
    const GENERATED: &[FamilyClaim] = &[claim(&[UNIFORM_V1], &[])];
    const ACCEPTED: &[FamilyClaim] = &[claim(&[UNIFORM_V1, ZERO_FIELD_V2], &[])];
    let table = table_of(&declaration(GENERATED, ACCEPTED))?;
    let found = |c: Option<&'static Contract>, revision: u32| {
        c.is_some_and(|c| c.family_id == "vectors/uniform-field" && c.revision == revision)
    };
    if !found(table.generated_contract("vectors/uniform-field", 1), 1)
        || table
            .generated_contract("vectors/uniform-field", 2)
            .is_some()
        || table.generated_contract("vectors/other", 1).is_some()
        || !found(table.accepted_contract("vectors/uniform-field", 1), 1)
        || !found(table.accepted_contract("vectors/uniform-field", 2), 2)
        || table
            .accepted_contract("vectors/uniform-field", 3)
            .is_some()
    {
        return Err(format!("contract lookups differ: {table:?}"));
    }
    Ok(())
}
