//! Family vector fixtures: schema checks now, recomputation tests in later tasks.

#[path = "family_vectors/schema_check.rs"]
mod schema_check;

use schema_check::{check_keywords, validate, validate_def};
use serde_json::{json, Value};

fn schema() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_SCHEMA).map_err(|e| e.to_string())
}

fn minimal_doc() -> Value {
    json!({"schema_revision": 1, "provenance": "specified", "about": ["x"], "sources": []})
}

/// Returns `base` with `key` set to `value`.
fn with(mut base: Value, key: &str, value: Value) -> Value {
    if let Some(map) = base.as_object_mut() {
        drop(map.insert(key.to_owned(), value));
    }
    base
}

/// Returns `base` without `key`.
fn without(mut base: Value, key: &str) -> Value {
    if let Some(map) = base.as_object_mut() {
        drop(map.remove(key));
    }
    base
}

fn rejected_at(result: Result<(), String>, pointer: &str) -> Result<(), String> {
    match result {
        Ok(()) => Err(format!("accepted, expected rejection at {pointer}")),
        Err(e) if e.contains(pointer) => Ok(()),
        Err(e) => Err(format!("rejected without {pointer}: {e}")),
    }
}

fn digest(len: usize) -> String {
    "ab".repeat(len / 2)
}

fn descriptor_case(params_hex: &str, descriptor_digest: &str) -> Value {
    json!({
        "name": "d", "evidence": [],
        "family_id": "example/uniform-field", "revision": 1,
        "params_hex": params_hex, "params_digest": digest(64),
        "material_hex": "15", "descriptor_digest": descriptor_digest
    })
}

fn lease_case(salt_start: &Value, salt_count: &Value) -> Value {
    json!({
        "name": "l", "evidence": [],
        "last_proof_block_hash": digest(64), "miner_account": digest(64),
        "base_salt": digest(64), "salt_start": salt_start, "salt_count": salt_count,
        "admitted": false, "wire_reason": null, "salts": []
    })
}

#[test]
fn schema_document_uses_only_supported_keywords() -> Result<(), String> {
    check_keywords(&schema()?)
}

#[test]
fn minimal_document_is_accepted() -> Result<(), String> {
    validate(&schema()?, &minimal_doc())
}

#[test]
fn schema_revision_two_is_rejected() -> Result<(), String> {
    let doc = with(minimal_doc(), "schema_revision", json!(2));
    rejected_at(validate(&schema()?, &doc), "/schema_revision")
}

#[test]
fn missing_schema_revision_is_rejected() -> Result<(), String> {
    let doc = without(minimal_doc(), "schema_revision");
    rejected_at(validate(&schema()?, &doc), "/schema_revision")
}

#[test]
fn unknown_top_level_key_is_rejected() -> Result<(), String> {
    rejected_at(
        validate(&schema()?, &with(minimal_doc(), "extra", json!(1))),
        "/extra",
    )
}

#[test]
fn unknown_provenance_is_rejected() -> Result<(), String> {
    let doc = with(minimal_doc(), "provenance", json!("generated"));
    rejected_at(validate(&schema()?, &doc), "/provenance")
}

#[test]
fn unknown_case_key_is_rejected() -> Result<(), String> {
    let valid = json!({"name": "r", "evidence": [], "revision": 1, "expected_code": null});
    validate_def(&schema()?, "revision_case", &valid)?;
    let with_note = with(valid, "note", json!("x"));
    rejected_at(
        validate_def(&schema()?, "revision_case", &with_note),
        "/note",
    )
}

#[test]
fn uppercase_hex_is_rejected() -> Result<(), String> {
    validate_def(
        &schema()?,
        "descriptor_case",
        &descriptor_case("ab", &digest(64)),
    )?;
    rejected_at(
        validate_def(
            &schema()?,
            "descriptor_case",
            &descriptor_case("AB", &digest(64)),
        ),
        "/params_hex",
    )
}

#[test]
fn odd_length_hex_is_rejected() -> Result<(), String> {
    rejected_at(
        validate_def(
            &schema()?,
            "descriptor_case",
            &descriptor_case("abc", &digest(64)),
        ),
        "/params_hex",
    )
}

#[test]
fn short_digest_is_rejected() -> Result<(), String> {
    rejected_at(
        validate_def(
            &schema()?,
            "descriptor_case",
            &descriptor_case("ab", &digest(62)),
        ),
        "/descriptor_digest",
    )
}

#[test]
fn u64_as_json_integer_is_rejected() -> Result<(), String> {
    validate_def(
        &schema()?,
        "lease_case",
        &lease_case(&json!("5"), &json!("1")),
    )?;
    rejected_at(
        validate_def(
            &schema()?,
            "lease_case",
            &lease_case(&json!(5), &json!("1")),
        ),
        "/salt_start",
    )
}

#[test]
fn negative_decimal_is_rejected_for_u64s() -> Result<(), String> {
    rejected_at(
        validate_def(
            &schema()?,
            "lease_case",
            &lease_case(&json!("5"), &json!("-1")),
        ),
        "/salt_count",
    )
}

#[test]
fn unsupported_keyword_in_schema_is_rejected() {
    assert!(check_keywords(&json!({"type": "string", "format": "x"})).is_err());
}

#[test]
fn unsupported_keyword_nested_in_schema_is_rejected() {
    let in_properties = json!({"properties": {"a": {"format": "x"}}});
    let in_items = json!({"items": {"uniqueItems": true}});
    let in_defs = json!({"$defs": {"a": {"oneOf": []}}});
    assert!(check_keywords(&in_properties).is_err());
    assert!(check_keywords(&in_items).is_err());
    assert!(check_keywords(&in_defs).is_err());
}

#[test]
fn instance_defect_expected_code_must_be_fifteen() -> Result<(), String> {
    let valid = json!({
        "name": "d", "evidence": [], "instance": "i", "edges": [[1, 0]],
        "rule": "FAM-32", "expected_code": 15
    });
    validate_def(&schema()?, "instance_defect_case", &valid)?;
    for bad in [json!(14), json!(null)] {
        let case = with(valid.clone(), "expected_code", bad);
        rejected_at(
            validate_def(&schema()?, "instance_defect_case", &case),
            "/expected_code",
        )?;
    }
    Ok(())
}

#[test]
fn nested_enum_mismatch_is_rejected() -> Result<(), String> {
    let case = json!({"name": "f", "evidence": [], "family_id_hex": "ab", "context": "other", "expected_code": null});
    rejected_at(
        validate_def(&schema()?, "family_id_case", &case),
        "/context",
    )
}

#[test]
fn unresolved_ref_is_rejected() {
    let dangling = json!({"$defs": {}, "properties": {"a": {"$ref": "#/$defs/missing"}}});
    assert!(check_keywords(&dangling).is_err());
    assert!(validate(&dangling, &json!({"a": 1})).is_err());
}

#[test]
fn integer_above_i64_max_breaks_maximum() {
    let bounded = json!({"type": "integer", "maximum": 5});
    assert!(validate(&bounded, &json!(5)).is_ok());
    assert!(validate(&bounded, &json!(u64::MAX)).is_err());
}

#[test]
fn float_does_not_skip_bounds() {
    assert!(validate(&json!({"maximum": 5}), &json!(1.5)).is_err());
    assert!(validate(&json!({"type": "integer"}), &json!(1.0)).is_err());
}
