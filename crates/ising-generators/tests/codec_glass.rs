//! Lossless glass instance and witness codecs.
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test failures identify broken codec invariants"
)]

use ising_generators::{
    family::{generate_instance, BaseTopology},
    ice::ICE_GLASS_V1,
    registry::{encode_instance, prepare_witness_codec},
};
#[path = "common/admit.rs"]
mod admit;
#[path = "common/codec.rs"]
mod codec;
#[path = "common/size_bounds.rs"]
mod size_bounds;

fn params() -> Vec<u8> {
    vec![
        1, 1, 0, 2, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 3, 0, 2, 0, 0, 0, 125, 0,
    ]
}
fn topology() -> BaseTopology {
    BaseTopology::new(
        vec![9, 4, 7, 2],
        &[(9, 4), (9, 7), (9, 2), (4, 7), (4, 2), (7, 2)],
    )
    .expect("clique")
}
#[test]
fn glass_instance_roundtrips() {
    let bytes = params();
    let admitted = admit::admitted(&ICE_GLASS_V1, &bytes).expect("admit");
    let topology = topology();
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let mut successes = 0;
    for seed in 0u32..1024 {
        let mut nonce = [0; 32];
        nonce[..4].copy_from_slice(&seed.to_le_bytes());
        if let Ok(instance) = generate_instance(&admitted, &topology, &bounds, nonce) {
            codec::assert_instance_roundtrip(&admitted, &instance);
            let compact = encode_instance(&admitted, &instance).expect("encode");
            let decoded = decode_instance(&admitted, &compact).expect("decode");
            assert_eq!(decoded.descriptor_digest(), instance.descriptor_digest());
            assert_eq!(
                decoded.topology_order_digest(),
                instance.topology_order_digest()
            );
            assert_eq!(decoded.nonce(), instance.nonce());
            assert_eq!(decoded.node_ids(), instance.node_ids());
            assert_eq!(decoded.metadata(), instance.metadata());
            successes += 1;
        }
    }
    assert_eq!(successes, 1024, "construction skips: {}", 1024 - successes);
}
#[test]
fn glass_global_gauge_roundtrip() {
    let bytes = params();
    let admitted = admit::admitted(&ICE_GLASS_V1, &bytes).expect("admit");
    let topology = topology();
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let instance = generate_instance(&admitted, &topology, &bounds, [3; 32]).expect("generate");
    let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    for mask in 0..16 {
        let spins: Vec<i8> = (0..4)
            .map(|i| if mask & (1 << i) == 0 { -1 } else { 1 })
            .collect();
        assert_eq!(
            codec
                .decode(&codec.encode(&spins).expect("encode"))
                .expect("decode"),
            spins
        );
    }
}

use ising_generators::{
    codec::CodecError,
    instance::{Instance, InstanceParts},
    registry::decode_instance,
};

fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("ascii"), 16).expect("hex"))
        .collect()
}
fn golden() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/codecs/glass.json"
    ))
    .expect("json")
}
fn text<'a>(row: &'a serde_json::Value, key: &str) -> &'a str {
    row[key].as_str().expect("text")
}
#[test]
fn glass_python_goldens() {
    let document = golden();
    let rows = document["cases"].as_array().expect("cases");
    assert!(!rows.is_empty());
    for row in rows {
        let bytes = unhex(text(row, "params_hex"));
        let admitted = admit::admitted(&ICE_GLASS_V1, &bytes).expect("admit");
        let packed = unhex(text(row, "compact_hex"));
        let instance = decode_instance(&admitted, &packed).expect("decode golden");
        assert_eq!(
            instance.canonical_bytes().expect("canonical"),
            unhex(text(row, "canonical_hex"))
        );
        codec::assert_instance_roundtrip(&admitted, &instance);
        let codec = prepare_witness_codec(&admitted, &instance).expect("prepare golden");
        for witness in row["witnesses"].as_array().expect("witnesses") {
            let spins: Vec<i8> = witness["spins"]
                .as_array()
                .expect("spins")
                .iter()
                .map(|v| i8::try_from(v.as_i64().expect("int")).expect("spin"))
                .collect();
            let data = unhex(text(witness, "hex"));
            assert_eq!(codec.encode(&spins).expect("encode"), data);
            assert_eq!(codec.decode(&data).expect("decode"), spins);
        }
        for bad in row["invalid_instances"]
            .as_array()
            .expect("negative instances")
        {
            assert_eq!(
                decode_instance(&admitted, &unhex(text(bad, "hex")))
                    .expect_err("reject")
                    .name(),
                text(bad, "error")
            );
        }
        for bad in row["invalid_witnesses"]
            .as_array()
            .expect("negative witnesses")
        {
            assert_eq!(
                codec
                    .decode(&unhex(text(bad, "hex")))
                    .expect_err("reject")
                    .name(),
                text(bad, "error")
            );
        }
    }
}

fn rebuilt(instance: &Instance, metadata: Vec<u8>, h: Vec<i32>, j: Vec<i32>) -> Instance {
    Instance::new(
        InstanceParts {
            descriptor_digest: *instance.descriptor_digest(),
            topology_order_digest: *instance.topology_order_digest(),
            nonce: instance.nonce().copied(),
            node_ids: instance.node_ids().to_vec(),
            edges: instance.edges().to_vec(),
            h,
            j,
            offset_milli: 0,
            metadata,
        },
        &ICE_GLASS_V1.limits,
        ICE_GLASS_V1.metadata_kinds,
        None,
    )
    .expect("canonical instance")
}
#[test]
fn glass_wider_image() {
    let bytes = params();
    let admitted = admit::admitted(&ICE_GLASS_V1, &bytes).expect("admit");
    let top = topology();
    let bounds = size_bounds::bounds(&admitted, &top).expect("bounds");
    let instance = generate_instance(&admitted, &top, &bounds, [3; 32]).expect("generate");
    let mut metadata = instance.metadata().to_vec();
    metadata[12..28].copy_from_slice(
        &[3u32, 1, 0, 2]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let wider = rebuilt(&instance, metadata.clone(), vec![37; 4], vec![-71; 6]);
    codec::assert_instance_roundtrip(&admitted, &wider);
    assert_eq!(
        prepare_witness_codec(&admitted, &wider).err(),
        Some(CodecError::InvalidInstance)
    );
    *metadata.last_mut().expect("gauge") = 2;
    let malformed = rebuilt(&instance, metadata, vec![0; 4], instance.j().to_vec());
    assert_eq!(
        encode_instance(&admitted, &malformed),
        Err(CodecError::UnencodableInstance)
    );
}

fn next(state: &mut u64) -> u8 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    state.to_le_bytes()[0]
}
#[test]
fn glass_instance_decoder_properties() {
    let bytes = params();
    let admitted = admit::admitted(&ICE_GLASS_V1, &bytes).expect("admit");
    let top = topology();
    let bounds = size_bounds::bounds(&admitted, &top).expect("bounds");
    let instance = generate_instance(&admitted, &top, &bounds, [3; 32]).expect("generate");
    let valid = encode_instance(&admitted, &instance).expect("encode");
    for end in 0..valid.len() {
        assert!(decode_instance(&admitted, &valid[..end]).is_err());
    }
    let mut seed = 2311;
    for i in 0..1024 {
        let arbitrary: Vec<_> = (0..i % 300).map(|_| next(&mut seed)).collect();
        let mut changed = valid.clone();
        let at = usize::from(next(&mut seed)) % changed.len();
        changed[at] ^= 1 << (next(&mut seed) % 8);
        for data in [arbitrary, changed] {
            if let Ok(decoded) = decode_instance(&admitted, &data) {
                assert_eq!(encode_instance(&admitted, &decoded).expect("encode"), data);
            }
        }
    }
    for (at, count) in [(96, 131_073u32), (100, 2_097_153)] {
        let mut changed = valid.clone();
        changed[at..at + 4].copy_from_slice(&count.to_le_bytes());
        assert_eq!(
            decode_instance(&admitted, &changed).err(),
            Some(CodecError::BoundsExceeded)
        );
    }
    let mut changed = valid.clone();
    changed[0] ^= 1;
    assert_eq!(
        decode_instance(&admitted, &changed).err(),
        Some(CodecError::DescriptorMismatch)
    );
    changed = valid;
    changed.push(0);
    assert_eq!(
        decode_instance(&admitted, &changed).err(),
        Some(CodecError::Length)
    );
}
#[test]
fn glass_soft_decoder_properties() {
    let bytes = params();
    let admitted = admit::admitted(&ICE_GLASS_V1, &bytes).expect("admit");
    let top = topology();
    let bounds = size_bounds::bounds(&admitted, &top).expect("bounds");
    let instance = generate_instance(&admitted, &top, &bounds, [3; 32]).expect("generate");
    let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    let mut seed = 2311;
    for i in 0..1024 {
        let spins: Vec<_> = (0..4)
            .map(|_| if next(&mut seed) & 1 == 0 { -1 } else { 1 })
            .collect();
        assert_eq!(
            codec
                .decode(&codec.encode(&spins).expect("encode"))
                .expect("decode"),
            spins
        );
        let data: Vec<_> = (0..i % 4).map(|_| next(&mut seed)).collect();
        if let Ok(spins) = codec.decode(&data) {
            assert_eq!(codec.encode(&spins).expect("encode"), data);
        }
    }
    assert_eq!(codec.decode(&[12]), Err(CodecError::InvalidSymbol));
    assert_eq!(codec.decode(&[14]), Err(CodecError::InvalidSymbol));
    assert_eq!(codec.decode(&[128]), Err(CodecError::Padding));
    assert_eq!(codec.encode(&[0; 4]), Err(CodecError::SpinValue));
    let spins = codec.decode(&[0]).expect("rank zero");
    let forced = spins
        .iter()
        .enumerate()
        .fold(1, |v, (i, s)| v | (u8::from(*s == 1) << (i + 1)));
    assert_eq!(codec.decode(&[forced]), Err(CodecError::NonCanonical));
}

#[test]
fn glass_shared_node_selection() {
    let bytes = params();
    let admitted = admit::admitted(&ICE_GLASS_V1, &bytes).expect("admit");
    let tiles = [[0u32, 1, 2, 3], [3, 4, 5, 6]];
    let gauge = [0u8, 1, 0, 1, 0, 1, 0];
    let mut metadata = vec![2, 0, 1, 0];
    metadata.extend_from_slice(&36u32.to_le_bytes());
    metadata.extend_from_slice(&2u32.to_le_bytes());
    for tile in tiles {
        for p in tile {
            metadata.extend_from_slice(&p.to_le_bytes());
        }
    }
    metadata.extend_from_slice(&[2, 0]);
    metadata.extend_from_slice(&11u32.to_le_bytes());
    metadata.extend_from_slice(&7u32.to_le_bytes());
    metadata.extend_from_slice(&gauge);
    let mut pairs = std::collections::BTreeSet::new();
    for tile in tiles {
        for (i, &u) in tile.iter().enumerate() {
            for &v in &tile[i + 1..] {
                assert!(pairs.insert((u, v)));
            }
        }
    }
    let edges: Vec<_> = pairs.into_iter().collect();
    let j = edges
        .iter()
        .map(|&(u, v)| {
            if gauge[u as usize] == gauge[v as usize] {
                1000
            } else {
                -1000
            }
        })
        .collect();
    let instance = Instance::new(
        InstanceParts {
            descriptor_digest: admitted.descriptor().digest(),
            topology_order_digest: [7; 32],
            nonce: None,
            node_ids: (0..7).collect(),
            edges,
            h: vec![0; 7],
            j,
            offset_milli: 0,
            metadata,
        },
        &ICE_GLASS_V1.limits,
        &[1, 2],
        None,
    )
    .expect("instance");
    codec::assert_instance_roundtrip(&admitted, &instance);
    let codec = prepare_witness_codec(&admitted, &instance).expect("prepare overlap");
    for mask in 0..128 {
        let spins: Vec<i8> = (0..7)
            .map(|i| if mask & (1 << i) == 0 { -1 } else { 1 })
            .collect();
        let data = codec.encode(&spins).expect("encode");
        let charge: i8 = (0..4)
            .map(|i| spins[i] * if gauge[i] == 0 { 1 } else { -1 })
            .sum();
        assert_eq!(data[0] & 1, u8::from(charge != 0));
        assert_eq!(codec.decode(&data).expect("decode"), spins);
    }
}
