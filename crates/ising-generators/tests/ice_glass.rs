//! The `quip/ice-glass@1` construction: admission codes, generation, and the
//! metadata check.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;

use ising_generators::descriptor::{admit, params_digest, Admitted, AdvertisedLimits, SizeBounds};
use ising_generators::family::{admit_topology, generate_instance, BaseTopology};
use ising_generators::ice::{
    candidates, select, sides, zephyr_lattice, Lattice, ICE_GLASS_V1, STRONG_MILLI,
};
use ising_generators::instance::{Instance, InstanceLimits, InstanceParts};
use ising_generators::Code;

#[expect(dead_code, reason = "this test uses only the live topology loader")]
mod common;

const CONTRACTS: &[ising_generators::descriptor::Contract] = &[ICE_GLASS_V1];

/// Canonical parameter bytes: format, then `(tag, len, payload)` per field.
fn params(m: u16, t: u8, glass: u16) -> Vec<u8> {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(&[1, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&m.to_le_bytes());
    bytes.extend_from_slice(&[2, 0, 1, 0, 0, 0, t]);
    bytes.extend_from_slice(&[3, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&glass.to_le_bytes());
    bytes
}

fn admitted(bytes: &[u8]) -> Result<Admitted<'_>, Code> {
    admit(
        CONTRACTS,
        b"quip/ice-glass",
        1,
        bytes,
        &params_digest(bytes),
    )
}

fn bounds(admitted: &Admitted<'_>, topology: &BaseTopology) -> Result<SizeBounds, Code> {
    admit_topology(
        admitted,
        topology,
        topology.order_digest(),
        &AdvertisedLimits::default(),
    )
}

fn lattice_topology(m: u16, t: u8) -> BaseTopology {
    let (nodes, edges) = zephyr_lattice(m, t).expect("lattice");
    BaseTopology::new(nodes, &edges).expect("topology")
}

fn nonce_of(i: u8) -> [u8; 32] {
    [i; 32]
}

fn build(bytes: &[u8], topology: &BaseTopology, nonce: [u8; 32]) -> Instance {
    let admitted = admitted(bytes).expect("admit");
    let bounds = bounds(&admitted, topology).expect("admit topology");
    generate_instance(&admitted, topology, &bounds, nonce).expect("generate")
}

/// The decoded metadata: tiles and gauge.
fn decode(instance: &Instance) -> (Vec<[u32; 4]>, Vec<i64>) {
    let region = instance.metadata();
    let word = |at: usize| u32::from_le_bytes(region[at..at + 4].try_into().expect("four bytes"));
    assert_eq!(&region[..2], &[2, 0], "two sections");
    assert_eq!(&region[2..4], &[1, 0], "tiles first");
    let tile_len = word(4) as usize;
    let count = word(8) as usize;
    assert_eq!(tile_len, 4 + 16 * count);
    let tiles = (0..count)
        .map(|k| {
            let at = 12 + 16 * k;
            [word(at), word(at + 4), word(at + 8), word(at + 12)]
        })
        .collect();
    let gauge_at = 8 + tile_len;
    assert_eq!(&region[gauge_at..gauge_at + 2], &[2, 0], "gauge second");
    let n = word(gauge_at + 6) as usize;
    assert_eq!(word(gauge_at + 2) as usize, 4 + n);
    let gauge = region[gauge_at + 10..]
        .iter()
        .map(|&b| if b == 0 { 1 } else { -1 })
        .collect::<Vec<i64>>();
    assert_eq!(gauge.len(), n);
    (tiles, gauge)
}

fn tile_pairs(tile: [u32; 4]) -> [(u32, u32); 6] {
    let [a, b, c, d] = tile;
    [(a, b), (a, c), (a, d), (b, c), (b, d), (c, d)]
}

/// A copy of `instance` with new couplings and metadata.
fn rebuilt(instance: &Instance, j: Vec<i32>, metadata: Vec<u8>) -> Instance {
    let parts = InstanceParts {
        descriptor_digest: *instance.descriptor_digest(),
        topology_order_digest: *instance.topology_order_digest(),
        nonce: instance.nonce().copied(),
        node_ids: instance.node_ids().to_vec(),
        edges: instance.edges().to_vec(),
        h: instance.h().to_vec(),
        j,
        offset_milli: instance.offset_milli(),
        metadata,
    };
    Instance::new(
        parts,
        &ICE_GLASS_V1.limits,
        ICE_GLASS_V1.metadata_kinds,
        None,
    )
    .expect("rebuilt instance is well formed")
}

fn check(bytes: &[u8], instance: &Instance) -> bool {
    let admitted = admitted(bytes).expect("admit");
    let construction = ICE_GLASS_V1.construction.expect("construction");
    (construction.check_metadata)(admitted.params(), instance)
}

#[test]
fn the_contract_limits_are_pinned() {
    let limits = ICE_GLASS_V1.limits;
    assert_eq!(limits.max_nodes(), 131_072);
    assert_eq!(limits.max_edges(), 2_097_152);
    assert_eq!(limits.max_coefficient(), 1_000);
    assert_eq!(limits.max_offset(), 0);
    assert_eq!(limits.max_metadata(), 8_388_608);
}

#[test]
fn generation_is_repeatable() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125);
    let mut seen = Vec::new();
    for nonce in [nonce_of(1), nonce_of(2)] {
        let first = build(&bytes, &topology, nonce)
            .canonical_bytes()
            .expect("bytes");
        let again = build(&bytes, &topology, nonce)
            .canonical_bytes()
            .expect("bytes");
        assert_eq!(first, again);
        seen.push(first);
    }
    assert_ne!(seen[0], seen[1], "the nonce changes the instance");
}

#[test]
fn both_modes_build_valid_instances() {
    let topology = lattice_topology(2, 2);
    for (m, t, lattice) in [
        (2, 2, Lattice::Zephyr { m: 2, t: 2 }),
        (0, 0, Lattice::Adjacency),
    ] {
        let bytes = params(m, t, 125);
        let instance = build(&bytes, &topology, nonce_of(3));
        assert!(check(&bytes, &instance), "mode ({m}, {t})");
        let found = candidates(lattice, &topology).expect("candidates");
        let split = sides(lattice, &topology, &found).expect("sides");
        let left: Vec<u32> = (0..u32::try_from(split.left.len()).expect("fits")).collect();
        let right: Vec<u32> = (0..u32::try_from(split.right.len()).expect("fits")).collect();
        let selected = select(&found, &split, &left, &right).expect("select");
        let (tiles, _) = decode(&instance);
        assert_eq!(tiles.len(), selected.len(), "mode ({m}, {t})");
    }
}

#[test]
fn admission_codes_follow_fam_47() {
    let topology = lattice_topology(2, 2);
    let mixed = params(2, 0, 125);
    assert_eq!(
        bounds(&admitted(&mixed).expect("admit"), &topology),
        Err(Code::TopologyIncompatible)
    );
    let cycle =
        BaseTopology::new(vec![0, 1, 2, 3], &[(0, 1), (1, 2), (2, 3), (3, 0)]).expect("cycle");
    let adjacency = params(0, 0, 125);
    assert_eq!(
        bounds(&admitted(&adjacency).expect("admit"), &cycle),
        Err(Code::TopologyIncompatible)
    );
    let too_strong = params(2, 2, 1000);
    assert_eq!(admitted(&too_strong).err(), Some(Code::ParamsOutOfBounds));
    let star_edges: Vec<(u32, u32)> = (1..=33).map(|leaf| (0, leaf)).collect();
    let star = BaseTopology::new((0..34).collect(), &star_edges).expect("star");
    assert_eq!(
        bounds(&admitted(&adjacency).expect("admit"), &star),
        Err(Code::BoundsExceeded)
    );
}

fn small_topology(nodes: Vec<u32>, edges: &[(u32, u32)]) -> BaseTopology {
    BaseTopology::new(nodes, edges).expect("test topology is valid")
}

fn clique(nodes: [u32; 4]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for (i, &a) in nodes.iter().enumerate() {
        for &b in &nodes[i + 1..] {
            out.push((a, b));
        }
    }
    out
}

fn union(parts: &[Vec<(u32, u32)>]) -> Vec<(u32, u32)> {
    let set: BTreeSet<_> = parts.iter().flatten().copied().collect();
    set.into_iter().collect()
}

fn adjacency_graph(edges: &[(u32, u32)]) -> BaseTopology {
    let n = edges
        .iter()
        .map(|&(a, b)| a.max(b))
        .max()
        .map_or(0, |x| x + 1);
    small_topology((0..n).collect(), edges)
}

#[test]
fn every_structure_fault_is_code_12_and_a_wide_node_wins_with_13() {
    // A node on three spine edges.
    let mut around = Vec::new();
    let mut next = 4;
    for p in 1..=3 {
        for _ in 0..2 {
            around.push(clique([0, p, next, next + 1]));
            next += 2;
        }
    }
    // Two shared edges of one clique meet at node 0.
    let conflict = union(&[
        clique([0, 1, 2, 3]),
        clique([0, 1, 4, 5]),
        clique([0, 2, 6, 7]),
    ]);
    // A ring of five tiles: the spine graph is an odd cycle.
    let ring: Vec<_> = (0..5u32)
        .map(|i| {
            let next = (i + 1) % 5;
            clique([2 * i, 2 * i + 1, 2 * next, 2 * next + 1])
        })
        .collect();
    let adjacency = params(0, 0, 125);
    let zephyr = params(1, 1, 125);
    let faults: Vec<(&str, &[u8], BaseTopology)> = vec![
        (
            "duplicate-edge",
            &adjacency,
            adjacency_graph(&[(0, 1), (1, 0)]),
        ),
        (
            "not-zephyr-node",
            &zephyr,
            small_topology(vec![0, 12], &[(0, 12)]),
        ),
        (
            "not-zephyr-coupler",
            &zephyr,
            small_topology(vec![0, 3], &[(3, 0)]),
        ),
        ("spine-conflict", &adjacency, adjacency_graph(&conflict)),
        (
            "membership-above-two",
            &adjacency,
            adjacency_graph(&union(&around)),
        ),
        ("not-bipartite", &adjacency, adjacency_graph(&union(&ring))),
    ];
    for (name, bytes, topology) in &faults {
        assert_eq!(
            bounds(&admitted(bytes).expect("admit"), topology),
            Err(Code::TopologyIncompatible),
            "{name}"
        );
    }
    let mut wide: Vec<(u32, u32)> = (1..=33).map(|leaf| (0, leaf)).collect();
    wide.push((1, 0));
    let star = adjacency_graph(&wide);
    assert_eq!(
        bounds(&admitted(&adjacency).expect("admit"), &star),
        Err(Code::BoundsExceeded)
    );
}

#[test]
fn gauge_hides_and_metadata_recovers_the_ice() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125);
    let instance = build(&bytes, &topology, nonce_of(4));
    let (tiles, gauge) = decode(&instance);
    assert!(!tiles.is_empty());
    for tile in tiles {
        for pair in tile_pairs(tile) {
            let at = instance.edges().binary_search(&pair).expect("tile edge");
            let hidden =
                i64::from(instance.j()[at]) * gauge[pair.0 as usize] * gauge[pair.1 as usize];
            assert_eq!(hidden, i64::from(STRONG_MILLI));
        }
    }
}

#[test]
fn check_metadata_rejects_a_flipped_tile_coupler() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125);
    let instance = build(&bytes, &topology, nonce_of(5));
    let (tiles, _) = decode(&instance);
    let pair = tile_pairs(*tiles.first().expect("a tile"))[0];
    let at = instance.edges().binary_search(&pair).expect("tile edge");
    let mut j = instance.j().to_vec();
    j[at] = -j[at];
    let flipped = rebuilt(&instance, j, instance.metadata().to_vec());
    assert!(!check(&bytes, &flipped));
}

#[test]
fn check_metadata_rejects_a_tile_off_the_instance() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125);
    let instance = build(&bytes, &topology, nonce_of(6));
    let edges: BTreeSet<(u32, u32)> = instance.edges().iter().copied().collect();
    let n = u32::try_from(instance.node_ids().len()).expect("fits");
    let off = [0, 1, n - 2, n - 1];
    assert!(
        tile_pairs(off).iter().any(|pair| !edges.contains(pair)),
        "the quad is not a clique"
    );
    let mut region = instance.metadata().to_vec();
    for (k, pos) in off.iter().enumerate() {
        let at = 12 + 4 * k;
        region[at..at + 4].copy_from_slice(&pos.to_le_bytes());
    }
    let moved = rebuilt(&instance, instance.j().to_vec(), region);
    assert!(!check(&bytes, &moved));
}

#[test]
fn check_metadata_rejects_a_nonzero_field() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125);
    let instance = build(&bytes, &topology, nonce_of(7));
    let mut h = instance.h().to_vec();
    h[0] = 1;
    let parts = InstanceParts {
        descriptor_digest: *instance.descriptor_digest(),
        topology_order_digest: *instance.topology_order_digest(),
        nonce: instance.nonce().copied(),
        node_ids: instance.node_ids().to_vec(),
        edges: instance.edges().to_vec(),
        h,
        j: instance.j().to_vec(),
        offset_milli: 0,
        metadata: instance.metadata().to_vec(),
    };
    let tilted = Instance::new(
        parts,
        &ICE_GLASS_V1.limits,
        ICE_GLASS_V1.metadata_kinds,
        None,
    )
    .expect("well formed");
    assert!(!check(&bytes, &tilted));
}

#[test]
fn check_metadata_survives_damaged_regions() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125);
    let instance = build(&bytes, &topology, nonce_of(8));
    let region = instance.metadata().to_vec();
    for len in 0..region.len() {
        let cut = region[..len].to_vec();
        let parts = InstanceParts {
            descriptor_digest: *instance.descriptor_digest(),
            topology_order_digest: *instance.topology_order_digest(),
            nonce: instance.nonce().copied(),
            node_ids: instance.node_ids().to_vec(),
            edges: instance.edges().to_vec(),
            h: instance.h().to_vec(),
            j: instance.j().to_vec(),
            offset_milli: 0,
            metadata: cut,
        };
        if let Ok(damaged) = Instance::new(
            parts,
            &ICE_GLASS_V1.limits,
            ICE_GLASS_V1.metadata_kinds,
            None,
        ) {
            assert!(!check(&bytes, &damaged), "cut at {len}");
        }
    }
}

#[test]
fn live_generation_has_1785_tiles() {
    let topology = common::live_topology();
    let bytes = params(12, 4, 125);
    let instance = build(&bytes, topology, nonce_of(9));
    let (tiles, gauge) = decode(&instance);
    assert_eq!(tiles.len(), 1785);
    let strong = instance
        .edges()
        .iter()
        .zip(instance.j())
        .filter(|(&(u, v), &j)| {
            i64::from(j) * gauge[u as usize] * gauge[v as usize] == i64::from(STRONG_MILLI)
        })
        .count();
    assert_eq!(strong, 10_710);
}

/// A kind 1 and kind 2 region from raw tiles and raw gauge bytes.
fn region_of(tiles: &[[u32; 4]], gauge: &[u8]) -> Vec<u8> {
    let count = u32::try_from(tiles.len()).expect("fits");
    let tile_len = 4 + 16 * count;
    let mut region = vec![2, 0, 1, 0];
    region.extend_from_slice(&tile_len.to_le_bytes());
    region.extend_from_slice(&count.to_le_bytes());
    for tile in tiles {
        for pos in tile {
            region.extend_from_slice(&pos.to_le_bytes());
        }
    }
    region.extend_from_slice(&[2, 0]);
    let n = u32::try_from(gauge.len()).expect("fits");
    region.extend_from_slice(&(4 + n).to_le_bytes());
    region.extend_from_slice(&n.to_le_bytes());
    region.extend_from_slice(gauge);
    region
}

/// The raw gauge bytes of a valid instance.
fn gauge_bytes(instance: &Instance) -> Vec<u8> {
    let (_, gauge) = decode(instance);
    gauge.iter().map(|&g| u8::from(g < 0)).collect()
}

/// A valid z(2,2) instance, its parameter bytes, tiles, and gauge bytes.
fn valid_z22(nonce: u8) -> (Vec<u8>, Instance, Vec<[u32; 4]>, Vec<u8>) {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125);
    let instance = build(&bytes, &topology, nonce_of(nonce));
    let (tiles, _) = decode(&instance);
    let gauge = gauge_bytes(&instance);
    assert!(check(&bytes, &instance), "the base instance is valid");
    (bytes, instance, tiles, gauge)
}

#[test]
fn check_metadata_rejects_a_gauge_byte_above_one() {
    let (bytes, instance, tiles, mut gauge) = valid_z22(10);
    gauge[0] = 2;
    let bad = rebuilt(&instance, instance.j().to_vec(), region_of(&tiles, &gauge));
    assert!(!check(&bytes, &bad));
}

#[test]
fn check_metadata_rejects_a_gauge_count_that_differs_from_the_node_count() {
    let (bytes, instance, tiles, gauge) = valid_z22(11);
    let mut region = region_of(&tiles, &gauge);
    let count_at = region.len() - gauge.len() - 4;
    let short = u32::try_from(gauge.len() - 1).expect("fits");
    region[count_at..count_at + 4].copy_from_slice(&short.to_le_bytes());
    let bad = rebuilt(&instance, instance.j().to_vec(), region);
    assert!(!check(&bytes, &bad));
}

#[test]
fn check_metadata_rejects_tiles_that_share_an_edge() {
    let (bytes, instance, tiles, gauge) = valid_z22(12);
    let n = u32::try_from(instance.node_ids().len()).expect("fits");
    let first = *tiles.first().expect("a tile");
    let overlap = [first[0], first[1], n - 2, n - 1];
    assert!(
        first < overlap,
        "the overlapping tile sorts after the first"
    );
    let bad = rebuilt(
        &instance,
        instance.j().to_vec(),
        region_of(&[first, overlap], &gauge),
    );
    assert!(!check(&bytes, &bad));
}

#[test]
fn check_metadata_rejects_a_node_in_three_tiles() {
    let (bytes, instance, tiles, gauge) = valid_z22(13);
    let n = u32::try_from(instance.node_ids().len()).expect("fits");
    let shared = tiles
        .iter()
        .enumerate()
        .flat_map(|(i, a)| tiles.iter().skip(i + 1).map(move |b| (*a, *b)))
        .find_map(|(a, b)| a.iter().find(|pos| b.contains(pos)).map(|&pos| (a, b, pos)));
    let (a, b, node) = shared.expect("two selected tiles share a node");
    let mut third = [node, n - 3, n - 2, n - 1];
    third.sort_unstable();
    assert!(b < third, "the third tile sorts last");
    let bad = rebuilt(
        &instance,
        instance.j().to_vec(),
        region_of(&[a, b, third], &gauge),
    );
    assert!(!check(&bytes, &bad));
}

#[test]
fn check_metadata_rejects_a_tile_count_the_payload_cannot_hold() {
    let (bytes, instance, tiles, gauge) = valid_z22(14);
    let mut region = region_of(&tiles, &gauge);
    region[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    let bad = rebuilt(&instance, instance.j().to_vec(), region);
    assert!(!check(&bytes, &bad));
}

#[test]
fn check_metadata_rejects_a_glass_edge_of_the_wrong_magnitude() {
    let (bytes, instance, tiles, _) = valid_z22(15);
    let strong: BTreeSet<(u32, u32)> = tiles.iter().flat_map(|t| tile_pairs(*t)).collect();
    let at = instance
        .edges()
        .iter()
        .position(|edge| !strong.contains(edge))
        .expect("a glass edge");
    let mut j = instance.j().to_vec();
    j[at] = 1;
    let bad = rebuilt(&instance, j, instance.metadata().to_vec());
    assert!(!check(&bytes, &bad));
}

#[test]
fn check_metadata_rejects_a_nonzero_offset() {
    let (bytes, instance, _, _) = valid_z22(16);
    let parts = InstanceParts {
        descriptor_digest: *instance.descriptor_digest(),
        topology_order_digest: *instance.topology_order_digest(),
        nonce: instance.nonce().copied(),
        node_ids: instance.node_ids().to_vec(),
        edges: instance.edges().to_vec(),
        h: instance.h().to_vec(),
        j: instance.j().to_vec(),
        offset_milli: 1,
        metadata: instance.metadata().to_vec(),
    };
    let limits = InstanceLimits::new(131_072, 2_097_152, 1_000, 1, 8_388_608).expect("limits");
    let shifted = Instance::new(parts, &limits, ICE_GLASS_V1.metadata_kinds, None)
        .expect("well formed under a looser offset limit");
    assert!(!check(&bytes, &shifted));
}

#[test]
fn check_metadata_rejects_tiles_that_are_not_ascending() {
    let (bytes, instance, tiles, gauge) = valid_z22(17);
    let mut swapped = tiles.clone();
    swapped[0].swap(0, 1);
    let bad = rebuilt(
        &instance,
        instance.j().to_vec(),
        region_of(&swapped, &gauge),
    );
    assert!(!check(&bytes, &bad), "positions out of order inside a tile");
    let mut reordered = tiles;
    reordered.swap(0, 1);
    let bad = rebuilt(
        &instance,
        instance.j().to_vec(),
        region_of(&reordered, &gauge),
    );
    assert!(!check(&bytes, &bad), "tiles out of order");
}

#[test]
fn check_metadata_rejects_tiles_that_are_not_distinct() {
    let (bytes, instance, tiles, gauge) = valid_z22(18);
    let first = *tiles.first().expect("a tile");
    let bad = rebuilt(
        &instance,
        instance.j().to_vec(),
        region_of(&[first, first], &gauge),
    );
    assert!(!check(&bytes, &bad));
}
