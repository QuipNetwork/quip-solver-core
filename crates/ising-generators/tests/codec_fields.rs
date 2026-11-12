//! Exact fields codec round trips and rejection properties.
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test assertions and fixture access"
)]
use ising_generators::{
    descriptor::{admit, params_digest},
    ice::ICE_FIELDS_V1,
    instance::{Instance, InstanceParts},
    registry::{decode_instance, encode_instance},
};
#[path = "common/codec.rs"]
mod codec_common;
fn params() -> Vec<u8> {
    let mut out = vec![1];
    for (tag, bytes) in [
        (1u16, vec![0, 0]),
        (2, vec![0]),
        (3, vec![125, 0]),
        (4, vec![244, 1]),
        (5, vec![16, 0]),
    ] {
        out.extend(tag.to_le_bytes());
        out.extend(u32::try_from(bytes.len()).expect("length").to_le_bytes());
        out.extend(bytes);
    }
    out
}
#[test]
fn fields_instance_roundtrips() {
    let params = params();
    let a = admit(
        &[ICE_FIELDS_V1],
        b"quip/ice-fields",
        1,
        &params,
        &params_digest(&params),
    )
    .expect("admit");
    let i = Instance::new(
        InstanceParts {
            descriptor_digest: a.descriptor().digest(),
            topology_order_digest: [9; 32],
            nonce: Some([7; 32]),
            node_ids: vec![42],
            edges: vec![],
            h: vec![-321],
            j: vec![],
            offset_milli: 0,
            metadata: vec![
                2, 0, 1, 0, 4, 0, 0, 0, 0, 0, 0, 0, 2, 0, 5, 0, 0, 0, 1, 0, 0, 0, 1,
            ],
        },
        &ICE_FIELDS_V1.limits,
        &[1, 2],
        None,
    )
    .expect("instance");
    codec_common::assert_instance_roundtrip(&a, &i);
    assert!(decode_instance(&a, &encode_instance(&a, &i).expect("encode")).is_ok());
    let data = encode_instance(&a, &i).expect("encode");
    let start = data.len() - 6;
    for at in [96, 100, start, 115] {
        let mut bad = data.clone();
        bad[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            decode_instance(&a, &bad).err(),
            Some(CodecError::BoundsExceeded)
        );
    }
    let mut width = data.clone();
    width[start + 4] = 1;
    assert_eq!(
        decode_instance(&a, &width).err(),
        Some(CodecError::NonCanonical)
    );
    // Descriptor faults precede width minimality.
    width[0] ^= 1;
    assert_eq!(
        decode_instance(&a, &width).err(),
        Some(CodecError::DescriptorMismatch)
    );
    let mut padding = data.clone();
    *padding.last_mut().expect("gauge") |= 128;
    assert_eq!(
        decode_instance(&a, &padding).err(),
        Some(CodecError::Padding)
    );
    let mut extra = data.clone();
    extra.push(0);
    assert_eq!(decode_instance(&a, &extra).err(), Some(CodecError::Length));
    let mut bad_edge = data[..112].to_vec();
    bad_edge[100..104].copy_from_slice(&1u32.to_le_bytes());
    bad_edge.extend_from_slice(&[6, 42, 0]);
    for coefficient in [-321i32, 123] {
        bad_edge.extend_from_slice(&1u32.to_le_bytes());
        bad_edge.extend_from_slice(&coefficient.to_le_bytes());
        bad_edge.push(0);
    }
    bad_edge.extend_from_slice(&[0, 0, 0, 0, 1, 1]);
    assert_eq!(
        decode_instance(&a, &bad_edge).err(),
        Some(CodecError::InvalidInstance)
    );
    let mut unused = data[..112].to_vec();
    unused[96..100].copy_from_slice(&2u32.to_le_bytes());
    unused.extend_from_slice(&[1, 2, 0]); // ids [0,1], empty endpoints
    unused.extend_from_slice(&2u32.to_le_bytes());
    unused.extend_from_slice(&(-321i32).to_le_bytes());
    unused.extend_from_slice(&0i32.to_le_bytes());
    unused.push(0); // both h ranks zero, so the second palette entry is unused
    unused.extend_from_slice(&[0; 4]);
    unused.push(0); // empty j palette
    unused.extend_from_slice(&[0; 4]);
    unused.extend_from_slice(&[0, 0]); // no tiles, two gauges
    assert_eq!(
        decode_instance(&a, &unused).err(),
        Some(CodecError::NonCanonical)
    );
}

use ising_generators::{
    codec::CodecError,
    family::{generate_instance, BaseTopology},
    registry::prepare_witness_codec,
};
#[path = "common/size_bounds.rs"]
mod size_bounds;

fn topology() -> BaseTopology {
    let mut edges = std::collections::BTreeSet::new();
    for tile in [[0, 1, 2, 3], [2, 3, 4, 5], [0, 6, 7, 8]] {
        for i in 0..4 {
            for j in i + 1..4 {
                let _ = edges.insert((tile[i], tile[j]));
            }
        }
    }
    BaseTopology::new((0..9).collect(), &edges.into_iter().collect::<Vec<_>>()).expect("topology")
}
fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).expect("ascii"), 16).expect("hex"))
        .collect()
}
fn vectors() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/codecs/fields.json"
    ))
    .expect("vectors")
}
#[test]
fn independent_python_vectors() {
    let document = vectors();
    let rows = document["cases"].as_array().expect("rows");
    assert!(!rows.is_empty());
    for row in rows {
        let p = hex(row["params_hex"].as_str().expect("params"));
        let a = admit(
            &[ICE_FIELDS_V1],
            b"quip/ice-fields",
            1,
            &p,
            &params_digest(&p),
        )
        .expect("admit");
        let compact = hex(row["compact_hex"].as_str().expect("compact"));
        let instance = decode_instance(&a, &compact).expect("decode independent bytes");
        assert_eq!(
            instance.canonical_bytes().expect("canonical"),
            hex(row["canonical_hex"].as_str().expect("canonical"))
        );
        codec_common::assert_instance_roundtrip(&a, &instance);
        let codec = prepare_witness_codec(&a, &instance).expect("prepare fields");
        for w in row["witnesses"].as_array().expect("witnesses") {
            let spins: Vec<i8> = w["spins"]
                .as_array()
                .expect("spins")
                .iter()
                .map(|v| i8::try_from(v.as_i64().expect("spin")).expect("i8"))
                .collect();
            let bytes = hex(w["compact_hex"].as_str().expect("bytes"));
            assert_eq!(codec.encode(&spins).expect("encode"), bytes);
            assert_eq!(codec.decode(&bytes).expect("decode"), spins);
        }
        for bad in row["malformed_instances"]
            .as_array()
            .expect("bad instances")
        {
            let error = decode_instance(&a, &hex(bad["compact_hex"].as_str().expect("bytes")))
                .expect_err("reject");
            assert_eq!(error.name(), bad["error"].as_str().expect("error"));
        }
        for bad in row["malformed_witnesses"]
            .as_array()
            .expect("bad witnesses")
        {
            let error = codec
                .decode(&hex(bad["compact_hex"].as_str().expect("bytes")))
                .expect_err("reject");
            assert_eq!(error.name(), bad["error"].as_str().expect("error"));
        }
    }
}

#[test]
fn fields_generated_instance_roundtrips() {
    let topology = topology();
    let mut successes = 0;
    let mut skips = 0;
    for seed in 0u32..1024 {
        let mut p = params();
        let divisor = [9u16, 16, 25, 65535][seed as usize % 4];
        let field = [1u16, 500, 1000][seed as usize % 3];
        let plen = p.len();
        p[plen - 2..].copy_from_slice(&divisor.to_le_bytes());
        p[plen - 10..plen - 8].copy_from_slice(&field.to_le_bytes());
        let a = admit(
            &[ICE_FIELDS_V1],
            b"quip/ice-fields",
            1,
            &p,
            &params_digest(&p),
        )
        .expect("admit");
        let bounds = size_bounds::bounds(&a, &topology).expect("bounds");
        let mut nonce = [0; 32];
        nonce[..4].copy_from_slice(&seed.to_le_bytes());
        let Ok(instance) = generate_instance(&a, &topology, &bounds, nonce) else {
            skips += 1;
            continue;
        };
        successes += 1;
        codec_common::assert_instance_roundtrip(&a, &instance);
        let compact = encode_instance(&a, &instance).expect("encode");
        let decoded = decode_instance(&a, &compact).expect("decode");
        assert_eq!(decoded.descriptor_digest(), instance.descriptor_digest());
        assert_eq!(
            decoded.topology_order_digest(),
            instance.topology_order_digest()
        );
        assert_eq!(decoded.nonce(), instance.nonce());
        assert_eq!(decoded.node_ids(), instance.node_ids());
        assert_eq!(decoded.metadata(), instance.metadata());
    }
    assert_eq!(successes + skips, 1024);
    assert!(successes > 512);
}

#[test]
fn fields_shared_gauge_roundtrips() {
    let doc = vectors();
    for row in doc["cases"].as_array().expect("cases") {
        let p = hex(row["params_hex"].as_str().expect("params"));
        let a = admit(
            &[ICE_FIELDS_V1],
            b"quip/ice-fields",
            1,
            &p,
            &params_digest(&p),
        )
        .expect("admit");
        let i = decode_instance(&a, &hex(row["compact_hex"].as_str().expect("compact")))
            .expect("instance");
        let c = prepare_witness_codec(&a, &i).expect("prepare");
        for mask in 0..1u32 << i.node_ids().len() {
            let spins: Vec<_> = (0..i.node_ids().len())
                .map(|j| if mask & (1 << j) == 0 { -1 } else { 1 })
                .collect();
            assert_eq!(
                c.decode(&c.encode(&spins).expect("encode"))
                    .expect("decode"),
                spins
            );
        }
    }
}

fn next(state: &mut u64) -> u8 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    state.to_le_bytes()[0]
}
#[test]
fn fields_decoder_properties() {
    let doc = vectors();
    let row = &doc["cases"][0];
    let p = hex(row["params_hex"].as_str().expect("params"));
    let a = admit(
        &[ICE_FIELDS_V1],
        b"quip/ice-fields",
        1,
        &p,
        &params_digest(&p),
    )
    .expect("admit");
    let i =
        decode_instance(&a, &hex(row["compact_hex"].as_str().expect("compact"))).expect("instance");
    let c = prepare_witness_codec(&a, &i).expect("codec");
    let mut rng = 877;
    for k in 0..1024 {
        let spins: Vec<_> = (0..c.spin_count())
            .map(|_| if next(&mut rng) & 1 == 0 { -1 } else { 1 })
            .collect();
        let data = c.encode(&spins).expect("encode");
        assert_eq!(c.decode(&data).expect("decode"), spins);
        for cut in 0..data.len() {
            assert!(c.decode(&data[..cut]).is_err());
        }
        let bytes: Vec<_> = (0..k % 5).map(|_| next(&mut rng)).collect();
        if let Ok(s) = c.decode(&bytes) {
            assert_eq!(c.encode(&s).expect("reencode"), bytes);
        }
    }
    assert_eq!(c.encode(&[]), Err(CodecError::Length));
    assert_eq!(
        c.encode(&vec![0; c.spin_count()]),
        Err(CodecError::SpinValue)
    );
}

#[test]
fn fields_instance_decoder_properties() {
    let doc = vectors();
    let row = &doc["cases"][0];
    let p = hex(row["params_hex"].as_str().expect("params"));
    let a = admit(
        &[ICE_FIELDS_V1],
        b"quip/ice-fields",
        1,
        &p,
        &params_digest(&p),
    )
    .expect("admit");
    let data = hex(row["compact_hex"].as_str().expect("compact"));
    for cut in 0..data.len() {
        assert!(decode_instance(&a, &data[..cut]).is_err(), "cut {cut}");
    }
    let mut rng = 925;
    for k in 0..1024 {
        let random: Vec<_> = (0..k % 300).map(|_| next(&mut rng)).collect();
        let mut mutated = data.clone();
        let at = k % mutated.len();
        mutated[at] ^= next(&mut rng);
        for bytes in [random, mutated] {
            if let Ok(instance) = decode_instance(&a, &bytes) {
                assert_eq!(encode_instance(&a, &instance).expect("reencode"), bytes);
            }
        }
    }
    for at in [96, 100] {
        let mut bad = data.clone();
        bad[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            decode_instance(&a, &bad).err(),
            Some(CodecError::BoundsExceeded)
        );
    }
    let mut bad = data;
    bad[112] = 33;
    assert_eq!(
        decode_instance(&a, &bad).err(),
        Some(CodecError::InvalidSymbol)
    );
}

#[test]
fn fields_do_not_change_eligibility() {
    let doc = vectors();
    for row in doc["cases"].as_array().expect("cases") {
        let parameters = hex(row["params_hex"].as_str().expect("params"));
        let admitted = admit(
            &[ICE_FIELDS_V1],
            b"quip/ice-fields",
            1,
            &parameters,
            &params_digest(&parameters),
        )
        .expect("admit");
        let instance = decode_instance(
            &admitted,
            &hex(row["compact_hex"].as_str().expect("compact")),
        )
        .expect("instance");
        assert!(instance.h().iter().any(|&h| h != 0));
        let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
        let witness = &row["witnesses"][0];
        let spins: Vec<i8> = witness["spins"]
            .as_array()
            .expect("spins")
            .iter()
            .map(|v| i8::try_from(v.as_i64().expect("spin")).expect("i8"))
            .collect();
        let opposite: Vec<_> = spins.iter().map(|&s| -s).collect();
        assert_eq!(codec.encode(&spins).expect("encode")[0] & 1, 0);
        assert_eq!(codec.encode(&opposite).expect("encode")[0] & 1, 0);
        // At each nonzero field these two vectors have opposite alignment.
        for (&s, &other) in spins.iter().zip(&opposite) {
            assert_eq!(s, -other);
        }
        let mut redundant = vec![0u8; (spins.len() + 1).div_ceil(8)];
        redundant[0] = 1;
        for (parameters, &s) in spins.iter().enumerate() {
            if s == 1 {
                redundant[(parameters + 1) / 8] |= 1 << ((parameters + 1) % 8);
            }
        }
        assert_eq!(codec.decode(&redundant), Err(CodecError::NonCanonical));
    }
}

#[test]
fn fields_existing_fixtures() {
    let doc: serde_json::Value = serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/ice_fields.json"
    ))
    .expect("fixtures");
    let mut count = 0;
    for case in doc["cases"].as_array().expect("cases") {
        let p = hex(case["params_hex"].as_str().expect("params"));
        let a = admit(
            &[ICE_FIELDS_V1],
            b"quip/ice-fields",
            1,
            &p,
            &params_digest(&p),
        )
        .expect("admit");
        for row in case["instances"].as_array().expect("instances") {
            let canonical = hex(row["canonical_hex"].as_str().expect("canonical"));
            let ints = |name: &str| -> Vec<i32> {
                row[name]
                    .as_array()
                    .expect("ints")
                    .iter()
                    .map(|v| i32::try_from(v.as_i64().expect("integer")).expect("i32"))
                    .collect()
            };
            let i = Instance::new(
                InstanceParts {
                    descriptor_digest: canonical[1..33].try_into().expect("digest"),
                    topology_order_digest: canonical[33..65].try_into().expect("digest"),
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
                        .map(|e| {
                            (
                                u32::try_from(e[0].as_u64().expect("u")).expect("u32"),
                                u32::try_from(e[1].as_u64().expect("v")).expect("u32"),
                            )
                        })
                        .collect(),
                    h: ints("h"),
                    j: ints("j"),
                    offset_milli: 0,
                    metadata: hex(row["metadata_hex"].as_str().expect("metadata")),
                },
                &ICE_FIELDS_V1.limits,
                &[1, 2],
                None,
            )
            .expect("instance");
            assert_eq!(i.canonical_bytes().expect("canonical"), canonical);
            codec_common::assert_instance_roundtrip(&a, &i);
            count += 1;
        }
    }
    assert!(count > 5);
}

#[test]
fn fields_malformed_metadata_is_unencodable() {
    let p = params();
    let a = admit(
        &[ICE_FIELDS_V1],
        b"quip/ice-fields",
        1,
        &p,
        &params_digest(&p),
    )
    .expect("admit");
    let instance = Instance::new(
        InstanceParts {
            descriptor_digest: a.descriptor().digest(),
            topology_order_digest: [0; 32],
            nonce: None,
            node_ids: vec![],
            edges: vec![],
            h: vec![],
            j: vec![],
            offset_milli: 0,
            metadata: vec![0, 0],
        },
        &ICE_FIELDS_V1.limits,
        &[1, 2],
        None,
    )
    .expect("instance");
    assert_eq!(
        encode_instance(&a, &instance),
        Err(CodecError::UnencodableInstance)
    );
}
