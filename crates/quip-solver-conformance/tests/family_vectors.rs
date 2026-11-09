//! Family golden vectors. Each check reads inputs from `core.json` or `legacy.json`,
//! recomputes the outputs with Rust code, and compares them with the stored values.
//! The checks cover parameters, digests, streams, leases, instances, decisions, size
//! bounds, evidence, and the pinned legacy references. A fixture schema check and
//! mutation tests show that each check rejects a changed fixture. Python is never run.

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

/// The nodes and edges of the topology file that `case["topology"]` names.
fn topology_inputs(case: &Value) -> Result<Topology, String> {
    let reference = str_field(case, "topology")?;
    let stored = quip_solver_conformance::topology(reference)
        .map_err(|e| format!("{}: {e}", case_name(case)))?;
    Ok((stored.nodes, stored.edges))
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
            let nonce = ising_generators::random::derive_nonce(
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
    check_size_bounds(doc)?;
    check_evidence(doc, &[doc, &legacy_doc()?])
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
fn t_sparse_loads_from_the_shared_blob() -> Result<(), String> {
    let stored = quip_solver_conformance::t_sparse().map_err(|e| e.to_string())?;
    let edges = [
        (907, 41),
        (41, 0),
        (0, u32::MAX),
        (u32::MAX, 12),
        (12, 3),
        (3, 907),
        (41, 12),
    ];
    if stored.nodes != [907, 41, 0, u32::MAX, 12, 3] || stored.edges != edges {
        return Err(format!("unexpected T_SPARSE: {stored:?}"));
    }
    Ok(())
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

/// Point `sparse` at the file of another topology case, as a changed fixture would.
fn repoint_sparse(other: &str) -> Result<Value, String> {
    let doc = core()?;
    let i = find_index(&doc, "topologies", "sparse")?;
    let j = find_index(&doc, "topologies", other)?;
    let reference = doc
        .pointer(&format!("/topologies/{j}/topology"))
        .cloned()
        .ok_or("no topology reference")?;
    Ok(mutate(
        &doc,
        &format!("/topologies/{i}/topology"),
        reference,
    ))
}

#[test]
fn reordered_topology_nodes_fails() -> Result<(), String> {
    let broken = repoint_sparse("sparse-nodes-reordered")?;
    expect_err(&check_topologies(&broken), "reordered topology nodes")
}

#[test]
fn swapped_topology_edges_fails() -> Result<(), String> {
    let broken = repoint_sparse("sparse-edges-reordered")?;
    expect_err(&check_topologies(&broken), "swapped topology edges")
}

#[test]
fn a_missing_topology_file_fails() -> Result<(), String> {
    let doc = core()?;
    let i = find_index(&doc, "topologies", "sparse")?;
    let broken = mutate(
        &doc,
        &format!("/topologies/{i}/topology"),
        json!("../topologies/absent.bin.gz"),
    );
    expect_err(&check_topologies(&broken), "a missing topology file")?;
    let outside = mutate(
        &doc,
        &format!("/topologies/{i}/topology"),
        json!("../families/core.json"),
    );
    expect_err(
        &check_topologies(&outside),
        "a reference outside topologies",
    )
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

/// One schema row: tag, name, type, scale, min, max.
type SchemaRow = (
    u64,
    &'static str,
    &'static str,
    Option<u64>,
    &'static str,
    &'static str,
);

const UNIFORM_FIELD_ROWS: [SchemaRow; 2] = [
    (1, "n_layers", "u16", Some(1), "1", "16"),
    (2, "field_milli", "i32", Some(1000), "-4000", "4000"),
];

const ALL_TYPES_ROWS: [SchemaRow; 9] = [
    (1, "a_u8", "u8", Some(1), "0", "255"),
    (2, "a_u16", "u16", Some(1), "0", "65535"),
    (3, "a_u32", "u32", Some(1), "0", "4294967295"),
    (4, "a_u64", "u64", Some(1), "1", "18446744073709551615"),
    (5, "a_i32", "i32", Some(1000), "-2147483648", "2147483647"),
    (
        6,
        "a_i64",
        "i64",
        Some(1),
        "-72057594037927936",
        "72057594037927936",
    ),
    (7, "a_bool", "bool", None, "0", "1"),
    (8, "a_bytes", "bytes", None, "0", "4096"),
    (9, "a_ascii", "ascii", None, "0", "64"),
];

fn check_schema_rows(doc: &Value, family: &str, rows: &[SchemaRow]) -> Result<(), String> {
    let entry = section(doc, "schemas")?
        .iter()
        .find(|s| {
            s.get("family_id").and_then(Value::as_str) == Some(family)
                && s.get("revision").and_then(Value::as_u64) == Some(1)
        })
        .ok_or_else(|| format!("no schema for {family} rev 1"))?;
    let fields = entry
        .get("fields")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{family}: no fields array"))?;
    if fields.len() != rows.len() {
        return Err(format!(
            "{family}: {} fields, spec has {}",
            fields.len(),
            rows.len()
        ));
    }
    for (field, (tag, name, kind, scale, min, max)) in fields.iter().zip(rows) {
        let scale_value = scale.map_or(Value::Null, |s| json!(s));
        let want = json!({
            "tag": tag, "name": name, "type": kind,
            "scale": scale_value, "min": min, "max": max,
        });
        for key in ["tag", "name", "type", "scale", "min", "max"] {
            compare(
                field,
                &format!("{family} {key}"),
                field.get(key).unwrap_or(&Value::Null),
                want.get(key).unwrap_or(&Value::Null),
            )?;
        }
    }
    Ok(())
}

#[test]
fn schemas_match_spec() -> Result<(), String> {
    let doc = core()?;
    check_schema_rows(&doc, "example/uniform-field", &UNIFORM_FIELD_ROWS)?;
    check_schema_rows(&doc, "example/all-types", &ALL_TYPES_ROWS)?;
    let case = find_named(&doc, "params", "spec-valid")?;
    compare(
        case,
        "spec literal params_hex",
        &str_field(case, "params_hex")?,
        &"01010002000000020002000400000024faffff",
    )
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

// ---- layered test construction (layered.json) ----

fn layered_doc() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_LAYERED).map_err(|e| e.to_string())
}

fn object_field<'a>(case: &'a Value, key: &str) -> Result<&'a Value, String> {
    case.get(key)
        .filter(|v| v.is_object())
        .ok_or_else(|| format!("{}: missing object field {key}", case_name(case)))
}

/// The largest vertex degree of a topology in plain node ids.
fn topology_max_degree(nodes: &[u32], edges: &[(u32, u32)]) -> u64 {
    nodes
        .iter()
        .map(|node| {
            edges
                .iter()
                .map(|&(u, v)| u64::from(u == *node) + u64::from(v == *node))
                .sum::<u64>()
        })
        .max()
        .unwrap_or(0)
}

/// `n_layers` of the uniform-field parameters: format byte, then tag 1 with a
/// 4-byte length, then the `u16` value.
fn layered_n_layers(case: &Value, params: &[u8]) -> Result<u64, String> {
    let raw: [u8; 2] = params
        .get(7..9)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| format!("{}: params too short for n_layers", case_name(case)))?;
    Ok(u64::from(u16::from_le_bytes(raw)))
}

/// Recompute one generated instance with Rust code that does not call the
/// construction, and compare each stored value.
fn check_layered_instance(
    case: &Value,
    entry: &Value,
    descriptor: [u8; 32],
    order: [u8; 32],
    kinds: &[u16],
    bound: i64,
) -> Result<(), String> {
    let parts = instance::InstanceParts {
        descriptor_digest: descriptor,
        topology_order_digest: order,
        nonce: codec::hex_decode(str_field(entry, "nonce")?)?,
        node_ids: u32_list(entry.get("node_ids").ok_or("no node_ids")?, "node_ids")?,
        edges: edge_list(entry.get("edges").ok_or("no edges")?)?,
        h: i32_list(entry, "h")?,
        j: i32_list(entry, "j")?,
        offset_milli: i64_decimal(entry, "offset_milli")?,
        metadata: codec::hex_decode(str_field(entry, "metadata_hex")?)?,
    };
    compare(
        case,
        "edge fault",
        &instance::edge_fault(&parts.edges, parts.node_ids.len()),
        &None,
    )?;
    compare(
        case,
        "metadata",
        &instance::metadata_code(&parts.metadata, kinds),
        &None,
    )?;
    compare(
        case,
        "offset bound",
        &instance::offset_code(parts.offset_milli),
        &None,
    )?;
    for value in parts.h.iter().chain(&parts.j) {
        compare(
            case,
            "coefficient bound",
            &(i64::from(*value).abs() <= bound),
            &true,
        )?;
    }
    let canonical = instance::canonical_bytes(&parts)?;
    compare(
        case,
        "canonical_hex",
        &codec::hex_encode(&canonical),
        &str_field(entry, "canonical_hex")?.to_owned(),
    )?;
    compare(
        case,
        "instance_digest",
        &codec::hex_encode(&digests::instance_digest(&canonical)),
        &str_field(entry, "instance_digest")?.to_owned(),
    )?;
    let all_up = vec![1_i8; parts.node_ids.len()];
    compare(
        case,
        "energy_all_up_milli",
        &instance::energy_with_offset(&parts, &all_up)?,
        &i64_decimal(entry, "energy_all_up_milli")?,
    )?;
    Ok(())
}

/// The size bounds a family states for a case: `(nodes, edges, degree)`, or
/// `None` when the family gives no bounds.
type SizeBoundsOf =
    fn(&Value, &[u8], &[u32], &[(u32, u32)]) -> Result<Option<(u64, u64, u64)>, String>;

/// The bounds of the layered construction: `n_layers` copies of the topology.
fn layered_size_bounds(
    case: &Value,
    params: &[u8],
    nodes: &[u32],
    edges: &[(u32, u32)],
) -> Result<Option<(u64, u64, u64)>, String> {
    Ok(decisions::example_size_bounds(
        layered_n_layers(case, params)?,
        u64::try_from(nodes.len()).map_err(|e| e.to_string())?,
        u64::try_from(edges.len()).map_err(|e| e.to_string())?,
        topology_max_degree(nodes, edges),
    ))
}

/// The bounds of `quip/ice-glass` and `quip/twisted-ice-dimer`: the topology
/// itself.
fn ice_glass_size_bounds(
    _: &Value,
    _: &[u8],
    nodes: &[u32],
    edges: &[(u32, u32)],
) -> Result<Option<(u64, u64, u64)>, String> {
    Ok(Some((
        u64::try_from(nodes.len()).map_err(|e| e.to_string())?,
        u64::try_from(edges.len()).map_err(|e| e.to_string())?,
        topology_max_degree(nodes, edges),
    )))
}

/// Recompute the digests and size bounds of a case. Returns the descriptor
/// digest and the topology order digest.
fn check_layered_identity(
    case: &Value,
    bounds: SizeBoundsOf,
) -> Result<([u8; 32], [u8; 32]), String> {
    let family = str_field(case, "family_id")?;
    let params = codec::hex_decode(str_field(case, "params_hex")?)?;
    let params_digest = digests::params_digest(&params);
    compare(
        case,
        "params_digest",
        &codec::hex_encode(&params_digest),
        &str_field(case, "params_digest")?.to_owned(),
    )?;
    let descriptor = digests::descriptor_digest(
        family.as_bytes(),
        u32_field(case, "revision")?,
        &params_digest,
    );
    compare(
        case,
        "descriptor_digest",
        &codec::hex_encode(&descriptor),
        &str_field(case, "descriptor_digest")?.to_owned(),
    )?;
    let topology = object_field(case, "topology")?;
    let (nodes, edges) = topology_inputs(topology)?;
    let order = digests::topology_order_digest(&nodes, &edges);
    compare(
        case,
        "order_digest",
        &codec::hex_encode(&order),
        &str_field(topology, "order_digest")?.to_owned(),
    )?;
    let sizes = object_field(case, "size_bounds")?;
    let want_bounds = bounds(case, &params, &nodes, &edges)?;
    compare(
        case,
        "size_bounds",
        &want_bounds,
        &Some((
            u64_decimal(sizes, "nodes")?,
            u64_decimal(sizes, "edges")?,
            u64_decimal(sizes, "degree")?,
        )),
    )?;
    Ok((descriptor, order))
}

fn check_layered_case(case: &Value, bounds: SizeBoundsOf) -> Result<(), String> {
    use quip_protocol::lease::{Generator, LeaseSpec};
    let (descriptor, order) = check_layered_identity(case, bounds)?;
    let lease = object_field(case, "lease")?;
    let spec = LeaseSpec::new(
        Generator::Blake3Chacha8V1,
        codec::hex32(str_field(lease, "last_proof_block_hash")?)?,
        codec::hex32(str_field(lease, "miner_account")?)?,
        codec::hex32(str_field(lease, "base_salt")?)?,
        u64_decimal(lease, "salt_start")?,
        u64_decimal(lease, "salt_count")?,
    )
    .map_err(|e| format!("{}: lease: {e:?}", case_name(case)))?;
    let nonce_of = |entry: &Value| -> Result<String, String> {
        let slot = u64_decimal(entry, "index")?;
        let salt = spec.salt(slot).ok_or("slot outside the lease")?;
        Ok(codec::hex_encode(&ising_generators::random::derive_nonce(
            spec.last_proof_block_hash,
            spec.miner_account,
            salt,
        )))
    };
    let kinds = contract_kinds(case, "metadata_kinds")?;
    let bound = case
        .get("coefficient_bound_milli")
        .and_then(Value::as_i64)
        .ok_or("no coefficient_bound_milli")?;
    let mut slots = Vec::new();
    for entry in entries_of(case, "skipped")? {
        let code = entry.get("skip_code").and_then(Value::as_u64);
        compare(
            case,
            "skip_code is 13 or 14",
            &matches!(code, Some(13 | 14)),
            &true,
        )?;
        compare(
            case,
            "skipped nonce",
            &nonce_of(entry)?,
            &str_field(entry, "nonce")?.to_owned(),
        )?;
        slots.push(u64_decimal(entry, "index")?);
    }
    for entry in entries_of(case, "instances")? {
        compare(
            case,
            "instance nonce",
            &nonce_of(entry)?,
            &str_field(entry, "nonce")?.to_owned(),
        )?;
        check_layered_instance(case, entry, descriptor, order, &kinds, bound)?;
        slots.push(u64_decimal(entry, "index")?);
    }
    slots.sort_unstable();
    compare(
        case,
        "skipped and generated slots",
        &slots,
        &(0..spec.salt_count).collect::<Vec<u64>>(),
    )
}

fn check_layered(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    check_names_unique(doc)?;
    for case in section(doc, "cases")? {
        check_layered_case(case, layered_size_bounds)?;
    }
    Ok(())
}

/// Generate every slot of one case with the Rust construction and compare it
/// with the Python port.
fn check_layered_against_rust(case: &Value) -> Result<(), String> {
    use ising_generators::descriptor::{admit, params_digest};
    use ising_generators::family::{admit_topology, generate_instance, BaseTopology, SaltFault};
    use ising_generators::ice::{CHARGED_ICE_V1, ICE_FIELDS_V1, ICE_GLASS_V1, TWISTED_DIMER_V1};
    use ising_generators::polariton::POLARITON_V1;
    use ising_generators::routing::ROUTING_V1;
    use quip_solver_conformance::families::layered::{RESTRICTED_V1, UNIFORM_V1};
    let family = str_field(case, "family_id")?;
    let contract = match family {
        "vectors/uniform-field" => UNIFORM_V1,
        "vectors/restricted-field" => RESTRICTED_V1,
        "quip/ice-glass" => ICE_GLASS_V1,
        "quip/twisted-ice-dimer" => TWISTED_DIMER_V1,
        "quip/charged-ice" => CHARGED_ICE_V1,
        "quip/ice-fields" => ICE_FIELDS_V1,
        "quip/two-commodity-routing" => ROUTING_V1,
        "quip/polariton-static" => POLARITON_V1,
        other => return Err(format!("{}: unknown family {other}", case_name(case))),
    };
    let params = codec::hex_decode(str_field(case, "params_hex")?)?;
    let contracts = [contract];
    let admitted = admit(
        &contracts,
        family.as_bytes(),
        u32_field(case, "revision")?,
        &params,
        &params_digest(&params),
    )
    .map_err(|c| format!("{}: admit failed with {c}", case_name(case)))?;
    let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
    let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
    let bounds = admit_topology(
        &admitted,
        &topology,
        topology.order_digest(),
        &ising_generators::descriptor::AdvertisedLimits {
            max_nodes: 0,
            max_edges: 0,
            max_degree: 0,
        },
    )
    .map_err(|c| format!("{}: admit_topology failed with {c}", case_name(case)))?;
    let mut slots = Vec::new();
    for entry in entries_of(case, "skipped")? {
        let nonce = codec::hex32(str_field(entry, "nonce")?)?;
        let want = entry
            .get("skip_code")
            .and_then(Value::as_u64)
            .and_then(|n| u8::try_from(n).ok())
            .ok_or("bad skip_code")?;
        match generate_instance(&admitted, &topology, &bounds, nonce) {
            Err(SaltFault::Skip(code)) => compare(case, "skip code", &code.value(), &want)?,
            other => return Err(format!("{}: skipped slot gave {other:?}", case_name(case))),
        }
        slots.push(u64_decimal(entry, "index")?);
    }
    for entry in entries_of(case, "instances")? {
        let nonce = codec::hex32(str_field(entry, "nonce")?)?;
        let built = generate_instance(&admitted, &topology, &bounds, nonce)
            .map_err(|fault| format!("{}: generation gave {fault:?}", case_name(case)))?;
        let canonical = built.canonical_bytes().map_err(|e| e.to_string())?;
        compare(
            case,
            "canonical bytes",
            &codec::hex_encode(&canonical),
            &str_field(entry, "canonical_hex")?.to_owned(),
        )?;
        compare(
            case,
            "instance digest",
            &codec::hex_encode(&built.digest()),
            &str_field(entry, "instance_digest")?.to_owned(),
        )?;
        slots.push(u64_decimal(entry, "index")?);
    }
    slots.sort_unstable();
    let count = u64_decimal(object_field(case, "lease")?, "salt_count")?;
    compare(case, "slots", &slots, &(0..count).collect::<Vec<u64>>())
}

#[test]
fn layered_fixture_matches_schema() -> Result<(), String> {
    validate(&schema()?, &layered_doc()?)?;
    match layered_doc()?.get("provenance").and_then(Value::as_str) {
        Some("test-construction") => {}
        other => {
            return Err(format!(
                "provenance is {other:?}, expected test-construction"
            ))
        }
    }
    check_names_unique(&layered_doc()?)
}

#[test]
fn layered_vectors_recompute() -> Result<(), String> {
    check_layered(&layered_doc()?)
}

#[test]
fn layered_vectors_match_the_rust_construction() -> Result<(), String> {
    for case in section(&layered_doc()?, "cases")? {
        check_layered_against_rust(case)?;
    }
    Ok(())
}

#[test]
fn uniform_p_spec_skips_match_the_session_capture() -> Result<(), String> {
    let doc = layered_doc()?;
    let case = find_named(&doc, "cases", "uniform-p-spec-sparse")?;
    let mut skipped = Vec::new();
    for entry in entries_of(case, "skipped")? {
        skipped.push(u64_decimal(entry, "index")?);
    }
    skipped.sort_unstable();
    compare(case, "skipped slots", &skipped, &vec![0, 3, 9, 13])
}

#[test]
fn changed_layered_coupling_fails() -> Result<(), String> {
    let doc = layered_doc()?;
    let pointer = "/cases/0/instances/0/j/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or("no coupling")?;
    let broken = mutate(&doc, pointer, json!(was + 1000));
    expect_err(&check_layered(&broken), "a changed coupling")
}

// ---- family registry (registry.json) ----

fn registry_doc() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_REGISTRY).map_err(|e| e.to_string())
}

/// The cases of one revision in its vectors file, read from the repository
/// root at run time, in file order.
fn registered_cases(
    entry: &quip_solver_conformance::families::registry::RegistryEntry,
) -> Result<Vec<Value>, String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join(&entry.vectors))
        .map_err(|e| format!("{}: {e}", entry.vectors))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut cases = Vec::new();
    for case in section(&doc, "cases")? {
        if str_field(case, "family_id")? == entry.family_id
            && u32_field(case, "revision")? == entry.revision
        {
            cases.push(case.clone());
        }
    }
    Ok(cases)
}

/// FAM-08: the BLAKE3 of the canonical JSON of the cases equals the pin.
fn check_pin(
    entry: &quip_solver_conformance::families::registry::RegistryEntry,
    cases: &[Value],
) -> Result<(), String> {
    let got = legacy::case_blake3(&Value::Array(cases.to_vec()))?;
    let want = codec::hex_encode(&entry.vectors_blake3);
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "{}@{}: vectors changed, pin {want}, found {got}",
            entry.family_id, entry.revision
        ))
    }
}

#[test]
fn registry_fixture_matches_schema() -> Result<(), String> {
    let doc = registry_doc()?;
    validate(&schema()?, &doc)?;
    match doc.get("provenance").and_then(Value::as_str) {
        Some("specified") => {}
        other => return Err(format!("provenance is {other:?}, expected specified")),
    }
    if doc.get("sources") != Some(&json!([])) {
        return Err("sources must be empty: the registry is written by hand".to_owned());
    }
    check_names_unique(&doc)
}

#[test]
fn registered_vectors_are_frozen() -> Result<(), String> {
    let registry = quip_solver_conformance::families::registry::parse_registry(
        quip_solver_conformance::FAMILY_REGISTRY,
    )?;
    if registry.is_empty() {
        return Err("empty registry".to_owned());
    }
    for entry in &registry {
        check_pin(entry, &registered_cases(entry)?)?;
    }
    Ok(())
}

#[test]
fn changed_registered_case_fails_the_pin() -> Result<(), String> {
    let registry = quip_solver_conformance::families::registry::parse_registry(
        quip_solver_conformance::FAMILY_REGISTRY,
    )?;
    if registry.is_empty() {
        return Err("empty registry".to_owned());
    }
    for entry in &registry {
        let mut cases = registered_cases(entry)?;
        let pointer = "/instances/0/j/0";
        let first = cases.first().ok_or("no cases")?.clone();
        let was = first
            .pointer(pointer)
            .and_then(Value::as_i64)
            .ok_or("no coupling")?;
        let slot = cases.first_mut().ok_or("no cases")?;
        *slot = mutate(&first, pointer, json!(was + 1000));
        expect_err(&check_pin(entry, &cases), "a changed registered case")?;
    }
    Ok(())
}

// ---- ice structure vectors (ice_structure.json) ----

#[test]
fn ice_structure_fixture_matches_schema() -> Result<(), String> {
    let doc: Value = serde_json::from_str(include_str!("../vectors/families/ice_structure.json"))
        .map_err(|e| e.to_string())?;
    validate(&schema()?, &doc)?;
    match doc.get("provenance").and_then(Value::as_str) {
        Some("specified") => {}
        other => return Err(format!("provenance is {other:?}, expected specified")),
    }
    check_names_unique(&doc)?;
    section(&doc, "structure").map(|_| ())?;
    section(&doc, "live").map(|_| ())?;
    section(&doc, "cases").map(|_| ())?;
    Ok(())
}

fn ice_glass_doc() -> Result<Value, String> {
    serde_json::from_str(include_str!("../vectors/families/ice_structure.json"))
        .map_err(|e| e.to_string())
}

/// Recompute the digests, nonces, and instance bytes of every registered
/// `quip/ice-glass` case with Rust code that does not call the construction.
fn check_ice_glass(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    for case in section(doc, "cases")? {
        check_layered_case(case, ice_glass_size_bounds)?;
    }
    Ok(())
}

#[test]
fn ice_glass_cases_recompute() -> Result<(), String> {
    check_ice_glass(&ice_glass_doc()?)
}

#[test]
fn ice_glass_cases_match_the_rust_construction() -> Result<(), String> {
    for case in section(&ice_glass_doc()?, "cases")? {
        check_layered_against_rust(case)?;
    }
    Ok(())
}

#[test]
fn ice_glass_params_constants_match_the_vectors() -> Result<(), String> {
    use quip_solver_conformance::families::ice::{P_ICE_ADJ_125, P_ICE_Z12_200};
    let doc = ice_glass_doc()?;
    let cases = section(&doc, "cases")?;
    for (name, want) in [
        ("ice-glass-adjacency-z12", P_ICE_ADJ_125),
        ("ice-glass-zephyr-z12", P_ICE_Z12_200),
    ] {
        let case = find_named(&doc, "cases", name)?;
        compare(
            case,
            "params_hex",
            &str_field(case, "params_hex")?.to_owned(),
            &codec::hex_encode(&want),
        )?;
    }
    compare(&doc, "case count", &cases.len(), &4)
}

#[test]
fn a_changed_ice_glass_coupling_fails_the_recompute() -> Result<(), String> {
    let doc = ice_glass_doc()?;
    let pointer = "/cases/0/instances/0/j/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or("no coupling")?;
    let broken = mutate(&doc, pointer, json!(was + 1000));
    expect_err(&check_ice_glass(&broken), "a changed coupling")
}

// ---- twisted ice dimer vectors (twisted_dimer.json) ----

fn twisted_doc() -> Result<Value, String> {
    serde_json::from_str(include_str!("../vectors/families/twisted_dimer.json"))
        .map_err(|e| e.to_string())
}

/// Recompute the digests, nonces, and instance bytes of every registered
/// `quip/twisted-ice-dimer` case with Rust code that does not call the
/// construction.
fn check_twisted(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    check_names_unique(doc)?;
    for case in section(doc, "cases")? {
        check_layered_case(case, ice_glass_size_bounds)?;
    }
    Ok(())
}

#[test]
fn twisted_fixture_matches_schema() -> Result<(), String> {
    let doc = twisted_doc()?;
    validate(&schema()?, &doc)?;
    match doc.get("provenance").and_then(Value::as_str) {
        Some("specified") => {}
        other => return Err(format!("provenance is {other:?}, expected specified")),
    }
    check_names_unique(&doc)?;
    for name in ["cases", "recipes", "obstructions", "admission"] {
        section(&doc, name).map(|_| ())?;
    }
    Ok(())
}

#[test]
fn twisted_cases_recompute() -> Result<(), String> {
    check_twisted(&twisted_doc()?)
}

#[test]
fn twisted_cases_match_the_rust_construction() -> Result<(), String> {
    for case in section(&twisted_doc()?, "cases")? {
        check_layered_against_rust(case)?;
    }
    Ok(())
}

#[test]
fn twisted_params_constants_match_the_vectors() -> Result<(), String> {
    use quip_solver_conformance::families::twisted::{
        P_TWISTED_ADJ_W45_P6, P_TWISTED_W1000, P_TWISTED_Z22_W62_P8,
    };
    let doc = twisted_doc()?;
    for (section_name, name, want) in [
        ("cases", "twisted-dimer-zephyr-z22", P_TWISTED_Z22_W62_P8),
        (
            "cases",
            "twisted-dimer-adjacency-z22-sparse",
            P_TWISTED_ADJ_W45_P6,
        ),
        (
            "admission",
            "weak-milli-1000-is-out-of-bounds",
            P_TWISTED_W1000,
        ),
    ] {
        let case = find_named(&doc, section_name, name)?;
        compare(
            case,
            "params_hex",
            &str_field(case, "params_hex")?.to_owned(),
            &codec::hex_encode(&want),
        )?;
    }
    compare(&doc, "case count", &section(&doc, "cases")?.len(), &5)
}

#[test]
fn the_twisted_shortfall_case_skips_with_code_14() -> Result<(), String> {
    let doc = twisted_doc()?;
    let case = find_named(&doc, "cases", "twisted-dimer-shortfall")?;
    let skipped = entries_of(case, "skipped")?;
    compare(case, "a skipped salt exists", &skipped.is_empty(), &false)?;
    for entry in skipped {
        compare(
            case,
            "skip code",
            &entry.get("skip_code"),
            &Some(&json!(14)),
        )?;
    }
    Ok(())
}

/// The nonce of one lease slot, derived from the lease fields.
fn lease_nonce(case: &Value, slot: u64) -> Result<String, String> {
    use quip_protocol::lease::{Generator, LeaseSpec};
    let lease = object_field(case, "lease")?;
    let spec = LeaseSpec::new(
        Generator::Blake3Chacha8V1,
        codec::hex32(str_field(lease, "last_proof_block_hash")?)?,
        codec::hex32(str_field(lease, "miner_account")?)?,
        codec::hex32(str_field(lease, "base_salt")?)?,
        u64_decimal(lease, "salt_start")?,
        u64_decimal(lease, "salt_count")?,
    )
    .map_err(|e| format!("{}: lease: {e:?}", case_name(case)))?;
    let salt = spec.salt(slot).ok_or("slot outside the lease")?;
    Ok(codec::hex_encode(&ising_generators::random::derive_nonce(
        spec.last_proof_block_hash,
        spec.miner_account,
        salt,
    )))
}

/// The pair count of a generated instance, read from its kind 3 section.
fn pair_count_of(metadata: &[u8]) -> Option<u32> {
    let mut rest = metadata.get(2..)?;
    while let (Some(kind), Some(len)) = (rest.get(0..2), rest.get(2..6)) {
        let len = usize::try_from(u32::from_le_bytes(len.try_into().ok()?)).ok()?;
        let payload = rest.get(6..6_usize.checked_add(len)?)?;
        if kind == [3, 0] {
            return Some(u32::from_le_bytes(payload.get(0..4)?.try_into().ok()?));
        }
        rest = rest.get(6_usize.checked_add(len)?..)?;
    }
    None
}

/// The obstruction values of the Rust diagnostic as a JSON object.
fn obstruction_json(found: ising_generators::ice::Obstructions) -> Value {
    json!({
        "components": found.components,
        "cycle_rank": found.cycle_rank,
        "frustrated_cycles": found.frustrated_cycles,
        "gauge_equivalent": found.gauge_equivalent,
    })
}

/// Rebuild every salt of one recipe with the Rust construction and compare
/// the hash, the counts, and the obstruction values.
fn check_twisted_recipe(recipe: &Value) -> Result<(), String> {
    use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits};
    use ising_generators::family::{admit_topology, generate_instance, BaseTopology};
    use ising_generators::ice::{gauge_obstructions, local_gauges, TWISTED_DIMER_V1};
    let params = codec::hex_decode(str_field(recipe, "params_hex")?)?;
    compare(
        recipe,
        "params_digest",
        &codec::hex_encode(&params_digest(&params)),
        &str_field(recipe, "params_digest")?.to_owned(),
    )?;
    let contracts = [TWISTED_DIMER_V1];
    let admitted = admit(
        &contracts,
        str_field(recipe, "family_id")?.as_bytes(),
        u32_field(recipe, "revision")?,
        &params,
        &params_digest(&params),
    )
    .map_err(|c| format!("{}: admit failed with {c}", case_name(recipe)))?;
    let (nodes, edges) = topology_inputs(object_field(recipe, "topology")?)?;
    let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
    compare(
        recipe,
        "order_digest",
        &codec::hex_encode(topology.order_digest()),
        &str_field(object_field(recipe, "topology")?, "order_digest")?.to_owned(),
    )?;
    let bounds = admit_topology(
        &admitted,
        &topology,
        topology.order_digest(),
        &AdvertisedLimits {
            max_nodes: 0,
            max_edges: 0,
            max_degree: 0,
        },
    )
    .map_err(|c| format!("{}: admit_topology failed with {c}", case_name(recipe)))?;
    let salts = entries_of(recipe, "salts")?;
    compare(
        recipe,
        "salts",
        &u64::try_from(salts.len()).map_err(|e| e.to_string())?,
        &u64_decimal(object_field(recipe, "lease")?, "salt_count")?,
    )?;
    for salt in salts {
        compare(
            recipe,
            "nonce",
            &lease_nonce(recipe, u64_decimal(salt, "index")?)?,
            &str_field(salt, "nonce")?.to_owned(),
        )?;
        let nonce = codec::hex32(str_field(salt, "nonce")?)?;
        let built = generate_instance(&admitted, &topology, &bounds, nonce)
            .map_err(|fault| format!("{}: generation gave {fault:?}", case_name(recipe)))?;
        let canonical = built.canonical_bytes().map_err(|e| e.to_string())?;
        compare(
            recipe,
            "canonical_blake3",
            &blake3::hash(&canonical).to_hex().to_string(),
            &str_field(salt, "canonical_blake3")?.to_owned(),
        )?;
        let (tiles, gauges) = local_gauges(&built).ok_or("no local gauges")?;
        compare(
            recipe,
            "tiles",
            &u64::try_from(tiles.len()).map_err(|e| e.to_string())?,
            &salt
                .get("tiles")
                .and_then(Value::as_u64)
                .ok_or("no tiles")?,
        )?;
        compare(
            recipe,
            "pairs",
            &pair_count_of(built.metadata()).map(u64::from),
            &salt.get("pairs").and_then(Value::as_u64),
        )?;
        let found = gauge_obstructions(&tiles, &gauges, built.node_ids().len())
            .map_err(|e| format!("{}: obstructions: {e:?}", case_name(recipe)))?;
        compare(
            recipe,
            "obstructions",
            &obstruction_json(found),
            salt.get("obstructions").ok_or("no obstructions")?,
        )?;
    }
    Ok(())
}

#[test]
fn twisted_recipes_match_the_rust_construction() -> Result<(), String> {
    let doc = twisted_doc()?;
    for recipe in section(&doc, "recipes")? {
        check_twisted_recipe(recipe)?;
    }
    Ok(())
}

#[test]
fn twisted_recipes_match_the_rust_constants() -> Result<(), String> {
    use ising_generators::descriptor::params_digest;
    use ising_generators::ice::RECIPES;
    let doc = twisted_doc()?;
    let recipes = section(&doc, "recipes")?;
    compare(&doc, "recipe count", &recipes.len(), &RECIPES.len())?;
    for (recipe, want) in recipes.iter().zip(RECIPES) {
        compare(recipe, "alias", &str_field(recipe, "alias")?, &want.alias)?;
        compare(recipe, "cell", &str_field(recipe, "cell")?, &want.cell)?;
        compare(
            recipe,
            "params_hex",
            &str_field(recipe, "params_hex")?.to_owned(),
            &codec::hex_encode(want.params),
        )?;
        compare(
            recipe,
            "params_digest",
            &str_field(recipe, "params_digest")?.to_owned(),
            &codec::hex_encode(&params_digest(want.params)),
        )?;
    }
    Ok(())
}

/// An array of four-entry integer arrays, as rows of the diagnostic input.
fn rows<T: TryFrom<i64>>(value: Option<&Value>) -> Result<Vec<[T; 4]>, String> {
    let bad = || "a row that is not four integers".to_owned();
    value
        .and_then(Value::as_array)
        .ok_or("rows is not an array")?
        .iter()
        .map(|row| {
            let ints: Vec<T> = row
                .as_array()
                .ok_or_else(bad)?
                .iter()
                .map(|v| v.as_i64().and_then(|n| T::try_from(n).ok()).ok_or_else(bad))
                .collect::<Result<_, _>>()?;
            ints.try_into().map_err(|_| bad())
        })
        .collect()
}

/// Run the Rust diagnostic on every obstruction case and compare.
fn check_obstructions(doc: &Value) -> Result<(), String> {
    use ising_generators::ice::{gauge_obstructions, StructureError};
    for case in section(doc, "obstructions")? {
        let tiles: Vec<[u32; 4]> = rows(case.get("tiles"))?;
        let gauges: Vec<[i8; 4]> = rows(case.get("gauges"))?;
        let n = case
            .get("n")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or("no n")?;
        match gauge_obstructions(&tiles, &gauges, n) {
            Ok(found) => compare(
                case,
                "obstructions",
                &Some(obstruction_json(found)),
                &case.get("expected").cloned(),
            )?,
            Err(error) => {
                let detail = match error {
                    StructureError::MembershipAboveTwo(position) => vec![position],
                    _ => vec![],
                };
                compare(
                    case,
                    "error",
                    &Some(error.name()),
                    &case.get("error").and_then(Value::as_str),
                )?;
                compare(
                    case,
                    "error_detail",
                    &Some(detail),
                    &case
                        .get("error_detail")
                        .map(|d| u32_list(d, "error_detail"))
                        .transpose()?,
                )?;
            }
        }
    }
    Ok(())
}

#[test]
fn twisted_obstructions_match_the_rust_diagnostic() -> Result<(), String> {
    check_obstructions(&twisted_doc()?)
}

#[test]
fn twisted_admission_codes_match_the_rust_admission() -> Result<(), String> {
    use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits};
    use ising_generators::family::{admit_topology, BaseTopology};
    use ising_generators::ice::TWISTED_DIMER_V1;
    let doc = twisted_doc()?;
    let contracts = [TWISTED_DIMER_V1];
    for case in section(&doc, "admission")? {
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
        let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
        let got = admit(
            &contracts,
            str_field(case, "family_id")?.as_bytes(),
            u32_field(case, "revision")?,
            &params,
            &params_digest(&params),
        )
        .and_then(|admitted| {
            admit_topology(
                &admitted,
                &topology,
                topology.order_digest(),
                &AdvertisedLimits {
                    max_nodes: 0,
                    max_edges: 0,
                    max_degree: 0,
                },
            )
        });
        let got = got.err().map(ising_generators::Code::value);
        compare(case, "admission code", &got, &expected_code(case)?)?;
    }
    Ok(())
}

#[test]
fn a_changed_twisted_coupling_fails_the_recompute() -> Result<(), String> {
    let doc = twisted_doc()?;
    let pointer = "/cases/0/instances/0/j/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or("no coupling")?;
    let broken = mutate(&doc, pointer, json!(was + 1000));
    expect_err(&check_twisted(&broken), "a changed coupling")
}

#[test]
fn a_changed_recipe_hash_fails_the_recompute() -> Result<(), String> {
    let doc = twisted_doc()?;
    let recipe = section(&doc, "recipes")?.first().ok_or("no recipe")?;
    let was = str_field(
        recipe.pointer("/salts/0").ok_or("no salt")?,
        "canonical_blake3",
    )?;
    let broken = mutate(
        &doc,
        "/recipes/0/salts/0/canonical_blake3",
        json!(flip_last_hex_digit(was)),
    );
    let recipe = section(&broken, "recipes")?.first().ok_or("no recipe")?;
    expect_err(&check_twisted_recipe(recipe), "a changed recipe hash")
}

#[test]
fn a_changed_obstruction_value_fails_the_check() -> Result<(), String> {
    let doc = twisted_doc()?;
    let pointer = "/obstructions/3/expected/frustrated_cycles";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or("no value")?;
    let broken = mutate(&doc, pointer, json!(was + 1));
    expect_err(&check_obstructions(&broken), "a changed obstruction value")
}

// ---- charged ice vectors (ice_charge.json) ----

fn charge_doc() -> Result<Value, String> {
    serde_json::from_str(include_str!("../vectors/families/ice_charge.json"))
        .map_err(|e| e.to_string())
}

/// Recompute the digests, nonces, and instance bytes of every registered
/// `quip/charged-ice` case with Rust code that does not call the construction.
fn check_charge(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    check_names_unique(doc)?;
    for case in section(doc, "cases")? {
        check_layered_case(case, ice_glass_size_bounds)?;
    }
    Ok(())
}

#[test]
fn charge_fixture_matches_schema() -> Result<(), String> {
    let doc = charge_doc()?;
    validate(&schema()?, &doc)?;
    match doc.get("provenance").and_then(Value::as_str) {
        Some("specified") => {}
        other => return Err(format!("provenance is {other:?}, expected specified")),
    }
    check_names_unique(&doc)?;
    for name in ["cases", "energies", "recipe", "admission"] {
        section(&doc, name).map(|_| ())?;
    }
    Ok(())
}

#[test]
fn charge_cases_recompute() -> Result<(), String> {
    check_charge(&charge_doc()?)
}

#[test]
fn charge_cases_match_the_rust_construction() -> Result<(), String> {
    for case in section(&charge_doc()?, "cases")? {
        check_layered_against_rust(case)?;
    }
    Ok(())
}

#[test]
fn charge_params_constants_match_the_vectors() -> Result<(), String> {
    use quip_solver_conformance::families::charge::{
        P_CHARGED_A251, P_CHARGED_ADJ_A100_B45_C256, P_CHARGED_Z22_A250_B31_C128,
    };
    let doc = charge_doc()?;
    for (section_name, name, want) in [
        (
            "cases",
            "charged-ice-zephyr-z22",
            P_CHARGED_Z22_A250_B31_C128,
        ),
        (
            "cases",
            "charged-ice-adjacency-z12",
            P_CHARGED_ADJ_A100_B45_C256,
        ),
        ("admission", "scale-251-is-out-of-bounds", P_CHARGED_A251),
    ] {
        let case = find_named(&doc, section_name, name)?;
        compare(
            case,
            "params_hex",
            &str_field(case, "params_hex")?.to_owned(),
            &codec::hex_encode(&want),
        )?;
    }
    compare(&doc, "case count", &section(&doc, "cases")?.len(), &4)
}

/// Build the instance of one lease slot of a case or recipe with the Rust
/// construction of `contract`, and give it to `check` together with the
/// admitted params.
fn with_ice_instance<T>(
    contract: ising_generators::descriptor::Contract,
    case: &Value,
    slot: u64,
    check: impl FnOnce(
        &ising_generators::descriptor::Params<'_>,
        &ising_generators::instance::Instance,
    ) -> Result<T, String>,
) -> Result<T, String> {
    use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits};
    use ising_generators::family::{admit_topology, generate_instance, BaseTopology};
    let params = codec::hex_decode(str_field(case, "params_hex")?)?;
    let contracts = [contract];
    let admitted = admit(
        &contracts,
        str_field(case, "family_id")?.as_bytes(),
        u32_field(case, "revision")?,
        &params,
        &params_digest(&params),
    )
    .map_err(|c| format!("{}: admit failed with {c}", case_name(case)))?;
    let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
    let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
    let bounds = admit_topology(
        &admitted,
        &topology,
        topology.order_digest(),
        &AdvertisedLimits {
            max_nodes: 0,
            max_edges: 0,
            max_degree: 0,
        },
    )
    .map_err(|c| format!("{}: admit_topology failed with {c}", case_name(case)))?;
    let nonce = codec::hex32(&lease_nonce(case, slot)?)?;
    let instance = generate_instance(&admitted, &topology, &bounds, nonce)
        .map_err(|fault| format!("{}: generation gave {fault:?}", case_name(case)))?;
    check(admitted.params(), &instance)
}

/// `sum h s + sum J s s + offset` of an instance, in `i128`.
fn encoded_energy(
    instance: &ising_generators::instance::Instance,
    spins: &[i8],
) -> Result<i128, String> {
    let spin = |at: usize| -> Result<i128, String> {
        spins
            .get(at)
            .map(|&s| i128::from(s))
            .ok_or_else(|| "a position outside the spins".to_owned())
    };
    let mut total = i128::from(instance.offset_milli());
    for (at, &h) in instance.h().iter().enumerate() {
        total += i128::from(h) * spin(at)?;
    }
    for (&(u, v), &j) in instance.edges().iter().zip(instance.j()) {
        let u = usize::try_from(u).map_err(|e| e.to_string())?;
        let v = usize::try_from(v).map_err(|e| e.to_string())?;
        total += i128::from(j) * spin(u)? * spin(v)?;
    }
    Ok(total)
}

/// The spins of a `spins_hex` value: one byte per node, `01` for +1 and `ff`
/// for -1.
fn spins_of_hex(hex: &str) -> Result<Vec<i8>, String> {
    codec::hex_decode(hex)?
        .into_iter()
        .map(|byte| match byte {
            0x01 => Ok(1),
            0xff => Ok(-1),
            other => Err(format!("spin byte {other:#04x} is not 01 or ff")),
        })
        .collect()
}

/// Rebuild every salt of every energy entry with the Rust construction.
/// Recompute the encoded energy and the charge defects from the instance, and
/// compare them with the recorded values. The encoded and direct energies in
/// the file must be equal.
fn check_charge_energies(doc: &Value) -> Result<(), String> {
    use ising_generators::ice::{charge_defects, CHARGED_ICE_V1};
    for entry in section(doc, "energies")? {
        let case = find_named(doc, "cases", str_field(entry, "case")?)?;
        let slot = u64_decimal(entry, "index")?;
        compare(
            entry,
            "nonce",
            &lease_nonce(case, slot)?,
            &str_field(entry, "nonce")?.to_owned(),
        )?;
        for state in entries_of(entry, "states")? {
            let spins = spins_of_hex(str_field(state, "spins_hex")?)?;
            let encoded = with_ice_instance(CHARGED_ICE_V1, case, slot, |params, built| {
                let defects =
                    charge_defects(params, built, &spins).ok_or("charge_defects gave None")?;
                Ok((encoded_energy(built, &spins)?, defects))
            })?;
            let want_encoded = i128::from(i64_decimal(state, "encoded_milli")?);
            let want_direct = i128::from(i64_decimal(state, "direct_milli")?);
            compare(state, "encoded energy", &encoded.0, &want_encoded)?;
            compare(state, "encoded equals direct", &want_direct, &want_encoded)?;
            compare(
                state,
                "charge_defects",
                &u64::from(encoded.1),
                &state
                    .get("charge_defects")
                    .and_then(Value::as_u64)
                    .ok_or("no charge_defects")?,
            )?;
        }
    }
    Ok(())
}

#[test]
fn charge_energies_match_the_rust_construction() -> Result<(), String> {
    check_charge_energies(&charge_doc()?)
}

/// The count of charged tiles and the largest `|h|` of a built instance.
fn charge_counts(built: &ising_generators::instance::Instance) -> Result<(u64, u64, u64), String> {
    let metadata = built.metadata();
    let mut rest = metadata.get(2..).ok_or("no metadata")?;
    let (mut tiles, mut charged) = (None, None);
    while let (Some(kind), Some(len)) = (rest.get(0..2), rest.get(2..6)) {
        let len = usize::try_from(u32::from_le_bytes(len.try_into().map_err(|_| "len")?))
            .map_err(|e| e.to_string())?;
        let end = 6_usize.checked_add(len).ok_or("overflow")?;
        let payload = rest.get(6..end).ok_or("short payload")?;
        let count = u32::from_le_bytes(
            payload
                .get(0..4)
                .ok_or("short count")?
                .try_into()
                .map_err(|_| "count")?,
        );
        match kind {
            [1, 0] => tiles = Some(u64::from(count)),
            [6, 0] => {
                let bytes = payload.get(4..).ok_or("short charges")?;
                let nonzero = bytes.iter().filter(|&&b| b != 0).count();
                charged = Some(u64::try_from(nonzero).map_err(|e| e.to_string())?);
            }
            _ => {}
        }
        rest = rest.get(end..).ok_or("short region")?;
    }
    let max_h = built
        .h()
        .iter()
        .map(|h| u64::from(h.unsigned_abs()))
        .max()
        .unwrap_or(0);
    Ok((
        tiles.ok_or("no tile section")?,
        charged.ok_or("no charge section")?,
        max_h,
    ))
}

/// Rebuild every salt of the recipe with the Rust construction and compare the
/// hash and the counts.
fn check_charge_recipe(recipe: &Value) -> Result<(), String> {
    use ising_generators::descriptor::params_digest;
    use ising_generators::ice::CHARGED_ICE_V1;
    let params = codec::hex_decode(str_field(recipe, "params_hex")?)?;
    compare(
        recipe,
        "params_digest",
        &codec::hex_encode(&params_digest(&params)),
        &str_field(recipe, "params_digest")?.to_owned(),
    )?;
    let salts = entries_of(recipe, "salts")?;
    compare(
        recipe,
        "salts",
        &u64::try_from(salts.len()).map_err(|e| e.to_string())?,
        &u64_decimal(object_field(recipe, "lease")?, "salt_count")?,
    )?;
    for salt in salts {
        let slot = u64_decimal(salt, "index")?;
        compare(
            recipe,
            "nonce",
            &lease_nonce(recipe, slot)?,
            &str_field(salt, "nonce")?.to_owned(),
        )?;
        let (hash, counts) = with_ice_instance(CHARGED_ICE_V1, recipe, slot, |_, built| {
            let canonical = built.canonical_bytes().map_err(|e| e.to_string())?;
            Ok((
                blake3::hash(&canonical).to_hex().to_string(),
                charge_counts(built)?,
            ))
        })?;
        compare(
            recipe,
            "canonical_blake3",
            &hash,
            &str_field(salt, "canonical_blake3")?.to_owned(),
        )?;
        let field = |key: &str| {
            salt.get(key)
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("no {key}"))
        };
        compare(recipe, "tiles", &counts.0, &field("tiles")?)?;
        compare(recipe, "charged_tiles", &counts.1, &field("charged_tiles")?)?;
        compare(recipe, "max_abs_h", &counts.2, &field("max_abs_h")?)?;
    }
    Ok(())
}

#[test]
fn charge_recipe_matches_the_rust_construction() -> Result<(), String> {
    let doc = charge_doc()?;
    for recipe in section(&doc, "recipe")? {
        check_charge_recipe(recipe)?;
    }
    Ok(())
}

#[test]
fn charge_recipe_matches_the_rust_constant() -> Result<(), String> {
    use ising_generators::descriptor::params_digest;
    use ising_generators::ice::ICE_CHARGE_RECIPE;
    let doc = charge_doc()?;
    let recipes = section(&doc, "recipe")?;
    compare(&doc, "recipe count", &recipes.len(), &1)?;
    for recipe in recipes {
        let want = ICE_CHARGE_RECIPE;
        compare(recipe, "alias", &str_field(recipe, "alias")?, &want.alias)?;
        compare(recipe, "cell", &str_field(recipe, "cell")?, &want.cell)?;
        compare(
            recipe,
            "params_hex",
            &str_field(recipe, "params_hex")?.to_owned(),
            &codec::hex_encode(want.params),
        )?;
        compare(
            recipe,
            "params_digest",
            &str_field(recipe, "params_digest")?.to_owned(),
            &codec::hex_encode(&params_digest(want.params)),
        )?;
    }
    Ok(())
}

#[test]
fn charge_admission_codes_match_the_rust_admission() -> Result<(), String> {
    use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits};
    use ising_generators::family::{admit_topology, BaseTopology};
    use ising_generators::ice::CHARGED_ICE_V1;
    let doc = charge_doc()?;
    let contracts = [CHARGED_ICE_V1];
    for case in section(&doc, "admission")? {
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
        let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
        let got = admit(
            &contracts,
            str_field(case, "family_id")?.as_bytes(),
            u32_field(case, "revision")?,
            &params,
            &params_digest(&params),
        )
        .and_then(|admitted| {
            admit_topology(
                &admitted,
                &topology,
                topology.order_digest(),
                &AdvertisedLimits {
                    max_nodes: 0,
                    max_edges: 0,
                    max_degree: 0,
                },
            )
        });
        let got = got.err().map(ising_generators::Code::value);
        compare(case, "admission code", &got, &expected_code(case)?)?;
        compare(case, "code is 9", &got, &Some(9))?;
    }
    compare(
        &doc,
        "admission count",
        &section(&doc, "admission")?.len(),
        &2,
    )
}

#[test]
fn a_changed_charge_coupling_fails_the_recompute() -> Result<(), String> {
    let doc = charge_doc()?;
    let pointer = "/cases/0/instances/0/j/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or("no coupling")?;
    let broken = mutate(&doc, pointer, json!(was + 1000));
    expect_err(&check_charge(&broken), "a changed coupling")
}

#[test]
fn a_changed_charge_energy_fails_the_check() -> Result<(), String> {
    let doc = charge_doc()?;
    let pointer = "/energies/0/states/3/encoded_milli";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or("no energy")?
        .parse::<i64>()
        .map_err(|e| e.to_string())?;
    let broken = mutate(&doc, pointer, json!((was + 1).to_string()));
    expect_err(&check_charge_energies(&broken), "a changed encoded energy")?;
    let pointer = "/energies/0/states/3/charge_defects";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or("no defect count")?;
    let broken = mutate(&doc, pointer, json!(was + 1));
    expect_err(&check_charge_energies(&broken), "a changed defect count")
}

#[test]
fn a_changed_charge_recipe_hash_fails_the_recompute() -> Result<(), String> {
    let doc = charge_doc()?;
    let recipe = section(&doc, "recipe")?.first().ok_or("no recipe")?;
    let was = str_field(
        recipe.pointer("/salts/0").ok_or("no salt")?,
        "canonical_blake3",
    )?;
    let broken = mutate(
        &doc,
        "/recipe/0/salts/0/canonical_blake3",
        json!(flip_last_hex_digit(was)),
    );
    let recipe = section(&broken, "recipe")?.first().ok_or("no recipe")?;
    expect_err(&check_charge_recipe(recipe), "a changed recipe hash")
}

// ---- ice fields vectors (ice_fields.json) ----

fn fields_doc() -> Result<Value, String> {
    serde_json::from_str(include_str!("../vectors/families/ice_fields.json"))
        .map_err(|e| e.to_string())
}

/// Recompute the digests, nonces, and instance bytes of every registered
/// `quip/ice-fields` case with Rust code that does not call the construction.
fn check_fields(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    check_names_unique(doc)?;
    for case in section(doc, "cases")? {
        check_layered_case(case, ice_glass_size_bounds)?;
    }
    Ok(())
}

#[test]
fn fields_fixture_matches_schema() -> Result<(), String> {
    let doc = fields_doc()?;
    validate(&schema()?, &doc)?;
    match doc.get("provenance").and_then(Value::as_str) {
        Some("specified") => {}
        other => return Err(format!("provenance is {other:?}, expected specified")),
    }
    check_names_unique(&doc)?;
    for name in ["cases", "selection", "field_recipes", "admission"] {
        section(&doc, name).map(|_| ())?;
    }
    Ok(())
}

#[test]
fn fields_cases_recompute() -> Result<(), String> {
    check_fields(&fields_doc()?)
}

#[test]
fn fields_cases_match_the_rust_construction() -> Result<(), String> {
    for case in section(&fields_doc()?, "cases")? {
        check_layered_against_rust(case)?;
    }
    Ok(())
}

#[test]
fn fields_params_constants_match_the_vectors() -> Result<(), String> {
    use quip_solver_conformance::families::fields::{
        P_FIELDS_ADJ_G45_F1000_D16, P_FIELDS_F1001, P_FIELDS_Z22_G125_F500_D16,
    };
    let doc = fields_doc()?;
    for (section_name, name, want) in [
        ("cases", "ice-fields-zephyr-z22", P_FIELDS_Z22_G125_F500_D16),
        (
            "cases",
            "ice-fields-adjacency-z22",
            P_FIELDS_ADJ_G45_F1000_D16,
        ),
        ("admission", "field-1001-is-out-of-bounds", P_FIELDS_F1001),
    ] {
        let case = find_named(&doc, section_name, name)?;
        compare(
            case,
            "params_hex",
            &str_field(case, "params_hex")?.to_owned(),
            &codec::hex_encode(&want),
        )?;
    }
    compare(&doc, "case count", &section(&doc, "cases")?.len(), &6)
}

/// The dense positions and values of the nonzero fields of an instance.
fn field_nodes(built: &ising_generators::instance::Instance) -> (Vec<u64>, Vec<i64>) {
    let mut nodes = Vec::new();
    let mut values = Vec::new();
    for (at, &h) in (0u64..).zip(built.h()) {
        if h != 0 {
            nodes.push(at);
            values.push(i64::from(h));
        }
    }
    (nodes, values)
}

/// The tile count of a built instance, from the kind 1 section.
fn tile_count(built: &ising_generators::instance::Instance) -> Result<u64, String> {
    let metadata = built.metadata();
    match (metadata.get(2..4), metadata.get(8..12)) {
        (Some([1, 0]), Some(count)) => Ok(u64::from(u32::from_le_bytes(
            count.try_into().map_err(|_| "count")?,
        ))),
        _ => Err("no tile section first".to_owned()),
    }
}

fn u64_array(entry: &Value, key: &str) -> Result<Vec<u64>, String> {
    entries_of(entry, key)?
        .iter()
        .map(|v| v.as_u64().ok_or_else(|| format!("{key}: not a u64")))
        .collect()
}

fn i64_array(entry: &Value, key: &str) -> Result<Vec<i64>, String> {
    entries_of(entry, key)?
        .iter()
        .map(|v| v.as_i64().ok_or_else(|| format!("{key}: not an i64")))
        .collect()
}

/// Rebuild every salt of every selection entry with the Rust construction and
/// compare the field nodes and their values. Every salt that builds has an
/// entry, and the entries are in the order of the instances.
fn check_fields_selection(doc: &Value) -> Result<(), String> {
    use ising_generators::ice::ICE_FIELDS_V1;
    let mut seen = 0usize;
    for case in section(doc, "cases")? {
        for instance in entries_of(case, "instances")? {
            let slot = u64_decimal(instance, "index")?;
            let name = format!("{}-salt-{slot}", case_name(case));
            let entry = find_named(doc, "selection", &name)?;
            compare(entry, "case", &str_field(entry, "case")?, &case_name(case))?;
            compare(entry, "index", &u64_decimal(entry, "index")?, &slot)?;
            compare(
                entry,
                "nonce",
                &lease_nonce(case, slot)?,
                &str_field(entry, "nonce")?.to_owned(),
            )?;
            let (nodes, values) = with_ice_instance(ICE_FIELDS_V1, case, slot, |params, built| {
                check_fields_metadata(params, built)?;
                Ok(field_nodes(built))
            })?;
            compare(entry, "field nodes", &nodes, &u64_array(entry, "nodes")?)?;
            compare(entry, "field values", &values, &i64_array(entry, "h")?)?;
            seen += 1;
        }
    }
    compare(
        doc,
        "selection count",
        &section(doc, "selection")?.len(),
        &seen,
    )
}

/// The Rust metadata check must accept every built instance.
fn check_fields_metadata(
    params: &ising_generators::descriptor::Params<'_>,
    built: &ising_generators::instance::Instance,
) -> Result<(), String> {
    use ising_generators::ice::ICE_FIELDS;
    if (ICE_FIELDS.check_metadata)(params, built) {
        Ok(())
    } else {
        Err("check_metadata rejected a built instance".to_owned())
    }
}

#[test]
fn fields_selection_matches_the_rust_construction() -> Result<(), String> {
    check_fields_selection(&fields_doc()?)
}

/// Every skipped slot of a shortfall case has code 14. The slot lists are
/// compared with the Rust construction by `fields_cases_match_the_rust_construction`.
fn check_shortfall_case(name: &str, every_slot: bool) -> Result<(), String> {
    let doc = fields_doc()?;
    let case = find_named(&doc, "cases", name)?;
    let skipped = entries_of(case, "skipped")?;
    let built = entries_of(case, "instances")?;
    compare(case, "a skipped salt exists", &skipped.is_empty(), &false)?;
    compare(case, "no built salt", &built.is_empty(), &every_slot)?;
    for entry in skipped {
        compare(
            case,
            "skip_code",
            &entry.get("skip_code"),
            &Some(&json!(14)),
        )?;
    }
    Ok(())
}

#[test]
fn the_shortfall_case_skips_every_salt_with_code_14() -> Result<(), String> {
    check_shortfall_case("ice-fields-shortfall", true)
}

#[test]
fn the_mixed_shortfall_case_skips_some_salts_and_builds_others() -> Result<(), String> {
    check_shortfall_case("ice-fields-shortfall-mixed", false)
}

/// Rebuild every salt of a recipe with the Rust construction and compare the
/// hash and the counts.
fn check_fields_recipe(recipe: &Value) -> Result<(), String> {
    use ising_generators::descriptor::params_digest;
    use ising_generators::ice::ICE_FIELDS_V1;
    let params = codec::hex_decode(str_field(recipe, "params_hex")?)?;
    compare(
        recipe,
        "params_digest",
        &codec::hex_encode(&params_digest(&params)),
        &str_field(recipe, "params_digest")?.to_owned(),
    )?;
    let salts = entries_of(recipe, "salts")?;
    compare(
        recipe,
        "salts",
        &u64::try_from(salts.len()).map_err(|e| e.to_string())?,
        &u64_decimal(object_field(recipe, "lease")?, "salt_count")?,
    )?;
    for salt in salts {
        let slot = u64_decimal(salt, "index")?;
        compare(
            recipe,
            "nonce",
            &lease_nonce(recipe, slot)?,
            &str_field(salt, "nonce")?.to_owned(),
        )?;
        let (hash, tiles, values) = with_ice_instance(ICE_FIELDS_V1, recipe, slot, |_, built| {
            let canonical = built.canonical_bytes().map_err(|e| e.to_string())?;
            Ok((
                blake3::hash(&canonical).to_hex().to_string(),
                tile_count(built)?,
                field_nodes(built).1,
            ))
        })?;
        compare(
            recipe,
            "canonical_blake3",
            &hash,
            &str_field(salt, "canonical_blake3")?.to_owned(),
        )?;
        let field = |key: &str| {
            salt.get(key)
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("no {key}"))
        };
        let count = |keep: fn(&i64) -> bool| {
            u64::try_from(values.iter().filter(|v| keep(v)).count()).map_err(|e| e.to_string())
        };
        compare(recipe, "tiles", &tiles, &field("tiles")?)?;
        compare(
            recipe,
            "fields",
            &u64::try_from(values.len()).map_err(|e| e.to_string())?,
            &field("fields")?,
        )?;
        compare(
            recipe,
            "positive_fields",
            &count(|v| *v > 0)?,
            &field("positive_fields")?,
        )?;
        compare(
            recipe,
            "negative_fields",
            &count(|v| *v < 0)?,
            &field("negative_fields")?,
        )?;
    }
    Ok(())
}

#[test]
fn fields_recipes_match_the_rust_construction() -> Result<(), String> {
    let doc = fields_doc()?;
    for recipe in section(&doc, "field_recipes")? {
        check_fields_recipe(recipe)?;
    }
    Ok(())
}

#[test]
fn fields_recipes_match_the_rust_constants() -> Result<(), String> {
    use ising_generators::descriptor::params_digest;
    use ising_generators::ice::ICE_FIELDS_RECIPES;
    let doc = fields_doc()?;
    let recipes = section(&doc, "field_recipes")?;
    compare(
        &doc,
        "recipe count",
        &recipes.len(),
        &ICE_FIELDS_RECIPES.len(),
    )?;
    for (recipe, want) in recipes.iter().zip(ICE_FIELDS_RECIPES) {
        compare(recipe, "alias", &str_field(recipe, "alias")?, &want.alias)?;
        compare(recipe, "cell", &str_field(recipe, "cell")?, &want.cell)?;
        compare(
            recipe,
            "params_hex",
            &str_field(recipe, "params_hex")?.to_owned(),
            &codec::hex_encode(want.params),
        )?;
        compare(
            recipe,
            "params_digest",
            &str_field(recipe, "params_digest")?.to_owned(),
            &codec::hex_encode(&params_digest(want.params)),
        )?;
    }
    Ok(())
}

#[test]
fn fields_admission_codes_match_the_rust_admission() -> Result<(), String> {
    use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits};
    use ising_generators::family::{admit_topology, BaseTopology};
    use ising_generators::ice::ICE_FIELDS_V1;
    let doc = fields_doc()?;
    let contracts = [ICE_FIELDS_V1];
    let mut codes = Vec::new();
    for case in section(&doc, "admission")? {
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
        let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
        let got = admit(
            &contracts,
            str_field(case, "family_id")?.as_bytes(),
            u32_field(case, "revision")?,
            &params,
            &params_digest(&params),
        )
        .and_then(|admitted| {
            admit_topology(
                &admitted,
                &topology,
                topology.order_digest(),
                &AdvertisedLimits {
                    max_nodes: 0,
                    max_edges: 0,
                    max_degree: 0,
                },
            )
        });
        let got = got.err().map(ising_generators::Code::value);
        compare(case, "admission code", &got, &expected_code(case)?)?;
        codes.push(got);
    }
    compare(&doc, "codes 9 present", &codes.contains(&Some(9)), &true)?;
    compare(&doc, "code 12 present", &codes.contains(&Some(12)), &true)?;
    compare(&doc, "code 13 present", &codes.contains(&Some(13)), &true)?;
    compare(&doc, "an admitted request", &codes.contains(&None), &true)
}

#[test]
fn a_changed_fields_coupling_fails_the_recompute() -> Result<(), String> {
    let doc = fields_doc()?;
    let pointer = "/cases/0/instances/0/j/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or("no coupling")?;
    let broken = mutate(&doc, pointer, json!(was + 1000));
    expect_err(&check_fields(&broken), "a changed coupling")
}

#[test]
fn a_changed_fields_selection_fails_the_check() -> Result<(), String> {
    let doc = fields_doc()?;
    let pointer = "/selection/0/h/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or("no field value")?;
    let broken = mutate(&doc, pointer, json!(-was));
    expect_err(&check_fields_selection(&broken), "a changed field value")?;
    let pointer = "/selection/0/nodes/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or("no field node")?;
    let broken = mutate(&doc, pointer, json!(was + 1));
    expect_err(&check_fields_selection(&broken), "a changed field node")
}

#[test]
fn a_changed_fields_recipe_hash_fails_the_recompute() -> Result<(), String> {
    let doc = fields_doc()?;
    let recipe = section(&doc, "field_recipes")?.first().ok_or("no recipe")?;
    let was = str_field(
        recipe.pointer("/salts/0").ok_or("no salt")?,
        "canonical_blake3",
    )?;
    let broken = mutate(
        &doc,
        "/field_recipes/0/salts/0/canonical_blake3",
        json!(flip_last_hex_digit(was)),
    );
    let recipe = section(&broken, "field_recipes")?
        .first()
        .ok_or("no recipe")?;
    expect_err(&check_fields_recipe(recipe), "a changed recipe hash")
}

// ---- two-commodity routing vectors (two_commodity.json) ----

fn routing_doc() -> Result<Value, String> {
    serde_json::from_str(include_str!("../vectors/families/two_commodity.json"))
        .map_err(|e| e.to_string())
}

/// Recompute the digests, nonces, and instance bytes of every registered
/// `quip/two-commodity-routing` case with Rust code that does not call the
/// construction.
fn check_routing(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    check_names_unique(doc)?;
    for case in section(doc, "cases")? {
        check_layered_case(case, ice_glass_size_bounds)?;
    }
    Ok(())
}

#[test]
fn routing_fixture_matches_schema() -> Result<(), String> {
    let doc = routing_doc()?;
    validate(&schema()?, &doc)?;
    match doc.get("provenance").and_then(Value::as_str) {
        Some("specified") => {}
        other => return Err(format!("provenance is {other:?}, expected specified")),
    }
    check_names_unique(&doc)?;
    for name in ["cases", "terminals", "routing_recipes", "admission"] {
        section(&doc, name).map(|_| ())?;
    }
    Ok(())
}

#[test]
fn routing_cases_recompute() -> Result<(), String> {
    check_routing(&routing_doc()?)
}

#[test]
fn routing_cases_match_the_rust_construction() -> Result<(), String> {
    for case in section(&routing_doc()?, "cases")? {
        check_layered_against_rust(case)?;
    }
    Ok(())
}

#[test]
fn routing_params_constants_match_the_vectors() -> Result<(), String> {
    use quip_solver_conformance::families::routing::{
        P_ROUTING_D3, P_ROUTING_Z24_D4, P_ROUTING_Z34_D6,
    };
    let doc = routing_doc()?;
    for (section_name, name, want) in [
        ("cases", "routing-small", P_ROUTING_Z24_D4),
        ("cases", "routing-retry", P_ROUTING_Z34_D6),
        ("admission", "divisor-3-is-out-of-bounds", P_ROUTING_D3),
    ] {
        let case = find_named(&doc, section_name, name)?;
        compare(
            case,
            "params_hex",
            &str_field(case, "params_hex")?.to_owned(),
            &codec::hex_encode(&want),
        )?;
    }
    compare(&doc, "case count", &section(&doc, "cases")?.len(), &5)
}

/// The routing section of a built instance, decoded by hand from the kind 7
/// layout: the supplies of every node and the end nodes of every arc.
struct RoutingMeta {
    supply: Vec<[i64; 2]>,
    cells: Vec<(u16, u16)>,
    arcs: Vec<[u32; 5]>,
    strength: u16,
}

fn le_u16(bytes: &[u8], at: usize) -> Result<u16, String> {
    let raw = bytes.get(at..at + 2).ok_or("short metadata")?;
    Ok(u16::from_le_bytes(raw.try_into().map_err(|_| "short")?))
}

fn le_u32(bytes: &[u8], at: usize) -> Result<u32, String> {
    let raw = bytes.get(at..at + 4).ok_or("short metadata")?;
    Ok(u32::from_le_bytes(raw.try_into().map_err(|_| "short")?))
}

fn routing_meta(built: &ising_generators::instance::Instance) -> Result<RoutingMeta, String> {
    let meta = built.metadata();
    if le_u16(meta, 0)? != 1 || le_u16(meta, 2)? != 7 {
        return Err("the region is not one kind 7 section".to_owned());
    }
    let length = usize::try_from(le_u32(meta, 4)?).map_err(|e| e.to_string())?;
    let payload = meta.get(8..8 + length).ok_or("section past the region")?;
    let n = usize::try_from(le_u32(payload, 0)?).map_err(|e| e.to_string())?;
    let (mut supply, mut cells) = (Vec::new(), Vec::new());
    for i in 0..n {
        let at = 4 + 6 * i;
        cells.push((le_u16(payload, at)?, le_u16(payload, at + 2)?));
        let raw = payload.get(at + 4..at + 6).ok_or("short node record")?;
        let [first, second] = <[u8; 2]>::try_from(raw).map_err(|_| "short node record")?;
        supply.push([
            i64::from(i8::from_le_bytes([first])),
            i64::from(i8::from_le_bytes([second])),
        ]);
    }
    let base = 4 + 6 * n;
    let a = usize::try_from(le_u32(payload, base)?).map_err(|e| e.to_string())?;
    let mut arcs = Vec::new();
    for i in 0..a {
        let at = base + 4 + 24 * i;
        let mut words = [0u32; 5];
        for (k, word) in words.iter_mut().enumerate() {
            *word = le_u32(payload, at + 4 * k)?;
        }
        arcs.push(words);
    }
    let strength = le_u16(payload, base + 4 + 24 * a)?;
    if payload.len() != base + 4 + 24 * a + 2 {
        return Err("trailing bytes in the routing section".to_owned());
    }
    Ok(RoutingMeta {
        supply,
        cells,
        arcs,
        strength,
    })
}

/// The unit-capacity maximum flow from `sources` to `sinks`, by augmenting
/// paths on a map of residual capacities.
fn unit_flow(n: usize, arcs: &[[u32; 5]], sources: &[usize], sinks: &[usize]) -> usize {
    use std::collections::{BTreeMap, VecDeque};
    let (source, sink) = (n, n + 1);
    let mut cap: BTreeMap<(usize, usize), i32> = BTreeMap::new();
    let mut add = |from: usize, to: usize| {
        *cap.entry((from, to)).or_insert(0) += 1;
        let _ = cap.entry((to, from)).or_insert(0);
    };
    for arc in arcs {
        add(
            arc.first().map_or(0, |&v| v as usize),
            arc.get(1).map_or(0, |&v| v as usize),
        );
    }
    for &v in sources {
        add(source, v);
    }
    for &v in sinks {
        add(v, sink);
    }
    let mut flow = 0;
    loop {
        let mut parent: BTreeMap<usize, usize> = BTreeMap::from([(source, source)]);
        let mut queue = VecDeque::from([source]);
        while let Some(v) = queue.pop_front() {
            for (&(from, to), &room) in cap.range((v, 0)..(v + 1, 0)) {
                if from == v && room > 0 && !parent.contains_key(&to) {
                    let _ = parent.insert(to, v);
                    queue.push_back(to);
                }
            }
        }
        if !parent.contains_key(&sink) {
            return flow;
        }
        let mut w = sink;
        while w != source {
            let Some(&v) = parent.get(&w) else {
                return flow;
            };
            *cap.entry((v, w)).or_insert(0) -= 1;
            *cap.entry((w, v)).or_insert(0) += 1;
            w = v;
        }
        flow += 1;
    }
}

/// The try that passed and its supply.
type Replay = (u64, Vec<[i64; 2]>);

/// Replay the `terminal` stream of one salt with the FAM-24 shuffle and the
/// maximum flow check. Returns the try that passed and its supply, or `None`
/// when all four tries fail. `meta` holds the template, which does not depend
/// on the nonce, and `divisor` is the `pairs_divisor` parameter.
fn replay_terminals(
    case: &Value,
    slot: u64,
    meta: &RoutingMeta,
    divisor: usize,
) -> Result<Option<Replay>, String> {
    use ising_generators::family::Streams;
    use ising_generators::rng::shuffle;
    let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
    let order = digests::topology_order_digest(&nodes, &edges);
    let mut streams = Streams::new(
        codec::hex32(&lease_nonce(case, slot)?)?,
        digests::descriptor_digest(
            str_field(case, "family_id")?.as_bytes(),
            u32_field(case, "revision")?,
            &digests::params_digest(&codec::hex_decode(str_field(case, "params_hex")?)?),
        ),
        order,
        ising_generators::routing::ROUTING.labels,
    );
    let mut stream = streams.open("terminal").ok_or("no terminal stream")?;
    let n = meta.cells.len();
    let pairs = n / divisor;
    for attempt in 1..=4u64 {
        let mut list: Vec<u32> = (0..u32::try_from(n).map_err(|e| e.to_string())?).collect();
        shuffle(&mut stream, &mut list).map_err(|e| format!("{e:?}"))?;
        let at = |i: usize| -> Result<usize, String> {
            list.get(i)
                .map(|&v| v as usize)
                .ok_or_else(|| "short list".to_owned())
        };
        let mut supply = vec![[0i64; 2]; n];
        let mut routes = true;
        for (k, start) in [0, 2 * pairs].into_iter().enumerate() {
            let (mut sources, mut sinks) = (Vec::new(), Vec::new());
            for i in 0..pairs {
                sources.push(at(start + 2 * i)?);
                sinks.push(at(start + 2 * i + 1)?);
            }
            routes &= unit_flow(n, &meta.arcs, &sources, &sinks) == pairs;
            for (nodes, value) in [(&sources, 1i64), (&sinks, -1)] {
                for &v in nodes {
                    let row = supply.get_mut(v).ok_or("a terminal outside the template")?;
                    let slot = row.get_mut(k).ok_or("a commodity outside 0 and 1")?;
                    *slot = value;
                }
            }
        }
        if routes {
            return Ok(Some((attempt, supply)));
        }
    }
    Ok(None)
}

/// The `pairs_divisor` of canonical routing params: the last two bytes.
fn routing_divisor(params: &[u8]) -> Result<usize, String> {
    Ok(usize::from(le_u16(params, params.len().saturating_sub(2))?))
}

/// Rebuild every salt that builds with the Rust construction, replay the
/// terminal draw, and compare the try and the supply with the entries. Every
/// salt that builds has an entry, in the order of the instances.
fn check_routing_terminals(doc: &Value) -> Result<(), String> {
    use ising_generators::routing::ROUTING_V1;
    let mut seen = 0usize;
    for case in section(doc, "cases")? {
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        for instance in entries_of(case, "instances")? {
            let slot = u64_decimal(instance, "index")?;
            let name = format!("{}-salt-{slot}", case_name(case));
            let entry = find_named(doc, "terminals", &name)?;
            compare(entry, "case", &str_field(entry, "case")?, &case_name(case))?;
            compare(entry, "index", &u64_decimal(entry, "index")?, &slot)?;
            compare(
                entry,
                "nonce",
                &lease_nonce(case, slot)?,
                &str_field(entry, "nonce")?.to_owned(),
            )?;
            let (meta, passed) = with_ice_instance(ROUTING_V1, case, slot, |params, built| {
                check_routing_metadata(params, built)?;
                routing_meta(built)
            })
            .map(|meta| {
                let replay = replay_terminals(case, slot, &meta, routing_divisor(&params)?)?;
                Ok::<_, String>((meta, replay))
            })??;
            let (attempt, supply) = passed.ok_or("a salt that builds has a passing try")?;
            compare(
                entry,
                "try",
                &attempt,
                &entry.get("try").and_then(Value::as_u64).ok_or("no try")?,
            )?;
            compare(entry, "metadata supply", &meta.supply, &supply)?;
            let recorded: Vec<[i64; 2]> = entries_of(entry, "supply")?
                .iter()
                .map(|row| {
                    let pair = row.as_array().ok_or("a supply row is not an array")?;
                    match pair.as_slice() {
                        [a, b] => Ok([
                            a.as_i64().ok_or("supply is not an integer")?,
                            b.as_i64().ok_or("supply is not an integer")?,
                        ]),
                        _ => Err("a supply row is not a pair".to_owned()),
                    }
                })
                .collect::<Result<_, String>>()?;
            compare(entry, "recorded supply", &recorded, &supply)?;
            seen += 1;
        }
    }
    compare(
        doc,
        "terminals count",
        &section(doc, "terminals")?.len(),
        &seen,
    )
}

/// The Rust metadata check must accept every built instance.
fn check_routing_metadata(
    params: &ising_generators::descriptor::Params<'_>,
    built: &ising_generators::instance::Instance,
) -> Result<(), String> {
    use ising_generators::routing::ROUTING;
    if (ROUTING.check_metadata)(params, built) {
        Ok(())
    } else {
        Err("check_metadata rejected a built instance".to_owned())
    }
}

#[test]
fn routing_terminals_match_a_replay_of_the_terminal_stream() -> Result<(), String> {
    check_routing_terminals(&routing_doc()?)
}

#[test]
fn the_retry_case_has_a_salt_that_passes_after_a_failed_try() -> Result<(), String> {
    let doc = routing_doc()?;
    let tries: Vec<u64> = section(&doc, "terminals")?
        .iter()
        .filter(|entry| entry.get("case").and_then(Value::as_str) == Some("routing-retry"))
        .filter_map(|entry| entry.get("try").and_then(Value::as_u64))
        .collect();
    compare(
        &doc,
        "a later try exists",
        &tries.iter().any(|&t| t > 1),
        &true,
    )?;
    compare(&doc, "the first try exists", &tries.contains(&1), &true)
}

#[test]
fn the_no_instance_case_skips_every_salt_with_code_14() -> Result<(), String> {
    let doc = routing_doc()?;
    let case = find_named(&doc, "cases", "routing-no-instance")?;
    compare(
        case,
        "no instance builds",
        &entries_of(case, "instances")?.len(),
        &0,
    )?;
    let skipped = entries_of(case, "skipped")?;
    compare(case, "a skipped salt exists", &skipped.is_empty(), &false)?;
    for entry in skipped {
        compare(
            case,
            "skip_code",
            &entry.get("skip_code"),
            &Some(&json!(14)),
        )?;
    }
    Ok(())
}

#[test]
fn the_no_pairs_case_has_no_terminals() -> Result<(), String> {
    let doc = routing_doc()?;
    for entry in section(&doc, "terminals")? {
        if entry.get("case").and_then(Value::as_str) == Some("routing-no-pairs") {
            for row in entries_of(entry, "supply")? {
                compare(entry, "supply row", row, &json!([0, 0]))?;
            }
        }
    }
    Ok(())
}

/// Rebuild every salt of a recipe with the Rust construction and compare the
/// hash, the counts, the chain strength, and the try that passed.
fn check_routing_recipe(recipe: &Value) -> Result<(), String> {
    use ising_generators::descriptor::params_digest;
    use ising_generators::routing::ROUTING_V1;
    let params = codec::hex_decode(str_field(recipe, "params_hex")?)?;
    compare(
        recipe,
        "params_digest",
        &codec::hex_encode(&params_digest(&params)),
        &str_field(recipe, "params_digest")?.to_owned(),
    )?;
    let salts = entries_of(recipe, "salts")?;
    compare(
        recipe,
        "salts",
        &u64::try_from(salts.len()).map_err(|e| e.to_string())?,
        &u64_decimal(object_field(recipe, "lease")?, "salt_count")?,
    )?;
    for salt in salts {
        let slot = u64_decimal(salt, "index")?;
        compare(
            recipe,
            "nonce",
            &lease_nonce(recipe, slot)?,
            &str_field(salt, "nonce")?.to_owned(),
        )?;
        let (hash, meta) = with_ice_instance(ROUTING_V1, recipe, slot, |params, built| {
            check_routing_metadata(params, built)?;
            let canonical = built.canonical_bytes().map_err(|e| e.to_string())?;
            Ok((
                blake3::hash(&canonical).to_hex().to_string(),
                routing_meta(built)?,
            ))
        })?;
        compare(
            recipe,
            "canonical_blake3",
            &hash,
            &str_field(salt, "canonical_blake3")?.to_owned(),
        )?;
        let field = |key: &str| {
            salt.get(key)
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("no {key}"))
        };
        let count = |len: usize| u64::try_from(len).map_err(|e| e.to_string());
        compare(recipe, "cells", &count(meta.cells.len())?, &field("cells")?)?;
        compare(recipe, "arcs", &count(meta.arcs.len())?, &field("arcs")?)?;
        compare(
            recipe,
            "strength",
            &u64::from(meta.strength),
            &field("strength")?,
        )?;
        let replay = replay_terminals(recipe, slot, &meta, routing_divisor(&params)?)?;
        let (attempt, supply) = replay.ok_or("a salt that builds has a passing try")?;
        compare(recipe, "try", &attempt, &field("try")?)?;
        compare(recipe, "supply", &meta.supply, &supply)?;
    }
    Ok(())
}

#[test]
fn routing_recipes_match_the_rust_construction() -> Result<(), String> {
    let doc = routing_doc()?;
    for recipe in section(&doc, "routing_recipes")? {
        check_routing_recipe(recipe)?;
    }
    Ok(())
}

#[test]
fn routing_recipes_match_the_rust_constants() -> Result<(), String> {
    use ising_generators::descriptor::params_digest;
    use ising_generators::routing::ROUTING_RECIPES;
    let doc = routing_doc()?;
    let recipes = section(&doc, "routing_recipes")?;
    compare(&doc, "recipe count", &recipes.len(), &ROUTING_RECIPES.len())?;
    for (recipe, want) in recipes.iter().zip(ROUTING_RECIPES) {
        compare(recipe, "alias", &str_field(recipe, "alias")?, &want.alias)?;
        compare(recipe, "cell", &str_field(recipe, "cell")?, &want.cell)?;
        compare(
            recipe,
            "params_hex",
            &str_field(recipe, "params_hex")?.to_owned(),
            &codec::hex_encode(want.params),
        )?;
        compare(
            recipe,
            "params_digest",
            &str_field(recipe, "params_digest")?.to_owned(),
            &codec::hex_encode(&params_digest(want.params)),
        )?;
    }
    Ok(())
}

/// The template of the research model, from `conformance/fixtures`. The fixture
/// holds native Zephyr ids, so CI never reads the research data root.
fn research_template() -> Result<Value, String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/fixtures/two_commodity_research_template.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// The nodes, arcs, arc spins, and chain spins of the Rust template on the
/// live topology equal the research template (D8). The instance spins map back
/// to native ids through the node list of the instance.
fn check_research_template(fixture: &Value) -> Result<(), String> {
    use ising_generators::routing::ROUTING_V1;
    let doc = routing_doc()?;
    let recipe = section(&doc, "routing_recipes")?
        .first()
        .ok_or("no recipe")?;
    let (cells, arcs, native) = with_ice_instance(ROUTING_V1, recipe, 0, |_, built| {
        let meta = routing_meta(built)?;
        let native = built.node_ids().to_vec();
        Ok((meta.cells, meta.arcs, native))
    })?;
    let id_at = |position: u32| -> Result<u64, String> {
        native
            .get(usize::try_from(position).map_err(|e| e.to_string())?)
            .map(|&id| u64::from(id))
            .ok_or_else(|| "a spin outside the instance".to_owned())
    };
    let want_cells: Vec<Value> = cells.iter().map(|&(c, r)| json!([c, r])).collect();
    let want_nodes: Vec<Value> = arcs.iter().map(|a| json!([a[0], a[1]])).collect();
    let want_x1 = arcs
        .iter()
        .map(|a| id_at(a[2]))
        .collect::<Result<Vec<_>, _>>()?;
    let want_chain = arcs
        .iter()
        .map(|a| Ok(json!([id_at(a[3])?, id_at(a[4])?])))
        .collect::<Result<Vec<Value>, String>>()?;
    compare(
        fixture,
        "nodes_cell",
        &entries_of(fixture, "nodes_cell")?.to_vec(),
        &want_cells,
    )?;
    compare(
        fixture,
        "arc_nodes",
        &entries_of(fixture, "arc_nodes")?.to_vec(),
        &want_nodes,
    )?;
    compare(
        fixture,
        "arc_spin_commodity1",
        &u64_array(fixture, "arc_spin_commodity1")?,
        &want_x1,
    )?;
    compare(
        fixture,
        "arc_chain_commodity2",
        &entries_of(fixture, "arc_chain_commodity2")?.to_vec(),
        &want_chain,
    )
}

#[test]
fn the_rust_template_matches_the_research_template_on_the_live_topology() -> Result<(), String> {
    check_research_template(&research_template()?)
}

#[test]
fn a_changed_research_template_fails_the_parity_check() -> Result<(), String> {
    let fixture = research_template()?;
    let was = fixture
        .pointer("/arc_spin_commodity1/0")
        .and_then(Value::as_u64)
        .ok_or("no arc spin")?;
    let broken = mutate(&fixture, "/arc_spin_commodity1/0", json!(was + 1));
    expect_err(&check_research_template(&broken), "a changed arc spin")
}

#[test]
fn routing_admission_codes_match_the_rust_admission() -> Result<(), String> {
    use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits};
    use ising_generators::family::{admit_topology, BaseTopology};
    use ising_generators::routing::ROUTING_V1;
    let doc = routing_doc()?;
    let contracts = [ROUTING_V1];
    let mut codes = Vec::new();
    for case in section(&doc, "admission")? {
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
        let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
        let got = admit(
            &contracts,
            str_field(case, "family_id")?.as_bytes(),
            u32_field(case, "revision")?,
            &params,
            &params_digest(&params),
        )
        .and_then(|admitted| {
            admit_topology(
                &admitted,
                &topology,
                topology.order_digest(),
                &AdvertisedLimits {
                    max_nodes: 0,
                    max_edges: 0,
                    max_degree: 0,
                },
            )
        });
        let got = got.err().map(ising_generators::Code::value);
        compare(case, "admission code", &got, &expected_code(case)?)?;
        codes.push(got);
    }
    compare(&doc, "codes 9 present", &codes.contains(&Some(9)), &true)?;
    compare(&doc, "code 12 present", &codes.contains(&Some(12)), &true)?;
    compare(&doc, "an admitted request", &codes.contains(&None), &true)
}

#[test]
fn a_changed_routing_coupling_fails_the_recompute() -> Result<(), String> {
    let doc = routing_doc()?;
    let pointer = "/cases/0/instances/0/j/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or("no coupling")?;
    let broken = mutate(&doc, pointer, json!(was + 4));
    expect_err(&check_routing(&broken), "a changed coupling")
}

#[test]
fn a_changed_routing_supply_fails_the_replay() -> Result<(), String> {
    let doc = routing_doc()?;
    let pointer = "/terminals/0/supply/0/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or("no supply")?;
    let broken = mutate(&doc, pointer, json!(was + 1));
    expect_err(&check_routing_terminals(&broken), "a changed supply")?;
    let pointer = "/terminals/0/try";
    let broken = mutate(&doc, pointer, json!(4));
    expect_err(&check_routing_terminals(&broken), "a changed try")
}

#[test]
fn a_changed_routing_recipe_hash_fails_the_recompute() -> Result<(), String> {
    let doc = routing_doc()?;
    let recipe = section(&doc, "routing_recipes")?
        .first()
        .ok_or("no recipe")?;
    let was = str_field(
        recipe.pointer("/salts/0").ok_or("no salt")?,
        "canonical_blake3",
    )?;
    let broken = mutate(
        &doc,
        "/routing_recipes/0/salts/0/canonical_blake3",
        json!(flip_last_hex_digit(was)),
    );
    let recipe = section(&broken, "routing_recipes")?
        .first()
        .ok_or("no recipe")?;
    expect_err(&check_routing_recipe(recipe), "a changed recipe hash")
}

// ---- polariton static vectors (polariton.json) ----

fn polariton_doc() -> Result<Value, String> {
    serde_json::from_str(include_str!("../vectors/families/polariton.json"))
        .map_err(|e| e.to_string())
}

/// `side` and `pairs` of canonical polariton params: the two last fields,
/// which have fixed offsets.
fn polariton_shape(params: &[u8]) -> Result<(u64, u64), String> {
    if params.len() != 32 {
        return Err(format!("polariton params have {} bytes", params.len()));
    }
    Ok((
        u64::from(le_u16(params, 22)?),
        u64::from(le_u16(params, 30)?),
    ))
}

/// The bounds of `quip/polariton-static`: the links, six couplers for each
/// interior vertex, and degree six.
fn polariton_size_bounds(
    _: &Value,
    params: &[u8],
    nodes: &[u32],
    edges: &[(u32, u32)],
) -> Result<Option<(u64, u64, u64)>, String> {
    let (side, _) = polariton_shape(params)?;
    let n = u64::try_from(nodes.len())
        .map_err(|e| e.to_string())?
        .min(2 * side * (side + 1));
    let e = u64::try_from(edges.len())
        .map_err(|e| e.to_string())?
        .min(6 * (side - 1) * (side - 1));
    let d = topology_max_degree(nodes, edges)
        .min(6)
        .min(n.saturating_sub(1));
    Ok(Some((n, e, d)))
}

/// Recompute the digests, nonces, and instance bytes of every registered
/// `quip/polariton-static` case with Rust code that does not call the
/// construction.
fn check_polariton(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    check_names_unique(doc)?;
    for case in section(doc, "cases")? {
        check_layered_case(case, polariton_size_bounds)?;
    }
    Ok(())
}

#[test]
fn polariton_fixture_matches_schema() -> Result<(), String> {
    let doc = polariton_doc()?;
    validate(&schema()?, &doc)?;
    match doc.get("provenance").and_then(Value::as_str) {
        Some("specified") => {}
        other => return Err(format!("provenance is {other:?}, expected specified")),
    }
    check_names_unique(&doc)?;
    for name in ["cases", "witnesses", "polariton_recipes", "admission"] {
        section(&doc, name).map(|_| ())?;
    }
    Ok(())
}

#[test]
fn polariton_cases_recompute() -> Result<(), String> {
    check_polariton(&polariton_doc()?)
}

#[test]
fn polariton_cases_match_the_rust_construction() -> Result<(), String> {
    for case in section(&polariton_doc()?, "cases")? {
        check_layered_against_rust(case)?;
    }
    Ok(())
}

#[test]
fn polariton_params_constants_match_the_vectors() -> Result<(), String> {
    use quip_solver_conformance::families::polariton::{
        P_POLARITON_SIDE1, P_POLARITON_Z24_L2_P0, P_POLARITON_Z34_L3_P2,
    };
    let doc = polariton_doc()?;
    for (section_name, name, want) in [
        ("cases", "polariton-l2", P_POLARITON_Z24_L2_P0),
        ("cases", "polariton-l3", P_POLARITON_Z34_L3_P2),
        ("admission", "side-1-is-out-of-bounds", P_POLARITON_SIDE1),
    ] {
        let case = find_named(&doc, section_name, name)?;
        compare(
            case,
            "params_hex",
            &str_field(case, "params_hex")?.to_owned(),
            &codec::hex_encode(&want),
        )?;
    }
    compare(&doc, "case count", &section(&doc, "cases")?.len(), &5)
}

/// The labels and the window of a built instance, decoded by hand from the
/// kind 4 and kind 5 layouts.
struct PolaritonMeta {
    side: u16,
    labels: Vec<i8>,
    window: (u16, u16),
    positions: Vec<u32>,
}

fn polariton_meta(built: &ising_generators::instance::Instance) -> Result<PolaritonMeta, String> {
    let region = built.metadata();
    if le_u16(region, 0)? != 2 || le_u16(region, 2)? != 4 {
        return Err("the region does not start with kinds 4 and 5".to_owned());
    }
    let first = usize::try_from(le_u32(region, 4)?).map_err(|e| e.to_string())?;
    let charges = region.get(8..8 + first).ok_or("section past the region")?;
    let at = 8 + first;
    if le_u16(region, at)? != 5 {
        return Err("the second section is not kind 5".to_owned());
    }
    let second = usize::try_from(le_u32(region, at + 2)?).map_err(|e| e.to_string())?;
    let incidence = region
        .get(at + 6..at + 6 + second)
        .ok_or("section past the region")?;
    if region.len() != at + 6 + second {
        return Err("trailing bytes in the region".to_owned());
    }
    let side = le_u16(charges, 0)?;
    let labels = charges
        .get(2..)
        .ok_or("short charges")?
        .iter()
        .map(|&b| i8::from_le_bytes([b]))
        .collect();
    let positions = incidence
        .get(4..)
        .ok_or("short incidence")?
        .chunks_exact(4)
        .map(|c| <[u8; 4]>::try_from(c).map(u32::from_le_bytes))
        .collect::<Result<_, _>>()
        .map_err(|_| "short position".to_owned())?;
    Ok(PolaritonMeta {
        side,
        labels,
        window: (le_u16(incidence, 0)?, le_u16(incidence, 2)?),
        positions,
    })
}

/// The `charges` stream of one salt, replayed with the FAM-24 shuffle and the
/// request check of `square`. Returns the try that passed and its labels, or
/// `None` when all four tries fail.
type ChargeReplay = (u64, Vec<Option<i8>>);

fn replay_charges(case: &Value, slot: u64) -> Result<Option<ChargeReplay>, String> {
    use ising_generators::family::Streams;
    use ising_generators::rng::shuffle;
    use ising_generators::square::{check_request, Boundary, Request, SquareLattice};
    let params = codec::hex_decode(str_field(case, "params_hex")?)?;
    let (side, pairs) = polariton_shape(&params)?;
    let (side, pairs) = (
        u16::try_from(side).map_err(|e| e.to_string())?,
        usize::try_from(pairs).map_err(|e| e.to_string())?,
    );
    let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
    let order = digests::topology_order_digest(&nodes, &edges);
    let mut streams = Streams::new(
        codec::hex32(&lease_nonce(case, slot)?)?,
        digests::descriptor_digest(
            str_field(case, "family_id")?.as_bytes(),
            u32_field(case, "revision")?,
            &digests::params_digest(&params),
        ),
        order,
        ising_generators::polariton::POLARITON.labels,
    );
    let mut stream = streams.open("charges").ok_or("no charges stream")?;
    let lattice = SquareLattice::new(side, Boundary::Open).map_err(|e| e.to_string())?;
    let width = u32::from(side) + 1;
    let interior: Vec<u32> = (1..u32::from(side))
        .flat_map(|y| (1..u32::from(side)).map(move |x| y * width + x))
        .collect();
    let vertices = usize::try_from(lattice.vertex_count()).map_err(|e| e.to_string())?;
    for attempt in 1..=4u64 {
        let mut list = interior.clone();
        shuffle(&mut stream, &mut list).map_err(|e| format!("{e:?}"))?;
        let mut labels = vec![None; vertices];
        for (rank, &v) in list.iter().enumerate() {
            let label = if rank < pairs {
                2
            } else if rank < 2 * pairs {
                -2
            } else {
                0
            };
            let slot = labels
                .get_mut(usize::try_from(v).map_err(|e| e.to_string())?)
                .ok_or("a vertex outside the lattice")?;
            *slot = Some(label);
        }
        let request = Request {
            charges: labels.clone(),
            fixed: Vec::new(),
            fields: Vec::new(),
        };
        if check_request(&lattice, &request).is_ok() {
            return Ok(Some((attempt, labels)));
        }
    }
    Ok(None)
}

/// The Rust metadata check must accept every built instance.
fn check_polariton_metadata(
    params: &ising_generators::descriptor::Params<'_>,
    built: &ising_generators::instance::Instance,
) -> Result<PolaritonMeta, String> {
    use ising_generators::polariton::POLARITON;
    if !(POLARITON.check_metadata)(params, built) {
        return Err("check_metadata rejected a built instance".to_owned());
    }
    polariton_meta(built)
}

/// Rebuild every salt of a case with the Rust construction, replay the charge
/// draw, and compare the labels. A salt that is skipped must fail all four
/// tries. Returns the tries of the salts that build.
fn check_polariton_draws(doc: &Value) -> Result<Vec<u64>, String> {
    use ising_generators::polariton::POLARITON_V1;
    let mut tries = Vec::new();
    for case in section(doc, "cases")? {
        for entry in entries_of(case, "skipped")? {
            let replay = replay_charges(case, u64_decimal(entry, "index")?)?;
            compare(case, "a skipped salt fails every try", &replay, &None)?;
        }
        for instance in entries_of(case, "instances")? {
            let slot = u64_decimal(instance, "index")?;
            let meta = with_ice_instance(POLARITON_V1, case, slot, |params, built| {
                check_polariton_metadata(params, built)
            })?;
            let (attempt, labels) =
                replay_charges(case, slot)?.ok_or("a salt that builds has a passing try")?;
            let want: Vec<i8> = labels.iter().map(|l| l.unwrap_or(127)).collect();
            compare(case, "labels", &meta.labels, &want)?;
            tries.push(attempt);
        }
    }
    Ok(tries)
}

#[test]
fn polariton_charges_match_a_replay_of_the_charges_stream() -> Result<(), String> {
    check_polariton_draws(&polariton_doc()?).map(|_| ())
}

#[test]
fn the_polariton_retry_case_has_a_salt_that_passes_after_a_failed_try() -> Result<(), String> {
    let doc = polariton_doc()?;
    let case = find_named(&doc, "cases", "polariton-retry")?;
    let mut tries = Vec::new();
    for instance in entries_of(case, "instances")? {
        let (attempt, _) = replay_charges(case, u64_decimal(instance, "index")?)?
            .ok_or("a salt that builds has a passing try")?;
        tries.push(attempt);
    }
    compare(
        case,
        "a later try exists",
        &tries.iter().any(|&t| t > 1),
        &true,
    )?;
    let skipped = entries_of(case, "skipped")?;
    compare(case, "a skipped salt exists", &skipped.is_empty(), &false)?;
    for entry in skipped {
        compare(
            case,
            "skip_code",
            &entry.get("skip_code"),
            &Some(&json!(14)),
        )?;
    }
    Ok(())
}

/// Recompute the encoded energy, the feasible result, and the observables of
/// one witness through the Rust construction. Returns the expected feasible
/// flag.
/// The energy of an infeasible witness: one violated label, in milli.
const POLARITON_LOW_ENERGY: i128 = 500;

fn check_polariton_witness(doc: &Value, witness: &Value) -> Result<bool, String> {
    use ising_generators::descriptor::{admit, params_digest};
    use ising_generators::family::{admit_topology, feasible, generate_instance, BaseTopology};
    use ising_generators::polariton::{polariton_observables, POLARITON_V1};
    let contracts = [POLARITON_V1];
    let case = find_named(doc, "cases", str_field(witness, "case")?)?;
    let slot = u64_decimal(witness, "index")?;
    let params = codec::hex_decode(str_field(case, "params_hex")?)?;
    let admitted = admit(
        &contracts,
        str_field(case, "family_id")?.as_bytes(),
        u32_field(case, "revision")?,
        &params,
        &params_digest(&params),
    )
    .map_err(|c| format!("{}: admit failed with {c}", case_name(case)))?;
    let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
    let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
    let bounds = admit_topology(
        &admitted,
        &topology,
        topology.order_digest(),
        &ising_generators::descriptor::AdvertisedLimits {
            max_nodes: 0,
            max_edges: 0,
            max_degree: 0,
        },
    )
    .map_err(|c| format!("{}: admit_topology failed with {c}", case_name(case)))?;
    let nonce = codec::hex32(&lease_nonce(case, slot)?)?;
    let built = generate_instance(&admitted, &topology, &bounds, nonce)
        .map_err(|fault| format!("{}: generation gave {fault:?}", case_name(case)))?;
    let spins: Vec<i8> = entries_of(witness, "spins")?
        .iter()
        .map(|v| {
            v.as_i64()
                .and_then(|n| i8::try_from(n).ok())
                .ok_or_else(|| "a spin is not an i8".to_owned())
        })
        .collect::<Result<_, _>>()?;
    let want_feasible = witness
        .get("feasible")
        .and_then(Value::as_bool)
        .ok_or("no feasible flag")?;
    compare(
        witness,
        "feasible",
        &feasible(&admitted, &built, &spins),
        &want_feasible,
    )?;
    let seen = polariton_observables(admitted.params(), &built, &spins)
        .ok_or("no observables for a witness")?;
    let int = |key: &str| {
        witness
            .get(key)
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("no {key}"))
    };
    compare(
        witness,
        "violations",
        &i64::from(seen.violations),
        &int("violations")?,
    )?;
    compare(
        witness,
        "loss_milli",
        &i64::try_from(seen.loss_milli).map_err(|e| e.to_string())?,
        &int("loss_milli")?,
    )?;
    compare(
        witness,
        "charges",
        &seen
            .charges
            .iter()
            .map(|&q| json!(q))
            .collect::<Vec<Value>>(),
        &entries_of(witness, "charges")?.to_vec(),
    )?;
    let energy = encoded_energy(&built, &spins)?;
    compare(
        witness,
        "energy_milli",
        &energy,
        &i128::from(int("energy_milli")?),
    )?;
    // The energy is the charge loss, so a state is feasible exactly when
    // its energy is 0. An infeasible witness has the lowest possible
    // violation, one violated label, which costs 500 milli.
    let want_energy = if want_feasible {
        0
    } else {
        POLARITON_LOW_ENERGY
    };
    compare(witness, "energy", &energy, &want_energy)?;
    compare(witness, "loss", &i128::from(seen.loss_milli), &want_energy)?;
    Ok(want_feasible)
}

/// Recompute every witness, and count one feasible and one infeasible witness
/// for each case.
fn check_polariton_witnesses(doc: &Value) -> Result<(), String> {
    use std::collections::BTreeMap;
    let mut seen: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for witness in section(doc, "witnesses")? {
        let feasible = check_polariton_witness(doc, witness)?;
        let case = str_field(witness, "case")?;
        let kind = if feasible { "feasible" } else { "infeasible" };
        compare(
            witness,
            "name",
            &str_field(witness, "name")?.to_owned(),
            &format!("{case}-{kind}"),
        )?;
        let slot = seen.entry(case.to_owned()).or_default();
        if feasible {
            slot.0 += 1;
        } else {
            slot.1 += 1;
        }
    }
    for case in section(doc, "cases")? {
        let got = seen.remove(case_name(case));
        compare(case, "witnesses of the case", &got, &Some((1, 1)))?;
    }
    compare(doc, "witnesses of unknown cases", &seen.len(), &0)
}

#[test]
fn polariton_witnesses_match_the_rust_feasibility_and_observables() -> Result<(), String> {
    check_polariton_witnesses(&polariton_doc()?)
}

/// Rebuild every salt of a recipe with the Rust construction and compare the
/// hash, the window, and the try that passed.
fn check_polariton_recipe(recipe: &Value) -> Result<(), String> {
    use ising_generators::descriptor::params_digest;
    use ising_generators::polariton::POLARITON_V1;
    let params = codec::hex_decode(str_field(recipe, "params_hex")?)?;
    compare(
        recipe,
        "params_digest",
        &codec::hex_encode(&params_digest(&params)),
        &str_field(recipe, "params_digest")?.to_owned(),
    )?;
    let salts = entries_of(recipe, "salts")?;
    compare(
        recipe,
        "salts",
        &u64::try_from(salts.len()).map_err(|e| e.to_string())?,
        &u64_decimal(object_field(recipe, "lease")?, "salt_count")?,
    )?;
    for salt in salts {
        let slot = u64_decimal(salt, "index")?;
        compare(
            recipe,
            "nonce",
            &lease_nonce(recipe, slot)?,
            &str_field(salt, "nonce")?.to_owned(),
        )?;
        let (hash, meta) = with_ice_instance(POLARITON_V1, recipe, slot, |params, built| {
            let meta = check_polariton_metadata(params, built)?;
            let canonical = built.canonical_bytes().map_err(|e| e.to_string())?;
            Ok((blake3::hash(&canonical).to_hex().to_string(), meta))
        })?;
        compare(
            recipe,
            "canonical_blake3",
            &hash,
            &str_field(salt, "canonical_blake3")?.to_owned(),
        )?;
        let window: Vec<u64> = u64_array(salt, "window")?;
        compare(
            recipe,
            "window",
            &window,
            &vec![u64::from(meta.window.0), u64::from(meta.window.1)],
        )?;
        let (attempt, _) =
            replay_charges(recipe, slot)?.ok_or("a salt that builds has a passing try")?;
        compare(
            recipe,
            "try",
            &attempt,
            &salt.get("try").and_then(Value::as_u64).ok_or("no try")?,
        )?;
    }
    Ok(())
}

#[test]
fn polariton_recipes_match_the_rust_construction() -> Result<(), String> {
    let doc = polariton_doc()?;
    for recipe in section(&doc, "polariton_recipes")? {
        check_polariton_recipe(recipe)?;
    }
    Ok(())
}

#[test]
fn polariton_recipes_match_the_rust_constants() -> Result<(), String> {
    use ising_generators::descriptor::params_digest;
    use ising_generators::polariton::POLARITON_RECIPES;
    let doc = polariton_doc()?;
    let recipes = section(&doc, "polariton_recipes")?;
    compare(
        &doc,
        "recipe count",
        &recipes.len(),
        &POLARITON_RECIPES.len(),
    )?;
    for (recipe, want) in recipes.iter().zip(POLARITON_RECIPES) {
        compare(recipe, "alias", &str_field(recipe, "alias")?, &want.alias)?;
        compare(recipe, "cell", &str_field(recipe, "cell")?, &want.cell)?;
        compare(
            recipe,
            "params_hex",
            &str_field(recipe, "params_hex")?.to_owned(),
            &codec::hex_encode(want.params),
        )?;
        compare(
            recipe,
            "params_digest",
            &str_field(recipe, "params_digest")?.to_owned(),
            &codec::hex_encode(&params_digest(want.params)),
        )?;
    }
    Ok(())
}

#[test]
fn the_polariton_windows_are_inside_the_cell_grid_and_the_dead_cell_moves_it() -> Result<(), String>
{
    use ising_generators::polariton::POLARITON_V1;
    let doc = polariton_doc()?;
    for case in section(&doc, "cases")? {
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        let (side, _) = polariton_shape(&params)?;
        let m = u64::from(le_u16(&params, 7)?);
        let first = entries_of(case, "instances")?
            .first()
            .ok_or("a case builds no instance")?;
        let meta = with_ice_instance(
            POLARITON_V1,
            case,
            u64_decimal(first, "index")?,
            check_polariton_metadata,
        )?;
        let (c0, r0) = (u64::from(meta.window.0), u64::from(meta.window.1));
        compare(case, "side", &u64::from(meta.side), &side)?;
        compare(
            case,
            "the window fits the cell grid",
            &(c0 >= 1 && r0 >= 1 && c0 + side < 2 * m && r0 + side < 2 * m),
            &true,
        )?;
        let expected = match case_name(case) {
            "polariton-dead-cell" => Some((1, 2)),
            "polariton-l2" | "polariton-l3" | "polariton-l3-permuted" => Some((1, 1)),
            _ => None,
        };
        if let Some(window) = expected {
            compare(case, "window", &(c0, r0), &window)?;
        }
        compare(
            case,
            "the link positions are every instance node",
            &{
                let mut sorted = meta.positions.clone();
                sorted.sort_unstable();
                sorted
            },
            &(0..u32::try_from(meta.positions.len()).map_err(|e| e.to_string())?).collect(),
        )?;
    }
    Ok(())
}

#[test]
fn polariton_admission_codes_match_the_rust_admission() -> Result<(), String> {
    use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits};
    use ising_generators::family::{admit_topology, BaseTopology};
    use ising_generators::polariton::POLARITON_V1;
    let doc = polariton_doc()?;
    let contracts = [POLARITON_V1];
    let mut codes = Vec::new();
    for case in section(&doc, "admission")? {
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
        let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
        let got = admit(
            &contracts,
            str_field(case, "family_id")?.as_bytes(),
            u32_field(case, "revision")?,
            &params,
            &params_digest(&params),
        )
        .and_then(|admitted| {
            admit_topology(
                &admitted,
                &topology,
                topology.order_digest(),
                &AdvertisedLimits {
                    max_nodes: 0,
                    max_edges: 0,
                    max_degree: 0,
                },
            )
        });
        let got = got.err().map(ising_generators::Code::value);
        compare(case, "admission code", &got, &expected_code(case)?)?;
        codes.push(got);
    }
    compare(&doc, "codes 9 present", &codes.contains(&Some(9)), &true)?;
    compare(&doc, "code 12 present", &codes.contains(&Some(12)), &true)?;
    compare(&doc, "an admitted request", &codes.contains(&None), &true)
}

#[test]
fn a_side_16_request_has_no_window_on_the_live_topology() -> Result<(), String> {
    use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits};
    use ising_generators::family::{admit_topology, BaseTopology};
    use ising_generators::polariton::POLARITON_V1;
    // The live z(12, 4) topology has dead qubits. Side 16 builds only on a
    // window without them, so the request gives code 12.
    let doc = polariton_doc()?;
    let recipe = section(&doc, "polariton_recipes")?
        .first()
        .ok_or("no recipe")?;
    let mut params = codec::hex_decode(str_field(recipe, "params_hex")?)?;
    for (at, byte) in [(22, 16u8), (30, 28)] {
        *params.get_mut(at).ok_or("short params")? = byte;
    }
    let (nodes, edges) = topology_inputs(object_field(recipe, "topology")?)?;
    let topology = BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))?;
    let contracts = [POLARITON_V1];
    let admitted = admit(
        &contracts,
        b"quip/polariton-static",
        1,
        &params,
        &params_digest(&params),
    )
    .map_err(|c| format!("admit failed with {c}"))?;
    let got = admit_topology(
        &admitted,
        &topology,
        topology.order_digest(),
        &AdvertisedLimits {
            max_nodes: 0,
            max_edges: 0,
            max_degree: 0,
        },
    )
    .err()
    .map(ising_generators::Code::value);
    compare(&doc, "admission code", &got, &Some(12))
}

#[test]
fn a_changed_polariton_coupling_fails_the_recompute() -> Result<(), String> {
    let doc = polariton_doc()?;
    let pointer = "/cases/0/instances/0/h/0";
    let was = doc
        .pointer(pointer)
        .and_then(Value::as_i64)
        .ok_or("no field")?;
    let broken = mutate(&doc, pointer, json!(was + 4));
    expect_err(&check_polariton(&broken), "a changed field")
}

#[test]
fn a_changed_polariton_witness_fails_the_check() -> Result<(), String> {
    let doc = polariton_doc()?;
    for pointer in [
        "/witnesses/0/feasible",
        "/witnesses/1/feasible",
        "/witnesses/1/loss_milli",
        "/witnesses/1/violations",
        "/witnesses/1/energy_milli",
    ] {
        let was = doc.pointer(pointer).ok_or("no value")?.clone();
        let changed = match was {
            Value::Bool(flag) => json!(!flag),
            Value::Number(n) => json!(n.as_i64().ok_or("not an integer")? + 500),
            _ => return Err("unexpected witness value".to_owned()),
        };
        let broken = mutate(&doc, pointer, changed);
        expect_err(&check_polariton_witnesses(&broken), pointer)?;
    }
    let spins = doc
        .pointer("/witnesses/0/spins")
        .and_then(Value::as_array)
        .ok_or("no spins")?;
    let mut flipped = spins.clone();
    let first = flipped.first_mut().ok_or("no spin")?;
    *first = json!(-first.as_i64().ok_or("not an integer")?);
    let broken = mutate(&doc, "/witnesses/0/spins", Value::Array(flipped));
    expect_err(&check_polariton_witnesses(&broken), "a changed spin")
}

#[test]
fn a_polariton_witness_set_must_cover_every_case_with_both_kinds() -> Result<(), String> {
    let doc = polariton_doc()?;
    let witnesses = section(&doc, "witnesses")?;
    let last = witnesses.len();
    // The retry witnesses replaced by the L2 witnesses under the retry name.
    let mut swapped = witnesses.to_vec();
    for (slot, kind) in [(last - 2, "feasible"), (last - 1, "infeasible")] {
        let source = witnesses
            .iter()
            .find(|w| {
                w.get("name").and_then(Value::as_str) == Some(&format!("polariton-l2-{kind}"))
            })
            .ok_or("no L2 witness")?;
        let mut copy = source.clone();
        for (key, value) in [
            ("name", json!(format!("polariton-retry-{kind}"))),
            ("case", json!("polariton-retry")),
        ] {
            if let Some(field) = copy.get_mut(key) {
                *field = value;
            }
        }
        *swapped.get_mut(slot).ok_or("no slot")? = copy;
    }
    let broken = mutate(&doc, "/witnesses", Value::Array(swapped));
    expect_err(&check_polariton_witnesses(&broken), "renamed witnesses")?;
    // A case with two feasible witnesses and no infeasible one.
    let mut twice = witnesses.to_vec();
    let first = twice.first().ok_or("no witness")?.clone();
    *twice.get_mut(1).ok_or("no slot")? = first;
    let broken = mutate(&doc, "/witnesses", Value::Array(twice));
    expect_err(
        &check_polariton_witnesses(&broken),
        "a missing infeasible witness",
    )?;
    // A case without witnesses.
    let broken = mutate(
        &doc,
        "/witnesses",
        Value::Array(witnesses.iter().take(last - 2).cloned().collect()),
    );
    expect_err(
        &check_polariton_witnesses(&broken),
        "a case without witnesses",
    )
}

#[test]
fn a_higher_energy_infeasible_witness_fails_the_check() -> Result<(), String> {
    use ising_generators::polariton::{polariton_observables, POLARITON, POLARITON_V1};
    let doc = polariton_doc()?;
    let witness = find_named(&doc, "witnesses", "polariton-l3-infeasible")?;
    let case = find_named(&doc, "cases", "polariton-l3")?;
    let slot = u64_decimal(witness, "index")?;
    // All spins up is a consistent witness with more violated labels.
    let replacement = with_ice_instance(POLARITON_V1, case, slot, |params, built| {
        let spins = vec![1i8; built.node_ids().len()];
        let seen = polariton_observables(params, built, &spins).ok_or("no observables")?;
        let feasible = (POLARITON.feasible.ok_or("no hook")?)(params, built, &spins);
        let energy = encoded_energy(built, &spins)?;
        compare(
            witness,
            "the replacement is consistent",
            &energy,
            &i128::from(seen.loss_milli),
        )?;
        compare(witness, "the replacement is infeasible", &feasible, &false)?;
        compare(
            witness,
            "the replacement costs more",
            &(energy > POLARITON_LOW_ENERGY),
            &true,
        )?;
        let mut copy = witness.clone();
        for (key, value) in [
            ("spins", json!(spins)),
            ("charges", json!(seen.charges)),
            ("violations", json!(seen.violations)),
            ("loss_milli", json!(seen.loss_milli)),
            (
                "energy_milli",
                json!(i64::try_from(energy).map_err(|e| e.to_string())?),
            ),
            ("feasible", json!(false)),
        ] {
            if let Some(field) = copy.get_mut(key) {
                *field = value;
            }
        }
        Ok(copy)
    })?;
    let index = section(&doc, "witnesses")?
        .iter()
        .position(|w| w.get("name") == witness.get("name"))
        .ok_or("no witness")?;
    check_polariton_witnesses(&doc)?;
    let broken = mutate(&doc, &format!("/witnesses/{index}"), replacement);
    expect_err(
        &check_polariton_witnesses(&broken),
        "a higher-energy witness",
    )
}

#[test]
fn a_changed_polariton_try_fails_the_replay() -> Result<(), String> {
    let doc = polariton_doc()?;
    let recipe = section(&doc, "polariton_recipes")?
        .first()
        .ok_or("no recipe")?;
    let broken = mutate(&doc, "/polariton_recipes/0/salts/0/try", json!(4));
    let changed = section(&broken, "polariton_recipes")?
        .first()
        .ok_or("no recipe")?;
    expect_err(&check_polariton_recipe(changed), "a changed try")?;
    let broken = mutate(&doc, "/polariton_recipes/0/salts/0/window/0", json!(1));
    let changed = section(&broken, "polariton_recipes")?
        .first()
        .ok_or("no recipe")?;
    expect_err(&check_polariton_recipe(changed), "a changed window")?;
    check_polariton_recipe(recipe)
}

#[test]
fn a_changed_polariton_recipe_hash_fails_the_recompute() -> Result<(), String> {
    let doc = polariton_doc()?;
    let recipe = section(&doc, "polariton_recipes")?
        .first()
        .ok_or("no recipe")?;
    let was = str_field(
        recipe.pointer("/salts/0").ok_or("no salt")?,
        "canonical_blake3",
    )?;
    let broken = mutate(
        &doc,
        "/polariton_recipes/0/salts/0/canonical_blake3",
        json!(flip_last_hex_digit(was)),
    );
    let recipe = section(&broken, "polariton_recipes")?
        .first()
        .ok_or("no recipe")?;
    expect_err(&check_polariton_recipe(recipe), "a changed recipe hash")
}

// ---- topology files (vectors/topologies/) ----

fn vectors_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vectors")
}

/// Every `.json` file below `dir`, sorted.
fn json_files(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        entries.push(entry.map_err(|e| e.to_string())?.path());
    }
    entries.sort();
    for path in entries {
        if path.is_dir() {
            json_files(&path, found)?;
        } else if path.extension().is_some_and(|ext| ext == "json") {
            found.push(path);
        }
    }
    Ok(())
}

/// Every string in `value` that ends in `.bin.gz`.
fn topology_references(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(text) if text.ends_with(".bin.gz") => found.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|v| topology_references(v, found)),
        Value::Object(map) => map.values().for_each(|v| topology_references(v, found)),
        _ => {}
    }
}

/// The `.bin.gz` file names in `vectors/topologies/`, sorted.
fn topology_directory() -> Result<Vec<String>, String> {
    let dir = vectors_dir().join("topologies");
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        let name = name
            .to_str()
            .ok_or("a file name that is not UTF-8")?
            .to_owned();
        if name != "index.json" {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn topology_index() -> Result<Vec<Value>, String> {
    let index: Value =
        serde_json::from_str(quip_solver_conformance::TOPOLOGY_INDEX).map_err(|e| e.to_string())?;
    index
        .as_array()
        .cloned()
        .ok_or_else(|| "the topology index is not an array".to_owned())
}

fn count_field(entry: &Value, key: &str) -> Result<u64, String> {
    entry
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{}: {key} is not a count", case_name(entry)))
}

/// Check one topology file against its index entry.
fn check_topology_entry(entry: &Value, bytes: &[u8]) -> Result<(), String> {
    let name = case_name(entry);
    let body = ising_generators::blob::decode(bytes).map_err(|e| format!("{name}: {e:?}"))?;
    let kind = match body {
        ising_generators::blob::Body::Explicit(_) => "explicit",
        ising_generators::blob::Body::ZephyrRemoved { .. } => "zephyr-removed",
        _ => return Err(format!("{name}: unknown body kind")),
    };
    compare(entry, "kind", &kind, &str_field(entry, "kind")?)?;
    let topology = body.topology().map_err(|e| format!("{name}: {e:?}"))?;
    compare(
        entry,
        "nodes",
        &(topology.nodes.len() as u64),
        &count_field(entry, "nodes")?,
    )?;
    compare(
        entry,
        "edges",
        &(topology.edges.len() as u64),
        &count_field(entry, "edges")?,
    )?;
    compare(
        entry,
        "order_digest",
        &codec::hex_encode(&digests::topology_order_digest(
            &topology.nodes,
            &topology.edges,
        )),
        &str_field(entry, "order_digest")?.to_owned(),
    )?;
    let payload =
        ising_generators::blob::decompress(bytes).map_err(|e| format!("{name}: {e:?}"))?;
    compare(
        entry,
        "payload_blake3",
        &blake3::hash(&payload).to_hex().to_string(),
        &str_field(entry, "payload_blake3")?.to_owned(),
    )
}

#[test]
fn every_topology_reference_resolves() -> Result<(), String> {
    let schema = schema()?;
    let index = topology_index()?;
    let mut names = Vec::new();
    for entry in &index {
        validate_def(&schema, "topology_index_entry", entry)?;
        names.push(case_name(entry).to_owned());
        let file = str_field(entry, "file")?;
        if file != format!("{}.bin.gz", case_name(entry)) {
            return Err(format!(
                "{}: file {file} does not match the name",
                case_name(entry)
            ));
        }
        let bytes = quip_solver_conformance::topology_bytes(&format!("../topologies/{file}"))
            .map_err(|e| e.to_string())?;
        check_topology_entry(entry, bytes)?;
    }
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    if names != sorted {
        return Err("the index is not sorted by name without repeats".to_owned());
    }
    let mut files = Vec::new();
    json_files(&vectors_dir().join("families"), &mut files)?;
    let mut referenced = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut found = Vec::new();
        topology_references(&doc, &mut found);
        for reference in found {
            let target = path
                .parent()
                .ok_or("a file without a directory")?
                .join(&reference);
            let on_disk = std::fs::read(&target)
                .map_err(|e| format!("{}: {reference}: {e}", path.display()))?;
            let embedded = quip_solver_conformance::topology_bytes(&reference)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if on_disk != embedded {
                return Err(format!(
                    "{reference}: the embedded bytes differ from the file"
                ));
            }
            let file = reference
                .rsplit('/')
                .next()
                .ok_or("an empty reference")?
                .to_owned();
            if !index
                .iter()
                .any(|entry| entry.get("file").and_then(Value::as_str) == Some(&file))
            {
                return Err(format!("{reference}: no index entry"));
            }
            referenced.push(file);
        }
    }
    let indexed: Vec<String> = index
        .iter()
        .map(|entry| str_field(entry, "file").map(str::to_owned))
        .collect::<Result<_, _>>()?;
    for file in topology_directory()? {
        if !referenced.contains(&file) {
            return Err(format!("{file}: no JSON file references it"));
        }
        if !indexed.contains(&file) {
            return Err(format!("{file}: not listed in the index"));
        }
    }
    let embedded: Vec<String> = quip_solver_conformance::TOPOLOGY_FILES
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    if embedded != topology_directory()? {
        return Err("TOPOLOGY_FILES does not list every file in vectors/topologies/".to_owned());
    }
    Ok(())
}

#[test]
fn topology_references_cannot_leave_the_topology_directory() {
    use quip_solver_conformance::{topology, TopologyError};
    for reference in [
        "../families/core.json",
        "../topologies/../families/core.json",
        "topologies/t-sparse.bin.gz",
        "../../topologies/t-sparse.bin.gz",
        "../topologies/absent.bin.gz",
        "../topologies/",
        "",
    ] {
        assert!(
            matches!(topology(reference), Err(TopologyError::NotFound(_))),
            "{reference}"
        );
    }
    assert!(topology("../topologies/t-sparse.bin.gz").is_ok());
}

#[test]
fn the_live_topology_file_gives_the_pinned_counts_and_digest() -> Result<(), String> {
    let live = quip_solver_conformance::topology("../topologies/zephyr-z12t4-live.bin.gz")
        .map_err(|e| e.to_string())?;
    compare_value(
        &(live.nodes.len(), live.edges.len()),
        &(4575, 41474),
        "live counts",
    )?;
    compare_value(
        &codec::hex_encode(&digests::topology_order_digest(&live.nodes, &live.edges)),
        &"e12cffc9392cc502c5d065d6e1a3e2eabcc08247ffd55d2fe1b41988d2bc37e3".to_owned(),
        "live order digest",
    )
}

fn compare_value<T: PartialEq + core::fmt::Debug>(
    got: &T,
    want: &T,
    what: &str,
) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}

#[test]
fn topology_reference_pattern_is_enforced() -> Result<(), String> {
    let schema = schema()?;
    validate_def(
        &schema,
        "topology_ref",
        &json!("../topologies/t-sparse.bin.gz"),
    )?;
    for bad in [
        "topologies/t-sparse.bin.gz",
        "../topologies/T-sparse.bin.gz",
        "../topologies/a/b.bin.gz",
        "../topologies/.bin.gz",
        "../other/t-sparse.bin.gz",
        "../topologies/t-sparse.json",
    ] {
        expect_err(&validate_def(&schema, "topology_ref", &json!(bad)), bad)?;
    }
    Ok(())
}
