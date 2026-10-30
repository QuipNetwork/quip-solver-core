//! Family primitives checked against `vectors/families/core.json`.

use core::num::NonZeroU32;

use ising_generators::descriptor::{
    check_family_id, params_digest, FieldSpec, IdContext, Params, Value as ParamValue, ValueType,
    MAX_PARAMS_LEN,
};
use ising_generators::legacy::derive_nonce;
use ising_generators::rng::{uniform, ChaCha8Rng};
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
