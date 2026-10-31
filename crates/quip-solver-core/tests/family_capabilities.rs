//! Typed family declarations and their validation (FAM-56 to FAM-62).

use quip_proto::v1::FamilySupport;
use quip_protocol::family::support_code;
use quip_solver_core::families::{
    Declaration, DeclarationError, FamilyClaim, FamilyList, FamilyTable,
};
use quip_solver_core::ising_generators::descriptor::{
    params_digest, Contract, FieldSpec, Params, SizeBounds, TopologyStats, Value as ParamValue,
    ValueType,
};
use quip_solver_core::ising_generators::instance::InstanceLimits;
use quip_solver_core::ising_generators::Code;
use serde_json::Value;

const UNIFORM_FIELD: &[FieldSpec] = &[
    FieldSpec {
        tag: 1,
        kind: ValueType::U16,
        min: 1,
        max: 16,
    },
    FieldSpec {
        tag: 2,
        kind: ValueType::I32,
        min: -4000,
        max: 4000,
    },
];

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

fn example_size_bounds(params: &Params<'_>, stats: &TopologyStats) -> Result<SizeBounds, Code> {
    let Some(ParamValue::Unsigned(layers)) = params.get(1) else {
        return Err(Code::ParamsMalformed);
    };
    let overflow = || Code::BoundsExceeded;
    let nodes = layers.checked_mul(stats.n_nodes).ok_or_else(overflow)?;
    let edges = layers
        .checked_mul(stats.n_edges)
        .and_then(|e| e.checked_add(layers.saturating_sub(1).checked_mul(stats.n_nodes)?))
        .ok_or_else(overflow)?;
    let degree = stats
        .max_degree
        .checked_add(layers.saturating_sub(1).min(2))
        .ok_or_else(overflow)?;
    Ok(SizeBounds {
        nodes,
        edges,
        degree,
    })
}

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

#[test]
fn a_claim_with_no_revision_is_refused() {
    const GENERATED: &[FamilyClaim] = &[claim(&[], &[])];
    assert_eq!(
        declaration(GENERATED, &[]).validate(),
        Err(DeclarationError::NoRevision {
            list: FamilyList::Generated,
            index: 0
        })
    );
}

#[test]
fn a_claim_that_mixes_families_is_refused() {
    const ACCEPTED: &[FamilyClaim] = &[claim(&[UNIFORM_V1, OTHER_V1], &[])];
    assert_eq!(
        declaration(&[], ACCEPTED).validate(),
        Err(DeclarationError::MixedFamilies {
            list: FamilyList::Accepted,
            index: 0
        })
    );
}

#[test]
fn a_repeated_revision_is_refused() {
    const GENERATED: &[FamilyClaim] = &[claim(&[UNIFORM_V1, UNIFORM_V1], &[])];
    assert_eq!(
        declaration(GENERATED, &[]).validate(),
        Err(DeclarationError::RepeatedRevision {
            list: FamilyList::Generated,
            index: 0,
            revision: 1
        })
    );
}

#[test]
fn two_claims_for_one_family_are_refused() {
    const GENERATED: &[FamilyClaim] = &[claim(&[UNIFORM_V1], &[]), claim(&[ZERO_FIELD_V2], &[])];
    assert_eq!(
        declaration(GENERATED, &[]).validate(),
        Err(DeclarationError::InvalidEntry {
            list: FamilyList::Generated,
            index: 0,
            family_id: "vectors/uniform-field"
        })
    );
}

#[test]
fn the_example_namespace_is_refused() {
    const CLAIMS: &[FamilyClaim] = &[claim(&[EXAMPLE_V1], &[])];
    let invalid = |list| {
        Err(DeclarationError::InvalidEntry {
            list,
            index: 0,
            family_id: "example/uniform-field",
        })
    };
    assert_eq!(
        declaration(CLAIMS, &[]).validate(),
        invalid(FamilyList::Generated)
    );
    assert_eq!(
        declaration(&[], CLAIMS).validate(),
        invalid(FamilyList::Accepted)
    );
}

#[test]
fn a_declared_parameter_set_outside_the_schema_is_refused() {
    const OUT_OF_BOUNDS: &[FamilyClaim] = &[claim(&[UNIFORM_V1], &[&FIELD_5000_PARAMS])];
    const MALFORMED: &[FamilyClaim] = &[claim(&[UNIFORM_V1], &[&MALFORMED_PARAMS])];
    assert_eq!(
        declaration(OUT_OF_BOUNDS, &[]).validate(),
        Err(DeclarationError::ParamsRejected {
            list: FamilyList::Generated,
            index: 0,
            param: 0,
            revision: 1,
            code: Code::ParamsOutOfBounds
        })
    );
    assert_eq!(
        declaration(&[], MALFORMED).validate(),
        Err(DeclarationError::ParamsRejected {
            list: FamilyList::Accepted,
            index: 0,
            param: 0,
            revision: 1,
            code: Code::ParamsMalformed
        })
    );
}

#[test]
fn a_parameter_set_must_fit_every_listed_revision() {
    const TOO_WIDE: &[FamilyClaim] = &[claim(&[UNIFORM_V1, ZERO_FIELD_V2], &[&FIELD_1000_PARAMS])];
    const ZERO: &[FamilyClaim] = &[claim(&[UNIFORM_V1, ZERO_FIELD_V2], &[&ZERO_FIELD_PARAMS])];
    assert_eq!(
        declaration(TOO_WIDE, &[]).validate(),
        Err(DeclarationError::ParamsRejected {
            list: FamilyList::Generated,
            index: 0,
            param: 0,
            revision: 2,
            code: Code::ParamsOutOfBounds
        })
    );
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
