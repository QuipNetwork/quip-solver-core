//! Measure generated instances and exact witnesses against independent inputs.
#![expect(
    clippy::indexing_slicing,
    reason = "indices follow validated generated metadata"
)]
use ising_generators::{
    descriptor::{admit, params_digest, AdvertisedLimits},
    family::{admit_topology, generate_instance, BaseTopology, SaltFault},
    instance::Instance,
    registry::{self, CONTRACTS},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn text<'offset>(v: &'offset Value, key: &str) -> Result<&'offset str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}").into())
}
fn unhex(sections: &str) -> Result<Vec<u8>> {
    sections
        .as_bytes()
        .chunks_exact(2)
        .map(|p| Ok(u8::from_str_radix(std::str::from_utf8(p)?, 16)?))
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| {
            [
                char::from(DIGITS[usize::from(b >> 4)]),
                char::from(DIGITS[usize::from(b & 15)]),
            ]
        })
        .collect()
}

fn word(data: &[u8], offset: usize) -> usize {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]) as usize
}
fn short(data: &[u8], offset: usize) -> usize {
    usize::from(u16::from_le_bytes([data[offset], data[offset + 1]]))
}
fn sections(instance: &Instance) -> BTreeMap<usize, &[u8]> {
    let data = instance.metadata();
    let mut out = BTreeMap::new();
    let mut offset = 2;
    while offset < data.len() {
        let k = short(data, offset);
        let nodes = word(data, offset + 2);
        offset += 6;
        let _ = out.insert(k, &data[offset..offset + nodes]);
        offset += nodes;
    }
    out
}
fn layout(instance: &Instance, family: &str) -> (usize, usize, usize, Option<usize>) {
    let sections = sections(instance);
    let nodes = instance.node_ids().len();
    if family.ends_with("-static") {
        let charges = sections[&4];
        let side = short(charges, 0);
        let w = side + 1;
        let mut pins = BTreeSet::new();
        if let Some(data) = sections.get(&8) {
            let offset = 50 + 4 * short(data, 48);
            for j in 0..short(data, offset) {
                let _ = pins.insert(word(data, offset + 2 + 4 * j));
            }
        }
        let mut adj = vec![Vec::new(); w * w];
        let mut edge = 0;
        for y in 0..w {
            for x in 0..w {
                let tail = y * w + x;
                for head in [
                    if x < side { Some(tail + 1) } else { None },
                    if y < side { Some(tail + w) } else { None },
                ]
                .into_iter()
                .flatten()
                {
                    if !pins.contains(&edge) {
                        adj[tail].push(head);
                        adj[head].push(tail);
                    }
                    edge += 1;
                }
            }
        }
        let mut seen: Vec<_> = charges[2..].iter().map(|&v| v == 127).collect();
        let mut queue: VecDeque<_> = seen
            .iter()
            .enumerate()
            .filter_map(|(v, &yes)| yes.then_some(v))
            .collect();
        let mut tree = 0;
        for root in 0..=w * w {
            if root > 0 && !seen[root - 1] {
                seen[root - 1] = true;
                queue.push_back(root - 1);
            }
            while let Some(v) = queue.pop_front() {
                for &other in &adj[v] {
                    if !seen[other] {
                        seen[other] = true;
                        queue.push_back(other);
                        tree += 1;
                    }
                }
            }
        }
        return (0, 0, 0, Some(nodes - pins.len() - tree));
    }
    let mut used = vec![false; nodes];
    let (mut pairs, mut zero, mut charged) = (0, 0, 0);
    let mut select = |positions: &[usize]| {
        if positions.iter().any(|&p| used[p]) {
            return false;
        }
        for &p in positions {
            used[p] = true;
        }
        true
    };
    if let Some(data) = sections.get(&3) {
        for j in 0..word(data, 0) {
            if select(&[word(data, 4 + 8 * j), word(data, 8 + 8 * j)]) {
                pairs += 1;
            }
        }
    }
    if let Some(data) = sections.get(&7) {
        let offset = 4 + 6 * word(data, 0);
        for j in 0..word(data, offset) {
            let start = offset + 4 + 24 * j;
            if select(&[word(data, start + 12), word(data, start + 16)]) {
                pairs += 1;
            }
        }
    }
    if let Some(data) = sections.get(&1) {
        for j in 0..word(data, 0) {
            let p: Vec<_> = (0..4).map(|k| word(data, 4 + 16 * j + 4 * k)).collect();
            if select(&p) {
                if sections.get(&6).is_some_and(|charges| charges[4 + j] != 0) {
                    charged += 1;
                } else {
                    zero += 1;
                }
            }
        }
    }
    (pairs, zero, charged, None)
}
fn column_size(v: &[u32]) -> usize {
    let w = 32 - v.iter().copied().max().unwrap_or(0).leading_zeros();
    1 + (v.len() * w as usize).div_ceil(8)
}
fn palette_size(v: &[i32]) -> usize {
    let k = v.iter().collect::<BTreeSet<_>>().len();
    let w = usize::BITS - k.saturating_sub(1).leading_zeros();
    5 + 4 * k + (v.len() * w as usize).div_ceil(8)
}
fn instance_formula(instance: &Instance, family: &str) -> Value {
    let sections = sections(instance);
    let nodes = instance.node_ids().len();
    let mut columns = Vec::new();
    let mut add = |values: Vec<u32>| {
        columns.push(json!({"count":values.len(),"width":32-values.iter().copied().max().unwrap_or(0).leading_zeros()}));
    };
    add(instance.node_ids().to_vec());
    add(instance.edges().iter().flat_map(|&(u, v)| [u, v]).collect());
    let mut scalar = 112;
    for values in [instance.h(), instance.j()] {
        let palette: BTreeSet<_> = values.iter().copied().collect();
        let ranks: BTreeMap<_, _> = palette
            .iter()
            .enumerate()
            .map(|(rank, &value)| (value, u32::try_from(rank).unwrap_or(u32::MAX)))
            .collect();
        scalar += 4 + 4 * palette.len();
        add(values.iter().map(|v| ranks[v]).collect());
    }
    let words = |data: &[u8], start: usize, count: usize| {
        (0..count)
            .map(|i| u32::try_from(word(data, start + 4 * i)).unwrap_or(u32::MAX))
            .collect::<Vec<_>>()
    };
    let mut arrays = Vec::new();
    if family.ends_with("-static") {
        let side = short(sections[&4], 0);
        scalar += 4;
        arrays.push(if family == "quip/flow-static" {
            4 * (side - 1)
        } else {
            2 * (side - 1) * (side - 1)
        });
        add(words(sections[&5], 4, nodes));
        if let Some(data) = sections.get(&8) {
            scalar += 52;
            let obstacles = short(data, 48);
            add(words(data, 50, obstacles));
            let offset = 50 + 4 * obstacles;
            add(words(data, offset + 2, short(data, offset)));
        }
        if let Some(data) = sections.get(&9) {
            scalar += 8;
            arrays.push(nodes);
            add((0..nodes)
                .map(|i| u32::try_from(word(data, 9 + 5 * i)).unwrap_or(u32::MAX))
                .collect());
        }
    } else if let Some(data) = sections.get(&7) {
        let cells = word(data, 0);
        let offset = 4 + 6 * cells;
        let arcs = word(data, offset);
        scalar += 10;
        arrays.push(4 * cells);
        add((0..cells)
            .flat_map(|i| {
                [
                    u32::try_from(short(data, 4 + 6 * i)).unwrap_or(0),
                    u32::try_from(short(data, 6 + 6 * i)).unwrap_or(0),
                ]
            })
            .collect());
        add((0..arcs)
            .flat_map(|i| words(data, offset + 4 + 24 * i, 5))
            .collect());
        add((0..arcs)
            .flat_map(|i| {
                [
                    u32::try_from(short(data, offset + 24 + 24 * i)).unwrap_or(0),
                    u32::try_from(short(data, offset + 26 + 24 * i)).unwrap_or(0),
                ]
            })
            .collect());
    } else {
        let tiles = word(sections[&1], 0);
        scalar += 4;
        add(words(sections[&1], 4, 4 * tiles));
        arrays.push(
            if family == "quip/ice-glass" || family == "quip/ice-fields" {
                nodes
            } else {
                4 * tiles
            },
        );
        if sections.contains_key(&6) {
            arrays.push(2 * tiles);
        }
        if let Some(data) = sections.get(&3) {
            scalar += 4;
            add(words(data, 4, 2 * word(data, 0)));
        }
    }
    json!({"scalar_bytes":scalar,"columns":columns,"bit_arrays":arrays})
}
fn refusal(input: &Value, status: &str, code: u8) -> Result<Value> {
    if input["expected_status"] != status || input["expected_code"] != code {
        return Err(format!(
            "oracle refusal differs: {}: {status}/{code}",
            text(input, "case")?
        )
        .into());
    }
    let mut row = json!({"family_id": text(input,"family_id")?, "revision":1,
        "case":text(input,"case")?, "params_hex":text(input,"params_hex")?, "nonce_hex":text(input,"nonce_hex")?,
        "status":status,"instance_status":status,"witness_status":status,"error_code":code});
    for key in [
        "canonical_instance_bytes",
        "canonical_instance_bits",
        "compact_instance_bytes",
        "compact_instance_bits",
        "one_bit_witness_bits",
        "one_bit_witness_bytes",
        "compact_witness_bits",
        "compact_witness_bytes",
    ] {
        row[key] = Value::Null;
    }
    Ok(row)
}
#[expect(
    clippy::too_many_lines,
    reason = "linear measurement pipeline keeps every recorded field beside its verification"
)]
fn measure(input: &Value) -> Result<Vec<Value>> {
    let family = text(input, "family_id")?;
    let params = unhex(text(input, "params_hex")?)?;
    let admission = admit(
        CONTRACTS,
        family.as_bytes(),
        1,
        &params,
        &params_digest(&params),
    );
    if let Err(code) = admission {
        return Ok(vec![refusal(input, "rejected", code.value())?]);
    }
    let admitted = admission?;
    let nodes: Vec<u32> = serde_json::from_value(input["topology_nodes"].clone())?;
    let edges: Vec<(u32, u32)> = serde_json::from_value(input["topology_edges"].clone())?;
    let top = BaseTopology::new(nodes, &edges).map_err(|edges| format!("topology: {edges:?}"))?;
    let nonce: [u8; 32] = unhex(text(input, "nonce_hex")?)?
        .try_into()
        .map_err(|_| "nonce")?;
    let generated = match admit_topology(
        &admitted,
        &top,
        top.order_digest(),
        &AdvertisedLimits::default(),
    ) {
        Err(code) => Err(("rejected", code.value())),
        Ok(bounds) => match generate_instance(&admitted, &top, &bounds, nonce) {
            Ok(instance) => Ok(instance),
            Err(SaltFault::Skip(c)) => Err(("skipped", c.value())),
            Err(edges) => return Err(format!("generator defect: {edges:?}").into()),
        },
    };
    let mut base = json!({"family_id":family,"revision":1,"case":text(input,"case")?,"params_hex":text(input,"params_hex")?,"nonce_hex":text(input,"nonce_hex")?,"topology_digest":hex(top.order_digest())});
    let instance = match generated {
        Err((status, code)) => {
            return Ok(vec![refusal(input, status, code)?]);
        }
        Ok(instance) => instance,
    };
    let canonical = instance.canonical_bytes()?;
    if canonical != unhex(text(input, "canonical_hex")?)? {
        return Err(format!("oracle canonical mismatch: {}", text(input, "case")?).into());
    }
    let compact = registry::encode_instance(&admitted, &instance)?;
    let decoded = registry::decode_instance(&admitted, &compact)?;
    if decoded.canonical_bytes()? != canonical
        || decoded.digest() != instance.digest()
        || registry::encode_instance(&admitted, &decoded)? != compact
    {
        return Err("instance roundtrip".into());
    }
    let codec = registry::prepare_witness_codec(&admitted, &instance)?;
    let nodes = instance.node_ids().len();
    let edges = instance.edges().len();
    let endpoints: Vec<u32> = instance.edges().iter().flat_map(|&(u, v)| [u, v]).collect();
    let prefix = 112
        + column_size(instance.node_ids())
        + column_size(&endpoints)
        + palette_size(instance.h())
        + palette_size(instance.j());
    let (pairs, zero, charged, independent) = layout(&instance, family);
    base["instance_formula"] = instance_formula(&instance, family);
    let metadata = sections(&instance);
    base["metadata_tiles"] = json!(metadata.get(&1).map(|data| word(data, 0)));
    base["metadata_pairs"] = json!(metadata.get(&3).map(|data| word(data, 0)));
    base["retained_arcs"] = json!(metadata
        .get(&7)
        .map(|data| word(data, 4 + 6 * word(data, 0))));
    base["fallback_available"] = json!(independent.is_none() && pairs + zero + charged > 0);
    base["instance_status"] = json!("measured");
    base["nodes"] = json!(nodes);
    base["edges"] = json!(edges);
    base["metadata_bytes"] = json!(instance.metadata().len());
    base["instance_digest"] = json!(hex(&instance.digest()));
    base["canonical_instance_bytes"] = json!(canonical.len());
    base["canonical_instance_bits"] = json!(8 * canonical.len());
    base["compact_instance_bytes"] = json!(compact.len());
    base["compact_instance_bits"] = json!(8 * compact.len());
    base["prefix_bytes"] = json!(prefix);
    base["compact_metadata_bytes"] = json!(compact.len() - prefix);
    base["instance_savings"] = json!(
        1.0 - f64::from(u32::try_from(compact.len())?) / f64::from(u32::try_from(canonical.len())?)
    );
    base["pairs"] = json!(pairs);
    base["zero_tiles"] = json!(zero);
    base["charged_tiles"] = json!(charged);
    base["independent_bits"] = json!(independent);
    let mut rows = Vec::new();
    for witness in input["witnesses"].as_array().ok_or("witnesses")? {
        let spins: Vec<i8> = serde_json::from_value(witness["spins"].clone())?;
        let data = codec.encode(&spins)?;
        if codec.decode(&data)? != spins {
            return Err("witness roundtrip".into());
        }
        let fallback = independent.is_none() && data[0] & 1 == 1;
        let bits = independent.unwrap_or(
            1 + nodes
                - if fallback {
                    0
                } else {
                    pairs + zero + 2 * charged
                },
        );
        if bits.div_ceil(8) != data.len() {
            return Err("witness formula".into());
        }
        let mut row = base.clone();
        row["status"] = json!("measured");
        row["witness_status"] = json!("measured");
        row["mode"] = witness["mode"].clone();
        row["one_bit_witness_bits"] = json!(nodes);
        row["one_bit_witness_bytes"] = json!(nodes.div_ceil(8));
        row["compact_witness_bits"] = json!(bits);
        row["compact_witness_bytes"] = json!(data.len());
        row["spins_blake3"] = json!(hex(blake3::hash(
            &spins
                .iter()
                .map(|sections| sections.to_le_bytes()[0])
                .collect::<Vec<_>>()
        )
        .as_bytes()));
        row["witness_savings"] = json!(
            1.0 - f64::from(u32::try_from(data.len())?)
                / f64::from(u32::try_from(nodes.div_ceil(8))?)
        );
        rows.push(row);
    }
    Ok(rows)
}
#[expect(
    clippy::print_stdout,
    reason = "the example emits its requested JSON or Markdown to stdout"
)]
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3
        || args[0] != "--inputs"
        || !["--json", "--markdown"].contains(&args[2].as_str())
    {
        return Err("usage: witness_codec_sizes --inputs PATH --json|--markdown".into());
    }
    let inputs: Vec<Value> = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let mut rows = Vec::new();
    for input in &inputs {
        rows.extend(measure(input)?);
    }
    if args[2] == "--json" {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        println!("| Family / case / mode | Canonical instance bits / bytes | Compact instance bits / bytes | One-bit witness bits / bytes | Compact witness bits / bytes | Status |\n|---|---:|---:|---:|---:|---|");
        for r in &rows {
            println!(
                "| {} / {} / {} | {} / {} | {} / {} | {} / {} | {} / {} | {} |",
                text(r, "family_id")?,
                text(r, "case")?,
                r["mode"].as_str().unwrap_or("unavailable"),
                r["canonical_instance_bits"],
                r["canonical_instance_bytes"],
                r["compact_instance_bits"],
                r["compact_instance_bytes"],
                r["one_bit_witness_bits"],
                r["one_bit_witness_bytes"],
                r["compact_witness_bits"],
                r["compact_witness_bytes"],
                text(r, "status")?
            );
        }
    }
    Ok(())
}
