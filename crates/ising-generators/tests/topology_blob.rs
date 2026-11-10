//! The topology blob codec: round trips, hostile inputs, and gzip framing.

#![cfg(feature = "blob")]
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code: a failed lookup is a test failure"
)]

use ising_generators::blob::{
    compress, decode, decode_payload, decompress, encode, encode_payload, BlobError, Body,
    Topology, MAX_PAYLOAD,
};
use ising_generators::ice::zephyr_lattice;

const HEAD: [u8; 6] = [0x51, 0x54, 0x4f, 0x50, 1, 0];

fn explicit(nodes: &[u32], edges: &[(u32, u32)]) -> Body {
    Body::Explicit(Topology {
        nodes: nodes.to_vec(),
        edges: edges.to_vec(),
    })
}

fn zephyr(m: u16, t: u8, nodes: &[u32], edges: &[(u32, u32)]) -> Body {
    Body::ZephyrRemoved {
        m,
        t,
        removed_nodes: nodes.to_vec(),
        removed_edges: edges.to_vec(),
    }
}

fn payload(prefix: &[u8], tail: &[u8]) -> Vec<u8> {
    let mut out = prefix.to_vec();
    out.extend_from_slice(tail);
    out
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).expect("seven bits") | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).expect("seven bits"));
    out
}

fn payload_err(bytes: &[u8]) -> BlobError {
    decode_payload(bytes).expect_err("payload must be rejected")
}

fn file_err(bytes: &[u8]) -> BlobError {
    decode(bytes).expect_err("file must be rejected")
}

fn round_trip(body: &Body) {
    let file = encode(body).expect("encode");
    assert_eq!(&decode(&file).expect("decode"), body);
    assert_eq!(&file[..10], &[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 0xff]);
    assert_eq!(file, encode(body).expect("encode again"));
}

#[test]
fn t_sparse_style_extremes_round_trip() {
    // u32::MAX next to 0, a reordered list, a flipped edge, a duplicate, a self-loop.
    let body = explicit(
        &[4_294_967_295, 0, 7, 3],
        &[
            (4_294_967_295, 0),
            (0, 4_294_967_295),
            (7, 3),
            (7, 3),
            (3, 3),
        ],
    );
    round_trip(&body);
}

#[test]
fn empty_explicit_round_trips() {
    round_trip(&explicit(&[], &[]));
}

#[test]
fn z22_bodies_round_trip() {
    let (nodes, edges) = zephyr_lattice(2, 2).expect("z(2,2)");
    round_trip(&Body::Explicit(Topology {
        nodes: nodes.clone(),
        edges: edges.clone(),
    }));
    let removed_edge = edges
        .iter()
        .copied()
        .find(|&(a, b)| a != 0 && b != 0 && a != 5 && b != 5)
        .expect("edge");
    let body = zephyr(2, 2, &[0, 5], &[removed_edge]);
    round_trip(&body);
    let topology = body.topology().expect("expand");
    assert_eq!(topology.nodes.len(), nodes.len() - 2);
    assert!(!topology.edges.contains(&removed_edge));
    assert!(topology
        .edges
        .iter()
        .all(|&(a, b)| a < b && ![0, 5].contains(&a) && ![0, 5].contains(&b)));
    assert!(topology.nodes.windows(2).all(|w| w[0] < w[1]));
    assert!(topology.edges.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(
        zephyr(2, 2, &[], &[]).topology().expect("full"),
        Topology { nodes, edges }
    );
}

#[test]
fn explicit_topology_returns_itself() {
    let body = explicit(&[2, 1], &[(2, 1)]);
    assert_eq!(
        body.topology().expect("topology"),
        Topology {
            nodes: vec![2, 1],
            edges: vec![(2, 1)]
        }
    );
}

#[test]
fn gzip_errors() {
    let good = encode(&explicit(&[1, 2], &[(1, 2)])).expect("encode");
    assert_eq!(file_err(&[]), BlobError::Gzip);
    assert_eq!(file_err(&good[..good.len() - 1]), BlobError::Gzip);
    assert_eq!(file_err(&good[..12]), BlobError::Gzip);
    let mut bad = good.clone();
    bad[2] = 7;
    assert_eq!(file_err(&bad), BlobError::Gzip);
    let mut bad = good.clone();
    bad[3] = 8;
    assert_eq!(file_err(&bad), BlobError::Gzip);
    let mut bad = good.clone();
    bad[0] = 0;
    assert_eq!(file_err(&bad), BlobError::Gzip);
    // A second member after the first.
    let mut two = good.clone();
    two.extend_from_slice(&good);
    assert_eq!(file_err(&two), BlobError::Gzip);
    let mut extra = good;
    extra.push(0);
    assert_eq!(file_err(&extra), BlobError::Gzip);
}

#[test]
fn checksum_errors() {
    let good = encode(&explicit(&[1, 2], &[(1, 2)])).expect("encode");
    let n = good.len();
    let mut bad = good.clone();
    bad[n - 8] ^= 1;
    assert_eq!(file_err(&bad), BlobError::Checksum);
    let mut bad = good;
    bad[n - 4] ^= 1;
    assert_eq!(file_err(&bad), BlobError::Checksum);
}

#[test]
fn python_style_header_is_accepted() {
    let body = explicit(&[9, 1], &[(9, 1)]);
    let mut file = encode(&body).expect("encode");
    file[4..8].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
    file[8] = 0;
    file[9] = 3;
    assert_eq!(decode(&file).expect("decode"), body);
}

/// The two gzip files hold only zero bytes: 64 MiB plus one byte, and exactly
/// 64 MiB. They are built once with:
/// `head -c 67108865 /dev/zero | gzip -9n > tests/data/zeros-64mib-plus-1.bin.gz`
/// `head -c 67108864 /dev/zero | gzip -9n > tests/data/zeros-64mib.bin.gz`
#[test]
fn oversize_payload_is_rejected() {
    let file = include_bytes!("data/zeros-64mib-plus-1.bin.gz");
    assert_eq!(decompress(file), Err(BlobError::TooLarge));
    let big = vec![0_u8; MAX_PAYLOAD + 1];
    assert_eq!(decode_payload(&big), Err(BlobError::TooLarge));
    assert_eq!(
        decompress(include_bytes!("data/zeros-64mib.bin.gz"))
            .expect("at the limit")
            .len(),
        MAX_PAYLOAD
    );
}

#[test]
fn header_errors() {
    assert_eq!(payload_err(&[]), BlobError::Magic);
    assert_eq!(payload_err(b"QTO"), BlobError::Magic);
    assert_eq!(payload_err(b"QTOQ\x01\x00\x00\x00"), BlobError::Magic);
    assert_eq!(payload_err(b"QTOP"), BlobError::Version);
    assert_eq!(payload_err(b"QTOP\x02\x00\x00\x00"), BlobError::Version);
    assert_eq!(payload_err(b"QTOP\x01"), BlobError::Kind);
    assert_eq!(payload_err(b"QTOP\x01\x02\x00\x00"), BlobError::Kind);
}

#[test]
fn varint_errors() {
    // 11 bytes.
    let long = [
        0x80_u8, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00,
    ];
    assert_eq!(payload_err(&payload(&HEAD, &long)), BlobError::Varint);
    // 10 bytes with a value above u64::MAX.
    let mut over = [0xff_u8; 10];
    over[9] = 0x02;
    assert_eq!(payload_err(&payload(&HEAD, &over)), BlobError::Varint);
    // Non-minimal zero.
    assert_eq!(
        payload_err(&payload(&HEAD, &[0x80, 0x00])),
        BlobError::Varint
    );
    // Truncated.
    assert_eq!(payload_err(&payload(&HEAD, &[0x80])), BlobError::Varint);
    assert_eq!(payload_err(&HEAD), BlobError::Varint);
    // u64::MAX is a valid varint but an impossible count.
    let mut max = [0xff_u8; 10];
    max[9] = 0x01;
    assert_eq!(payload_err(&payload(&HEAD, &max)), BlobError::Range);
}

#[test]
fn explicit_range_errors() {
    // One node with zigzag delta 2^32 (id above u32::MAX).
    let mut high = vec![1_u8];
    high.extend(varint(1 << 33));
    assert_eq!(payload_err(&payload(&HEAD, &high)), BlobError::Range);
    // One node with delta -1 (id below zero).
    assert_eq!(payload_err(&payload(&HEAD, &[1, 1])), BlobError::Range);
    // An edge end above u32::MAX: node list empty, one edge, du = 0, dv = 2^32.
    let mut edge = vec![0_u8, 1, 0];
    edge.extend(varint(1 << 33));
    assert_eq!(payload_err(&payload(&HEAD, &edge)), BlobError::Range);
}

#[test]
fn counts_larger_than_the_remaining_bytes_are_rejected() {
    assert_eq!(payload_err(&payload(&HEAD, &[5, 0, 0])), BlobError::Range);
    assert_eq!(
        payload_err(&payload(&HEAD, &[0, 2, 0, 0, 0])),
        BlobError::Range
    );
    // A huge count of nodes with no data after it.
    assert_eq!(
        payload_err(&payload(&HEAD, &[0xff, 0xff, 0xff, 0xff, 0x0f])),
        BlobError::Range
    );
    let zeph = [0x51, 0x54, 0x4f, 0x50, 1, 1];
    assert_eq!(payload_err(&payload(&zeph, &[2, 2, 9])), BlobError::Range);
}

#[test]
fn trailing_bytes_are_rejected() {
    let mut bytes = encode_payload(&explicit(&[1], &[])).expect("payload");
    bytes.push(0);
    assert_eq!(payload_err(&bytes), BlobError::Trailing);
    let mut bytes = encode_payload(&zephyr(2, 2, &[], &[])).expect("payload");
    bytes.push(0);
    assert_eq!(payload_err(&bytes), BlobError::Trailing);
    assert_eq!(file_err(&compress(&bytes)), BlobError::Trailing);
}

#[test]
fn zephyr_range_errors() {
    let zeph = [0x51, 0x54, 0x4f, 0x50, 1, 1];
    for tail in [[0_u8, 1, 0, 0], [33, 1, 0, 0], [2, 0, 0, 0], [2, 9, 0, 0]] {
        assert_eq!(payload_err(&payload(&zeph, &tail)), BlobError::Range);
    }
    assert_eq!(
        encode_payload(&zephyr(0, 1, &[], &[])),
        Err(BlobError::Range)
    );
    assert_eq!(
        encode_payload(&zephyr(33, 1, &[], &[])),
        Err(BlobError::Range)
    );
    assert_eq!(
        encode_payload(&zephyr(1, 9, &[], &[])),
        Err(BlobError::Range)
    );
}

#[test]
fn zephyr_order_errors() {
    let zeph = [0x51, 0x54, 0x4f, 0x50, 1, 1];
    // Removed nodes 3 then delta 0.
    assert_eq!(
        payload_err(&payload(&zeph, &[2, 2, 2, 3, 0, 0])),
        BlobError::Order
    );
    // Edge gap 0.
    assert_eq!(
        payload_err(&payload(&zeph, &[2, 2, 0, 1, 1, 0])),
        BlobError::Order
    );
    assert_eq!(
        encode_payload(&zephyr(2, 2, &[5, 3], &[])),
        Err(BlobError::Order)
    );
    assert_eq!(
        encode_payload(&zephyr(2, 2, &[3, 3], &[])),
        Err(BlobError::Order)
    );
    let (_, edges) = zephyr_lattice(2, 2).expect("z(2,2)");
    assert_eq!(
        encode_payload(&zephyr(2, 2, &[], &[edges[1], edges[0]])),
        Err(BlobError::Order)
    );
    assert_eq!(
        encode_payload(&zephyr(2, 2, &[], &[edges[0], edges[0]])),
        Err(BlobError::Order)
    );
    let (a, b) = edges[0];
    assert_eq!(
        encode_payload(&zephyr(2, 2, &[], &[(b, a)])),
        Err(BlobError::Order)
    );
}

#[test]
fn zephyr_lattice_errors() {
    let zeph = [0x51, 0x54, 0x4f, 0x50, 1, 1];
    // Node 1000 is not in z(2,2).
    assert_eq!(
        payload_err(&payload(&zeph, &[2, 2, 1, 0xe8, 0x07, 0])),
        BlobError::NotLattice
    );
    // Edge (0, 1) of the lattice is not a coupler for some pair: find a non-coupler.
    let (nodes, edges) = zephyr_lattice(2, 2).expect("z(2,2)");
    let non = nodes
        .iter()
        .flat_map(|&a| nodes.iter().map(move |&b| (a, b)))
        .find(|&(a, b)| a < b && !edges.contains(&(a, b)))
        .expect("a non-coupler");
    assert_eq!(
        encode_payload(&zephyr(2, 2, &[], &[non])),
        Err(BlobError::NotLattice)
    );
    // A removed edge with a removed end.
    let (a, b) = edges[0];
    assert_eq!(
        encode_payload(&zephyr(2, 2, &[a], &[(a, b)])),
        Err(BlobError::NotLattice)
    );
    assert_eq!(
        encode_payload(&zephyr(2, 2, &[u32::MAX], &[])),
        Err(BlobError::NotLattice)
    );
    // The same removed-edge fault through decode: node a removed, then edge (a, b).
    let (da, db) = (
        u8::try_from(a).expect("small"),
        u8::try_from(b - a).expect("small"),
    );
    let bytes = [2_u8, 2, 1, da, 1, da, db];
    assert_eq!(payload_err(&payload(&zeph, &bytes)), BlobError::NotLattice);
}

#[test]
fn invalid_body_does_not_expand() {
    assert_eq!(
        zephyr(2, 2, &[999], &[]).topology(),
        Err(BlobError::NotLattice)
    );
}

#[test]
fn z11_lattice_matches_the_spec_example() {
    // The same list is printed in docs/topology-format.md.
    let (nodes, edges) = zephyr_lattice(1, 1).expect("z(1,1)");
    assert_eq!(nodes, (0..12).collect::<Vec<u32>>());
    let expected = [
        (0, 1),
        (0, 6),
        (0, 8),
        (1, 8),
        (1, 10),
        (2, 3),
        (2, 6),
        (2, 7),
        (2, 8),
        (2, 9),
        (3, 8),
        (3, 9),
        (3, 10),
        (3, 11),
        (4, 5),
        (4, 7),
        (4, 9),
        (5, 9),
        (5, 11),
        (6, 7),
        (8, 9),
        (10, 11),
    ];
    assert_eq!(edges, expected);
}

#[test]
fn truncated_deflate_with_a_full_tail_is_rejected() {
    let good = encode(&explicit(&[1, 2, 3], &[(1, 2), (2, 3)])).expect("encode");
    let n = good.len();
    // Keep the header, drop the last deflate byte, keep the 8-byte tail.
    let mut cut = good[..n - 9].to_vec();
    cut.extend_from_slice(&good[n - 8..]);
    assert_eq!(file_err(&cut), BlobError::Gzip);
    // Keep only part of the deflate stream with the tail.
    let mut half = good[..12].to_vec();
    half.extend_from_slice(&good[n - 8..]);
    assert_eq!(file_err(&half), BlobError::Gzip);
}

#[test]
fn partial_second_member_is_rejected() {
    let good = encode(&explicit(&[1, 2], &[(1, 2)])).expect("encode");
    for extra in 1..=7 {
        let mut file = good.clone();
        file.extend_from_slice(&good[..extra]);
        assert_eq!(file_err(&file), BlobError::Gzip, "extra bytes: {extra}");
    }
}

#[test]
fn python_written_blobs_decode() {
    // Both files were written by the Python oracle encoder:
    // `python -m family_oracle topology encode IN.json OUT.bin.gz` (from conformance/).
    let sparse = decode(include_bytes!("data/python-t-sparse.bin.gz")).expect("sparse");
    assert_eq!(
        sparse,
        explicit(
            &[4_294_967_295, 0, 7, 3],
            &[
                (4_294_967_295, 0),
                (0, 4_294_967_295),
                (7, 3),
                (7, 3),
                (3, 3),
            ],
        )
    );
    let removed = decode(include_bytes!("data/python-z22-removed.bin.gz")).expect("z22");
    assert_eq!(removed, zephyr(2, 2, &[5], &[(1, 2)]));
    let (nodes, edges) = zephyr_lattice(2, 2).expect("z(2,2)");
    let topology = removed.topology().expect("expand");
    assert_eq!(topology.nodes.len(), nodes.len() - 1);
    assert!(!topology.nodes.contains(&5));
    assert!(!topology.edges.contains(&(1, 2)));
    let expected: Vec<(u32, u32)> = edges
        .into_iter()
        .filter(|&(a, b)| a != 5 && b != 5 && (a, b) != (1, 2))
        .collect();
    assert_eq!(topology.edges, expected);
}
