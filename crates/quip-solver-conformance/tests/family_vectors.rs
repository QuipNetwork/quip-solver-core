//! Family vector fixtures: schema checks now, recomputation tests in later tasks.

#[path = "family_vectors/codec.rs"]
mod codec;
#[path = "family_vectors/digests.rs"]
mod digests;
#[path = "family_vectors/schema_check.rs"]
mod schema_check;

use schema_check::{check_keywords, validate, validate_def};
use serde_json::{json, Value};

fn core() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_CORE).map_err(|e| e.to_string())
}

fn section<'a>(doc: &'a Value, name: &str) -> Result<&'a [Value], String> {
    let cases = doc
        .get(name)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("section {name} is missing or not an array"))?;
    if cases.is_empty() {
        return Err(format!("section {name} is empty"));
    }
    Ok(cases)
}

fn str_field<'a>(case: &'a Value, key: &str) -> Result<&'a str, String> {
    case.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{}: missing string field {key}", case_name(case)))
}

fn case_name(case: &Value) -> &str {
    case.get("name")
        .and_then(Value::as_str)
        .unwrap_or("<unnamed>")
}

fn u32_field(case: &Value, key: &str) -> Result<u32, String> {
    case.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("{}: missing u32 field {key}", case_name(case)))
}

/// `expected_code` as `Some(code)` or `None` for null.
fn expected_code(case: &Value) -> Result<Option<u8>, String> {
    match case.get("expected_code") {
        Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("{}: bad expected_code", case_name(case))),
        None => Err(format!("{}: no expected_code", case_name(case))),
    }
}

fn compare<T: PartialEq + std::fmt::Debug>(
    case: &Value,
    what: &str,
    got: &T,
    want: &T,
) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "{}: {what}: got {got:?}, fixture says {want:?}",
            case_name(case)
        ))
    }
}

/// Case names must be unique within each case section (the schema subset
/// cannot express this).
fn check_names_unique(doc: &Value) -> Result<(), String> {
    let map = doc.as_object().ok_or("document is not an object")?;
    for (key, value) in map {
        let Some(cases) = value.as_array() else {
            continue;
        };
        let mut seen = std::collections::BTreeSet::new();
        for name in cases
            .iter()
            .filter_map(|c| c.get("name").and_then(Value::as_str))
        {
            if !seen.insert(name) {
                return Err(format!("section {key}: duplicate case name {name}"));
            }
        }
    }
    Ok(())
}

fn check_family_ids(doc: &Value) -> Result<(), String> {
    for case in section(doc, "family_ids")? {
        let id = codec::hex_decode(str_field(case, "family_id_hex")?)?;
        let lease = match str_field(case, "context")? {
            "lease" => true,
            "direct" => false,
            other => return Err(format!("{}: context {other}", case_name(case))),
        };
        compare(
            case,
            "code",
            &codec::family_id_code(&id, lease),
            &expected_code(case)?,
        )?;
    }
    Ok(())
}

fn check_revisions(doc: &Value) -> Result<(), String> {
    for case in section(doc, "revisions")? {
        let revision = u32_field(case, "revision")?;
        compare(
            case,
            "code",
            &codec::revision_code(revision),
            &expected_code(case)?,
        )?;
    }
    Ok(())
}

fn check_solver_ids(doc: &Value) -> Result<(), String> {
    for case in section(doc, "solver_ids")? {
        let id = codec::hex_decode(str_field(case, "solver_id_hex")?)?;
        let want = match str_field(case, "expected")? {
            "valid" => codec::SolverIdClass::Valid,
            "invalid" => codec::SolverIdClass::Invalid,
            "empty" => codec::SolverIdClass::Empty,
            other => return Err(format!("{}: expected {other}", case_name(case))),
        };
        compare(case, "class", &codec::solver_id_class(&id), &want)?;
    }
    Ok(())
}

fn find_schema(doc: &Value, case: &Value) -> Result<codec::ParamSchema, String> {
    let family = str_field(case, "family_id")?;
    let revision = u32_field(case, "revision")?;
    let entry = section(doc, "schemas")?
        .iter()
        .find(|s| {
            s.get("family_id").and_then(Value::as_str) == Some(family)
                && s.get("revision").and_then(Value::as_u64) == Some(u64::from(revision))
        })
        .ok_or_else(|| format!("{}: no schema for {family} rev {revision}", case_name(case)))?;
    codec::ParamSchema::from_json(entry)
}

fn check_params(doc: &Value) -> Result<(), String> {
    for case in section(doc, "params")? {
        let schema = find_schema(doc, case)?;
        let bytes = codec::hex_decode(str_field(case, "params_hex")?)?;
        let want_code = expected_code(case)?;
        let values = case.get("values").ok_or("no values key")?;
        let digest = case.get("params_digest").ok_or("no params_digest key")?;
        match (codec::decode_params(&schema, &bytes), want_code) {
            (Ok(decoded), None) => {
                compare(case, "values decoded from params_hex", &decoded, values)?;
                compare(
                    case,
                    "re-encoded bytes",
                    &codec::encode_params(&schema, values)?,
                    &bytes,
                )?;
                let got = codec::hex_encode(&digests::params_digest(&bytes));
                compare(case, "params_digest", &Some(got.as_str()), &digest.as_str())?;
            }
            (Err(code), Some(want)) => {
                compare(case, "code", &code, &want)?;
                compare(case, "values", values, &Value::Null)?;
                compare(case, "params_digest", digest, &Value::Null)?;
            }
            (Ok(_), Some(want)) => {
                return Err(format!(
                    "{}: params_hex decoded, fixture says code {want}",
                    case_name(case)
                ))
            }
            (Err(code), None) => {
                return Err(format!(
                    "{}: params_hex gives code {code}, fixture says valid",
                    case_name(case)
                ))
            }
        }
    }
    Ok(())
}

fn check_descriptors(doc: &Value) -> Result<(), String> {
    for case in section(doc, "descriptors")? {
        let family = str_field(case, "family_id")?;
        let revision = u32_field(case, "revision")?;
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        let params_digest = digests::params_digest(&params);
        compare(
            case,
            "params_digest",
            &codec::hex_encode(&params_digest),
            &str_field(case, "params_digest")?.to_owned(),
        )?;
        compare(
            case,
            "params_digest bytes",
            &codec::hex32(str_field(case, "params_digest")?)?,
            &params_digest,
        )?;
        let material = digests::descriptor_material(family.as_bytes(), revision, &params_digest);
        compare(
            case,
            "material_hex",
            &codec::hex_encode(&material),
            &str_field(case, "material_hex")?.to_owned(),
        )?;
        let digest = digests::descriptor_digest(family.as_bytes(), revision, &params_digest);
        compare(
            case,
            "descriptor_digest",
            &codec::hex_encode(&digest),
            &str_field(case, "descriptor_digest")?.to_owned(),
        )?;
    }
    Ok(())
}

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

/// Returns `doc` with the value at JSON pointer `pointer` replaced.
fn mutate(doc: &Value, pointer: &str, value: Value) -> Value {
    let mut copy = doc.clone();
    if let Some(slot) = copy.pointer_mut(pointer) {
        *slot = value;
    }
    copy
}

#[test]
fn core_fixture_matches_schema() -> Result<(), String> {
    validate(&schema()?, &core()?)?;
    match core()?.get("provenance").and_then(Value::as_str) {
        Some("specified") => Ok(()),
        other => Err(format!("provenance is {other:?}, expected specified")),
    }
}

#[test]
fn core_case_names_are_unique() -> Result<(), String> {
    check_names_unique(&core()?)
}

#[test]
fn duplicate_case_name_is_detected() -> Result<(), String> {
    let doc = core()?;
    let first = doc
        .pointer("/revisions/0/name")
        .cloned()
        .ok_or("no first revision case")?;
    let broken = mutate(&doc, "/revisions/1/name", first);
    match check_names_unique(&broken) {
        Err(_) => Ok(()),
        Ok(()) => Err("a duplicate name was accepted".to_owned()),
    }
}

#[test]
fn family_id_cases_recompute() -> Result<(), String> {
    check_family_ids(&core()?)
}

#[test]
fn revision_cases_recompute() -> Result<(), String> {
    check_revisions(&core()?)
}

#[test]
fn solver_id_cases_recompute() -> Result<(), String> {
    check_solver_ids(&core()?)
}

#[test]
fn params_cases_recompute() -> Result<(), String> {
    check_params(&core()?)
}

#[test]
fn descriptor_digests_recompute() -> Result<(), String> {
    let doc = core()?;
    check_descriptors(&doc)?;
    let spec = section(&doc, "descriptors")?
        .iter()
        .find(|c| c.get("name").and_then(Value::as_str) == Some("spec-example"))
        .ok_or("no spec-example descriptor")?;
    let material = codec::hex_decode(str_field(spec, "material_hex")?)?;
    if material.len() == 58 {
        Ok(())
    } else {
        Err(format!(
            "spec material is {} bytes, expected 58",
            material.len()
        ))
    }
}

#[test]
fn derive_key_is_not_a_plain_hash() -> Result<(), String> {
    let doc = core()?;
    let case = section(&doc, "descriptors")?
        .first()
        .ok_or("no descriptor case")?;
    let digest = codec::hex32(str_field(case, "descriptor_digest")?)?;
    let material = codec::hex_decode(str_field(case, "material_hex")?)?;
    let plain: [u8; 32] = blake3::hash(&material).into();
    if digest == plain {
        Err("descriptor_digest equals the plain hash of the material".to_owned())
    } else {
        Ok(())
    }
}

#[test]
fn digest_contexts_separate_empty_material() {
    assert_ne!(digests::params_digest(&[]), digests::instance_digest(&[]));
}

#[test]
fn label_length_and_byte_limits() {
    assert!(!codec::label_valid(&[]));
    assert!(codec::label_valid(b"a"));
    assert!(codec::label_valid(&[b'a'; 32]));
    assert!(!codec::label_valid(&[b'a'; 33]));
    assert!(!codec::label_valid(&[b'a', 0x80]));
    assert!(codec::label_valid(&[0x00, 0x7F]));
}

#[test]
fn core_schema_revision_two_fails() -> Result<(), String> {
    let doc = mutate(&core()?, "/schema_revision", json!(2));
    rejected_at(validate(&schema()?, &doc), "/schema_revision")
}

#[test]
fn wrong_descriptor_digest_fails() -> Result<(), String> {
    let doc = core()?;
    let good = doc
        .pointer("/descriptors/0/descriptor_digest")
        .and_then(Value::as_str)
        .ok_or("no descriptor digest")?;
    let flipped = flip_last_hex_digit(good);
    let broken = mutate(&doc, "/descriptors/0/descriptor_digest", json!(flipped));
    match check_descriptors(&broken) {
        Err(e) if e.contains("descriptor_digest") => Ok(()),
        Err(e) => Err(format!("error does not name descriptor_digest: {e}")),
        Ok(()) => Err("a wrong descriptor digest was accepted".to_owned()),
    }
}

#[test]
fn changed_params_byte_fails() -> Result<(), String> {
    let doc = core()?;
    let index = section(&doc, "params")?
        .iter()
        .position(|c| c.get("name").and_then(Value::as_str) == Some("spec-valid"))
        .ok_or("no spec-valid params case")?;
    let pointer = format!("/params/{index}/params_hex");
    let good = doc
        .pointer(&pointer)
        .and_then(Value::as_str)
        .ok_or("no params_hex")?;
    let broken = mutate(&doc, &pointer, json!(flip_last_hex_digit(good)));
    match check_params(&broken) {
        Err(e) if e.contains("params_hex") => Ok(()),
        Err(e) => Err(format!("error does not name params_hex: {e}")),
        Ok(()) => Err("a changed params byte was accepted".to_owned()),
    }
}

#[test]
fn unknown_params_schema_is_an_error() -> Result<(), String> {
    let doc = mutate(&core()?, "/params/0/revision", json!(99));
    match check_params(&doc) {
        Err(_) => Ok(()),
        Ok(()) => Err("a params case without a schema was accepted".to_owned()),
    }
}

#[test]
fn params_values_must_match_decoded_values() -> Result<(), String> {
    let doc = mutate(&core()?, "/params/0/values/n_layers", json!("3"));
    match check_params(&doc) {
        Err(_) => Ok(()),
        Ok(()) => Err("wrong values were accepted".to_owned()),
    }
}

#[test]
fn wrong_family_id_expected_code_fails() -> Result<(), String> {
    let doc = mutate(&core()?, "/family_ids/0/expected_code", json!(3));
    match check_family_ids(&doc) {
        Err(_) => Ok(()),
        Ok(()) => Err("a wrong expected code was accepted".to_owned()),
    }
}

#[test]
fn layout_fault_wins_over_bounds_in_codec() -> Result<(), String> {
    let doc = core()?;
    let schemas = section(&doc, "schemas")?;
    let schema_json = schemas.first().ok_or("no schema")?;
    let parsed = codec::ParamSchema::from_json(schema_json)?;
    // n_layers = 17 (out of bounds) plus an unknown tag 3 (layout fault).
    let bytes = codec::hex_decode("01010002000000110002000400000024faffff03000100000000")?;
    match codec::decode_params(&parsed, &bytes) {
        Err(7) => Ok(()),
        other => Err(format!("expected Err(7), got {other:?}")),
    }
}

fn flip_last_hex_digit(hex: &str) -> String {
    let mut text = hex.to_owned();
    let last = text.pop();
    text.push(if last == Some('0') { '1' } else { '0' });
    text
}

#[test]
fn empty_section_is_rejected() -> Result<(), String> {
    let doc = mutate(&core()?, "/params", json!([]));
    match check_params(&doc) {
        Err(e) if e.contains("empty") => Ok(()),
        Err(e) => Err(format!("unexpected error: {e}")),
        Ok(()) => Err("an empty params section was accepted".to_owned()),
    }
}
