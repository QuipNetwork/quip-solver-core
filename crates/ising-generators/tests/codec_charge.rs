//! Charged ice compact wire contracts.
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test fixtures fail on invalid setup"
)]
use ising_generators::{
    ice::CHARGED_ICE_V1,
    instance::{Instance, InstanceParts},
    registry,
};
#[path = "common/admit.rs"]
mod admit;
#[path = "common/codec.rs"]
mod codec_common;
fn params() -> Vec<u8> {
    let mut bytes = vec![1];
    for (tag, value) in [
        (1_u16, vec![0, 0]),
        (2, vec![0]),
        (3, vec![250, 0]),
        (4, vec![31, 0]),
        (5, vec![128, 0]),
    ] {
        bytes.extend(tag.to_le_bytes());
        bytes.extend(u32::try_from(value.len()).expect("length").to_le_bytes());
        bytes.extend(value);
    }
    bytes
}
fn fixture(q: i8, gauge: [i8; 4]) -> Instance {
    let p = params();
    let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
    let mut metadata = vec![3, 0];
    let mut tiles = 1_u32.to_le_bytes().to_vec();
    for i in 0_u32..4 {
        tiles.extend(i.to_le_bytes());
    }
    let mut gauges = 1_u32.to_le_bytes().to_vec();
    gauges.extend(gauge.map(|g| u8::from(g == -1)));
    let mut charges = 1_u32.to_le_bytes().to_vec();
    charges.extend(q.to_le_bytes());
    for (kind, payload) in [(1_u16, tiles), (2, gauges), (6, charges)] {
        metadata.extend(kind.to_le_bytes());
        metadata.extend(u32::try_from(payload.len()).expect("length").to_le_bytes());
        metadata.extend(payload);
    }
    let edges = vec![(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
    let j = edges
        .iter()
        .map(|&(a, b)| 250 * i32::from(gauge[a as usize]) * i32::from(gauge[b as usize]))
        .collect();
    Instance::new(
        InstanceParts {
            descriptor_digest: a.descriptor().digest(),
            topology_order_digest: [5; 32],
            nonce: Some([7; 32]),
            node_ids: vec![8, 2, 4, 1],
            edges,
            h: gauge.map(|g| -250 * i32::from(q) * i32::from(g)).to_vec(),
            j,
            offset_milli: 250 * (4 + i64::from(q).pow(2)) / 2,
            metadata,
        },
        &CHARGED_ICE_V1.limits,
        CHARGED_ICE_V1.metadata_kinds,
        None,
    )
    .expect("instance")
}
#[test]
fn charge_instance_roundtrips() {
    let p = params();
    let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
    for q in [0, 2, -2] {
        codec_common::assert_instance_roundtrip(&a, &fixture(q, [1; 4]));
    }
}
#[test]
fn charged_rank_tables() {
    let p = params();
    let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
    for (q, masks) in [
        (0, vec![3, 5, 6, 9, 10, 12]),
        (2, vec![7, 11, 13, 14]),
        (-2, vec![1, 2, 4, 8]),
    ] {
        let codec = registry::prepare_witness_codec(&a, &fixture(q, [1; 4])).expect("prepare");
        for mask in 0..16 {
            let spins: Vec<i8> = (0..4)
                .map(|i| if mask & (1 << i) != 0 { 1 } else { -1 })
                .collect();
            let encoded = codec.encode(&spins).expect("encode");
            let expected = masks
                .iter()
                .position(|&m| m == mask)
                .map_or(1 + 2 * mask, |rank| 2 * rank);
            assert_eq!(encoded, vec![u8::try_from(expected).expect("byte")]);
            assert_eq!(codec.decode(&encoded).expect("decode"), spins);
        }
    }
}

#[path = "common/size_bounds.rs"]
mod size_bounds;
use ising_generators::{
    codec::CodecError,
    family::{generate_instance, BaseTopology},
};

#[test]
fn charge_generated_roundtrips() {
    let topology = BaseTopology::new(
        vec![0, 1, 2, 3],
        &[(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)],
    )
    .expect("topology");
    let mut skipped = 0;
    for seed in 0_u32..1024 {
        let mut p = params();
        p[38..40].copy_from_slice(&u16::try_from(seed % 257).expect("chance").to_le_bytes());
        let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
        let bounds = size_bounds::bounds(&a, &topology).expect("bounds");
        let mut nonce = [0; 32];
        nonce[..4].copy_from_slice(&seed.to_le_bytes());
        let Ok(instance) = generate_instance(&a, &topology, &bounds, nonce) else {
            skipped += 1;
            continue;
        };
        codec_common::assert_instance_roundtrip(&a, &instance);
        let codec = registry::prepare_witness_codec(&a, &instance).expect("prepare");
        let spins: Vec<i8> = (0..4)
            .map(|i| if seed & (1 << i) == 0 { -1 } else { 1 })
            .collect();
        let bytes = codec.encode(&spins).expect("encode");
        assert_eq!(codec.decode(&bytes).expect("decode"), spins);
    }
    assert_eq!(skipped, 0, "construction skips");
}

fn random(state: &mut u64) -> u8 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    state.to_le_bytes()[7]
}
#[test]
fn charged_soft_roundtrips() {
    let p = params();
    let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
    for q in [0, 2, -2] {
        for gauge_mask in 0..16 {
            let gauge = core::array::from_fn(|i| if gauge_mask & (1 << i) == 0 { 1 } else { -1 });
            let codec = registry::prepare_witness_codec(&a, &fixture(q, gauge)).expect("prepare");
            for mask in 0..16 {
                let spins: Vec<i8> = (0..4)
                    .map(|i| if mask & (1 << i) == 0 { -1 } else { 1 })
                    .collect();
                let bytes = codec.encode(&spins).expect("encode");
                assert_eq!(codec.decode(&bytes).expect("decode"), spins);
                if bytes[0] & 1 == 0 {
                    assert_eq!(codec.decode(&[1 + 2 * mask]), Err(CodecError::NonCanonical));
                }
            }
        }
    }
}
#[test]
fn charged_decoder_properties() {
    let p = params();
    let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
    let codec = registry::prepare_witness_codec(&a, &fixture(0, [1; 4])).expect("prepare");
    let mut rng = 2311;
    for _ in 0..1024 {
        let spins: Vec<i8> = (0..4)
            .map(|_| if random(&mut rng) & 1 == 0 { -1 } else { 1 })
            .collect();
        assert_eq!(
            codec
                .decode(&codec.encode(&spins).expect("encode"))
                .expect("decode"),
            spins
        );
        let bytes: Vec<u8> = (0..random(&mut rng) % 4)
            .map(|_| random(&mut rng))
            .collect();
        if let Ok(spins) = codec.decode(&bytes) {
            assert_eq!(codec.encode(&spins).expect("encode"), bytes);
        }
    }
    assert_eq!(codec.decode(&[14]), Err(CodecError::InvalidSymbol));
    assert_eq!(codec.decode(&[12]), Err(CodecError::InvalidSymbol));
    assert_eq!(codec.decode(&[128]), Err(CodecError::Padding));
    assert_eq!(codec.decode(&[]), Err(CodecError::Length));
    assert_eq!(codec.decode(&[0, 0]), Err(CodecError::Length));
    assert_eq!(codec.encode(&[0; 4]), Err(CodecError::SpinValue));
    assert_eq!(codec.encode(&[]), Err(CodecError::Length));
}
#[test]
fn charge_instance_decoder_properties() {
    let p = params();
    let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
    let compact = registry::encode_instance(&a, &fixture(0, [1; 4])).expect("encode");
    for end in 0..compact.len() {
        assert!(registry::decode_instance(&a, &compact[..end]).is_err());
    }
    let mut rng = 2311;
    for _ in 0..1024 {
        let bytes: Vec<u8> = (0..random(&mut rng)).map(|_| random(&mut rng)).collect();
        if let Ok(instance) = registry::decode_instance(&a, &bytes) {
            assert_eq!(
                registry::encode_instance(&a, &instance).expect("encode"),
                bytes
            );
        }
        let mut bytes = compact.clone();
        let at = usize::from(random(&mut rng)) % bytes.len();
        bytes[at] ^= random(&mut rng);
        if let Ok(instance) = registry::decode_instance(&a, &bytes) {
            assert_eq!(
                registry::encode_instance(&a, &instance).expect("encode"),
                bytes
            );
        }
    }
    let mut extra = compact.clone();
    extra.push(0);
    assert_eq!(
        registry::decode_instance(&a, &extra),
        Err(CodecError::Length)
    );
    for (at, value) in [(96, 131_073_u32), (100, 2_097_153)] {
        let mut bad = compact.clone();
        bad[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert_eq!(
            registry::decode_instance(&a, &bad),
            Err(CodecError::BoundsExceeded)
        );
    }
    let mut bad = compact.clone();
    bad[0] ^= 1;
    assert_eq!(
        registry::decode_instance(&a, &bad),
        Err(CodecError::DescriptorMismatch)
    );
    let mut bad = compact;
    bad[112] = 33;
    assert_eq!(
        registry::decode_instance(&a, &bad),
        Err(CodecError::InvalidSymbol)
    );
}
fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(core::str::from_utf8(pair).expect("ascii"), 16).expect("hex")
        })
        .collect()
}
#[test]
fn independent_python_goldens() {
    let document: serde_json::Value = serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/codecs/charge.json"
    ))
    .expect("json");
    let rows = document["cases"].as_array().expect("cases");
    assert!(!rows.is_empty());
    for row in rows {
        let p = unhex(row["params"].as_str().expect("params"));
        let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
        let compact = unhex(row["compact"].as_str().expect("compact"));
        let instance = registry::decode_instance(&a, &compact).expect("decode");
        assert_eq!(
            instance.canonical_bytes().expect("canonical"),
            unhex(row["canonical"].as_str().expect("canonical"))
        );
        assert_eq!(
            registry::encode_instance(&a, &instance).expect("encode"),
            compact
        );
        let codec = registry::prepare_witness_codec(&a, &instance).expect("prepare");
        for w in row["witnesses"].as_array().expect("witnesses") {
            let spins: Vec<i8> = w["spins"]
                .as_array()
                .expect("spins")
                .iter()
                .map(|s| i8::try_from(s.as_i64().expect("spin")).expect("i8"))
                .collect();
            let data = unhex(w["compact"].as_str().expect("compact"));
            assert_eq!(codec.encode(&spins).expect("encode"), data);
            assert_eq!(codec.decode(&data).expect("decode"), spins);
        }
        for bad in row["malformed"].as_array().expect("malformed") {
            assert_eq!(
                registry::decode_instance(&a, &unhex(bad["compact"].as_str().expect("compact")))
                    .expect_err("reject")
                    .name(),
                bad["error"].as_str().expect("error")
            );
        }
        for bad in row["bad_witnesses"].as_array().expect("bad witnesses") {
            assert_eq!(
                codec
                    .decode(&unhex(bad["compact"].as_str().expect("compact")))
                    .expect_err("reject")
                    .name(),
                bad["error"].as_str().expect("error")
            );
        }
    }
}

fn rebuilt(instance: &Instance, metadata: Vec<u8>, h: Vec<i32>, offset: i64) -> Instance {
    Instance::new(
        InstanceParts {
            descriptor_digest: *instance.descriptor_digest(),
            topology_order_digest: *instance.topology_order_digest(),
            nonce: instance.nonce().copied(),
            node_ids: instance.node_ids().to_vec(),
            edges: instance.edges().to_vec(),
            h,
            j: instance.j().to_vec(),
            offset_milli: offset,
            metadata,
        },
        &CHARGED_ICE_V1.limits,
        CHARGED_ICE_V1.metadata_kinds,
        None,
    )
    .expect("rebuild")
}
#[test]
fn charge_wider_image_and_schema_rejection() {
    let p = params();
    let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
    let base = fixture(0, [1; 4]);
    let instance = rebuilt(
        &base,
        base.metadata().to_vec(),
        vec![17, -19, 23, -29],
        -123,
    );
    codec_common::assert_instance_roundtrip(&a, &instance);
    assert!(registry::prepare_witness_codec(&a, &instance).is_err());
    let mut metadata = base.metadata().to_vec();
    let last = metadata.len() - 1;
    metadata[last] = 4;
    let bad = rebuilt(&base, metadata, base.h().to_vec(), base.offset_milli());
    assert_eq!(
        registry::encode_instance(&a, &bad),
        Err(CodecError::UnencodableInstance)
    );
    let bad = rebuilt(&base, vec![0, 0], base.h().to_vec(), base.offset_milli());
    assert_eq!(
        registry::encode_instance(&a, &bad),
        Err(CodecError::UnencodableInstance)
    );
}
#[test]
fn charged_overlap_is_deterministic() {
    let p = params();
    let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
    let tiles = [[0_u32, 1, 2, 3], [3, 4, 5, 6]];
    let mut edges = Vec::new();
    for tile in tiles {
        for i in 0..4 {
            for j in i + 1..4 {
                edges.push((tile[i], tile[j]));
            }
        }
    }
    edges.sort_unstable();
    let mut tile_bytes = 2_u32.to_le_bytes().to_vec();
    for tile in tiles {
        for position in tile {
            tile_bytes.extend(position.to_le_bytes());
        }
    }
    let mut gauges = 2_u32.to_le_bytes().to_vec();
    gauges.extend([0; 8]);
    let mut charges = 2_u32.to_le_bytes().to_vec();
    charges.extend([0, 0]);
    let mut metadata = vec![3, 0];
    for (kind, payload) in [(1_u16, tile_bytes), (2, gauges), (6, charges)] {
        metadata.extend(kind.to_le_bytes());
        metadata.extend(u32::try_from(payload.len()).expect("length").to_le_bytes());
        metadata.extend(payload);
    }
    let instance = Instance::new(
        InstanceParts {
            descriptor_digest: a.descriptor().digest(),
            topology_order_digest: [0; 32],
            nonce: None,
            node_ids: (0..7).collect(),
            j: vec![250; edges.len()],
            edges,
            h: vec![0; 7],
            offset_milli: 1000,
            metadata,
        },
        &CHARGED_ICE_V1.limits,
        CHARGED_ICE_V1.metadata_kinds,
        None,
    )
    .expect("overlap");
    codec_common::assert_instance_roundtrip(&a, &instance);
    let codec = registry::prepare_witness_codec(&a, &instance).expect("prepare");
    for mask in 0_u8..128 {
        let spins: Vec<i8> = (0..7)
            .map(|i| if mask & (1 << i) == 0 { -1 } else { 1 })
            .collect();
        let data = codec.encode(&spins).expect("encode");
        assert_eq!(data[0] & 1, u8::from((mask & 15).count_ones() != 2));
        assert_eq!(codec.decode(&data).expect("decode"), spins);
    }
}

#[test]
fn charge_metadata_and_palette_rejections() {
    let p = params();
    let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
    let data = registry::encode_instance(&a, &fixture(0, [1; 4])).expect("encode");
    // Four IDs use width four, twelve endpoints use width two, and both palettes have one entry.
    let f = 137;
    assert_eq!(data.len(), 145);
    for (at, replacement, error) in [
        (
            f,
            u32::MAX.to_le_bytes().to_vec(),
            CodecError::BoundsExceeded,
        ),
        (
            119,
            5_u32.to_le_bytes().to_vec(),
            CodecError::BoundsExceeded,
        ),
        (f + 4, vec![33], CodecError::InvalidSymbol),
        (f + 5, vec![0], CodecError::InvalidInstance),
        (f + 6, vec![128], CodecError::Padding),
        (f + 7, vec![3], CodecError::InvalidSymbol),
        (f + 7, vec![128], CodecError::Padding),
    ] {
        let mut bad = data.clone();
        bad[at..at + replacement.len()].copy_from_slice(&replacement);
        assert_eq!(registry::decode_instance(&a, &bad), Err(error));
    }
    let mut wide = data[..f + 4].to_vec();
    wide.extend([3, 0x88, 0x06]);
    wide.extend(&data[f + 6..]);
    assert_eq!(
        registry::decode_instance(&a, &wide),
        Err(CodecError::NonCanonical)
    );
    // Broken edge order wins over a nonminimal metadata column.
    wide[116] &= !0b1100;
    assert_eq!(
        registry::decode_instance(&a, &wide),
        Err(CodecError::InvalidInstance)
    );
    let mut unused = data[..119].to_vec();
    unused.extend(2_u32.to_le_bytes());
    unused.extend(0_i32.to_le_bytes());
    unused.extend(1_i32.to_le_bytes());
    unused.push(0);
    unused.extend(&data[128..]);
    assert_eq!(
        registry::decode_instance(&a, &unused),
        Err(CodecError::NonCanonical)
    );
}

#[test]
fn charge_existing_family_fixtures() {
    let document: serde_json::Value = serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/ice_charge.json"
    ))
    .expect("json");
    for row in document["cases"].as_array().expect("cases") {
        let p = unhex(row["params_hex"].as_str().expect("params"));
        let a = admit::admitted(&CHARGED_ICE_V1, &p).expect("admit");
        for item in row["instances"].as_array().expect("instances") {
            let instance = Instance::new(
                InstanceParts {
                    descriptor_digest: a.descriptor().digest(),
                    topology_order_digest: unhex(
                        row["topology"]["order_digest"].as_str().expect("order"),
                    )
                    .try_into()
                    .expect("digest"),
                    nonce: Some(
                        unhex(item["nonce"].as_str().expect("nonce"))
                            .try_into()
                            .expect("nonce"),
                    ),
                    node_ids: serde_json::from_value(item["node_ids"].clone()).expect("ids"),
                    edges: serde_json::from_value(item["edges"].clone()).expect("edges"),
                    h: serde_json::from_value(item["h"].clone()).expect("h"),
                    j: serde_json::from_value(item["j"].clone()).expect("j"),
                    offset_milli: item["offset_milli"]
                        .as_str()
                        .expect("offset")
                        .parse()
                        .expect("i64"),
                    metadata: unhex(item["metadata_hex"].as_str().expect("metadata")),
                },
                &CHARGED_ICE_V1.limits,
                CHARGED_ICE_V1.metadata_kinds,
                None,
            )
            .expect("instance");
            assert_eq!(
                instance.canonical_bytes().expect("canonical"),
                unhex(item["canonical_hex"].as_str().expect("canonical"))
            );
            codec_common::assert_instance_roundtrip(&a, &instance);
        }
    }
}
