//! Routing compact codec contracts.
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test fixtures fail on invalid data"
)]
use ising_generators::{
    family::{generate_instance, BaseTopology},
    ice::zephyr_lattice,
    registry::{decode_instance, encode_instance, prepare_witness_codec},
    routing::ROUTING_V1,
};
#[path = "common/admit.rs"]
mod admit;
#[path = "common/codec.rs"]
mod codec_common;
#[path = "common/size_bounds.rs"]
mod size_bounds;
fn params() -> Vec<u8> {
    vec![
        1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 32, 0,
    ]
}
fn topology() -> BaseTopology {
    let (nodes, edges) = zephyr_lattice(2, 4).expect("lattice");
    BaseTopology::new(nodes, &edges).expect("topology")
}
#[test]
fn routing_instance_roundtrips() {
    let bytes = params();
    let admitted = admit::admitted(&ROUTING_V1, &bytes).expect("admit");
    let topology = topology();
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    let mut successes = 0;
    let mut skips = 0;
    for seed in 0u32..1024 {
        let mut nonce = [0; 32];
        nonce[..4].copy_from_slice(&seed.to_le_bytes());
        match generate_instance(&admitted, &topology, &bounds, nonce) {
            Ok(instance) => {
                codec_common::assert_instance_roundtrip(&admitted, &instance);
                successes += 1;
            }
            Err(ising_generators::family::SaltFault::Skip(_)) => skips += 1,
            Err(error) => unreachable!("construction defect: {error:?}"),
        }
    }
    assert_eq!(successes + skips, 1024);
    assert_eq!(skips, 0, "unexpected construction skips");
    assert!(successes > 0);
    let instance = generate_instance(&admitted, &topology, &bounds, [0; 32]).expect("generate");
    codec_common::assert_instance_roundtrip(&admitted, &instance);
    let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    let spins = vec![-1; instance.node_ids().len()];
    assert_eq!(
        codec
            .decode(&codec.encode(&spins).expect("encode"))
            .expect("decode"),
        spins
    );
    let compact = encode_instance(&admitted, &instance).expect("encode instance");
    assert_eq!(
        decode_instance(&admitted, &compact)
            .expect("decode instance")
            .digest(),
        instance.digest()
    );
}

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let s = std::str::from_utf8(pair).expect("hex utf8");
            u8::from_str_radix(s, 16).expect("hex byte")
        })
        .collect()
}
fn vectors() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/codecs/routing.json"
    ))
    .expect("vectors")
}
#[test]
fn routing_python_goldens() {
    let vectors = vectors();
    for row in vectors["cases"].as_array().expect("cases") {
        let params = unhex(row["params"].as_str().expect("params"));
        let admitted = admit::admitted(&ROUTING_V1, &params).expect("admit");
        let compact = unhex(row["compact"].as_str().expect("compact"));
        let instance = decode_instance(&admitted, &compact).expect("decode golden");
        assert_eq!(
            instance.canonical_bytes().expect("canonical"),
            unhex(row["canonical"].as_str().expect("canonical"))
        );
        assert_eq!(
            encode_instance(&admitted, &instance).expect("reencode"),
            compact
        );
        for bad in row["invalid_instances"].as_array().expect("bad instances") {
            let bytes = unhex(bad["compact"].as_str().expect("bytes"));
            assert_eq!(
                decode_instance(&admitted, &bytes)
                    .expect_err("reject")
                    .name(),
                bad["error"].as_str().expect("error")
            );
        }
        // Wider structural instances need not pass generator validation for witnesses.
        if row["name"] != "generated" {
            continue;
        }
        let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
        for witness in row["witnesses"].as_array().expect("witnesses") {
            let spins: Vec<i8> = witness["spins"]
                .as_array()
                .expect("spins")
                .iter()
                .map(|v| i8::try_from(v.as_i64().expect("spin")).expect("i8"))
                .collect();
            let bytes = unhex(witness["compact"].as_str().expect("payload"));
            assert_eq!(codec.encode(&spins).expect("encode"), bytes);
            assert_eq!(codec.decode(&bytes).expect("decode"), spins);
        }
        for bad in row["invalid_witnesses"].as_array().expect("bad witnesses") {
            let bytes = unhex(bad["compact"].as_str().expect("bytes"));
            assert_eq!(
                codec.decode(&bytes).expect_err("reject").name(),
                bad["error"].as_str().expect("error")
            );
        }
    }
}
fn generated() -> ising_generators::instance::Instance {
    let bytes = params();
    let admitted = admit::admitted(&ROUTING_V1, &bytes).expect("admit");
    let topology = topology();
    let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
    generate_instance(&admitted, &topology, &bounds, [0; 32]).expect("generate")
}
fn arc_positions(instance: &ising_generators::instance::Instance) -> Vec<[usize; 3]> {
    let data = instance.metadata();
    let word = |at| {
        usize::try_from(u32::from_le_bytes(
            data[at..at + 4].try_into().expect("word"),
        ))
        .expect("usize")
    };
    let start = 16 + 6 * word(8);
    data[start..data.len() - 2]
        .chunks_exact(24)
        .map(|arc| {
            [8, 12, 16].map(|at| {
                usize::try_from(u32::from_le_bytes(
                    arc[at..at + 4].try_into().expect("word"),
                ))
                .expect("usize")
            })
        })
        .collect()
}
#[test]
fn routing_broken_chains_roundtrip() {
    let bytes = params();
    let admitted = admit::admitted(&ROUTING_V1, &bytes).expect("admit");
    let instance = generated();
    let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    for [x, a, b] in arc_positions(&instance) {
        for first in [-1, 1] {
            for second in [-1, 1] {
                for commodity in [-1, 1] {
                    let mut spins = vec![-1; codec.spin_count()];
                    spins[x] = commodity;
                    spins[a] = first;
                    spins[b] = second;
                    let payload = codec.encode(&spins).expect("encode");
                    assert_eq!(payload[0] & 1, u8::from(first != second));
                    assert_eq!(codec.decode(&payload).expect("decode"), spins);
                }
            }
        }
    }
}
#[test]
fn routing_violations_still_structured() {
    let bytes = params();
    let admitted = admit::admitted(&ROUTING_V1, &bytes).expect("admit");
    let instance = generated();
    let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    let mut spins = vec![1; codec.spin_count()];
    for [x, a, b] in arc_positions(&instance) {
        spins[x] = -1;
        spins[a] = -1;
        spins[b] = -1;
        let payload = codec.encode(&spins).expect("encode");
        assert_eq!(payload[0] & 1, 0);
        assert_eq!(codec.decode(&payload).expect("decode"), spins);
    }
}
fn random(seed: &mut u64) -> u8 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    seed.to_le_bytes()[0]
}
#[test]
fn routing_decoder_properties() {
    let bytes = params();
    let admitted = admit::admitted(&ROUTING_V1, &bytes).expect("admit");
    let instance = generated();
    let codec = prepare_witness_codec(&admitted, &instance).expect("prepare");
    let mut seed = 91;
    for case in 0..1024 {
        let spins: Vec<_> = (0..codec.spin_count())
            .map(|_| if random(&mut seed) & 1 == 0 { -1 } else { 1 })
            .collect();
        let bytes = codec.encode(&spins).expect("encode");
        assert_eq!(codec.decode(&bytes).expect("decode"), spins);
        let arbitrary: Vec<_> = (0..case % (codec.max_encoded_len() + 3))
            .map(|_| random(&mut seed))
            .collect();
        if let Ok(spins) = codec.decode(&arbitrary) {
            assert_eq!(codec.encode(&spins).expect("encode"), arbitrary);
        }
    }
    let valid = codec.encode(&vec![1; codec.spin_count()]).expect("encode");
    for end in 0..valid.len() {
        assert!(codec.decode(&valid[..end]).is_err());
    }
    let mut padded = valid;
    *padded.last_mut().expect("last") |= 128;
    assert!(codec.decode(&padded).is_err());
    assert!(codec.encode(&vec![0; codec.spin_count()]).is_err());
    assert!(codec.encode(&[]).is_err());
}
#[test]
fn routing_instance_decoder_properties() {
    let bytes = params();
    let admitted = admit::admitted(&ROUTING_V1, &bytes).expect("admit");
    let instance = generated();
    let valid = encode_instance(&admitted, &instance).expect("encode");
    let mut seed = 42;
    for case in 0..1024 {
        let arbitrary: Vec<_> = (0..case % (valid.len() + 3))
            .map(|_| random(&mut seed))
            .collect();
        if let Ok(instance) = decode_instance(&admitted, &arbitrary) {
            assert_eq!(
                encode_instance(&admitted, &instance).expect("encode"),
                arbitrary
            );
        }
        let mut mutated = valid.clone();
        let at = case % mutated.len();
        mutated[at] ^= random(&mut seed);
        if let Ok(instance) = decode_instance(&admitted, &mutated) {
            assert_eq!(
                encode_instance(&admitted, &instance).expect("encode"),
                mutated
            );
        }
    }
    for end in 0..valid.len() {
        assert!(decode_instance(&admitted, &valid[..end]).is_err());
    }
    for at in [96, 100] {
        let mut excessive = valid.clone();
        excessive[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            decode_instance(&admitted, &excessive).err(),
            Some(ising_generators::codec::CodecError::BoundsExceeded)
        );
    }
    let mut width = valid;
    width[112] = 33;
    assert_eq!(
        decode_instance(&admitted, &width).err(),
        Some(ising_generators::codec::CodecError::InvalidSymbol)
    );
}

#[test]
fn routing_schema_and_registry_errors() {
    use ising_generators::{
        codec::CodecError,
        instance::{Instance, InstanceParts},
    };
    let bytes = params();
    let admitted = admit::admitted(&ROUTING_V1, &bytes).expect("admit");
    let instance = generated();
    let mut metadata = instance.metadata().to_vec();
    metadata[16] = 3;
    let malformed = Instance::new(
        InstanceParts {
            descriptor_digest: *instance.descriptor_digest(),
            topology_order_digest: *instance.topology_order_digest(),
            nonce: instance.nonce().copied(),
            node_ids: instance.node_ids().to_vec(),
            edges: instance.edges().to_vec(),
            h: instance.h().to_vec(),
            j: instance.j().to_vec(),
            offset_milli: instance.offset_milli(),
            metadata,
        },
        &ROUTING_V1.limits,
        &[7],
        None,
    )
    .expect("canonical malformed metadata");
    assert_eq!(
        encode_instance(&admitted, &malformed).err(),
        Some(CodecError::UnencodableInstance)
    );
    let mut unknown = ROUTING_V1;
    unknown.revision = 2;
    let admitted = admit::admitted(&unknown, &bytes).expect("admit unknown revision");
    assert_eq!(
        encode_instance(&admitted, &instance).err(),
        Some(CodecError::UnsupportedFamily)
    );
    assert_eq!(
        decode_instance(&admitted, &[]).err(),
        Some(CodecError::UnsupportedFamily)
    );
    assert_eq!(
        prepare_witness_codec(&admitted, &instance)
            .err()
            .map(CodecError::name),
        Some("unsupported_family")
    );
}

#[test]
fn routing_pruned_topology_roundtrips() {
    let bytes = params();
    let admitted = admit::admitted(&ROUTING_V1, &bytes).expect("admit");
    let ids = [17, 21, 25, 27, 33, 41, 114, 117, 118, 122, 130, 134];
    let (_, edges) = zephyr_lattice(2, 4).expect("lattice");
    let edges: Vec<_> = edges
        .into_iter()
        .filter(|(a, b)| ids.contains(a) && ids.contains(b))
        .collect();
    for nodes in [ids.to_vec(), ids.into_iter().rev().collect()] {
        let topology = BaseTopology::new(nodes, &edges).expect("pruned topology");
        let bounds = size_bounds::bounds(&admitted, &topology).expect("bounds");
        for seed in 0u8..8 {
            let instance =
                generate_instance(&admitted, &topology, &bounds, [seed; 32]).expect("generate");
            codec_common::assert_instance_roundtrip(&admitted, &instance);
        }
    }
}
