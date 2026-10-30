//! Canonical JSON, case hashes, and bit-for-bit replay of the legacy
//! production vectors (`golden_vectors.json` and `golden_lease.json`).

use super::{case_name, codec, str_field, u64_decimal};
use quip_protocol::chacha8::{draw_ising_milli, ChaCha8Rng};
use quip_protocol::derive::derive_nonce;
use quip_protocol::lease::{Generator, LeaseSpec, TopologyView};
use serde_json::Value;

/// Canonical JSON: keys sorted, no whitespace, integers, ASCII strings,
/// booleans, null, arrays, and objects only. A float or a non-ASCII string is
/// an error. Keys are sorted here, so the result never depends on map order.
pub(crate) fn canonical_json(value: &Value) -> Result<String, String> {
    let mut out = String::new();
    write_canonical(value, &mut out)?;
    Ok(out)
}

fn write_string(text: &str, out: &mut String) -> Result<(), String> {
    if !text.is_ascii() {
        return Err(format!("non-ASCII string {text:?}"));
    }
    out.push_str(&serde_json::to_string(text).map_err(|e| e.to_string())?);
    Ok(())
}

fn write_canonical(value: &Value, out: &mut String) -> Result<(), String> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => {
            if !number.is_i64() && !number.is_u64() {
                return Err(format!("float {number} in canonical JSON"));
            }
            out.push_str(&number.to_string());
        }
        Value::String(text) => write_string(text, out)?,
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out)?;
                out.push(':');
                write_canonical(item, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Plain BLAKE3 (not `derive_key`) of the canonical JSON bytes, as hex.
pub(crate) fn case_blake3(value: &Value) -> Result<String, String> {
    Ok(blake3::hash(canonical_json(value)?.as_bytes())
        .to_hex()
        .to_string())
}

/// The referenced golden file, parsed.
pub(crate) fn golden_file(file: &str) -> Result<Value, String> {
    let text = match file {
        "golden_vectors.json" => quip_solver_conformance::GOLDEN_VECTORS,
        "golden_lease.json" => quip_solver_conformance::GOLDEN_LEASE,
        other => return Err(format!("unknown legacy file {other}")),
    };
    serde_json::from_str(text).map_err(|e| format!("{file}: {e}"))
}

/// The case a reference names.
pub(crate) fn referenced_case(reference: &Value) -> Result<Value, String> {
    let name = case_name(reference);
    let file = golden_file(str_field(reference, "file")?)?;
    let index = reference
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| format!("{name}: bad index"))?;
    file.get(str_field(reference, "section")?)
        .and_then(Value::as_array)
        .and_then(|cases| cases.get(index))
        .cloned()
        .ok_or_else(|| format!("{name}: no such case in the golden file"))
}

fn field<'a>(case: &'a Value, key: &str) -> Result<&'a Value, String> {
    case.get(key)
        .ok_or_else(|| format!("{}: missing {key}", case_name(case)))
}

fn len_of(case: &Value, key: &str) -> Result<usize, String> {
    field(case, key)?
        .as_array()
        .map(Vec::len)
        .ok_or_else(|| format!("{}: {key} is not an array", case_name(case)))
}

fn check(what: &str, ok: bool) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("replay mismatch: {what}"))
    }
}

fn replay_chacha8(label: &str, case: &Value) -> Result<(), String> {
    let mut rng = ChaCha8Rng::from_seed(codec::hex32(str_field(case, "seed_hex")?)?);
    let words = field(case, "words")?
        .as_array()
        .ok_or_else(|| format!("{label}: words is not an array"))?;
    check(
        &format!("{label} n_words"),
        field(case, "n_words")?.as_u64() == u64::try_from(words.len()).ok(),
    )?;
    for (i, word) in words.iter().enumerate() {
        check(
            &format!("{label} word {i}"),
            word.as_u64() == Some(u64::from(rng.next_u32())),
        )?;
    }
    Ok(())
}

fn replay_derive_nonce(label: &str, case: &Value) -> Result<(), String> {
    let nonce = derive_nonce(
        codec::hex32(str_field(case, "last_proof_hex")?)?,
        codec::hex32(str_field(case, "miner_hex")?)?,
        codec::hex32(str_field(case, "salt_hex")?)?,
    );
    check(
        label,
        codec::hex_encode(&nonce) == str_field(case, "nonce_hex")?,
    )
}

fn replay_ising(label: &str, case: &Value) -> Result<(), String> {
    let (h, j) = draw_ising_milli(
        codec::hex32(str_field(case, "nonce_hex")?)?,
        len_of(case, "nodes")?,
        len_of(case, "edges")?,
        &super::i32_list(case, "allowed_h_milli")?,
        &super::i32_list(case, "allowed_j_milli")?,
    )
    .map_err(|e| format!("{label}: {e:?}"))?;
    check(
        &format!("{label} h"),
        h == super::i32_list(case, "h_milli")?,
    )?;
    check(
        &format!("{label} j"),
        j == super::i32_list(case, "j_milli")?,
    )
}

fn le_hash(values: &[i32]) -> String {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    blake3::hash(&bytes).to_hex().to_string()
}

fn count_of(case: &Value, key: &str) -> Result<usize, String> {
    field(case, key)?
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| format!("{}: bad {key}", case_name(case)))
}

/// A `golden_lease.json` case: topology edge `k` is
/// `(k mod n_nodes, (k + 1) mod n_nodes)`.
fn replay_lease(label: &str, case: &Value) -> Result<(), String> {
    let spec = LeaseSpec::new(
        Generator::Blake3Chacha8V1,
        codec::hex32(str_field(case, "last_proof_block_hash")?)?,
        codec::hex32(str_field(case, "miner_account")?)?,
        codec::hex32(str_field(case, "base_salt")?)?,
        u64_decimal(case, "salt_start")?,
        field(case, "salt_count")?
            .as_u64()
            .ok_or_else(|| format!("{label}: bad salt_count"))?,
    )
    .map_err(|e| format!("{label}: {e}"))?;
    let n_nodes = count_of(case, "n_nodes")?;
    let n_edges = count_of(case, "n_edges")?;
    if n_nodes == 0 {
        return Err(format!("{label}: n_nodes is zero"));
    }
    let topology = TopologyView {
        num_nodes: n_nodes,
        edges: (0..n_edges)
            .map(|k| (k % n_nodes, (k + 1) % n_nodes))
            .collect(),
        allowed_h_milli: super::i32_list(case, "allowed_h_milli")?,
        allowed_j_milli: super::i32_list(case, "allowed_j_milli")?,
    };
    let salts = field(case, "salts")?
        .as_array()
        .ok_or_else(|| format!("{label}: salts is not an array"))?;
    check(
        &format!("{label} salt count"),
        u64::try_from(salts.len()).ok() == Some(spec.salt_count),
    )?;
    for (i, entry) in salts.iter().enumerate() {
        let index = u64::try_from(i).map_err(|e| e.to_string())?;
        let at = format!("{label} salt {i}");
        let salt = spec.salt(index).ok_or_else(|| format!("{at}: no salt"))?;
        let nonce = spec.nonce(index).ok_or_else(|| format!("{at}: no nonce"))?;
        check(
            &format!("{at} salt"),
            codec::hex_encode(&salt) == str_field(entry, "salt")?,
        )?;
        check(
            &format!("{at} index_of"),
            spec.index_of(&salt) == Some(index),
        )?;
        check(
            &format!("{at} nonce"),
            codec::hex_encode(&nonce) == str_field(entry, "nonce")?,
        )?;
        let (h, j) = topology.draw(nonce).map_err(|e| format!("{at}: {e:?}"))?;
        check(
            &format!("{at} h"),
            le_hash(&h) == str_field(entry, "h_blake3")?,
        )?;
        check(
            &format!("{at} j"),
            le_hash(&j) == str_field(entry, "j_blake3")?,
        )?;
    }
    Ok(())
}

/// Replay one referenced case bit for bit.
pub(crate) fn replay(reference: &Value, case: &Value) -> Result<(), String> {
    let label = case_name(reference);
    match str_field(reference, "section")? {
        "chacha8" => replay_chacha8(label, case),
        "derive_nonce" => replay_derive_nonce(label, case),
        "ising" => replay_ising(label, case),
        "cases" => replay_lease(label, case),
        other => Err(format!("{label}: no replay for section {other}")),
    }
}
