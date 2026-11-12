//! Lossless twisted instance and witness codec checks.
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test failures identify invalid fixtures"
)]
use ising_generators::{
    family::generate_instance,
    ice::TWISTED_DIMER_V1,
    registry::{encode_instance, prepare_witness_codec},
};
#[path = "common/admit.rs"]
mod admit;
#[path = "common/codec.rs"]
mod codec_common;
#[path = "common/lattice.rs"]
mod lattice;
#[path = "common/size_bounds.rs"]
mod size_bounds;

fn params(pairs: u32) -> Vec<u8> {
    let mut bytes = vec![
        1, 1, 0, 2, 0, 0, 0, 1, 0, 2, 0, 1, 0, 0, 0, 1, 3, 0, 2, 0, 0, 0, 62, 0, 4, 0, 4, 0, 0, 0,
    ];
    bytes.extend_from_slice(&pairs.to_le_bytes());
    bytes
}

#[test]
fn twisted_instance_roundtrips() {
    let bytes = params(0);
    let admitted = admit::admitted(&TWISTED_DIMER_V1, &bytes).expect("admit");
    let topology = lattice::lattice_topology(1, 1);
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let instance = generate_instance(&admitted, &topology, &bounds, [0; 32]).expect("generate");
    codec_common::assert_instance_roundtrip(&admitted, &instance);
    assert!(encode_instance(&admitted, &instance).is_ok());
    let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    let spins = vec![1; instance.node_ids().len()];
    assert_eq!(
        codec
            .decode(&codec.encode(&spins).expect("encode"))
            .expect("decode"),
        spins
    );
}

use ising_generators::{
    codec::CodecError,
    instance::{Instance, InstanceParts},
    registry::decode_instance,
};

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("hex text");
            u8::from_str_radix(text, 16).expect("hex byte")
        })
        .collect()
}

fn fixture(
    digest: [u8; 32],
    n: u32,
    tiles: &[[u32; 4]],
    gauges: &[[u8; 4]],
    pairs: &[(u32, u32)],
) -> Instance {
    let mut metadata = vec![3, 0];
    let mut tile_bytes = u32::try_from(tiles.len())
        .expect("count")
        .to_le_bytes()
        .to_vec();
    for tile in tiles {
        for p in tile {
            tile_bytes.extend_from_slice(&p.to_le_bytes());
        }
    }
    let mut gauge_bytes = u32::try_from(gauges.len())
        .expect("count")
        .to_le_bytes()
        .to_vec();
    for g in gauges {
        gauge_bytes.extend_from_slice(g);
    }
    let mut pair_bytes = u32::try_from(pairs.len())
        .expect("count")
        .to_le_bytes()
        .to_vec();
    for (a, b) in pairs {
        pair_bytes.extend_from_slice(&a.to_le_bytes());
        pair_bytes.extend_from_slice(&b.to_le_bytes());
    }
    for (kind, payload) in [(1u16, tile_bytes), (2, gauge_bytes), (3, pair_bytes)] {
        metadata.extend_from_slice(&kind.to_le_bytes());
        metadata.extend_from_slice(&u32::try_from(payload.len()).expect("length").to_le_bytes());
        metadata.extend_from_slice(&payload);
    }
    Instance::new(
        InstanceParts {
            descriptor_digest: digest,
            topology_order_digest: [19; 32],
            nonce: Some([23; 32]),
            node_ids: (0..n).rev().collect(),
            edges: vec![],
            h: vec![17; n as usize],
            j: vec![],
            offset_milli: 0,
            metadata,
        },
        &TWISTED_DIMER_V1.limits,
        &[1, 2, 3],
        None,
    )
    .expect("fixture")
}

#[test]
fn structural_image_and_malformed_metadata() {
    let bytes = params(0);
    let admitted = admit::admitted(&TWISTED_DIMER_V1, &bytes).expect("admit");
    let digest = admitted.descriptor().digest();
    let inst = fixture(digest, 6, &[[0, 1, 2, 3]], &[[0, 1, 0, 1]], &[(0, 4)]);
    codec_common::assert_instance_roundtrip(&admitted, &inst);
    // Nonzero fields and mismatched parameter pair counts belong to the wider instance image.
    assert!(prepare_witness_codec(&admitted, &inst).is_err());
    for bad in [
        fixture(digest, 6, &[[0, 0, 2, 3]], &[[0; 4]], &[]),
        fixture(digest, 6, &[[0, 1, 2, 3]], &[[2; 4]], &[]),
        fixture(digest, 6, &[], &[], &[(1, 0)]),
    ] {
        assert_eq!(
            encode_instance(&admitted, &bad),
            Err(CodecError::UnencodableInstance)
        );
    }
}

#[test]
fn golden_python_instances_and_witnesses() {
    let doc: serde_json::Value = serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/codecs/twisted.json"
    ))
    .expect("goldens");
    let rows = doc["cases"].as_array().expect("cases");
    assert!(!rows.is_empty());
    for row in rows {
        let bytes = unhex(row["params"].as_str().expect("params"));
        let admitted = admit::admitted(&TWISTED_DIMER_V1, &bytes).expect("admit");
        let wire = unhex(row["compact"].as_str().expect("compact"));
        let instance = decode_instance(&admitted, &wire).expect("decode golden");
        assert_eq!(
            instance.canonical_bytes().expect("canonical"),
            unhex(row["canonical"].as_str().expect("canonical hex"))
        );
        codec_common::assert_instance_roundtrip(&admitted, &instance);
        let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
        for witness in row["witnesses"].as_array().expect("witnesses") {
            let spins: Vec<i8> = witness["spins"]
                .as_array()
                .expect("spins")
                .iter()
                .map(|s| i8::try_from(s.as_i64().expect("spin")).expect("i8"))
                .collect();
            let compact = unhex(witness["compact"].as_str().expect("hex"));
            assert_eq!(codec.encode(&spins).expect("encode"), compact);
            assert_eq!(codec.decode(&compact).expect("decode"), spins);
        }
        for bad in row["invalid_instances"].as_array().expect("bad instances") {
            let err = decode_instance(&admitted, &unhex(bad["compact"].as_str().expect("hex")))
                .expect_err("reject");
            assert_eq!(err.name(), bad["error"].as_str().expect("error"));
        }
        for bad in row["invalid_witnesses"].as_array().expect("bad witnesses") {
            let err = codec
                .decode(&unhex(bad["compact"].as_str().expect("hex")))
                .expect_err("reject");
            assert_eq!(err.name(), bad["error"].as_str().expect("error"));
        }
    }
}

fn random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    *state
}

#[test]
fn twisted_instance_decoder_properties() {
    let bytes = params(0);
    let admitted = admit::admitted(&TWISTED_DIMER_V1, &bytes).expect("admit");
    let inst = fixture(
        admitted.descriptor().digest(),
        6,
        &[[0, 1, 2, 3]],
        &[[1, 0, 1, 0]],
        &[(0, 4)],
    );
    let wire = encode_instance(&admitted, &inst).expect("encode");
    for end in 0..wire.len() {
        assert!(decode_instance(&admitted, &wire[..end]).is_err());
    }
    let mut state = 2311;
    for _ in 0..1024 {
        let len = usize::try_from(random(&mut state) % 300).expect("length");
        let arbitrary: Vec<u8> = (0..len)
            .map(|_| random(&mut state).to_le_bytes()[4])
            .collect();
        let mut mutated = wire.clone();
        let at = usize::try_from(random(&mut state)).expect("index") % mutated.len();
        mutated[at] ^= random(&mut state).to_le_bytes()[4] | 1;
        for payload in [arbitrary, mutated] {
            if let Ok(decoded) = decode_instance(&admitted, &payload) {
                assert_eq!(
                    encode_instance(&admitted, &decoded).expect("reencode"),
                    payload
                );
            }
        }
    }
    for offset in [96, 100] {
        let mut bad = wire.clone();
        bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            decode_instance(&admitted, &bad).expect_err("bounds"),
            CodecError::BoundsExceeded
        );
    }
    let mut bad = wire.clone();
    bad[112] = 33;
    assert_eq!(
        decode_instance(&admitted, &bad).expect_err("width"),
        CodecError::InvalidSymbol
    );
    let mut bad = wire.clone();
    bad.push(0);
    assert_eq!(
        decode_instance(&admitted, &bad).expect_err("extra"),
        CodecError::Length
    );
    let mut bad = wire;
    bad[0] ^= 1;
    assert_eq!(
        decode_instance(&admitted, &bad).expect_err("descriptor"),
        CodecError::DescriptorMismatch
    );
}

#[test]
fn generated_sweep() {
    let topology = lattice::lattice_topology(1, 1);
    let mut skips = 0;
    for seed in 0..1024u32 {
        let bytes = params(seed % 3);
        let admitted = admit::admitted(&TWISTED_DIMER_V1, &bytes).expect("admit");
        let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
        let mut nonce = [0; 32];
        nonce[..4].copy_from_slice(&seed.to_le_bytes());
        let Ok(instance) = generate_instance(&admitted, &topology, &bounds, nonce) else {
            skips += 1;
            continue;
        };
        codec_common::assert_instance_roundtrip(&admitted, &instance);
        let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
        let mut state = u64::from(seed);
        let spins: Vec<_> = (0..instance.node_ids().len())
            .map(|_| {
                if random(&mut state) & (1 << 32) == 0 {
                    -1
                } else {
                    1
                }
            })
            .collect();
        assert_eq!(
            codec
                .decode(&codec.encode(&spins).expect("encode"))
                .expect("decode"),
            spins
        );
    }
    assert_eq!(skips, 0);
}

#[test]
fn existing_family_fixtures() {
    let document: serde_json::Value = serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/twisted_dimer.json"
    ))
    .expect("fixtures");
    let mut count = 0;
    for case in document["cases"].as_array().expect("cases") {
        let bytes = unhex(case["params_hex"].as_str().expect("params"));
        let admitted = admit::admitted(&TWISTED_DIMER_V1, &bytes).expect("admit");
        for row in case["instances"].as_array().expect("instances") {
            let canonical = unhex(row["canonical_hex"].as_str().expect("canonical"));
            let ids = row["node_ids"]
                .as_array()
                .expect("ids")
                .iter()
                .map(|v| u32::try_from(v.as_u64().expect("id")).expect("u32"))
                .collect();
            let edges = row["edges"]
                .as_array()
                .expect("edges")
                .iter()
                .map(|v| {
                    (
                        u32::try_from(v[0].as_u64().expect("a")).expect("u32"),
                        u32::try_from(v[1].as_u64().expect("b")).expect("u32"),
                    )
                })
                .collect();
            let coefficients = |name: &str| {
                row[name]
                    .as_array()
                    .expect("coefficients")
                    .iter()
                    .map(|v| i32::try_from(v.as_i64().expect("coefficient")).expect("i32"))
                    .collect()
            };
            let instance = Instance::new(
                InstanceParts {
                    descriptor_digest: admitted.descriptor().digest(),
                    topology_order_digest: canonical[33..65].try_into().expect("topology"),
                    nonce: Some(canonical[65..97].try_into().expect("nonce")),
                    node_ids: ids,
                    edges,
                    h: coefficients("h"),
                    j: coefficients("j"),
                    offset_milli: row["offset_milli"]
                        .as_str()
                        .expect("offset text")
                        .parse()
                        .expect("offset"),
                    metadata: unhex(row["metadata_hex"].as_str().expect("metadata")),
                },
                &TWISTED_DIMER_V1.limits,
                &[1, 2, 3],
                None,
            )
            .expect("instance");
            assert_eq!(instance.canonical_bytes().expect("canonical"), canonical);
            codec_common::assert_instance_roundtrip(&admitted, &instance);
            count += 1;
        }
    }
    assert!(count > 0);
}

#[test]
fn malformed_instance_fields() {
    let bytes = params(0);
    let admitted = admit::admitted(&TWISTED_DIMER_V1, &bytes).expect("admit");
    // Empty columns and palettes make the family start offset exact and auditable.
    let instance = fixture(admitted.descriptor().digest(), 0, &[], &[], &[]);
    let wire = encode_instance(&admitted, &instance).expect("encode");
    assert_eq!(wire.len(), 134);
    let cases = [
        (
            124usize,
            u32::MAX.to_le_bytes().to_vec(),
            CodecError::BoundsExceeded,
        ),
        (
            129,
            u32::MAX.to_le_bytes().to_vec(),
            CodecError::BoundsExceeded,
        ),
        (
            114,
            u32::MAX.to_le_bytes().to_vec(),
            CodecError::BoundsExceeded,
        ),
        (128, vec![33], CodecError::InvalidSymbol),
        (128, vec![1], CodecError::NonCanonical),
        (133, vec![1], CodecError::NonCanonical),
    ];
    for (at, replacement, error) in cases {
        let mut bad = wire.clone();
        bad[at..at + replacement.len()].copy_from_slice(&replacement);
        assert_eq!(
            decode_instance(&admitted, &bad).expect_err("invalid field"),
            error
        );
    }
    // A descriptor mismatch wins over a nonminimal family column.
    let mut bad = wire;
    bad[0] ^= 1;
    bad[128] = 1;
    assert_eq!(
        decode_instance(&admitted, &bad).expect_err("priority"),
        CodecError::DescriptorMismatch
    );

    let instance = fixture(
        admitted.descriptor().digest(),
        6,
        &[[0, 1, 2, 3]],
        &[[0; 4]],
        &[],
    );
    let wire = encode_instance(&admitted, &instance).expect("encode");
    // N=6 ids: width 3 plus 3 bytes; endpoints: width 0;
    // h palette: count + one coefficient + width 0; j palette: count + width 0.
    let family = 112 + 4 + 1 + 9 + 5;
    let mut bad = wire.clone();
    bad[family + 6] |= 0x80;
    assert_eq!(
        decode_instance(&admitted, &bad).expect_err("gauge padding"),
        CodecError::Padding
    );
    let mut bad = wire.clone();
    bad[family + 5] = 0;
    assert_eq!(
        decode_instance(&admitted, &bad).expect_err("repeated position"),
        CodecError::InvalidInstance
    );
    let mut bad = wire.clone();
    bad[112..116].copy_from_slice(&[4, 0x45, 0x23, 0x01]);
    assert_eq!(
        decode_instance(&admitted, &bad).expect_err("nonminimal ids"),
        CodecError::NonCanonical
    );
    let mut bad = wire.clone();
    bad[115] |= 0x80;
    assert_eq!(
        decode_instance(&admitted, &bad).expect_err("id padding"),
        CodecError::Padding
    );
    let mut bad = wire.clone();
    bad[117..121].copy_from_slice(&2u32.to_le_bytes());
    drop(bad.splice(125..125, 18i32.to_le_bytes()));
    assert_eq!(
        decode_instance(&admitted, &bad).expect_err("unused palette"),
        CodecError::NonCanonical
    );
    let mut bad = wire;
    bad[family + 4] = 1;
    assert!(decode_instance(&admitted, &bad).is_err());
}

#[test]
fn exhaustive_small_generated_witnesses() {
    let topology = lattice::lattice_topology(1, 1);
    for pair_count in [0, 1] {
        let bytes = params(pair_count);
        let admitted = admit::admitted(&TWISTED_DIMER_V1, &bytes).expect("admit");
        let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
        let instance = generate_instance(&admitted, &topology, &bounds, [0; 32]).expect("generate");
        let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
        let n = instance.node_ids().len();
        assert!(n <= 16);
        for mask in 0..(1u32 << n) {
            let spins: Vec<_> = (0..n)
                .map(|i| if mask & (1 << i) == 0 { -1 } else { 1 })
                .collect();
            let wire = codec.encode(&spins).expect("encode");
            assert_eq!(codec.decode(&wire).expect("decode"), spins);
            assert!(wire.len() <= codec.max_encoded_len());
        }
    }
}
