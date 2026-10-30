//! Family primitives checked against `vectors/families/core.json`.

use core::cell::Cell;
use core::num::NonZeroU32;
use std::alloc::{GlobalAlloc, Layout, System};

use ising_generators::descriptor::{
    admit, check_family_id, check_size_bounds, params_digest, topology_order_digest,
    AdvertisedLimits, Contract, Descriptor, FieldSpec, IdContext, Params, SizeBounds,
    TopologyStats, Value as ParamValue, ValueType, MAX_PARAMS_LEN,
};
use ising_generators::instance::{validate_metadata, Instance, InstanceLimits, InstanceParts};
use ising_generators::legacy::derive_nonce;
use ising_generators::rng::{stream_seed, uniform, ChaCha8Rng};
use ising_generators::Code;
use serde_json::Value;

fn core() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_CORE).map_err(|e| e.to_string())
}

fn section<'a>(doc: &'a Value, name: &str) -> Result<&'a [Value], String> {
    doc.get(name)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing array section {name}"))
}

fn hex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err(format!("odd hex length {}", s.len()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i + 2)
                .ok_or_else(|| "short hex".to_string())
                .and_then(|p| u8::from_str_radix(p, 16).map_err(|e| e.to_string()))
        })
        .collect()
}

fn hex32(s: &str) -> Result<[u8; 32], String> {
    hex(s)?
        .try_into()
        .map_err(|v: Vec<u8>| format!("expected 32 bytes, got {}", v.len()))
}

#[test]
fn codes_match_the_fixture_table() -> Result<(), String> {
    let doc = core()?;
    let rows = section(&doc, "codes")?;
    if rows.len() != 16 || Code::ALL.len() != 16 {
        return Err(format!(
            "expected 16 codes, fixture {} ALL {}",
            rows.len(),
            Code::ALL.len()
        ));
    }
    if !Code::ALL
        .windows(2)
        .all(|w| matches!(w, [a, b] if a.value() < b.value()))
    {
        return Err("Code::ALL is not ascending".into());
    }
    for row in rows {
        let value = row
            .get("code")
            .and_then(Value::as_u64)
            .ok_or("row without code")?;
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .ok_or("row without name")?;
        let found = Code::ALL
            .iter()
            .find(|c| u64::from(c.value()) == value)
            .ok_or_else(|| format!("no variant for code {value}"))?;
        if found.to_string() != name {
            return Err(format!("code {value}: display {found} but fixture {name}"));
        }
    }
    Ok(())
}

fn nz(n: u64) -> Result<NonZeroU32, String> {
    u32::try_from(n)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| format!("n {n} is not a non-zero u32"))
}

#[test]
fn uniform_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "uniform")? {
        let seed = hex32(case.get("seed").and_then(Value::as_str).ok_or("no seed")?)?;
        let calls = case
            .get("calls")
            .and_then(Value::as_array)
            .ok_or("no calls")?;
        let mut stream = ChaCha8Rng::from_seed(seed);
        let words: Vec<u32> = (0..calls.len() * 64 + 64)
            .map(|_| stream.next_u32())
            .collect();
        for k in 0..calls.len() {
            let mut rng = ChaCha8Rng::from_seed(seed);
            for (i, call) in calls.iter().enumerate().take(k + 1) {
                let n = nz(call.get("n").and_then(Value::as_u64).ok_or("no n")?)?;
                let want = call
                    .get("result")
                    .and_then(Value::as_u64)
                    .ok_or("no result")?;
                let got = uniform(&mut rng, n);
                if u64::from(got) != want {
                    return Err(format!("call {i}: got {got}, want {want}"));
                }
            }
            let used = call_words_used(calls, k)?;
            let next = rng.next_u32();
            if words.get(used) != Some(&next) {
                return Err(format!(
                    "call {k}: words_used {used} does not match the stream"
                ));
            }
        }
    }
    Ok(())
}

fn call_words_used(calls: &[Value], k: usize) -> Result<usize, String> {
    let v = calls
        .get(k)
        .and_then(|c| c.get("words_used"))
        .and_then(Value::as_u64)
        .ok_or("no words_used")?;
    usize::try_from(v).map_err(|e| e.to_string())
}

#[test]
fn uniform_of_one_reads_one_word() -> Result<(), String> {
    let seed = [7u8; 32];
    let mut a = ChaCha8Rng::from_seed(seed);
    let mut b = ChaCha8Rng::from_seed(seed);
    let one = nz(1)?;
    if uniform(&mut a, one) != 0 {
        return Err("uniform(1) is not 0".into());
    }
    let _ = b.next_u32();
    if a.next_u32() != b.next_u32() {
        return Err("uniform(1) did not read exactly one word".into());
    }
    Ok(())
}

#[test]
fn lease_vector_nonces() -> Result<(), String> {
    let doc = core()?;
    let mut checked = 0usize;
    for case in section(&doc, "leases")? {
        if case.get("admitted").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let field = |key: &str| {
            case.get(key)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("lease case without {key}"))
        };
        let last = hex32(field("last_proof_block_hash")?)?;
        let miner = hex32(field("miner_account")?)?;
        let salts = case
            .get("salts")
            .and_then(Value::as_array)
            .ok_or("lease case without salts")?;
        for entry in salts {
            let salt = hex32(
                entry
                    .get("salt")
                    .and_then(Value::as_str)
                    .ok_or("salt entry without salt")?,
            )?;
            let want = hex32(
                entry
                    .get("nonce")
                    .and_then(Value::as_str)
                    .ok_or("salt entry without nonce")?,
            )?;
            if derive_nonce(last, miner, salt) != want {
                return Err(format!("lease {}: nonce differs", field("name")?));
            }
            checked += 1;
        }
    }
    if checked == 0 {
        return Err("no admitted lease salts checked".into());
    }
    Ok(())
}

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

const ALL_TYPES: &[FieldSpec] = &[
    FieldSpec {
        tag: 1,
        kind: ValueType::U8,
        min: 0,
        max: 255,
    },
    FieldSpec {
        tag: 2,
        kind: ValueType::U16,
        min: 0,
        max: 65535,
    },
    FieldSpec {
        tag: 3,
        kind: ValueType::U32,
        min: 0,
        max: 4_294_967_295,
    },
    FieldSpec {
        tag: 4,
        kind: ValueType::U64,
        min: 1,
        max: 18_446_744_073_709_551_615,
    },
    FieldSpec {
        tag: 5,
        kind: ValueType::I32,
        min: -2_147_483_648,
        max: 2_147_483_647,
    },
    FieldSpec {
        tag: 6,
        kind: ValueType::I64,
        min: -72_057_594_037_927_936,
        max: 72_057_594_037_927_936,
    },
    FieldSpec {
        tag: 7,
        kind: ValueType::Bool,
        min: 0,
        max: 1,
    },
    FieldSpec {
        tag: 8,
        kind: ValueType::Bytes,
        min: 0,
        max: 4096,
    },
    FieldSpec {
        tag: 9,
        kind: ValueType::Ascii,
        min: 0,
        max: 64,
    },
];

fn kind_of(name: &str) -> Result<ValueType, String> {
    Ok(match name {
        "u8" => ValueType::U8,
        "u16" => ValueType::U16,
        "u32" => ValueType::U32,
        "u64" => ValueType::U64,
        "i32" => ValueType::I32,
        "i64" => ValueType::I64,
        "bool" => ValueType::Bool,
        "bytes" => ValueType::Bytes,
        "ascii" => ValueType::Ascii,
        other => return Err(format!("unknown type {other}")),
    })
}

fn schema_entry<'a>(doc: &'a Value, family_id: &str) -> Result<&'a Value, String> {
    section(doc, "schemas")?
        .iter()
        .find(|s| s.get("family_id").and_then(Value::as_str) == Some(family_id))
        .ok_or_else(|| format!("no schema {family_id}"))
}

fn schema_fields<'a>(doc: &'a Value, family_id: &str) -> Result<&'a [Value], String> {
    schema_entry(doc, family_id)?
        .get("fields")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| "schema without fields".to_string())
}

/// Tag-to-name map of one fixture schema.
fn schema_names(doc: &Value, family_id: &str) -> Result<Vec<(u16, String)>, String> {
    schema_fields(doc, family_id)?
        .iter()
        .map(|f| {
            let tag = f
                .get("tag")
                .and_then(Value::as_u64)
                .ok_or("field without tag")?;
            let name = f
                .get("name")
                .and_then(Value::as_str)
                .ok_or("field without name")?;
            Ok((
                u16::try_from(tag).map_err(|e| e.to_string())?,
                name.to_string(),
            ))
        })
        .collect()
}

fn bound(f: &Value, key: &str) -> Result<i128, String> {
    f.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("field without {key}"))?
        .parse::<i128>()
        .map_err(|e| e.to_string())
}

fn schema_for(family_id: &str) -> Result<&'static [FieldSpec], String> {
    match family_id {
        "example/uniform-field" => Ok(UNIFORM_FIELD),
        "example/all-types" => Ok(ALL_TYPES),
        other => Err(format!("no schema for {other}")),
    }
}

#[test]
fn test_schemas_match_the_fixture() -> Result<(), String> {
    let doc = core()?;
    for family_id in ["example/uniform-field", "example/all-types"] {
        let fields = schema_fields(&doc, family_id)?;
        let specs = schema_for(family_id)?;
        if fields.len() != specs.len() {
            return Err(format!("{family_id}: field count differs"));
        }
        for (f, spec) in fields.iter().zip(specs) {
            let tag = f.get("tag").and_then(Value::as_u64).ok_or("no tag")?;
            let kind = kind_of(f.get("type").and_then(Value::as_str).ok_or("no type")?)?;
            if tag != u64::from(spec.tag)
                || kind != spec.kind
                || bound(f, "min")? != spec.min
                || bound(f, "max")? != spec.max
            {
                return Err(format!("{family_id}: tag {tag} differs from {spec:?}"));
            }
        }
    }
    Ok(())
}

fn context_of(name: &str) -> Result<IdContext, String> {
    match name {
        "lease" => Ok(IdContext::Lease),
        "direct" => Ok(IdContext::Direct),
        other => Err(format!("unknown context {other}")),
    }
}

fn code_of<T>(result: &Result<T, Code>) -> Option<u8> {
    result.as_ref().err().map(|c| c.value())
}

fn expected(case: &Value) -> Result<Option<u8>, String> {
    match case.get("expected_code") {
        Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| "bad expected_code".to_string()),
        None => Err("case without expected_code".into()),
    }
}

#[test]
fn family_id_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "family_ids")? {
        let name = case.get("name").and_then(Value::as_str).ok_or("no name")?;
        let id = hex(case
            .get("family_id_hex")
            .and_then(Value::as_str)
            .ok_or("no id")?)?;
        let context = context_of(
            case.get("context")
                .and_then(Value::as_str)
                .ok_or("no context")?,
        )?;
        let got = code_of(&check_family_id(&id, context));
        if got != expected(case)? {
            return Err(format!("{name}: got {got:?}"));
        }
    }
    Ok(())
}

fn check_value(kind: ValueType, want: &Value, got: Option<ParamValue<'_>>) -> Result<(), String> {
    let text = || {
        want.as_str()
            .ok_or_else(|| "value is not a string".to_string())
    };
    let ok = match (kind, got) {
        (
            ValueType::U8 | ValueType::U16 | ValueType::U32 | ValueType::U64,
            Some(ParamValue::Unsigned(g)),
        ) => text()?.parse::<u64>().map_err(|e| e.to_string())? == g,
        (ValueType::I32 | ValueType::I64, Some(ParamValue::Signed(g))) => {
            text()?.parse::<i64>().map_err(|e| e.to_string())? == g
        }
        (ValueType::Bool, Some(ParamValue::Bool(g))) => want.as_bool() == Some(g),
        (ValueType::Bytes, Some(ParamValue::Bytes(g))) => hex(text()?)? == g,
        (ValueType::Ascii, Some(ParamValue::Ascii(g))) => hex(text()?)? == g.as_bytes(),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("want {want}, got {got:?}"))
    }
}

#[test]
fn params_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "params")? {
        let name = case.get("name").and_then(Value::as_str).ok_or("no name")?;
        let family_id = case
            .get("family_id")
            .and_then(Value::as_str)
            .ok_or("no family")?;
        let bytes = hex(case
            .get("params_hex")
            .and_then(Value::as_str)
            .ok_or("no params")?)?;
        let schema = schema_for(family_id)?;
        let outcome = Params::parse(schema, &bytes).and_then(|p| p.check_bounds().map(|()| p));
        if code_of(&outcome) != expected(case)? {
            return Err(format!("{name}: got {:?}", code_of(&outcome)));
        }
        let Ok(params) = outcome else { continue };
        if params.bytes() != bytes.as_slice() {
            return Err(format!("{name}: bytes() differs"));
        }
        let want_digest = hex32(
            case.get("params_digest")
                .and_then(Value::as_str)
                .ok_or("no digest")?,
        )?;
        if params_digest(&bytes) != want_digest {
            return Err(format!("{name}: digest differs"));
        }
        let values = case
            .get("values")
            .and_then(Value::as_object)
            .ok_or("no values")?;
        let names = schema_names(&doc, family_id)?;
        if values.len() != names.len() {
            return Err(format!("{name}: values count differs"));
        }
        for ((tag, field), spec) in names.iter().zip(schema) {
            let want = values
                .get(field)
                .ok_or_else(|| format!("{name}: no value {field}"))?;
            check_value(spec.kind, want, params.get(*tag))
                .map_err(|e| format!("{name}/{field}: {e}"))?;
        }
        if params.get(0xFFFF).is_some() {
            return Err(format!("{name}: get of an unknown tag"));
        }
    }
    Ok(())
}

fn named_params(doc: &Value, name: &str) -> Result<Vec<u8>, String> {
    section(doc, "params")?
        .iter()
        .find(|c| c.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|c| c.get("params_hex").and_then(Value::as_str))
        .ok_or_else(|| format!("no params case {name}"))
        .and_then(hex)
}

#[test]
fn layout_beats_bounds() -> Result<(), String> {
    let doc = core()?;
    let with_7f = named_params(&doc, "ascii-65-with-7f")?;
    if code_of(&Params::parse(ALL_TYPES, &with_7f)) != Some(7) {
        return Err("ascii-65-with-7f must give 7 at parse".into());
    }
    let long = named_params(&doc, "ascii-65-bytes")?;
    let parsed = Params::parse(ALL_TYPES, &long).map_err(|c| format!("parse gave {c}"))?;
    if code_of(&parsed.check_bounds()) != Some(9) {
        return Err("ascii-65-bytes must give 9 at check_bounds".into());
    }
    Ok(())
}

#[test]
fn params_longer_than_4096_bytes_fail_before_parsing() -> Result<(), String> {
    let mut bytes = hex("01010002000000020002000400000024faffff")?;
    bytes.resize(MAX_PARAMS_LEN + 1, 0);
    if code_of(&Params::parse(UNIFORM_FIELD, &bytes)) != Some(7) {
        return Err("4097 bytes must give 7".into());
    }
    Ok(())
}

fn str_field<'a>(case: &'a Value, key: &str) -> Result<&'a str, String> {
    case.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string {key}"))
}

fn u32_of(v: &Value) -> Result<u32, String> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("not a u32: {v}"))
}

#[test]
fn descriptor_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "descriptors")? {
        let name = str_field(case, "name")?;
        let id = str_field(case, "family_id")?;
        let revision = u32_of(case.get("revision").ok_or("no revision")?)?;
        let params = hex(str_field(case, "params_hex")?)?;
        let d = Descriptor::new(id.as_bytes(), revision, &params, IdContext::Direct)
            .map_err(|c| format!("{name}: new gave {c}"))?;
        if d.family_id() != id || d.revision() != revision || d.params() != params.as_slice() {
            return Err(format!("{name}: accessors differ"));
        }
        if d.params_digest() != hex32(str_field(case, "params_digest")?)? {
            return Err(format!("{name}: params digest differs"));
        }
        if d.digest() != hex32(str_field(case, "descriptor_digest")?)? {
            return Err(format!("{name}: descriptor digest differs"));
        }
    }
    Ok(())
}

#[test]
fn descriptor_new_order() -> Result<(), String> {
    let uppercase = Descriptor::new(b"Quip/X", 0, &[], IdContext::Direct);
    if code_of(&uppercase) != Some(3) {
        return Err(format!("uppercase id gave {:?}", code_of(&uppercase)));
    }
    let zero = Descriptor::new(b"quip/x", 0, &[], IdContext::Direct);
    if code_of(&zero) != Some(4) {
        return Err(format!("revision zero gave {:?}", code_of(&zero)));
    }
    Ok(())
}

fn edges_of(case: &Value) -> Result<Vec<(u32, u32)>, String> {
    section(case, "edges")?
        .iter()
        .map(|e| {
            let pair = e.as_array().ok_or("edge not an array")?;
            match pair.as_slice() {
                [u, v] => Ok((u32_of(u)?, u32_of(v)?)),
                _ => Err("edge not a pair".to_string()),
            }
        })
        .collect()
}

#[test]
fn topology_vectors() -> Result<(), String> {
    let doc = core()?;
    let mut sparse = Vec::new();
    for case in section(&doc, "topologies")? {
        let name = str_field(case, "name")?;
        let nodes = section(case, "nodes")?
            .iter()
            .map(u32_of)
            .collect::<Result<Vec<_>, _>>()?;
        let edges = edges_of(case)?;
        let got = topology_order_digest(&nodes, &edges).ok_or_else(|| format!("{name}: None"))?;
        if got != hex32(str_field(case, "order_digest")?)? {
            return Err(format!("{name}: digest differs"));
        }
        if name.starts_with("sparse") {
            sparse.push(got);
        }
    }
    if sparse.len() != 4 {
        return Err(format!("expected 4 sparse cases, found {}", sparse.len()));
    }
    for (i, a) in sparse.iter().enumerate() {
        if sparse.iter().skip(i + 1).any(|b| a == b) {
            return Err("two sparse digests are equal".into());
        }
    }
    Ok(())
}

#[test]
fn label_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "labels")? {
        let name = str_field(case, "name")?;
        let label = hex(str_field(case, "label_hex")?)?;
        let valid = case
            .get("valid")
            .and_then(Value::as_bool)
            .ok_or("no valid flag")?;
        if stream_seed(&[0; 32], &[0; 32], &[0; 32], &label).is_ok() != valid {
            return Err(format!("{name}: validity differs from {valid}"));
        }
    }
    Ok(())
}

#[test]
fn stream_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "streams")? {
        let name = str_field(case, "name")?;
        let seed = stream_seed(
            &hex32(str_field(case, "nonce")?)?,
            &hex32(str_field(case, "descriptor_digest")?)?,
            &hex32(str_field(case, "topology_order_digest")?)?,
            &hex(str_field(case, "label_hex")?)?,
        )
        .map_err(|e| format!("{name}: {e}"))?;
        if seed != hex32(str_field(case, "seed")?)? {
            return Err(format!("{name}: seed differs"));
        }
        for checkpoint in section(case, "checkpoints")? {
            let start = checkpoint
                .get("start")
                .and_then(Value::as_u64)
                .ok_or("no start")?;
            let mut rng = ChaCha8Rng::from_seed(seed);
            for _ in 0..start {
                let _skipped = rng.next_u32();
            }
            for (i, want) in section(checkpoint, "words")?.iter().enumerate() {
                if u64::from(rng.next_u32()) != want.as_u64().ok_or("word not a number")? {
                    return Err(format!("{name}: word {i} after {start} differs"));
                }
            }
        }
    }
    Ok(())
}

fn i64_of(v: &Value) -> Result<i64, String> {
    v.as_str()
        .ok_or("not a decimal string")?
        .parse::<i64>()
        .map_err(|e| e.to_string())
}

fn i32_list(case: &Value, key: &str) -> Result<Vec<i32>, String> {
    section(case, key)?
        .iter()
        .map(|v| {
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| format!("{key}: bad i32"))
        })
        .collect()
}

fn instance_parts(case: &Value) -> Result<InstanceParts, String> {
    let doc = core()?;
    let topology = str_field(case, "topology")?;
    let order = section(&doc, "topologies")?
        .iter()
        .find(|t| t.get("name").and_then(Value::as_str) == Some(topology))
        .ok_or_else(|| format!("no topology {topology}"))?;
    let nonce = match str_field(case, "nonce")? {
        "" => None,
        other => Some(hex32(other)?),
    };
    Ok(InstanceParts {
        descriptor_digest: hex32(str_field(case, "descriptor_digest")?)?,
        topology_order_digest: hex32(str_field(order, "order_digest")?)?,
        nonce,
        node_ids: section(case, "node_ids")?
            .iter()
            .map(u32_of)
            .collect::<Result<_, _>>()?,
        edges: edges_of(case)?,
        h: i32_list(case, "h")?,
        j: i32_list(case, "j")?,
        offset_milli: i64_of(case.get("offset_milli").ok_or("no offset_milli")?)?,
        metadata: hex(str_field(case, "metadata_hex")?)?,
    })
}

fn kinds_of(case: &Value, key: &str) -> Result<Vec<u16>, String> {
    section(case, key)?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .ok_or_else(|| format!("{key}: bad kind"))
        })
        .collect()
}

#[test]
fn instance_vectors() -> Result<(), String> {
    let doc = core()?;
    let cases = section(&doc, "instances")?;
    if cases.len() != 4 {
        return Err(format!("expected 4 instances, found {}", cases.len()));
    }
    for case in cases {
        let name = str_field(case, "name")?;
        let kinds = kinds_of(case, "metadata_kinds")?;
        let instance = Instance::new(instance_parts(case)?, &InstanceLimits::FAM_37, &kinds)
            .map_err(|c| format!("{name}: code {c}"))?;
        let want = hex(str_field(case, "canonical_hex")?)?;
        let bytes = instance
            .canonical_bytes()
            .map_err(|e| format!("{name}: {e}"))?;
        if bytes != want {
            return Err(format!("{name}: canonical bytes differ"));
        }
        if instance.canonical_len() != want.len() {
            return Err(format!("{name}: canonical_len differs"));
        }
        if instance.digest() != hex32(str_field(case, "instance_digest")?)? {
            return Err(format!("{name}: digest differs"));
        }
    }
    Ok(())
}

#[test]
fn instance_defect_vectors() -> Result<(), String> {
    let doc = core()?;
    let bases = section(&doc, "instances")?;
    let defects = section(&doc, "instance_defects")?;
    if defects.len() != 5 {
        return Err(format!("expected 5 defects, found {}", defects.len()));
    }
    for defect in defects {
        let name = str_field(defect, "name")?;
        let base_name = str_field(defect, "instance")?;
        let base = bases
            .iter()
            .find(|b| b.get("name").and_then(Value::as_str) == Some(base_name))
            .ok_or_else(|| format!("{name}: no base {base_name}"))?;
        let mut parts = instance_parts(base)?;
        parts.edges = edges_of(defect)?;
        let kinds = kinds_of(base, "metadata_kinds")?;
        let got = Instance::new(parts, &InstanceLimits::FAM_37, &kinds);
        if code_of(&got) != Some(15) || expected(defect)? != Some(15) {
            return Err(format!("{name}: got {:?}", code_of(&got)));
        }
    }
    Ok(())
}

fn one_node_parts(h: i32, j: i32, offset_milli: i64) -> InstanceParts {
    InstanceParts {
        descriptor_digest: [1; 32],
        topology_order_digest: [2; 32],
        nonce: None,
        node_ids: vec![0, 1],
        edges: vec![(0, 1)],
        h: vec![h, 0],
        j: vec![j],
        offset_milli,
        metadata: vec![0, 0],
    }
}

#[test]
fn instance_bound_vectors() -> Result<(), String> {
    let doc = core()?;
    let cases = section(&doc, "instance_bounds")?;
    if cases.len() != 17 {
        return Err(format!("expected 17 bounds, found {}", cases.len()));
    }
    for case in cases {
        let name = str_field(case, "name")?;
        let value = str_field(case, "value")?;
        let parts = match str_field(case, "quantity")? {
            "h" => one_node_parts(value.parse::<i32>().unwrap_or(i32::MAX), 0, 0),
            "j" => one_node_parts(0, value.parse::<i32>().unwrap_or(i32::MAX), 0),
            "offset" => one_node_parts(0, 0, i64_of(case.get("value").ok_or("no value")?)?),
            other => return Err(format!("{name}: quantity {other}")),
        };
        let got = Instance::new(parts, &InstanceLimits::FAM_37, &[]);
        if code_of(&got) != expected(case)? {
            return Err(format!("{name}: got {:?}", code_of(&got)));
        }
    }
    Ok(())
}

#[test]
fn metadata_vectors() -> Result<(), String> {
    let doc = core()?;
    let cases = section(&doc, "metadata")?;
    if cases.len() != 13 {
        return Err(format!("expected 13 metadata cases, found {}", cases.len()));
    }
    for case in cases {
        let name = str_field(case, "name")?;
        let region = hex(str_field(case, "metadata_hex")?)?;
        let kinds = kinds_of(case, "contract_kinds")?;
        let got = validate_metadata(&region, &kinds);
        if code_of(&got) != expected(case)? {
            return Err(format!("{name}: got {:?}", code_of(&got)));
        }
    }
    Ok(())
}

#[test]
fn bounds_beat_edges_beat_metadata() {
    let mut parts = one_node_parts(0, 0, 72_057_594_037_927_937);
    parts.node_ids = vec![0, 1, 2];
    parts.h = vec![0, 0, 0];
    parts.edges = vec![(1, 2), (0, 1)];
    parts.j = vec![0, 0];
    parts.metadata = vec![1, 0, 6, 0, 0, 0, 0, 0];
    let limits = InstanceLimits::FAM_37;
    let code = |p: &InstanceParts| code_of(&Instance::new(p.clone(), &limits, &[1, 2, 3]));
    assert_eq!(code(&parts), Some(13));
    parts.offset_milli = 0;
    assert_eq!(code(&parts), Some(15));
    parts.edges = vec![(0, 1), (1, 2)];
    assert_eq!(code(&parts), Some(16));
}

#[test]
fn length_mismatch_is_code_15() {
    let mut parts = one_node_parts(0, 0, 0);
    parts.h = vec![0];
    assert_eq!(
        code_of(&Instance::new(parts, &InstanceLimits::FAM_37, &[])),
        Some(15)
    );
}

#[test]
fn edge_position_beats_edge_order() {
    let mut parts = one_node_parts(0, 0, 0);
    parts.edges = vec![(1, 0)];
    assert_eq!(
        code_of(&Instance::new(parts.clone(), &InstanceLimits::FAM_37, &[])),
        Some(15)
    );
    parts.edges = vec![(0, 2)];
    assert_eq!(
        code_of(&Instance::new(parts, &InstanceLimits::FAM_37, &[])),
        Some(15)
    );
}

#[test]
fn tighter_limits_apply() {
    let tight = InstanceLimits::new(4, 1, 1_073_741_824, 72_057_594_037_927_936, 67_108_864);
    let mut parts = one_node_parts(0, 0, 0);
    parts.node_ids = vec![0, 1, 2, 3, 4];
    parts.h = vec![0; 5];
    assert_eq!(
        tight.and_then(|l| code_of(&Instance::new(parts, &l, &[])).into()),
        Some(Some(13))
    );
    assert!(InstanceLimits::new(16_777_217, 1, 1, 1, 1).is_none());
    assert!(InstanceLimits::new(1, 67_108_865, 1, 1, 1).is_none());
    assert!(InstanceLimits::new(1, 1, 1_073_741_825, 1, 1).is_none());
    assert!(InstanceLimits::new(1, 1, 1, 72_057_594_037_927_937, 1).is_none());
    assert!(InstanceLimits::new(1, 1, 1, 1, 67_108_865).is_none());
    assert_eq!(
        InstanceLimits::new(
            16_777_216,
            67_108_864,
            1_073_741_824,
            72_057_594_037_927_936,
            67_108_864
        ),
        Some(InstanceLimits::FAM_37)
    );
}

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

const VECTOR_CONTRACT: Contract = Contract {
    family_id: "vectors/uniform-field",
    revision: 1,
    schema: UNIFORM_FIELD,
    metadata_kinds: &[1, 3],
    limits: InstanceLimits::FAM_37,
    size_bounds: example_size_bounds,
};

/// Canonical `UNIFORM_FIELD` bytes for `layers` and `field_milli`.
fn uniform_params(layers: u16, field_milli: i32) -> Vec<u8> {
    let mut b = vec![0x01, 1, 0, 2, 0, 0, 0];
    b.extend_from_slice(&layers.to_le_bytes());
    b.extend_from_slice(&[2, 0, 4, 0, 0, 0]);
    b.extend_from_slice(&field_milli.to_le_bytes());
    b
}

fn num(case: &Value, key: &str) -> Result<u64, String> {
    match case.get(key) {
        Some(Value::Null) | None => Ok(0),
        Some(Value::String(s)) => s.parse().map_err(|_| format!("bad number {key}: {s}")),
        Some(v) => v.as_u64().ok_or_else(|| format!("bad number {key}: {v}")),
    }
}

#[test]
fn size_bound_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "size_bounds")? {
        let name = str_field(case, "name")?;
        let layers = u16::try_from(num(case, "n_layers")?).map_err(|e| e.to_string())?;
        let bytes = uniform_params(layers, 0);
        let params = Params::parse(UNIFORM_FIELD, &bytes).map_err(|c| format!("{name}: {c}"))?;
        let stats = TopologyStats {
            n_nodes: num(case, "n_nodes")?,
            n_edges: num(case, "n_edges")?,
            max_degree: num(case, "max_degree")?,
        };
        let got = example_size_bounds(&params, &stats).map_err(|c| format!("{name}: {c}"))?;
        let want = SizeBounds {
            nodes: num(case, "nodes")?,
            edges: num(case, "edges")?,
            degree: num(case, "degree")?,
        };
        if got != want {
            return Err(format!("{name}: sizes {got:?} want {want:?}"));
        }
        let advertised = AdvertisedLimits {
            max_nodes: num(case, "limit_max_nodes")?,
            max_edges: num(case, "limit_max_edges")?,
            max_degree: num(case, "limit_max_degree")?,
        };
        let code = code_of(&check_size_bounds(
            &got,
            &InstanceLimits::FAM_37,
            &advertised,
        ));
        if code != expected(case)? {
            return Err(format!("{name}: code {code:?}"));
        }
    }
    Ok(())
}

#[test]
fn size_bound_overflow_is_code_13() -> Result<(), String> {
    let bytes = uniform_params(16, 0);
    let params = Params::parse(UNIFORM_FIELD, &bytes).map_err(|c| c.to_string())?;
    let stats = TopologyStats {
        n_nodes: u64::MAX,
        n_edges: 0,
        max_degree: 0,
    };
    if code_of(&example_size_bounds(&params, &stats)) != Some(13) {
        return Err("overflow must give 13".into());
    }
    Ok(())
}

#[test]
fn revision_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "revisions")? {
        let name = str_field(case, "name")?;
        let revision = u32_of(case.get("revision").ok_or("no revision")?)?;
        let want = if expected(case)? == Some(4) { 4 } else { 5 };
        let got = code_of(&admit(&[], b"quip/x", revision, &[], &[0; 32]));
        if got != Some(want) {
            return Err(format!("{name}: gave {got:?}, want {want}"));
        }
    }
    Ok(())
}

#[test]
fn unknown_family_beats_params_length() {
    let big = [0_u8; 4097];
    let got = admit(&[VECTOR_CONTRACT], b"quip/routing", 1, &big, &[0; 32]);
    assert_eq!(code_of(&got), Some(5));
}

#[test]
fn unknown_revision_is_code_6() {
    let got = admit(
        &[VECTOR_CONTRACT],
        b"vectors/uniform-field",
        2,
        &[],
        &[0; 32],
    );
    assert_eq!(code_of(&got), Some(6));
}

#[test]
fn example_namespace_is_code_3_in_a_lease() {
    let got = admit(
        &[VECTOR_CONTRACT],
        b"example/uniform-field",
        1,
        &[],
        &[0; 32],
    );
    assert_eq!(code_of(&got), Some(3));
}

#[test]
fn admission_codes_7_8_9() {
    let id = b"vectors/uniform-field";
    let good = uniform_params(2, -4);
    let digest = params_digest(&good);
    assert!(admit(&[VECTOR_CONTRACT], id, 1, &good, &digest).is_ok());
    let bad = [0x01, 0x01];
    assert_eq!(
        code_of(&admit(
            &[VECTOR_CONTRACT],
            id,
            1,
            &bad,
            &params_digest(&bad)
        )),
        Some(7)
    );
    let short = digest.get(..31).unwrap_or(&[]);
    assert_eq!(
        code_of(&admit(&[VECTOR_CONTRACT], id, 1, &good, short)),
        Some(8)
    );
    assert_eq!(
        code_of(&admit(&[VECTOR_CONTRACT], id, 1, &good, &[0; 32])),
        Some(8)
    );
    let out = uniform_params(2, -4001);
    assert_eq!(
        code_of(&admit(
            &[VECTOR_CONTRACT],
            id,
            1,
            &out,
            &params_digest(&out)
        )),
        Some(9)
    );
}

#[test]
fn admitted_descriptor_reports_size_bounds() -> Result<(), String> {
    let doc = core()?;
    let bytes = named_params(&doc, "spec-valid")?;
    let digest = params_digest(&bytes);
    let contracts = [VECTOR_CONTRACT];
    let admitted = admit(&contracts, b"vectors/uniform-field", 1, &bytes, &digest)
        .map_err(|c| c.to_string())?;
    let stats = TopologyStats {
        n_nodes: 100,
        n_edges: 150,
        max_degree: 4,
    };
    let got = admitted.size_bounds(&stats).map_err(|c| c.to_string())?;
    let want = SizeBounds {
        nodes: 200,
        edges: 400,
        degree: 5,
    };
    if got != want {
        return Err(format!("{got:?}"));
    }
    Ok(())
}

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

/// Counts allocation calls and bytes on the thread that armed `ARMED`.
struct Counting;

// SAFETY: every method delegates to `System` with the arguments it received,
// so `System` upholds the `GlobalAlloc` contract. The counting touches only
// thread-local `Cell`s and never allocates.
#[expect(
    unsafe_code,
    reason = "test-only counting allocator that delegates to System"
)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            let _count = COUNT.try_with(|c| c.set(c.get() + 1));
            let _bytes = BYTES.try_with(|b| b.set(b.get() + layout.size()));
        }
        // SAFETY: the caller upholds the `GlobalAlloc::alloc` contract for `layout`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller passes a pointer from this allocator with its `layout`,
        // and `alloc` returned `System` memory.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Debug, PartialEq, Eq)]
struct Allocs {
    count: usize,
    bytes: usize,
}

const NONE: Allocs = Allocs { count: 0, bytes: 0 };

/// Runs `f` and reports the allocations that this thread made inside it.
fn measure<R>(f: impl FnOnce() -> R) -> (R, Allocs) {
    COUNT.with(|c| c.set(0));
    BYTES.with(|b| b.set(0));
    ARMED.with(|a| a.set(true));
    let result = f();
    ARMED.with(|a| a.set(false));
    let allocs = Allocs {
        count: COUNT.with(Cell::get),
        bytes: BYTES.with(Cell::get),
    };
    (result, allocs)
}

#[test]
fn measure_sees_an_allocation() {
    let (v, allocs) = measure(|| vec![0_u8; 100]);
    assert_eq!(v.len(), 100);
    assert_eq!(
        allocs,
        Allocs {
            count: 1,
            bytes: 100
        }
    );
}

#[test]
fn unknown_family_admission_allocates_nothing() {
    let big = vec![0_u8; 1 << 20];
    let contracts = [VECTOR_CONTRACT];
    let (got, allocs) = measure(|| code_of(&admit(&contracts, b"quip/routing", 1, &big, &[0; 32])));
    assert_eq!(got, Some(5));
    assert_eq!(allocs, NONE);
}

#[test]
fn oversized_params_allocate_nothing() {
    let big = vec![0_u8; MAX_PARAMS_LEN + 1];
    let contracts = [VECTOR_CONTRACT];
    let (got, allocs) = measure(|| {
        code_of(&admit(
            &contracts,
            b"vectors/uniform-field",
            1,
            &big,
            &[0; 32],
        ))
    });
    assert_eq!(got, Some(7));
    assert_eq!(allocs, NONE);
}

#[test]
fn params_vectors_allocate_nothing() -> Result<(), String> {
    let doc = core()?;
    let mut cases = Vec::new();
    for case in section(&doc, "params")? {
        let family_id = str_field(case, "family_id")?;
        let bytes = hex(str_field(case, "params_hex")?)?;
        cases.push((schema_for(family_id)?, bytes));
    }
    let (sum, allocs) = measure(|| {
        let mut sum = 0_u32;
        for (schema, bytes) in &cases {
            let outcome = Params::parse(schema, bytes).and_then(|p| p.check_bounds().map(|()| p));
            sum += u32::from(code_of(&outcome).unwrap_or(0));
            if let Ok(params) = outcome {
                for spec in *schema {
                    sum += u32::from(params.get(spec.tag).is_some());
                }
            }
        }
        sum
    });
    if allocs != NONE {
        return Err(format!("{allocs:?}"));
    }
    if sum == 0 {
        return Err("no case ran".into());
    }
    Ok(())
}

#[test]
fn digests_allocate_nothing() -> Result<(), String> {
    let params = uniform_params(2, -1500);
    let descriptor = Descriptor::new(b"vectors/uniform-field", 1, &params, IdContext::Direct)
        .map_err(|c| c.to_string())?;
    let nodes: Vec<u32> = (0..100_000).collect();
    let edges: Vec<(u32, u32)> = (0..99_999).map(|i| (i, i + 1)).chain([(0, 1)]).collect();
    let (digests, allocs) = measure(|| {
        (
            descriptor.digest(),
            descriptor.params_digest(),
            params_digest(&params),
            topology_order_digest(&nodes, &edges),
            stream_seed(&[1; 32], &[2; 32], &[3; 32], b"fields"),
        )
    });
    if allocs != NONE {
        return Err(format!("{allocs:?}"));
    }
    if digests.3.is_none() || digests.4.is_err() {
        return Err("a digest was refused".into());
    }
    Ok(())
}

#[test]
fn stream_draws_allocate_nothing() -> Result<(), String> {
    let mut rng = ChaCha8Rng::from_seed([7; 32]);
    let n = nz(1000)?;
    let (sum, allocs) = measure(|| {
        let mut sum = 0_u64;
        for _ in 0..1000 {
            sum += u64::from(uniform(&mut rng, n));
        }
        sum
    });
    if allocs != NONE {
        return Err(format!("{allocs:?}"));
    }
    if sum == 0 {
        return Err("all draws were zero".into());
    }
    Ok(())
}

#[test]
fn instance_checks_allocate_nothing() {
    let limits = InstanceLimits::FAM_37;
    let ok = one_node_parts(0, 0, 0);
    let over_bound = one_node_parts(1 << 30 | 1, 0, 0);
    let mut bad_length = one_node_parts(0, 0, 0);
    bad_length.h = vec![0];
    let mut bad_metadata = one_node_parts(0, 0, 0);
    bad_metadata.metadata = vec![1, 0, 9, 0, 0, 0, 0, 0];
    let region = [0_u8, 0];

    let (instance, allocs) = measure(|| Instance::new(ok, &limits, &[]));
    assert_eq!(allocs, NONE);
    assert!(instance.is_ok());

    for (parts, code) in [(over_bound, 13), (bad_length, 15), (bad_metadata, 16)] {
        let (got, allocs) = measure(|| code_of(&Instance::new(parts, &limits, &[])));
        assert_eq!(got, Some(code));
        assert_eq!(allocs, NONE);
    }

    let (got, allocs) = measure(|| code_of(&validate_metadata(&region, &[])));
    assert_eq!(got, None);
    assert_eq!(allocs, NONE);

    if let Ok(instance) = instance {
        let (_digest, allocs) = measure(|| instance.digest());
        assert_eq!(allocs, NONE);
    }
}

#[test]
fn canonical_bytes_allocate_exactly_once() -> Result<(), String> {
    let instance = Instance::new(one_node_parts(5, -7, 9), &InstanceLimits::FAM_37, &[])
        .map_err(|c| c.to_string())?;
    let (bytes, allocs) = measure(|| instance.canonical_bytes());
    let bytes = bytes.map_err(|e| e.to_string())?;
    let want = Allocs {
        count: 1,
        bytes: instance.canonical_len(),
    };
    if allocs != want {
        return Err(format!("{allocs:?}, want {want:?}"));
    }
    if bytes.len() != instance.canonical_len() {
        return Err("length differs from canonical_len".into());
    }
    Ok(())
}

#[test]
fn size_bound_check_allocates_nothing() {
    let bounds = SizeBounds {
        nodes: 200,
        edges: 400,
        degree: 5,
    };
    let advertised = AdvertisedLimits {
        max_nodes: 100,
        max_edges: 0,
        max_degree: 4,
    };
    let (got, allocs) = measure(|| {
        code_of(&check_size_bounds(
            &bounds,
            &InstanceLimits::FAM_37,
            &advertised,
        ))
    });
    assert_eq!(got, Some(13));
    assert_eq!(allocs, NONE);
}
