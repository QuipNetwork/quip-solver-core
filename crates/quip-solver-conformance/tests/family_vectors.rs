//! Family vector fixtures: schema checks now, recomputation tests in later tasks.

#[path = "family_vectors/codec.rs"]
mod codec;
#[path = "family_vectors/decisions.rs"]
mod decisions;
#[path = "family_vectors/digests.rs"]
mod digests;
#[path = "family_vectors/instance.rs"]
mod instance;
#[path = "family_vectors/legacy.rs"]
mod legacy;
#[path = "family_vectors/schema_check.rs"]
mod schema_check;

use schema_check::{check_keywords, validate, validate_def};
use serde_json::{json, Value};

fn core() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_CORE).map_err(|e| e.to_string())
}

fn legacy_doc() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_LEGACY).map_err(|e| e.to_string())
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
fn u32_list(value: &Value, what: &str) -> Result<Vec<u32>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{what} is not an array"))?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("{what} has a non-u32 entry"))
        })
        .collect()
}

fn u64_decimal(case: &Value, key: &str) -> Result<u64, String> {
    str_field(case, key)?
        .parse::<u64>()
        .map_err(|e| format!("{}: {key}: {e}", case_name(case)))
}

type Topology = (Vec<u32>, Vec<(u32, u32)>);

fn topology_inputs(case: &Value) -> Result<Topology, String> {
    let nodes = u32_list(case.get("nodes").ok_or("no nodes")?, "nodes")?;
    let edges = case
        .get("edges")
        .and_then(Value::as_array)
        .ok_or("no edges")?
        .iter()
        .map(|e| {
            let pair = u32_list(e, "edge")?;
            match pair.as_slice() {
                [u, v] => Ok((*u, *v)),
                _ => Err("edge is not a pair".to_owned()),
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((nodes, edges))
}

fn check_topologies(doc: &Value) -> Result<(), String> {
    for case in section(doc, "topologies")? {
        let (nodes, edges) = topology_inputs(case)?;
        let material = digests::topology_order_material(&nodes, &edges);
        compare(
            case,
            "material_hex",
            &codec::hex_encode(&material),
            &str_field(case, "material_hex")?.to_owned(),
        )?;
        compare(
            case,
            "order_digest",
            &codec::hex_encode(&digests::topology_order_digest(&nodes, &edges)),
            &str_field(case, "order_digest")?.to_owned(),
        )?;
    }
    Ok(())
}

fn check_labels(doc: &Value) -> Result<(), String> {
    for case in section(doc, "labels")? {
        let label = codec::hex_decode(str_field(case, "label_hex")?)?;
        let want = case
            .get("valid")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("{}: no valid flag", case_name(case)))?;
        compare(case, "valid", &codec::label_valid(&label), &want)?;
    }
    Ok(())
}

fn check_streams(doc: &Value) -> Result<(), String> {
    for case in section(doc, "streams")? {
        let label = codec::hex_decode(str_field(case, "label_hex")?)?;
        let seed = digests::stream_seed(
            &codec::hex32(str_field(case, "nonce")?)?,
            &codec::hex32(str_field(case, "descriptor_digest")?)?,
            &codec::hex32(str_field(case, "topology_order_digest")?)?,
            &label,
        )?;
        compare(
            case,
            "seed",
            &codec::hex_encode(&seed),
            &str_field(case, "seed")?.to_owned(),
        )?;
        let checkpoints = case
            .get("checkpoints")
            .and_then(Value::as_array)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| format!("{}: no checkpoints", case_name(case)))?;
        for checkpoint in checkpoints {
            let start =
                usize::try_from(u32_field(checkpoint, "start")?).map_err(|e| e.to_string())?;
            let want = u32_list(checkpoint.get("words").ok_or("no words")?, "words")?;
            let mut rng = digests::CountingRng::new(seed);
            let got: Vec<u32> = (0..start + want.len()).map(|_| rng.next_u32()).collect();
            let window = got.get(start..).ok_or("window out of range")?.to_vec();
            compare(case, &format!("checkpoint {start}"), &window, &want)?;
        }
    }
    Ok(())
}

fn check_uniform(doc: &Value) -> Result<(), String> {
    for case in section(doc, "uniform")? {
        let mut rng = digests::CountingRng::new(codec::hex32(str_field(case, "seed")?)?);
        let calls = case
            .get("calls")
            .and_then(Value::as_array)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| format!("{}: no calls", case_name(case)))?;
        for call in calls {
            let n = u32_field(call, "n")?;
            compare(
                case,
                "result",
                &digests::uniform(&mut rng, n)?,
                &u32_field(call, "result")?,
            )?;
            compare(
                case,
                "words_used",
                &rng.used,
                &u64::from(u32_field(call, "words_used")?),
            )?;
        }
    }
    Ok(())
}

fn check_shuffle(doc: &Value) -> Result<(), String> {
    for case in section(doc, "shuffle")? {
        let mut rng = digests::CountingRng::new(codec::hex32(str_field(case, "seed")?)?);
        let length = u32_field(case, "length")?;
        let mut items: Vec<u32> = (0..length).collect();
        digests::shuffle(&mut rng, &mut items)?;
        compare(
            case,
            "permutation",
            &items,
            &u32_list(
                case.get("permutation").ok_or("no permutation")?,
                "permutation",
            )?,
        )?;
        compare(
            case,
            "words_used",
            &rng.used,
            &u64::from(u32_field(case, "words_used")?),
        )?;
    }
    Ok(())
}

fn check_leases(doc: &Value) -> Result<(), String> {
    use quip_protocol::lease::{Generator, LeaseSpec};
    for case in section(doc, "leases")? {
        let spec = LeaseSpec::new(
            Generator::Blake3Chacha8V1,
            codec::hex32(str_field(case, "last_proof_block_hash")?)?,
            codec::hex32(str_field(case, "miner_account")?)?,
            codec::hex32(str_field(case, "base_salt")?)?,
            u64_decimal(case, "salt_start")?,
            u64_decimal(case, "salt_count")?,
        );
        let admitted = case
            .get("admitted")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("{}: no admitted flag", case_name(case)))?;
        compare(case, "admitted", &spec.is_ok(), &admitted)?;
        let fixture_salts = case
            .get("salts")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{}: no salts", case_name(case)))?;
        let Ok(spec) = spec else {
            compare(
                case,
                "wire_reason",
                &case.get("wire_reason").and_then(Value::as_str),
                &Some("MALFORMED"),
            )?;
            compare(case, "salt count", &fixture_salts.len(), &0)?;
            continue;
        };
        compare(
            case,
            "wire_reason",
            &case.get("wire_reason"),
            &Some(&Value::Null),
        )?;
        compare(
            case,
            "salt count",
            &u64::try_from(fixture_salts.len()).map_err(|e| e.to_string())?,
            &spec.salt_count,
        )?;
        for (i, entry) in (0u64..).zip(fixture_salts) {
            let salt = spec.salt(i).ok_or("salt index outside the lease")?;
            let nonce = quip_protocol::derive::derive_nonce(
                spec.last_proof_block_hash,
                spec.miner_account,
                salt,
            );
            compare(
                case,
                "index",
                &u64_decimal(entry, "index")?,
                &(spec.salt_start + i),
            )?;
            compare(
                case,
                "salt",
                &codec::hex_encode(&salt),
                &str_field(entry, "salt")?.to_owned(),
            )?;
            compare(
                case,
                "nonce",
                &codec::hex_encode(&nonce),
                &str_field(entry, "nonce")?.to_owned(),
            )?;
        }
    }
    Ok(())
}

fn i64_decimal(case: &Value, key: &str) -> Result<i64, String> {
    str_field(case, key)?
        .parse::<i64>()
        .map_err(|e| format!("{}: {key}: {e}", case_name(case)))
}

/// A decimal-string field that may be null.
fn opt_i64_decimal(case: &Value, key: &str) -> Result<Option<i64>, String> {
    match case.get(key) {
        Some(Value::Null) => Ok(None),
        Some(_) => i64_decimal(case, key).map(Some),
        None => Err(format!("{}: missing field {key}", case_name(case))),
    }
}

fn i32_list(case: &Value, key: &str) -> Result<Vec<i32>, String> {
    case.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: {key} is not an array", case_name(case)))?
        .iter()
        .map(|v| {
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| format!("{}: {key} has a non-i32 entry", case_name(case)))
        })
        .collect()
}

fn edge_list(value: &Value) -> Result<Vec<(u32, u32)>, String> {
    value
        .as_array()
        .ok_or("edges is not an array")?
        .iter()
        .map(|e| match u32_list(e, "edge")?.as_slice() {
            [u, v] => Ok((*u, *v)),
            _ => Err("edge is not a pair".to_owned()),
        })
        .collect()
}

fn find_named<'a>(doc: &'a Value, section_name: &str, name: &str) -> Result<&'a Value, String> {
    section(doc, section_name)?
        .iter()
        .find(|c| c.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| format!("no {name} case in {section_name}"))
}

/// Instance inputs from a fixture case. Digests and nonce are decoded from the
/// fixture, and `descriptor_digest` is checked separately.
fn instance_parts(case: &Value) -> Result<instance::InstanceParts, String> {
    let nonce = codec::hex_decode(str_field(case, "nonce")?)?;
    if !nonce.is_empty() && nonce.len() != 32 {
        return Err(format!(
            "{}: nonce is {} bytes",
            case_name(case),
            nonce.len()
        ));
    }
    Ok(instance::InstanceParts {
        descriptor_digest: codec::hex32(str_field(case, "descriptor_digest")?)?,
        topology_order_digest: [0; 32],
        nonce,
        node_ids: u32_list(case.get("node_ids").ok_or("no node_ids")?, "node_ids")?,
        edges: edge_list(case.get("edges").ok_or("no edges")?)?,
        h: i32_list(case, "h")?,
        j: i32_list(case, "j")?,
        offset_milli: i64_decimal(case, "offset_milli")?,
        metadata: codec::hex_decode(str_field(case, "metadata_hex")?)?,
    })
}

fn contract_kinds(case: &Value, key: &str) -> Result<Vec<u16>, String> {
    case.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: {key} is not an array", case_name(case)))?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .ok_or_else(|| format!("{}: {key} has a non-u16 entry", case_name(case)))
        })
        .collect()
}

fn check_witness(case: &Value, parts: &instance::InstanceParts, w: &Value) -> Result<(), String> {
    let name = format!("{}/{}", case_name(case), case_name(w));
    let tagged = json!({"name": name});
    let bytes = codec::hex_decode(str_field(w, "spins_hex")?)?;
    let claimed = i64_decimal(w, "claimed_energy_milli")?;
    let valid = w.get("valid").and_then(Value::as_bool).ok_or("no valid")?;
    let reason = w.get("reason").and_then(Value::as_str);
    match instance::unpack_spins(&bytes, parts.node_ids.len()) {
        Err(why) => {
            compare(&tagged, "reason", &reason, &Some(why))?;
            compare(&tagged, "spins", &w.get("spins"), &Some(&Value::Null))?;
            compare(
                &tagged,
                "energy",
                &opt_i64_decimal(w, "energy_milli")?,
                &None,
            )?;
            compare(
                &tagged,
                "plain",
                &opt_i64_decimal(w, "energy_milli_plain")?,
                &None,
            )?;
            compare(&tagged, "claimed", &claimed, &0)?;
            compare(&tagged, "valid", &valid, &false)
        }
        Ok(spins) => {
            let listed = w
                .get("spins")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("{name}: spins is null"))?
                .iter()
                .map(|v| v.as_i64().and_then(|n| i8::try_from(n).ok()))
                .collect::<Option<Vec<i8>>>()
                .ok_or_else(|| format!("{name}: bad spins"))?;
            compare(&tagged, "spins", &spins, &listed)?;
            compare(&tagged, "pack", &instance::pack_spins(&listed), &bytes)?;
            let plain = instance::energy_plain(parts, &spins)?;
            let total = instance::energy_with_offset(parts, &spins)?;
            compare(
                &tagged,
                "energy_milli_plain",
                &opt_i64_decimal(w, "energy_milli_plain")?,
                &Some(plain),
            )?;
            compare(
                &tagged,
                "energy_milli",
                &opt_i64_decimal(w, "energy_milli")?,
                &Some(total),
            )?;
            compare(&tagged, "valid", &valid, &(claimed == total))?;
            let want = if claimed == total {
                None
            } else {
                Some("energy-mismatch")
            };
            compare(&tagged, "reason", &reason, &want)
        }
    }
}

fn check_instances(doc: &Value) -> Result<(), String> {
    for case in section(doc, "instances")? {
        let family = str_field(case, "family_id")?;
        let revision = u32_field(case, "revision")?;
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        let descriptor = digests::descriptor_digest(
            family.as_bytes(),
            revision,
            &digests::params_digest(&params),
        );
        compare(
            case,
            "descriptor_digest",
            &codec::hex_encode(&descriptor),
            &str_field(case, "descriptor_digest")?.to_owned(),
        )?;
        let topology = find_named(doc, "topologies", str_field(case, "topology")?)?;
        let (nodes, topo_edges) = topology_inputs(topology)?;
        let mut parts = instance_parts(case)?;
        parts.topology_order_digest = digests::topology_order_digest(&nodes, &topo_edges);
        let expected_nodes: Vec<u32> = nodes
            .iter()
            .copied()
            .filter(|n| parts.node_ids.contains(n))
            .collect();
        compare(case, "node_ids (FAM-31)", &parts.node_ids, &expected_nodes)?;
        compare(
            case,
            "edge fault",
            &instance::edge_fault(&parts.edges, parts.node_ids.len()),
            &None,
        )?;
        for value in parts.h.iter().chain(&parts.j) {
            compare(
                case,
                "coefficient bound",
                &instance::coefficient_code(i64::from(*value)),
                &None,
            )?;
        }
        compare(
            case,
            "offset bound",
            &instance::offset_code(parts.offset_milli),
            &None,
        )?;
        let kinds = contract_kinds(case, "metadata_kinds")?;
        compare(
            case,
            "metadata",
            &instance::metadata_code(&parts.metadata, &kinds),
            &None,
        )?;
        let canonical = instance::canonical_bytes(&parts)?;
        compare(
            case,
            "canonical_hex",
            &codec::hex_encode(&canonical),
            &str_field(case, "canonical_hex")?.to_owned(),
        )?;
        compare(
            case,
            "instance_digest",
            &codec::hex_encode(&digests::instance_digest(&canonical)),
            &str_field(case, "instance_digest")?.to_owned(),
        )?;
        let witnesses = case
            .get("witnesses")
            .and_then(Value::as_array)
            .filter(|w| !w.is_empty())
            .ok_or_else(|| format!("{}: no witnesses", case_name(case)))?;
        for w in witnesses {
            check_witness(case, &parts, w)?;
        }
    }
    Ok(())
}

fn check_instance_defects(doc: &Value) -> Result<(), String> {
    for case in section(doc, "instance_defects")? {
        let base = find_named(doc, "instances", str_field(case, "instance")?)?;
        let n_nodes = u32_list(base.get("node_ids").ok_or("no node_ids")?, "node_ids")?.len();
        let edges = edge_list(case.get("edges").ok_or("no edges")?)?;
        let fault = instance::edge_fault(&edges, n_nodes);
        compare(case, "rule", &fault, &Some(str_field(case, "rule")?))?;
        let code = fault.map(|_| 15_u8);
        compare(case, "code", &code, &expected_code(case)?)?;
    }
    Ok(())
}

fn check_instance_bounds(doc: &Value) -> Result<(), String> {
    for case in section(doc, "instance_bounds")? {
        let value = i64_decimal(case, "value")?;
        let code = match str_field(case, "quantity")? {
            "h" | "j" => instance::coefficient_code(value),
            "offset" => instance::offset_code(value),
            other => return Err(format!("{}: quantity {other}", case_name(case))),
        };
        compare(case, "code", &code, &expected_code(case)?)?;
    }
    Ok(())
}

fn check_metadata(doc: &Value) -> Result<(), String> {
    for case in section(doc, "metadata")? {
        let region = codec::hex_decode(str_field(case, "metadata_hex")?)?;
        let kinds = contract_kinds(case, "contract_kinds")?;
        compare(
            case,
            "code",
            &instance::metadata_code(&region, &kinds),
            &expected_code(case)?,
        )?;
    }
    Ok(())
}

fn mutate(doc: &Value, pointer: &str, value: Value) -> Value {
    let mut copy = doc.clone();
    if let Some(slot) = copy.pointer_mut(pointer) {
        *slot = value;
    }
    copy
}

fn opt_u64_decimal(case: &Value, key: &str) -> Result<Option<u64>, String> {
    match case.get(key) {
        Some(Value::Null) => Ok(None),
        Some(_) => u64_decimal(case, key).map(Some),
        None => Err(format!("{}: missing {key}", case_name(case))),
    }
}

fn usize_field(case: &Value, key: &str) -> Result<usize, String> {
    usize::try_from(u32_field(case, key)?).map_err(|e| e.to_string())
}

fn string_list(value: &Value, what: &str) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or_else(|| format!("{what} is not an array"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{what} has a non-string item"))
        })
        .collect()
}

fn section_of_strings(doc: &Value, name: &str) -> Result<Vec<String>, String> {
    let items = string_list(doc.get(name).ok_or("missing order")?, name)?;
    if items.is_empty() {
        return Err(format!("section {name} is empty"));
    }
    Ok(items)
}

fn check_codes(doc: &Value) -> Result<(), String> {
    let rows = section(doc, "codes")?;
    if rows.len() != decisions::CODES.len() {
        return Err(format!("codes has {} rows", rows.len()));
    }
    let wire = quip_solver_conformance::golden_wire();
    let mut numbers: std::collections::BTreeMap<String, i64> = wire
        .enums
        .get("RejectReason")
        .ok_or("golden_wire has no RejectReason")?
        .iter()
        .map(|(k, v)| (k.clone(), i64::from(*v)))
        .collect();
    match numbers.get("FAMILY_UNSUPPORTED") {
        Some(10) | None => {}
        Some(other) => return Err(format!("golden FAMILY_UNSUPPORTED is {other}, expected 10")),
    }
    *numbers.entry("FAMILY_UNSUPPORTED".to_owned()).or_insert(10) = 10;
    if numbers.iter().filter(|(_, v)| **v == 10).count() != 1 {
        return Err("number 10 is not used by exactly FAMILY_UNSUPPORTED".to_owned());
    }
    for (row, want) in rows.iter().zip(&decisions::CODES) {
        let phases = string_list(row.get("phases").ok_or("no phases")?, "phases")?;
        let want_phases: Vec<String> = want.phases.iter().map(|p| (*p).to_owned()).collect();
        let opt = |key: &str| -> Result<Option<String>, String> {
            match row.get(key) {
                Some(Value::Null) => Ok(None),
                Some(v) => v
                    .as_str()
                    .map(|s| Some(s.to_owned()))
                    .ok_or_else(|| format!("{key} is not a string")),
                None => Err(format!("missing {key}")),
            }
        };
        let code = u32_field(row, "code")?;
        if code != u32::from(want.code) {
            return Err(format!("code row {}: fixture says {code}", want.code));
        }
        let ctx = |what: &str| format!("code {}: {what}", want.code);
        compare(row, &ctx("name"), &str_field(row, "name")?, &want.name)?;
        compare(row, &ctx("phases"), &phases, &want_phases)?;
        compare(
            row,
            &ctx("wire_admission"),
            &opt("wire_admission")?,
            &want.wire_admission.map(str::to_owned),
        )?;
        compare(
            row,
            &ctx("wire_generation"),
            &opt("wire_generation")?,
            &None,
        )?;
        compare(
            row,
            &ctx("wire_explicit_job"),
            &opt("wire_explicit_job")?,
            &want.wire_explicit_job.map(str::to_owned),
        )?;
        for name in [want.wire_admission, want.wire_explicit_job]
            .into_iter()
            .flatten()
        {
            if !numbers.contains_key(name) {
                return Err(format!("code {}: wire name {name} is unknown", want.code));
            }
        }
    }
    let fixture: std::collections::BTreeMap<String, i64> = section(doc, "wire_reasons")?
        .iter()
        .map(|r| {
            let value = r
                .get("value")
                .and_then(Value::as_i64)
                .ok_or("wire reason has no value")?;
            Ok((str_field(r, "reason")?.to_owned(), value))
        })
        .collect::<Result<_, String>>()?;
    if fixture != numbers {
        return Err(format!(
            "wire_reasons {fixture:?} differs from enum {numbers:?}"
        ));
    }
    Ok(())
}

fn check_orders(doc: &Value) -> Result<(), String> {
    let want = |order: &[&str]| order.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let admission = section_of_strings(doc, "lease_admission_order")?;
    if admission != want(&decisions::LEASE_ADMISSION_ORDER) {
        return Err(format!("lease_admission_order is {admission:?}"));
    }
    let explicit = section_of_strings(doc, "explicit_job_order")?;
    if explicit != want(&decisions::EXPLICIT_JOB_ORDER) {
        return Err(format!("explicit_job_order is {explicit:?}"));
    }
    Ok(())
}

fn check_order_cases(
    doc: &Value,
    name: &str,
    order: &[&str],
    explicit_job: bool,
) -> Result<(), String> {
    for case in section(doc, name)? {
        let faults = string_list(case.get("faults").ok_or("no faults")?, "faults")?;
        let step = order
            .get(decisions::winner(order, &faults)?)
            .ok_or("winner out of range")?;
        compare(case, "step", step, &str_field(case, "expected_step")?)?;
        let (code, wire) = decisions::step_outcome(step, explicit_job);
        compare(case, "code", &code, &expected_code(case)?)?;
        let want_wire = match case.get("expected_wire") {
            Some(Value::Null) => None,
            Some(v) => Some(v.as_str().ok_or("expected_wire is not a string")?),
            None => return Err(format!("{}: no expected_wire", case_name(case))),
        };
        compare(case, "wire", &wire, &want_wire)?;
    }
    Ok(())
}

fn check_lease_admission(doc: &Value) -> Result<(), String> {
    check_order_cases(
        doc,
        "lease_admission",
        &decisions::LEASE_ADMISSION_ORDER,
        false,
    )
}

fn check_explicit_job(doc: &Value) -> Result<(), String> {
    check_order_cases(doc, "explicit_job", &decisions::EXPLICIT_JOB_ORDER, true)
}

fn check_field_lengths(doc: &Value) -> Result<(), String> {
    for case in section(doc, "field_lengths")? {
        let plain = match case.get("plain_node_count") {
            Some(Value::Null) => None,
            Some(_) => Some(usize_field(case, "plain_node_count")?),
            None => return Err(format!("{}: no plain_node_count", case_name(case))),
        };
        let code = decisions::field_length_code(
            str_field(case, "field")?,
            usize_field(case, "length")?,
            plain,
        )?;
        compare(case, "code", &code, &expected_code(case)?)?;
    }
    Ok(())
}

fn entries_of<'a>(case: &'a Value, key: &str) -> Result<&'a [Value], String> {
    case.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{}: {key} is not an array", case_name(case)))
}

fn check_family_support(doc: &Value) -> Result<(), String> {
    for case in section(doc, "family_support")? {
        let got = decisions::entry_claims(entries_of(case, "entries")?)?;
        let want = case
            .get("entry_claims")
            .and_then(Value::as_array)
            .ok_or("no entry_claims")?
            .iter()
            .map(|v| v.as_bool().ok_or("entry_claims item is not a bool"))
            .collect::<Result<Vec<_>, _>>()?;
        compare(case, "entry_claims", &got, &want)?;
    }
    Ok(())
}

fn check_lease_codes(doc: &Value) -> Result<(), String> {
    for case in section(doc, "lease_codes")? {
        let digest = codec::hex32(str_field(case, "params_digest")?)?;
        let got = decisions::lease_code(
            entries_of(case, "generated_families")?,
            str_field(case, "family_id")?,
            u32_field(case, "revision")?,
            &digest,
        )?;
        compare(case, "code", &got, &expected_code(case)?)?;
    }
    Ok(())
}

fn check_size_bounds(doc: &Value) -> Result<(), String> {
    for case in section(doc, "size_bounds")? {
        let bounds = decisions::example_size_bounds(
            u64::from(u32_field(case, "n_layers")?),
            u64_decimal(case, "n_nodes")?,
            u64_decimal(case, "n_edges")?,
            u64_decimal(case, "max_degree")?,
        );
        let want = (
            u64_decimal(case, "nodes")?,
            u64_decimal(case, "edges")?,
            u64_decimal(case, "degree")?,
        );
        compare(case, "bounds", &bounds, &Some(want))?;
        let code = decisions::size_bound_code(
            bounds,
            opt_u64_decimal(case, "limit_max_nodes")?,
            opt_u64_decimal(case, "limit_max_edges")?,
            opt_u64_decimal(case, "limit_max_degree")?,
        );
        compare(case, "code", &code, &expected_code(case)?)?;
    }
    Ok(())
}

/// Number of `SC-2:` ids in the spec Traceability table.
const EVIDENCE_ID_COUNT: usize = 51;

/// The evidence lists come from `core`. Case evidence is cited across `docs`.
fn check_evidence(core: &Value, docs: &[&Value]) -> Result<(), String> {
    let evidence = core.get("evidence").ok_or("no evidence section")?;
    let ids_of = |key: &str| -> Result<Vec<String>, String> {
        let items = evidence
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("evidence.{key} is not an array"))?;
        items
            .iter()
            .map(|item| {
                let id = if key == "provided" {
                    Some(item)
                } else {
                    item.get("id")
                };
                id.and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("evidence.{key} item has no id"))
            })
            .collect()
    };
    let provided = ids_of("provided")?;
    let partial = ids_of("partial")?;
    let deferred = ids_of("deferred")?;
    let all: std::collections::BTreeSet<&String> =
        provided.iter().chain(&partial).chain(&deferred).collect();
    if all.len() != provided.len() + partial.len() + deferred.len() {
        return Err("evidence lists overlap or repeat an id".to_owned());
    }
    if all.len() != EVIDENCE_ID_COUNT {
        return Err(format!(
            "{} evidence ids, expected {EVIDENCE_ID_COUNT}",
            all.len()
        ));
    }
    let mut cited = std::collections::BTreeSet::new();
    for doc in docs {
        for (key, cases) in doc.as_object().ok_or("document is not an object")? {
            if key == "evidence" {
                continue;
            }
            let Some(cases) = cases.as_array() else {
                continue;
            };
            for case in cases {
                for id in case
                    .get("evidence")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let id = id.as_str().ok_or("evidence item is not a string")?;
                    cited.extend([id.strip_prefix("SC-2:").unwrap_or(id).to_owned()]);
                }
            }
        }
    }
    for id in provided.iter().chain(&partial) {
        if !cited.contains(id) {
            return Err(format!("evidence id {id} has no case"));
        }
    }
    Ok(())
}

fn check_core(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    check_names_unique(doc)?;
    check_family_ids(doc)?;
    check_revisions(doc)?;
    check_solver_ids(doc)?;
    check_params(doc)?;
    check_descriptors(doc)?;
    check_topologies(doc)?;
    check_labels(doc)?;
    check_streams(doc)?;
    check_uniform(doc)?;
    check_shuffle(doc)?;
    check_leases(doc)?;
    check_instances(doc)?;
    check_instance_defects(doc)?;
    check_instance_bounds(doc)?;
    check_metadata(doc)?;
    check_codes(doc)?;
    check_orders(doc)?;
    check_lease_admission(doc)?;
    check_explicit_job(doc)?;
    check_field_lengths(doc)?;
    check_family_support(doc)?;
    check_lease_codes(doc)?;
    check_size_bounds(doc)
}

fn check_legacy(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    check_names_unique(doc)?;
    for reference in section(doc, "legacy_references")? {
        let case = legacy::referenced_case(reference)?;
        compare(
            reference,
            "case_blake3",
            &legacy::case_blake3(&case)?,
            &str_field(reference, "case_blake3")?.to_owned(),
        )?;
        legacy::replay(reference, &case)?;
    }
    Ok(())
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

fn find_index(doc: &Value, section_name: &str, name: &str) -> Result<usize, String> {
    section(doc, section_name)?
        .iter()
        .position(|c| c.get("name").and_then(Value::as_str) == Some(name))
        .ok_or_else(|| format!("no {name} case in {section_name}"))
}

fn swap_pointers(doc: &Value, a: &str, b: &str) -> Result<Value, String> {
    let first = doc.pointer(a).cloned().ok_or("missing first pointer")?;
    let second = doc.pointer(b).cloned().ok_or("missing second pointer")?;
    Ok(mutate(&mutate(doc, a, second), b, first))
}

fn expect_err(result: &Result<(), String>, what: &str) -> Result<(), String> {
    if result.is_err() {
        Ok(())
    } else {
        Err(format!("{what} was accepted"))
    }
}

#[test]
fn topology_order_digests_recompute() -> Result<(), String> {
    check_topologies(&core()?)
}

#[test]
fn topology_digests_are_pairwise_distinct() -> Result<(), String> {
    let doc = core()?;
    let names = [
        "sparse",
        "sparse-nodes-reordered",
        "sparse-edges-reordered",
        "sparse-edge-flipped",
    ];
    let mut digests = Vec::new();
    for name in names {
        let case = section(&doc, "topologies")?
            .get(find_index(&doc, "topologies", name)?)
            .ok_or("index")?;
        let (nodes, edges) = topology_inputs(case)?;
        digests.push(digests::topology_order_digest(&nodes, &edges));
    }
    for (i, a) in digests.iter().enumerate() {
        for b in digests.iter().skip(i + 1) {
            if a == b {
                return Err("two topology variants share an order digest".to_owned());
            }
        }
    }
    Ok(())
}

#[test]
fn label_cases_recompute() -> Result<(), String> {
    check_labels(&core()?)
}

#[test]
fn stream_checkpoints_recompute() -> Result<(), String> {
    check_streams(&core()?)
}

#[test]
fn uniform_cases_recompute() -> Result<(), String> {
    check_uniform(&core()?)
}

#[test]
fn uniform_redraw_case_redraws() -> Result<(), String> {
    let doc = core()?;
    let case = section(&doc, "uniform")?
        .iter()
        .find(|c| case_name(c).starts_with("redraw-then-small"))
        .ok_or("no redraw-then-small case")?;
    let used = case
        .pointer("/calls/0/words_used")
        .and_then(Value::as_u64)
        .ok_or("no words_used")?;
    if used >= 2 {
        Ok(())
    } else {
        Err(format!("first call used {used} words, expected a redraw"))
    }
}

#[test]
fn shuffle_cases_recompute() -> Result<(), String> {
    check_shuffle(&core()?)
}

#[test]
fn lease_nonces_recompute() -> Result<(), String> {
    check_leases(&core()?)
}

#[test]
fn reordered_topology_nodes_fails() -> Result<(), String> {
    let doc = core()?;
    let i = find_index(&doc, "topologies", "sparse")?;
    let broken = swap_pointers(
        &doc,
        &format!("/topologies/{i}/nodes/0"),
        &format!("/topologies/{i}/nodes/1"),
    )?;
    expect_err(&check_topologies(&broken), "reordered topology nodes")
}

#[test]
fn swapped_topology_edges_fails() -> Result<(), String> {
    let doc = core()?;
    let i = find_index(&doc, "topologies", "sparse")?;
    let broken = swap_pointers(
        &doc,
        &format!("/topologies/{i}/edges/0"),
        &format!("/topologies/{i}/edges/1"),
    )?;
    expect_err(&check_topologies(&broken), "swapped topology edges")
}

#[test]
fn changed_checkpoint_word_fails() -> Result<(), String> {
    let doc = core()?;
    let i = find_index(&doc, "streams", "fields")?;
    let pointer = format!("/streams/{i}/checkpoints/3/words/0");
    let word = doc
        .pointer(&pointer)
        .and_then(Value::as_u64)
        .ok_or("no checkpoint word")?;
    let broken = mutate(&doc, &pointer, json!(word.wrapping_add(1)));
    expect_err(&check_streams(&broken), "a changed checkpoint word")
}

#[test]
fn bad_labels_and_zero_bounds_are_errors() {
    let nonce = [0u8; 32];
    assert!(digests::stream_seed(&nonce, &nonce, &nonce, &[]).is_err());
    assert!(digests::stream_seed(&nonce, &nonce, &nonce, &[b'a'; 33]).is_err());
    assert!(digests::stream_seed(&nonce, &nonce, &nonce, &[b'a'; 32]).is_ok());
    let mut rng = digests::CountingRng::new(nonce);
    assert!(digests::uniform(&mut rng, 0).is_err());
    assert_eq!(rng.used, 0);
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

fn instance_digest_of(case: &Value) -> Result<String, String> {
    let parts = instance_parts(case)?;
    Ok(codec::hex_encode(&digests::instance_digest(
        &instance::canonical_bytes(&parts)?,
    )))
}

fn instance_index(doc: &Value, name: &str) -> Result<usize, String> {
    find_index(doc, "instances", name)
}

#[test]
fn instances_rebuild_canonical_bytes() -> Result<(), String> {
    check_instances(&core()?)
}

#[test]
fn archived_instance_uses_zero_nonce() -> Result<(), String> {
    let doc = core()?;
    let cases = section(&doc, "instances")?;
    let archived = cases
        .get(instance_index(&doc, "archived-zero-nonce")?)
        .ok_or("no archived case")?;
    let sparse = cases
        .get(instance_index(&doc, "sparse-default-order")?)
        .ok_or("no sparse case")?;
    let bytes = codec::hex_decode(str_field(archived, "canonical_hex")?)?;
    if bytes.get(65..97) != Some(&[0_u8; 32][..]) {
        return Err("archived nonce bytes 65..97 are not zero".to_owned());
    }
    if instance_digest_of(archived)? == instance_digest_of(sparse)? {
        return Err("archived digest equals the sparse digest".to_owned());
    }
    Ok(())
}

#[test]
fn metadata_change_changes_digest() -> Result<(), String> {
    let doc = core()?;
    let cases = section(&doc, "instances")?;
    let changed = cases
        .get(instance_index(&doc, "metadata-changed")?)
        .ok_or("no metadata-changed case")?;
    let sparse = cases
        .get(instance_index(&doc, "sparse-default-order")?)
        .ok_or("no sparse case")?;
    if instance_digest_of(changed)? == instance_digest_of(sparse)? {
        return Err("metadata change did not change the digest".to_owned());
    }
    Ok(())
}

#[test]
fn instance_witnesses_recompute() -> Result<(), String> {
    let doc = core()?;
    check_instances(&doc)?;
    let mut valid = 0_usize;
    let mut invalid = 0_usize;
    for case in section(&doc, "instances")? {
        for w in case
            .get("witnesses")
            .and_then(Value::as_array)
            .ok_or("no witnesses")?
        {
            if w.get("valid").and_then(Value::as_bool) == Some(true) {
                valid += 1;
            } else {
                invalid += 1;
            }
        }
    }
    if valid >= 2 && invalid >= 6 {
        Ok(())
    } else {
        Err(format!("witness mix {valid} valid, {invalid} invalid"))
    }
}

#[test]
fn instance_defects_are_detected() -> Result<(), String> {
    check_instance_defects(&core()?)
}

#[test]
fn instance_bounds_recompute() -> Result<(), String> {
    check_instance_bounds(&core()?)
}

#[test]
fn metadata_cases_recompute() -> Result<(), String> {
    check_metadata(&core()?)
}

#[test]
fn pack_spins_eight_nodes_has_no_padding_byte() -> Result<(), String> {
    let spins = [1_i8, -1, 1, 1, -1, -1, -1, 1];
    let packed = instance::pack_spins(&spins);
    if packed != [0x8d] {
        return Err(format!("packed {packed:?}"));
    }
    if instance::unpack_spins(&packed, 8) != Ok(spins.to_vec()) {
        return Err("round trip failed".to_owned());
    }
    if instance::unpack_spins(&[0x8d, 0], 8) != Err("wrong-length") {
        return Err("a padding byte was accepted".to_owned());
    }
    Ok(())
}

#[test]
fn minimum_values_exceed_bounds_without_panic() -> Result<(), String> {
    compare(
        &Value::Null,
        "coefficient",
        &instance::coefficient_code(i64::from(i32::MIN)),
        &Some(13),
    )?;
    compare(
        &Value::Null,
        "offset",
        &instance::offset_code(i64::MIN),
        &Some(13),
    )
}

#[test]
fn changed_canonical_byte_fails() -> Result<(), String> {
    let doc = core()?;
    let pointer = format!(
        "/instances/{}/canonical_hex",
        instance_index(&doc, "sparse-default-order")?
    );
    let good = doc
        .pointer(&pointer)
        .and_then(Value::as_str)
        .ok_or("no canonical_hex")?;
    let broken = mutate(&doc, &pointer, json!(flip_last_hex_digit(good)));
    expect_err(&check_instances(&broken), "a changed canonical byte")
}

#[test]
fn wrong_instance_digest_fails() -> Result<(), String> {
    let doc = core()?;
    let pointer = format!(
        "/instances/{}/instance_digest",
        instance_index(&doc, "reverse-ten")?
    );
    let good = doc
        .pointer(&pointer)
        .and_then(Value::as_str)
        .ok_or("no instance_digest")?;
    let broken = mutate(&doc, &pointer, json!(flip_last_hex_digit(good)));
    expect_err(&check_instances(&broken), "a wrong instance digest")
}

#[test]
fn changed_metadata_byte_fails() -> Result<(), String> {
    let doc = core()?;
    let pointer = format!(
        "/instances/{}/metadata_hex",
        instance_index(&doc, "sparse-default-order")?
    );
    let good = doc
        .pointer(&pointer)
        .and_then(Value::as_str)
        .ok_or("no metadata_hex")?;
    let broken = mutate(&doc, &pointer, json!(flip_last_hex_digit(good)));
    expect_err(&check_instances(&broken), "a changed metadata byte")
}

#[test]
fn code_table_matches_spec_and_wire() -> Result<(), String> {
    check_codes(&core()?)
}

#[test]
fn step_orders_match_spec() -> Result<(), String> {
    check_orders(&core()?)
}

#[test]
fn lease_admission_cases_recompute() -> Result<(), String> {
    check_lease_admission(&core()?)
}

#[test]
fn explicit_job_cases_recompute() -> Result<(), String> {
    check_explicit_job(&core()?)
}

#[test]
fn field_length_cases_recompute() -> Result<(), String> {
    check_field_lengths(&core()?)
}

#[test]
fn family_support_cases_recompute() -> Result<(), String> {
    check_family_support(&core()?)
}

#[test]
fn lease_code_cases_recompute() -> Result<(), String> {
    check_lease_codes(&core()?)
}

#[test]
fn size_bound_cases_recompute() -> Result<(), String> {
    let doc = core()?;
    check_size_bounds(&doc)?;
    let example = find_named(&doc, "size_bounds", "spec-example-no-limits")?;
    compare(
        example,
        "spec example",
        &decisions::example_size_bounds(2, 100, 150, 4),
        &Some((200, 400, 5)),
    )?;
    let max_degree_4 = find_named(&doc, "size_bounds", "spec-example-max-degree-4")?;
    compare(
        max_degree_4,
        "code",
        &expected_code(max_degree_4)?,
        &Some(13),
    )
}

#[test]
fn every_provided_evidence_id_has_a_case() -> Result<(), String> {
    check_evidence(&core()?, &[&core()?, &legacy_doc()?])
}

#[test]
fn legacy_evidence_alone_is_not_enough() -> Result<(), String> {
    expect_err(
        &check_evidence(&core()?, &[&core()?]),
        "a core-only evidence check",
    )
}

#[test]
fn legacy_fixture_matches_schema() -> Result<(), String> {
    validate(&schema()?, &legacy_doc()?)?;
    match legacy_doc()?.get("provenance").and_then(Value::as_str) {
        Some("legacy-production") => Ok(()),
        other => Err(format!(
            "provenance is {other:?}, expected legacy-production"
        )),
    }
}

#[test]
fn legacy_case_hashes_match_referenced_vectors() -> Result<(), String> {
    for reference in section(&legacy_doc()?, "legacy_references")? {
        let case = legacy::referenced_case(reference)?;
        compare(
            reference,
            "case_blake3",
            &legacy::case_blake3(&case)?,
            &str_field(reference, "case_blake3")?.to_owned(),
        )?;
    }
    Ok(())
}

#[test]
fn legacy_cases_replay_bit_for_bit() -> Result<(), String> {
    check_legacy(&legacy_doc()?)
}

#[test]
fn canonical_json_ignores_key_order() -> Result<(), String> {
    let first: Value =
        serde_json::from_str(r#"{"b":[1,{"y":2,"x":1}],"a":"s"}"#).map_err(|e| e.to_string())?;
    let second: Value =
        serde_json::from_str(r#"{"a":"s","b":[1,{"x":1,"y":2}]}"#).map_err(|e| e.to_string())?;
    compare(
        &first,
        "canonical",
        &legacy::canonical_json(&first)?,
        &r#"{"a":"s","b":[1,{"x":1,"y":2}]}"#.to_owned(),
    )?;
    compare(
        &first,
        "hash",
        &legacy::case_blake3(&first)?,
        &legacy::case_blake3(&second)?,
    )
}

#[test]
fn canonical_json_rejects_floats_and_non_ascii() {
    assert!(legacy::canonical_json(&json!({"a": 1.5})).is_err());
    assert!(legacy::canonical_json(&json!(["caf\u{e9}"])).is_err());
    assert!(legacy::canonical_json(&json!({"caf\u{e9}": 1})).is_err());
}

#[test]
fn changed_legacy_case_fails() -> Result<(), String> {
    let reference = legacy_doc()?
        .pointer("/legacy_references")
        .and_then(Value::as_array)
        .and_then(|refs| {
            refs.iter()
                .find(|r| r.get("section").and_then(Value::as_str) == Some("cases"))
                .cloned()
        })
        .ok_or("no golden_lease reference")?;
    let case = legacy::referenced_case(&reference)?;
    let nonce = case
        .pointer("/salts/0/nonce")
        .and_then(Value::as_str)
        .ok_or("no nonce")?;
    let changed = mutate(&case, "/salts/0/nonce", json!(flip_last_hex_digit(nonce)));
    if legacy::case_blake3(&changed)? == str_field(&reference, "case_blake3")? {
        return Err("a changed case kept its hash".to_owned());
    }
    expect_err(&legacy::replay(&reference, &changed), "a changed nonce")
}

#[test]
fn changed_legacy_hash_fails() -> Result<(), String> {
    let doc = mutate(
        &legacy_doc()?,
        "/legacy_references/0/case_blake3",
        json!("0".repeat(64)),
    );
    expect_err(&check_legacy(&doc), "a wrong case hash")
}

#[test]
fn whole_core_fixture_passes() -> Result<(), String> {
    check_core(&core()?)
}

#[test]
fn swapped_admission_order_fails() -> Result<(), String> {
    let doc = swap_pointers(
        &core()?,
        "/lease_admission_order/0",
        "/lease_admission_order/1",
    )?;
    expect_err(&check_orders(&doc), "a swapped admission order")
}

#[test]
fn changed_code_row_fails() -> Result<(), String> {
    let doc = mutate(&core()?, "/codes/12/wire_admission", json!("MALFORMED"));
    expect_err(&check_codes(&doc), "a changed wire mapping")
}

#[test]
fn changed_wire_reason_number_fails() -> Result<(), String> {
    let doc = mutate(&core()?, "/wire_reasons/2/value", json!(3));
    expect_err(&check_codes(&doc), "a changed wire number")
}

#[test]
fn changed_expected_step_fails() -> Result<(), String> {
    let doc = mutate(
        &core()?,
        "/lease_admission/0/expected_step",
        json!("code:1"),
    );
    expect_err(&check_lease_admission(&doc), "a changed expected step")
}

#[test]
fn changed_support_claim_fails() -> Result<(), String> {
    let index = find_index(&core()?, "family_support", "duplicate-family-id")?;
    let pointer = format!("/family_support/{index}/entry_claims/0");
    let doc = mutate(&core()?, &pointer, json!(true));
    expect_err(&check_family_support(&doc), "a changed claim")
}

#[test]
fn zero_limit_is_no_claim() {
    let bounds = decisions::example_size_bounds(2, 100, 150, 4);
    assert_eq!(
        decisions::size_bound_code(bounds, Some(0), Some(0), Some(0)),
        None
    );
    assert_eq!(
        decisions::size_bound_code(bounds, None, None, Some(4)),
        Some(13)
    );
}

#[test]
fn size_bound_overflow_is_code_thirteen() {
    assert_eq!(decisions::example_size_bounds(u64::MAX, 2, 0, 0), None);
    assert_eq!(decisions::size_bound_code(None, None, None, None), Some(13));
}

#[test]
fn unlisted_evidence_id_fails() -> Result<(), String> {
    let doc = mutate(&core()?, "/evidence/provided/0", json!("no-such-case"));
    expect_err(
        &check_evidence(&doc, &[&doc, &legacy_doc()?]),
        "an id with no case",
    )
}

#[test]
fn unknown_field_length_field_is_an_error() {
    assert!(decisions::field_length_code("Nope.field", 1, None).is_err());
}

#[test]
fn unknown_fault_is_an_error() {
    assert!(decisions::winner(&decisions::EXPLICIT_JOB_ORDER, &["code:1".to_owned()]).is_err());
}
