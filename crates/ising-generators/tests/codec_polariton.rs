//! Polariton compact codec conformance.
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test failures identify invalid fixtures"
)]
use ising_generators::{
    family::{feasible, generate_instance},
    polariton::POLARITON_V1,
    registry::{decode_instance, encode_instance, prepare_witness_codec},
};
#[path = "common/admit.rs"]
mod admit;
#[path = "common/codec.rs"]
mod codec;
#[path = "common/lattice.rs"]
mod lattice;
#[path = "common/size_bounds.rs"]
mod size_bounds;
fn params(side: u16) -> Vec<u8> {
    let mut parameter_bytes = vec![
        1, 1, 0, 2, 0, 0, 0, 6, 0, 2, 0, 1, 0, 0, 0, 1, 3, 0, 2, 0, 0, 0,
    ];
    parameter_bytes.extend(side.to_le_bytes());
    parameter_bytes.extend([4, 0, 2, 0, 0, 0, 0, 0]);
    parameter_bytes
}
#[test]
fn polariton_all_small_feasible_roundtrips() {
    let parameter_bytes = params(2);
    let admitted = admit::admitted(&POLARITON_V1, &parameter_bytes).expect("admit");
    let topology = lattice::lattice_topology(6, 1);
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let instance = generate_instance(&admitted, &topology, &bounds, [0; 32]).expect("generate");
    codec::assert_instance_roundtrip(&admitted, &instance);
    let witness_codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    let mut count = 0;
    for bits in 0..4096u32 {
        let spins: Vec<i8> = (0..12)
            .map(|k| if bits >> k & 1 == 1 { 1 } else { -1 })
            .collect();
        if feasible(&admitted, &instance, &spins) {
            count += 1;
            assert_eq!(
                witness_codec
                    .decode(&witness_codec.encode(&spins).expect("encode"))
                    .expect("decode"),
                spins
            );
        }
    }
    assert_eq!(count, 1536);
}
#[test]
fn polariton_instance_roundtrips() {
    let topology = lattice::lattice_topology(6, 1);
    let mut successes = 0;
    let mut skips = 0;
    for group in 0..3u16 {
        let mut parameter_bytes = params(group + 2);
        if group != 0 {
            let at = parameter_bytes.len() - 2;
            parameter_bytes[at] = 1;
        }
        let admitted = admit::admitted(&POLARITON_V1, &parameter_bytes).expect("admit");
        let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
        for seed in (u32::from(group)..1024).step_by(3) {
            let mut nonce = [0; 32];
            nonce[..4].copy_from_slice(&seed.to_le_bytes());
            let Ok(instance) = generate_instance(&admitted, &topology, &bounds, nonce) else {
                skips += 1;
                continue;
            };
            successes += 1;
            codec::assert_instance_roundtrip(&admitted, &instance);
            let bytes = encode_instance(&admitted, &instance).expect("encode");
            let decoded = decode_instance(&admitted, &bytes).expect("decode");
            assert_eq!(decoded.node_ids(), instance.node_ids());
            assert_eq!(decoded.metadata(), instance.metadata());
            assert_eq!(decoded.descriptor_digest(), instance.descriptor_digest());
            assert_eq!(
                decoded.topology_order_digest(),
                instance.topology_order_digest()
            );
            assert_eq!(decoded.nonce(), instance.nonce());
        }
    }
    assert_eq!(successes + skips, 1024);
    assert_eq!(skips, 0, "construction skips");
}

#[test]
fn polariton_python_goldens() {
    let rows: serde_json::Value = serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/codecs/polariton.json"
    ))
    .expect("json");
    for row in rows["cases"].as_array().expect("cases") {
        let parameter_bytes = hex(row["params"].as_str().expect("params"));
        let admitted = admit::admitted(&POLARITON_V1, &parameter_bytes).expect("admit");
        let compact = hex(row["compact_instance"].as_str().expect("compact"));
        let instance = decode_instance(&admitted, &compact).expect("decode");
        assert_eq!(
            instance.canonical_bytes().expect("canonical"),
            hex(row["canonical"].as_str().expect("canonical"))
        );
        codec::assert_instance_roundtrip(&admitted, &instance);
        let spins: Vec<i8> = row["spins"]
            .as_array()
            .expect("spins")
            .iter()
            .map(|v| i8::try_from(v.as_i64().expect("spin")).expect("i8"))
            .collect();
        let witness_codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
        let witness = hex(row["witness"].as_str().expect("witness"));
        assert_eq!(witness_codec.encode(&spins).expect("encode"), witness);
        assert_eq!(witness_codec.decode(&witness).expect("decode"), spins);
        for negative in row["negative_witness"]
            .as_array()
            .expect("negative witnesses")
        {
            let error = if let Some(data) = negative["bytes"].as_str() {
                witness_codec
                    .decode(&hex(data))
                    .expect_err("reject witness")
            } else {
                let spins: Vec<i8> = negative["spins"]
                    .as_array()
                    .expect("spins")
                    .iter()
                    .map(|v| i8::try_from(v.as_i64().expect("spin")).expect("i8"))
                    .collect();
                witness_codec.encode(&spins).expect_err("reject spins")
            };
            assert_eq!(error.name(), negative["error"].as_str().expect("error"));
        }
        for negative in row["negative_instance"].as_array().expect("negatives") {
            assert_eq!(
                decode_instance(&admitted, &hex(negative["bytes"].as_str().expect("bytes")))
                    .expect_err("reject")
                    .name(),
                negative["error"].as_str().expect("error")
            );
        }
    }
}
fn hex(spins: &str) -> Vec<u8> {
    spins
        .as_bytes()
        .chunks_exact(2)
        .map(|witness_codec| {
            u8::from_str_radix(std::str::from_utf8(witness_codec).expect("utf8"), 16).expect("hex")
        })
        .collect()
}

#[test]
fn polariton_order_and_size() {
    use ising_generators::{family::BaseTopology, ice::zephyr_lattice};
    let parameter_bytes = params(8);
    let admitted = admit::admitted(&POLARITON_V1, &parameter_bytes).expect("admit");
    let (mut nodes, edges) = zephyr_lattice(6, 1).expect("lattice");
    nodes.reverse();
    let topology = BaseTopology::new(nodes, &edges).expect("topology");
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let instance = generate_instance(&admitted, &topology, &bounds, [0; 32]).expect("generate");
    let witness_codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    let spins = vec![1; 144];
    assert!(feasible(&admitted, &instance, &spins));
    let bytes = witness_codec.encode(&spins).expect("encode");
    assert_eq!(bytes.len(), 12);
    assert_eq!(spins.len().div_ceil(8), 18);
    assert_eq!(witness_codec.decode(&bytes).expect("decode"), spins);
    let reverse = vec![-1; 144];
    let reversed = witness_codec.encode(&reverse).expect("reverse");
    assert_ne!(bytes, reversed);
    assert_eq!(
        witness_codec.decode(&reversed).expect("decode reverse"),
        reverse
    );
}

#[test]
fn polariton_rejects_infeasible_and_padding() {
    use ising_generators::codec::CodecError;
    let parameter_bytes = params(2);
    let admitted = admit::admitted(&POLARITON_V1, &parameter_bytes).expect("admit");
    let topology = lattice::lattice_topology(6, 1);
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let instance = generate_instance(&admitted, &topology, &bounds, [0; 32]).expect("generate");
    let witness_codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    assert_eq!(witness_codec.encode(&[1; 11]), Err(CodecError::Length));
    assert_eq!(witness_codec.encode(&[0; 12]), Err(CodecError::SpinValue));
    let mut rejected = false;
    for k in 0..12 {
        let mut spins = vec![1; 12];
        spins[k] = -1;
        if !feasible(&admitted, &instance, &spins) {
            assert_eq!(witness_codec.encode(&spins), Err(CodecError::Infeasible));
            rejected = true;
            break;
        }
    }
    assert!(rejected);
    let mut data = witness_codec.encode(&[1; 12]).expect("encode");
    data[1] |= 128;
    assert_eq!(witness_codec.decode(&data), Err(CodecError::Padding));
    assert_eq!(witness_codec.decode(&[0; 3]), Err(CodecError::Length));
    for len in 0..=witness_codec.max_encoded_len() + 2 {
        let zeros = vec![0; len];
        match witness_codec.decode(&zeros) {
            Ok(spins) => {
                assert_eq!(len, data.len());
                assert_eq!(witness_codec.encode(&spins).expect("re-encode"), zeros);
            }
            Err(error) if len != data.len() => assert_eq!(error, CodecError::Length),
            Err(_) => {}
        }
    }
}

#[test]
fn polariton_instance_decoder_properties() {
    let parameter_bytes = params(2);
    let admitted = admit::admitted(&POLARITON_V1, &parameter_bytes).expect("admit");
    let topology = lattice::lattice_topology(6, 1);
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let instance = generate_instance(&admitted, &topology, &bounds, [0; 32]).expect("generate");
    let bytes = encode_instance(&admitted, &instance).expect("encode");
    for end in 0..bytes.len() {
        assert!(decode_instance(&admitted, &bytes[..end]).is_err());
    }
    let witness_codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    let mut seed = 2311u64;
    for _ in 0..1024 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let mut data = bytes.clone();
        let at = usize::try_from(seed >> 32).expect("usize") % data.len();
        data[at] ^= 1 << (seed % 8);
        if let Ok(decoded) = decode_instance(&admitted, &data) {
            assert_eq!(encode_instance(&admitted, &decoded).expect("encode"), data);
        }
        let random = seed.to_le_bytes();
        let data = &random[..2];
        if let Ok(spins) = witness_codec.decode(data) {
            assert!(feasible(&admitted, &instance, &spins));
            assert_eq!(witness_codec.encode(&spins).expect("encode"), data);
        }
        let data: Vec<u8> = (0..bytes.len()).map(|k| random[k % 8]).collect();
        if let Ok(decoded) = decode_instance(&admitted, &data) {
            assert_eq!(encode_instance(&admitted, &decoded).expect("encode"), data);
        }
    }
}

#[test]
fn polariton_instance_wider_image_and_bounds() {
    use ising_generators::{
        codec::CodecError,
        instance::{Instance, InstanceParts},
    };
    let parameter_bytes = params(2);
    let admitted = admit::admitted(&POLARITON_V1, &parameter_bytes).expect("admit");
    let topology = lattice::lattice_topology(6, 1);
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let instance = generate_instance(&admitted, &topology, &bounds, [0; 32]).expect("generate");
    let make = |metadata: Vec<u8>| {
        Instance::new(
            InstanceParts {
                descriptor_digest: *instance.descriptor_digest(),
                topology_order_digest: [9; 32],
                nonce: None,
                node_ids: instance.node_ids().to_vec(),
                edges: vec![(0, 1)],
                h: vec![13; 12],
                j: vec![-17],
                offset_milli: -123,
                metadata,
            },
            &POLARITON_V1.limits,
            POLARITON_V1.metadata_kinds,
            None,
        )
        .expect("canonical instance")
    };
    let wider = make(instance.metadata().to_vec());
    codec::assert_instance_roundtrip(&admitted, &wider);
    assert!(prepare_witness_codec(&admitted, &wider).is_err());
    let mut malformed = instance.metadata().to_vec();
    malformed[10] = 0;
    assert_eq!(
        encode_instance(&admitted, &make(malformed)),
        Err(CodecError::UnencodableInstance)
    );
    let bytes = encode_instance(&admitted, &instance).expect("encode");
    for at in [96, 100] {
        let mut data = bytes.clone();
        data[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            decode_instance(&admitted, &data).err(),
            Some(CodecError::BoundsExceeded)
        );
    }
    let mut data = bytes.clone();
    data[112] = 33;
    assert_eq!(
        decode_instance(&admitted, &data).err(),
        Some(CodecError::InvalidSymbol)
    );
    let mut data = bytes;
    data[0] ^= 1;
    assert_eq!(
        decode_instance(&admitted, &data).err(),
        Some(CodecError::DescriptorMismatch)
    );
}

#[test]
fn polariton_existing_instance_fixtures() {
    use ising_generators::instance::{Instance, InstanceParts};
    let fixtures: serde_json::Value = serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/polariton.json"
    ))
    .expect("fixtures");
    let integers = |v: &serde_json::Value| -> Vec<i32> {
        v.as_array()
            .expect("array")
            .iter()
            .map(|v| i32::try_from(v.as_i64().expect("integer")).expect("i32"))
            .collect()
    };
    for case in fixtures["cases"].as_array().expect("cases") {
        let parameter_bytes = hex(case["params_hex"].as_str().expect("params"));
        let admitted = admit::admitted(&POLARITON_V1, &parameter_bytes).expect("admit");
        for row in case["instances"].as_array().expect("instances") {
            let canonical = hex(row["canonical_hex"].as_str().expect("canonical"));
            let instance = Instance::new(
                InstanceParts {
                    descriptor_digest: canonical[1..33].try_into().expect("digest"),
                    topology_order_digest: canonical[33..65].try_into().expect("order"),
                    nonce: Some(canonical[65..97].try_into().expect("nonce")),
                    node_ids: row["node_ids"]
                        .as_array()
                        .expect("ids")
                        .iter()
                        .map(|v| u32::try_from(v.as_u64().expect("id")).expect("u32"))
                        .collect(),
                    edges: row["edges"]
                        .as_array()
                        .expect("edges")
                        .iter()
                        .map(|v| {
                            (
                                u32::try_from(v[0].as_u64().expect("admitted")).expect("u32"),
                                u32::try_from(v[1].as_u64().expect("bounds")).expect("u32"),
                            )
                        })
                        .collect(),
                    h: integers(&row["h"]),
                    j: integers(&row["j"]),
                    offset_milli: row["offset_milli"]
                        .as_str()
                        .expect("offset string")
                        .parse()
                        .expect("offset"),
                    metadata: hex(row["metadata_hex"].as_str().expect("metadata")),
                },
                &POLARITON_V1.limits,
                POLARITON_V1.metadata_kinds,
                None,
            )
            .expect("instance");
            assert_eq!(instance.canonical_bytes().expect("bytes"), canonical);
            codec::assert_instance_roundtrip(&admitted, &instance);
        }
    }
}
