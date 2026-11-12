//! Exact flow witness and instance codec checks.
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "bounded test fixtures fail on invalid setup"
)]
use ising_generators::{
    codec::CodecError,
    family::{feasible, generate_instance},
    flow::FLOW_V1,
    instance::{Instance, InstanceParts},
    registry::{decode_instance, encode_instance, prepare_witness_codec},
};
#[path = "common/admit.rs"]
mod admit;
#[path = "common/codec.rs"]
mod codec_common;
#[path = "common/lattice.rs"]
mod lattice;
#[path = "common/size_bounds.rs"]
mod size_bounds;

fn params(side: u16, half: u16) -> Vec<u8> {
    let mut bytes = vec![1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 4];
    bytes[7] = side.midpoint(2) as u8;
    for (tag, value) in [(3u16, side), (4, half)] {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}
fn build(bytes: &[u8], seed: u32) -> Instance {
    let admitted = admit::admitted(&FLOW_V1, bytes).expect("admit");
    let topology = lattice::lattice_topology(u16::from(bytes[7]), 4);
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let mut nonce = [0; 32];
    nonce[..4].copy_from_slice(&seed.to_le_bytes());
    generate_instance(&admitted, &topology, &bounds, nonce).expect("generate")
}
fn parts(i: &Instance) -> InstanceParts {
    InstanceParts {
        descriptor_digest: *i.descriptor_digest(),
        topology_order_digest: *i.topology_order_digest(),
        nonce: Some([0; 32]),
        node_ids: i.node_ids().to_vec(),
        edges: i.edges().to_vec(),
        h: i.h().to_vec(),
        j: i.j().to_vec(),
        offset_milli: i.offset_milli(),
        metadata: i.metadata().to_vec(),
    }
}
#[test]
fn flow_instance_roundtrips() {
    let bytes = params(2, 30);
    let admitted = admit::admitted(&FLOW_V1, &bytes).expect("admit");
    for seed in 0..1024 {
        codec_common::assert_instance_roundtrip(&admitted, &build(&bytes, seed));
    }
}
#[test]
fn flow_all_small_feasible_roundtrips() {
    let bytes = params(2, 30);
    let admitted = admit::admitted(&FLOW_V1, &bytes).expect("admit");
    let i = build(&bytes, 0);
    let codec = prepare_witness_codec(&admitted, &i).expect("prepare");
    let mut count = 0;
    for mask in 0..4096 {
        let spins: Vec<i8> = (0..12)
            .map(|b| if mask & (1 << b) == 0 { -1 } else { 1 })
            .collect();
        if feasible(&admitted, &i, &spins) {
            let encoded = codec.encode(&spins).expect("feasible");
            assert_eq!(codec.decode(&encoded).expect("decode"), spins);
            count += 1;
        } else {
            assert_eq!(codec.encode(&spins), Err(CodecError::Infeasible));
        }
    }
    assert!(count > 0);
}
#[test]
fn flow_cycle_size_and_root() {
    for (side, size) in [(2, 1), (8, 8)] {
        let bytes = params(side, 30);
        let admitted = admit::admitted(&FLOW_V1, &bytes).expect("admit");
        let i = build(&bytes, 0);
        let codec = prepare_witness_codec(&admitted, &i).expect("prepare");
        assert_eq!(codec.max_encoded_len(), size);
    }
}
#[test]
fn flow_malformed_properties() {
    let bytes = params(2, 0);
    let admitted = admit::admitted(&FLOW_V1, &bytes).expect("admit");
    let i = build(&bytes, 0);
    let codec = prepare_witness_codec(&admitted, &i).expect("prepare");
    assert_eq!(codec.encode(&[0; 12]), Err(CodecError::SpinValue));
    assert_eq!(codec.encode(&[]), Err(CodecError::Length));
    for seed in 0u32..1024 {
        let payload = seed.to_le_bytes();
        for len in 0..=4 {
            if let Ok(spins) = codec.decode(&payload[..len]) {
                assert_eq!(codec.encode(&spins).expect("re-encode"), payload[..len]);
                assert!(feasible(&admitted, &i, &spins));
            }
        }
    }
}
#[test]
fn flow_instance_decoder_properties() {
    let bytes = params(2, 30);
    let admitted = admit::admitted(&FLOW_V1, &bytes).expect("admit");
    let i = build(&bytes, 0);
    let payload = encode_instance(&admitted, &i).expect("encode");
    for len in 0..payload.len() {
        assert!(decode_instance(&admitted, &payload[..len]).is_err());
    }
    let mut appended = payload.clone();
    appended.push(0);
    assert_eq!(
        decode_instance(&admitted, &appended),
        Err(CodecError::Length)
    );
    for seed in 0..1024usize {
        let mut changed = payload.clone();
        let at = seed % changed.len();
        changed[at] ^= (seed as u8).wrapping_mul(73).wrapping_add(1);
        if let Ok(decoded) = decode_instance(&admitted, &changed) {
            assert_eq!(
                encode_instance(&admitted, &decoded).expect("re-encode"),
                changed
            );
        }
    }
    let mut state = 2311u32;
    for len in 0..1024 {
        let arbitrary: Vec<u8> = (0..len % 256)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        if let Ok(decoded) = decode_instance(&admitted, &arbitrary) {
            assert_eq!(
                encode_instance(&admitted, &decoded).expect("re-encode arbitrary"),
                arbitrary
            );
        }
    }
    let mut changed = payload;
    changed[0] ^= 1;
    assert_eq!(
        decode_instance(&admitted, &changed),
        Err(CodecError::DescriptorMismatch)
    );
    let mut wider = parts(&i);
    wider.h[0] += 1;
    let wider =
        Instance::new(wider, &FLOW_V1.limits, FLOW_V1.metadata_kinds, None).expect("wider image");
    codec_common::assert_instance_roundtrip(&admitted, &wider);
    let mut malformed = parts(&i);
    malformed.metadata[10] = 1;
    let malformed = Instance::new(malformed, &FLOW_V1.limits, FLOW_V1.metadata_kinds, None)
        .expect("canonical metadata");
    assert_eq!(
        encode_instance(&admitted, &malformed),
        Err(CodecError::UnencodableInstance)
    );
}

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let s = std::str::from_utf8(pair).expect("hex");
            u8::from_str_radix(s, 16).expect("hex byte")
        })
        .collect()
}
#[test]
fn flow_python_golden_vectors() {
    let doc: serde_json::Value = serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/codecs/flow.json"
    ))
    .expect("vectors");
    let rows = doc["cases"].as_array().expect("cases");
    assert!(!rows.is_empty());
    for row in rows {
        let kind = row["kind"].as_str().expect("kind");
        if kind == "instance" || kind == "instance_error" {
            let bytes = unhex(row["params"].as_str().expect("params"));
            let admitted = admit::admitted(&FLOW_V1, &bytes).expect("admit");
            let compact = unhex(row["compact"].as_str().expect("compact"));
            if kind == "instance_error" {
                assert_eq!(
                    decode_instance(&admitted, &compact)
                        .expect_err("malformed")
                        .name(),
                    row["error"].as_str().expect("error")
                );
                continue;
            }
            let decoded = decode_instance(&admitted, &compact).expect("Python compact instance");
            assert_eq!(
                decoded.canonical_bytes().expect("canonical"),
                unhex(row["canonical"].as_str().expect("canonical"))
            );
            assert_eq!(
                decoded.digest().to_vec(),
                unhex(row["digest"].as_str().expect("digest"))
            );
            codec_common::assert_instance_roundtrip(&admitted, &decoded);
            let mut witnesses = rows
                .iter()
                .filter(|w| w["instance"] == row["id"])
                .peekable();
            if witnesses.peek().is_none() {
                continue;
            }
            let codec = prepare_witness_codec(&admitted, &decoded).expect("prepare");
            for witness in witnesses {
                let compact = unhex(witness["compact"].as_str().expect("compact"));
                if witness["kind"] == "witness" {
                    let spins: Vec<i8> = witness["spins"]
                        .as_array()
                        .expect("spins")
                        .iter()
                        .map(|v| v.as_i64().expect("spin") as i8)
                        .collect();
                    assert_eq!(codec.encode(&spins).expect("encode"), compact);
                    assert_eq!(codec.decode(&compact).expect("decode"), spins);
                } else {
                    assert_eq!(
                        codec.decode(&compact).expect_err("malformed").name(),
                        witness["error"].as_str().expect("error")
                    );
                }
            }
        }
    }
}
#[test]
fn flow_targets_do_not_transform_spins() {
    let bytes = params(2, 0);
    let admitted = admit::admitted(&FLOW_V1, &bytes).expect("admit");
    let instance = build(&bytes, 0);
    let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    let (payload, spins) = (0u8..16)
        .find_map(|v| codec.decode(&[v]).ok().map(|s| (vec![v], s)))
        .expect("feasible payload");
    let mut changed = parts(&instance);
    let start = changed.metadata.len() - 12 * 5;
    for i in 0..12 {
        changed.metadata[start + 5 * i] = if i % 2 == 0 { 1 } else { 255 };
    }
    let changed = Instance::new(changed, &FLOW_V1.limits, FLOW_V1.metadata_kinds, None)
        .expect("zero cost targets");
    let other = prepare_witness_codec(&admitted, &changed).expect("prepare");
    assert_eq!(other.encode(&spins).expect("encode"), payload);
    assert_eq!(other.decode(&payload).expect("decode"), spins);
    let mut inconsistent = parts(&instance);
    inconsistent.metadata[10] = 1;
    let inconsistent = Instance::new(inconsistent, &FLOW_V1.limits, FLOW_V1.metadata_kinds, None)
        .expect("canonical");
    assert!(prepare_witness_codec(&admitted, &inconsistent).is_err());
}
