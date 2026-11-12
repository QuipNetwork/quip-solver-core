//! Defects instance and witness wire contracts.
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::print_stderr,
    reason = "bounded test cases"
)]
use ising_generators::{
    defects::DEFECTS_V1,
    family::{generate_instance, BaseTopology},
    instance::Instance,
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
fn params(side: u16, pins: u16, obstacles: u16) -> Vec<u8> {
    let mut b = vec![1];
    for (tag, value) in [
        (1u16, side),
        (2, 4),
        (3, side),
        (4, 0),
        (5, obstacles),
        (6, pins),
    ] {
        b.extend_from_slice(&tag.to_le_bytes());
        let v = value.to_le_bytes();
        let bytes = if tag == 2 { &v[..1] } else { &v[..] };
        b.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        b.extend_from_slice(bytes);
    }
    for tag in [7u16, 8] {
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
    }
    b
}
fn build(bytes: &[u8], topology: &BaseTopology, seed: u32) -> Option<Instance> {
    let a = admit::admitted(&DEFECTS_V1, bytes).expect("admit");
    let b = size_bounds::bounds(&a, topology).expect("bounds");
    let mut nonce = [9; 32];
    nonce[..4].copy_from_slice(&seed.to_le_bytes());
    generate_instance(&a, topology, &b, nonce).ok()
}
#[test]
fn defects_instance_roundtrips() {
    let mut skipped = 0;
    for i in 0..1024 {
        let side = 2 + (i % 2) as u16;
        let p = params(side, (i % 3) as u16, (i % 2) as u16);
        let a = admit::admitted(&DEFECTS_V1, &p).expect("admit");
        if let Some(instance) = build(&p, &lattice::lattice_topology(side, 4), i) {
            codec::assert_instance_roundtrip(&a, &instance);
        } else {
            skipped += 1;
        }
    }
    eprintln!("construction skips: {skipped}/1024");
    assert!(skipped < 1024);
}
#[test]
fn defects_all_small_feasible_roundtrips() {
    for pins in [0, 2] {
        let p = params(2, pins, 0);
        let a = admit::admitted(&DEFECTS_V1, &p).expect("admit");
        let instance = build(&p, &lattice::lattice_topology(2, 4), 0).expect("generate");
        let c = prepare_witness_codec(&a, &instance).expect("prepare");
        for mask in 0..4096u32 {
            let spins: Vec<i8> = (0..12)
                .map(|bit| if mask & (1 << bit) == 0 { -1 } else { 1 })
                .collect();
            let feasible = ising_generators::family::feasible(&a, &instance, &spins);
            let encoded = c.encode(&spins);
            assert_eq!(encoded.is_ok(), feasible);
            if let Ok(bytes) = encoded {
                assert_eq!(c.decode(&bytes).expect("decode"), spins);
            }
        }
        let encoded = encode_instance(&a, &instance).expect("encode");
        assert!(decode_instance(&a, &encoded[..encoded.len() - 1]).is_err());
    }
}

fn parts(instance: &Instance) -> ising_generators::instance::InstanceParts {
    ising_generators::instance::InstanceParts {
        descriptor_digest: *instance.descriptor_digest(),
        topology_order_digest: *instance.topology_order_digest(),
        nonce: instance.nonce().copied(),
        node_ids: instance.node_ids().to_vec(),
        edges: instance.edges().to_vec(),
        h: instance.h().to_vec(),
        j: instance.j().to_vec(),
        offset_milli: instance.offset_milli(),
        metadata: instance.metadata().to_vec(),
    }
}
fn rebuilt(p: ising_generators::instance::InstanceParts) -> Instance {
    Instance::new(p, &DEFECTS_V1.limits, DEFECTS_V1.metadata_kinds, None)
        .expect("canonical instance")
}
fn random(state: &mut u64) -> u8 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state as u8
}
#[test]
fn defects_instance_decoder_properties() {
    let p = params(2, 2, 0);
    let a = admit::admitted(&DEFECTS_V1, &p).expect("admit");
    let instance = build(&p, &lattice::lattice_topology(2, 4), 0).expect("generate");
    let valid = encode_instance(&a, &instance).expect("encode");
    for end in 0..valid.len() {
        assert!(decode_instance(&a, &valid[..end]).is_err());
    }
    let mut state = 123_456_789;
    for i in 0..1024 {
        let mut bytes = if i % 2 == 0 {
            valid.clone()
        } else {
            (0..i % 400).map(|_| random(&mut state)).collect()
        };
        if !bytes.is_empty() {
            let at = usize::from(random(&mut state)) % bytes.len();
            bytes[at] ^= random(&mut state);
        }
        if let Ok(decoded) = decode_instance(&a, &bytes) {
            assert_eq!(encode_instance(&a, &decoded).expect("reencode"), bytes);
        }
    }
    for at in [96, 100] {
        let mut b = valid.clone();
        b[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            decode_instance(&a, &b).err(),
            Some(ising_generators::codec::CodecError::BoundsExceeded)
        );
    }
    let mut b = valid.clone();
    b[0] ^= 1;
    assert_eq!(
        decode_instance(&a, &b).err(),
        Some(ising_generators::codec::CodecError::DescriptorMismatch)
    );
    let mut b = valid.clone();
    b[112] = 33;
    assert_eq!(
        decode_instance(&a, &b).err(),
        Some(ising_generators::codec::CodecError::InvalidSymbol)
    );
    let mut b = valid;
    b.push(0);
    assert!(decode_instance(&a, &b).is_err());
}
#[test]
fn nongenerated_structural_image() {
    let p = params(2, 2, 0);
    let a = admit::admitted(&DEFECTS_V1, &p).expect("admit");
    let instance = build(&p, &lattice::lattice_topology(2, 4), 0).expect("generate");
    let mut changed = parts(&instance);
    changed.h[0] += 1;
    changed.offset_milli += 3;
    changed.node_ids.reverse();
    changed.nonce = None;
    let changed = rebuilt(changed);
    codec::assert_instance_roundtrip(&a, &changed);
    assert!(prepare_witness_codec(&a, &changed).is_err());
    let mut bad = parts(&instance);
    bad.metadata[10] = 3;
    let bad = rebuilt(bad);
    assert_eq!(
        encode_instance(&a, &bad).err(),
        Some(ising_generators::codec::CodecError::UnencodableInstance)
    );
}
#[test]
fn obstacles_do_not_remove_spins() {
    let p = params(2, 0, 1);
    let a = admit::admitted(&DEFECTS_V1, &p).expect("admit");
    let instance = build(&p, &lattice::lattice_topology(2, 4), 0).expect("generate");
    let c = prepare_witness_codec(&a, &instance).expect("prepare");
    assert_eq!(c.spin_count(), 12);
    let spins = vec![1; 12];
    assert!(ising_generators::family::feasible(&a, &instance, &spins));
    assert_eq!(
        c.decode(&c.encode(&spins).expect("encode"))
            .expect("decode"),
        spins
    );
    assert!(
        ising_generators::defects::check_route(a.params(), &instance, &spins, &[vec![0, 1]])
            .is_err()
    );
}
#[test]
fn defects_malformed_properties() {
    let mut state = 987_654_321;
    for pins in [0, 2] {
        let p = params(2, pins, 0);
        let a = admit::admitted(&DEFECTS_V1, &p).expect("admit");
        let instance = build(&p, &lattice::lattice_topology(2, 4), 0).expect("generate");
        let c = prepare_witness_codec(&a, &instance).expect("prepare");
        assert_eq!(
            c.encode(&[0; 12]).err(),
            Some(ising_generators::codec::CodecError::SpinValue)
        );
        for i in 0..1024 {
            let bytes: Vec<u8> = (0..i % 5).map(|_| random(&mut state)).collect();
            if let Ok(spins) = c.decode(&bytes) {
                assert_eq!(c.encode(&spins).expect("encode"), bytes);
                assert!(ising_generators::family::feasible(&a, &instance, &spins));
            }
        }
        let bits = 12 - pins as usize - 1;
        for bit in bits..c.max_encoded_len() * 8 {
            let mut bytes = vec![0; c.max_encoded_len()];
            bytes[bit / 8] |= 1 << (bit % 8);
            assert_eq!(
                c.decode(&bytes).err(),
                Some(ising_generators::codec::CodecError::Padding)
            );
        }
    }
}

fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks_exact(2)
        .map(|b| {
            let text = std::str::from_utf8(b).expect("hex text");
            u8::from_str_radix(text, 16).expect("hex digit")
        })
        .collect()
}
fn canonical(bytes: &[u8]) -> Instance {
    fn array<const N: usize>(bytes: &[u8], at: &mut usize) -> [u8; N] {
        let out = bytes[*at..*at + N].try_into().expect("field");
        *at += N;
        out
    }
    let mut at = 1;
    let descriptor_digest = array(bytes, &mut at);
    let topology_order_digest = array(bytes, &mut at);
    let nonce = Some(array(bytes, &mut at));
    let n = u32::from_le_bytes(array(bytes, &mut at));
    let node_ids = (0..n)
        .map(|_| u32::from_le_bytes(array(bytes, &mut at)))
        .collect();
    let e = u32::from_le_bytes(array(bytes, &mut at));
    let edges = (0..e)
        .map(|_| {
            (
                u32::from_le_bytes(array(bytes, &mut at)),
                u32::from_le_bytes(array(bytes, &mut at)),
            )
        })
        .collect();
    let h = (0..n)
        .map(|_| i32::from_le_bytes(array(bytes, &mut at)))
        .collect();
    let j = (0..e)
        .map(|_| i32::from_le_bytes(array(bytes, &mut at)))
        .collect();
    let offset_milli = i64::from_le_bytes(array(bytes, &mut at));
    let metadata = bytes[at..].to_vec();
    rebuilt(ising_generators::instance::InstanceParts {
        descriptor_digest,
        topology_order_digest,
        nonce,
        node_ids,
        edges,
        h,
        j,
        offset_milli,
        metadata,
    })
}
#[test]
fn independent_python_golden_vectors() {
    let document: serde_json::Value = serde_json::from_str(include_str!(
        "../../quip-solver-conformance/vectors/families/codecs/defects.json"
    ))
    .expect("vectors");
    let rows = document["cases"].as_array().expect("cases");
    assert!(!rows.is_empty());
    for row in rows {
        let p = unhex(row["params"].as_str().expect("params"));
        let a = admit::admitted(&DEFECTS_V1, &p).expect("admit");
        let instance = canonical(&unhex(row["canonical"].as_str().expect("canonical")));
        let compact = unhex(row["compact"].as_str().expect("compact"));
        assert_eq!(encode_instance(&a, &instance).expect("encode"), compact);
        codec::assert_instance_roundtrip(&a, &instance);
        let c = prepare_witness_codec(&a, &instance).expect("prepare");
        let spins: Vec<i8> = row["spins"]
            .as_array()
            .expect("spins")
            .iter()
            .map(|s| i8::try_from(s.as_i64().expect("spin")).expect("unit"))
            .collect();
        let witness = unhex(row["witness"].as_str().expect("witness"));
        assert_eq!(c.encode(&spins).expect("encode"), witness);
        assert_eq!(c.decode(&witness).expect("decode"), spins);
        for bad in row["invalid_instances"].as_array().expect("negative cases") {
            assert_eq!(
                decode_instance(&a, &unhex(bad["data"].as_str().expect("data")))
                    .expect_err("reject")
                    .name(),
                bad["error"].as_str().expect("error")
            );
        }
        for bad in row["invalid_witnesses"].as_array().expect("negative cases") {
            assert_eq!(
                c.decode(&unhex(bad["data"].as_str().expect("data")))
                    .expect_err("reject")
                    .name(),
                bad["error"].as_str().expect("error")
            );
        }
    }
}
