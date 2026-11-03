//! Twisted ice: local gauges, obstructions, and the
//! `quip/twisted-ice-dimer@1` construction.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;

use ising_generators::descriptor::{
    admit, params_digest, Admitted, AdvertisedLimits, Contract, SizeBounds, Value,
};
use ising_generators::family::{admit_topology, generate_instance, BaseTopology, SaltFault};
use ising_generators::ice::{
    candidates, gauge_obstructions, local_gauges, select, sides, zephyr_lattice, Lattice,
    Obstructions, StructureError, PAIR_MILLI, RECIPES, STRONG_MILLI, TWISTED_DIMER_V1,
};
use ising_generators::instance::{Instance, InstanceParts};
use ising_generators::Code;

#[expect(dead_code, reason = "this test uses only the live topology loader")]
mod common;

/// Three tiles of four positions in a ring. Tiles 0 and 1 share position 3,
/// tiles 1 and 2 share position 6, and tiles 2 and 0 share position 0.
const RING: [[u32; 4]; 3] = [[0, 1, 2, 3], [3, 4, 5, 6], [6, 7, 8, 0]];
const RING_NODES: usize = 9;

#[test]
fn obstructions_of_no_tiles_are_zero() {
    let result = gauge_obstructions(&[], &[], 0);
    assert_eq!(
        result,
        Ok(Obstructions {
            components: 0,
            cycle_rank: 0,
            frustrated_cycles: 0,
            gauge_equivalent: true,
        })
    );
}

#[test]
fn a_consistent_ring_is_gauge_equivalent() {
    let gauges = [[1_i8; 4]; 3];
    let result = gauge_obstructions(&RING, &gauges, RING_NODES);
    assert_eq!(
        result,
        Ok(Obstructions {
            components: 1,
            cycle_rank: 1,
            frustrated_cycles: 0,
            gauge_equivalent: true,
        })
    );
}

#[test]
fn a_flipped_ring_has_one_frustrated_cycle() {
    let mut gauges = [[1_i8; 4]; 3];
    // Flip tile 0 at position 3 only, which flips one link relation.
    gauges[0][3] = -1;
    let result = gauge_obstructions(&RING, &gauges, RING_NODES);
    assert_eq!(
        result,
        Ok(Obstructions {
            components: 1,
            cycle_rank: 1,
            frustrated_cycles: 1,
            gauge_equivalent: false,
        })
    );
}

#[test]
fn a_node_in_three_tiles_is_an_error() {
    let tiles = [[0, 1, 2, 3], [3, 4, 5, 6], [3, 7, 8, 9]];
    let gauges = [[1_i8; 4]; 3];
    assert_eq!(
        gauge_obstructions(&tiles, &gauges, 10),
        Err(StructureError::MembershipAboveTwo(3))
    );
}

#[test]
fn bad_gauge_inputs_are_errors() {
    let gauges = [[1_i8; 4]; 3];
    assert_eq!(
        gauge_obstructions(&RING, &gauges[..2], RING_NODES),
        Err(StructureError::Overflow)
    );
    let mut zero = gauges;
    zero[1][0] = 0;
    assert_eq!(
        gauge_obstructions(&RING, &zero, RING_NODES),
        Err(StructureError::Overflow)
    );
    assert_eq!(
        gauge_obstructions(&RING, &gauges, RING_NODES - 1),
        Err(StructureError::Overflow)
    );
}

#[test]
fn parallel_links_with_different_relations_frustrate_one_cycle() {
    // Tiles 0 and 1 share positions 0 and 3. The relation at position 0 is
    // -1 and the relation at position 3 is +1.
    let tiles = [[0, 1, 2, 3], [0, 3, 4, 5]];
    let gauges = [[-1_i8, 1, 1, 1], [1, 1, 1, 1]];
    let result = gauge_obstructions(&tiles, &gauges, 6);
    assert_eq!(
        result,
        Ok(Obstructions {
            components: 1,
            cycle_rank: 1,
            frustrated_cycles: 1,
            gauge_equivalent: false,
        })
    );
}

#[test]
fn separate_tiles_count_as_components() {
    let tiles = [[0, 1, 2, 3], [4, 5, 6, 7]];
    let gauges = [[1_i8; 4]; 2];
    assert_eq!(
        gauge_obstructions(&tiles, &gauges, 8),
        Ok(Obstructions {
            components: 2,
            cycle_rank: 0,
            frustrated_cycles: 0,
            gauge_equivalent: true,
        })
    );
}

#[test]
fn the_lowest_overfull_position_is_reported() {
    let tiles = [[0, 1, 2, 3], [3, 1, 4, 5], [1, 3, 6, 7]];
    let gauges = [[1_i8; 4]; 3];
    assert_eq!(
        gauge_obstructions(&tiles, &gauges, 8),
        Err(StructureError::MembershipAboveTwo(1))
    );
}

#[test]
fn a_huge_node_bound_does_not_allocate() {
    assert!(gauge_obstructions(&[], &[], usize::MAX).is_ok());
}

// ---- quip/twisted-ice-dimer@1 ----

const CONTRACTS: &[Contract] = &[TWISTED_DIMER_V1];

/// Canonical parameter bytes: format, then `(tag, len, payload)` per field.
fn params(m: u16, t: u8, weak: u16, pairs: u32) -> Vec<u8> {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(&[1, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&m.to_le_bytes());
    bytes.extend_from_slice(&[2, 0, 1, 0, 0, 0, t]);
    bytes.extend_from_slice(&[3, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&weak.to_le_bytes());
    bytes.extend_from_slice(&[4, 0, 4, 0, 0, 0]);
    bytes.extend_from_slice(&pairs.to_le_bytes());
    bytes
}

fn admitted(bytes: &[u8]) -> Result<Admitted<'_>, Code> {
    admit(
        CONTRACTS,
        b"quip/twisted-ice-dimer",
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

fn try_build(
    bytes: &[u8],
    topology: &BaseTopology,
    nonce: [u8; 32],
) -> Result<Instance, SaltFault> {
    let admitted = admitted(bytes).expect("admit");
    let bounds = bounds(&admitted, topology).expect("admit topology");
    generate_instance(&admitted, topology, &bounds, nonce)
}

fn build(bytes: &[u8], topology: &BaseTopology, nonce: [u8; 32]) -> Instance {
    try_build(bytes, topology, nonce)
        .map_err(|fault| format!("{fault:?}"))
        .expect("generate")
}

fn check(bytes: &[u8], instance: &Instance) -> bool {
    let admitted = admitted(bytes).expect("admit");
    let construction = TWISTED_DIMER_V1.construction.expect("construction");
    (construction.check_metadata)(admitted.params(), instance)
}

/// The three metadata payloads, in kind order.
fn split(instance: &Instance) -> [Vec<u8>; 3] {
    let region = instance.metadata();
    assert_eq!(&region[..2], &[3, 0], "three sections");
    let mut at = 2;
    [1u16, 2, 3].map(|kind| {
        assert_eq!(&region[at..at + 2], &kind.to_le_bytes());
        let len = u32::from_le_bytes(region[at + 2..at + 6].try_into().expect("four bytes"));
        let end = at + 6 + len as usize;
        let payload = region[at + 6..end].to_vec();
        at = end;
        payload
    })
}

/// A region from three payloads.
fn join(payloads: &[Vec<u8>; 3]) -> Vec<u8> {
    let mut region = vec![3, 0];
    for (kind, payload) in [1u16, 2, 3].into_iter().zip(payloads) {
        region.extend_from_slice(&kind.to_le_bytes());
        region.extend_from_slice(&u32::try_from(payload.len()).expect("fits").to_le_bytes());
        region.extend_from_slice(payload);
    }
    region
}

/// The pairs of a valid instance.
fn pairs_of(instance: &Instance) -> Vec<(u32, u32)> {
    let [_, _, payload] = split(instance);
    let word = |at: usize| u32::from_le_bytes(payload[at..at + 4].try_into().expect("four bytes"));
    let count = word(0) as usize;
    assert_eq!(payload.len(), 4 + 8 * count);
    (0..count)
        .map(|k| (word(4 + 8 * k), word(8 + 8 * k)))
        .collect()
}

fn tile_pairs(tile: [u32; 4]) -> [(u32, u32); 6] {
    let [a, b, c, d] = tile;
    [(a, b), (a, c), (a, d), (b, c), (b, d), (c, d)]
}

fn tile_edge_set(instance: &Instance) -> BTreeSet<(u32, u32)> {
    let (tiles, _) = local_gauges(instance).expect("local gauges");
    tiles.into_iter().flat_map(tile_pairs).collect()
}

fn edge_position(instance: &Instance, edge: (u32, u32)) -> usize {
    instance.edges().binary_search(&edge).expect("an edge")
}

/// A copy of `instance` with new fields.
fn rebuilt_with(instance: &Instance, h: Vec<i32>, j: Vec<i32>, metadata: Vec<u8>) -> Instance {
    let parts = InstanceParts {
        descriptor_digest: *instance.descriptor_digest(),
        topology_order_digest: *instance.topology_order_digest(),
        nonce: instance.nonce().copied(),
        node_ids: instance.node_ids().to_vec(),
        edges: instance.edges().to_vec(),
        h,
        j,
        offset_milli: instance.offset_milli(),
        metadata,
    };
    Instance::new(
        parts,
        &TWISTED_DIMER_V1.limits,
        TWISTED_DIMER_V1.metadata_kinds,
        None,
    )
    .expect("rebuilt instance is well formed")
}

fn rebuilt(instance: &Instance, j: Vec<i32>, metadata: Vec<u8>) -> Instance {
    rebuilt_with(instance, instance.h().to_vec(), j, metadata)
}

#[test]
fn generation_is_repeatable() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 62, 8);
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
        let bytes = params(m, t, 62, 8);
        let instance = build(&bytes, &topology, nonce_of(3));
        assert!(check(&bytes, &instance), "mode ({m}, {t})");
        let found = candidates(lattice, &topology).expect("candidates");
        let split = sides(lattice, &topology, &found).expect("sides");
        let left: Vec<u32> = (0..u32::try_from(split.left.len()).expect("fits")).collect();
        let right: Vec<u32> = (0..u32::try_from(split.right.len()).expect("fits")).collect();
        let selected = select(&found, &split, &left, &right).expect("select");
        let (tiles, gauges) = local_gauges(&instance).expect("local gauges");
        assert_eq!(tiles.len(), selected.len(), "mode ({m}, {t})");
        assert_eq!(gauges.len(), selected.len(), "mode ({m}, {t})");
        assert_eq!(pairs_of(&instance).len(), 8, "mode ({m}, {t})");
    }
}

#[test]
fn zero_pairs_gives_an_empty_pair_section() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 62, 0);
    let instance = build(&bytes, &topology, nonce_of(4));
    assert!(check(&bytes, &instance));
    let [_, _, pairs] = split(&instance);
    assert_eq!(pairs, vec![0, 0, 0, 0]);
    let tile_edges = tile_edge_set(&instance);
    for (edge, &j) in instance.edges().iter().zip(instance.j()) {
        assert!(tile_edges.contains(edge) || j != PAIR_MILLI);
    }
}

#[test]
fn the_pair_bound_is_floor_half_the_nodes() {
    let topology = lattice_topology(2, 2);
    let half = u32::try_from(topology.node_ids().len() / 2).expect("fits");
    let at_bound = params(2, 2, 62, half);
    assert!(bounds(&admitted(&at_bound).expect("admit"), &topology).is_ok());
    let above = params(2, 2, 62, half + 1);
    assert_eq!(
        bounds(&admitted(&above).expect("admit"), &topology),
        Err(Code::TopologyIncompatible)
    );
}

#[test]
fn a_shortfall_skips_the_salt() {
    let topology = lattice_topology(2, 2);
    let half = u32::try_from(topology.node_ids().len() / 2).expect("fits");
    let bytes = params(2, 2, 62, half);
    let skipped = (0..16u8).any(|i| {
        matches!(
            try_build(&bytes, &topology, nonce_of(i)),
            Err(SaltFault::Skip(Code::NoInstance))
        )
    });
    assert!(skipped, "some nonce of the first 16 falls short");
}

#[test]
fn each_tile_edge_uses_its_own_tile_gauges() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 62, 8);
    let mut shared_nodes = 0;
    for i in 0..4u8 {
        let instance = build(&bytes, &topology, nonce_of(i));
        let (tiles, gauges) = local_gauges(&instance).expect("local gauges");
        for (tile, gauge) in tiles.iter().zip(&gauges) {
            let local = |pos: u32| {
                let at = tile.iter().position(|&x| x == pos).expect("a tile node");
                i32::from(gauge[at])
            };
            for (a, b) in tile_pairs(*tile) {
                let j = instance.j()[edge_position(&instance, (a, b))];
                assert_eq!(j, STRONG_MILLI * local(a) * local(b), "tile edge {a}-{b}");
            }
        }
        let mut seen = BTreeSet::new();
        for tile in &tiles {
            for &pos in tile {
                if !seen.insert(pos) {
                    shared_nodes += 1;
                }
            }
        }
    }
    assert!(shared_nodes > 0, "z(2,2) has nodes in two tiles");
}

#[test]
fn pairs_are_never_tile_edges_and_are_disjoint() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 62, 12);
    for i in 0..4u8 {
        let instance = build(&bytes, &topology, nonce_of(i));
        let tile_edges = tile_edge_set(&instance);
        let pairs = pairs_of(&instance);
        assert_eq!(pairs.len(), 12);
        let mut nodes = BTreeSet::new();
        for &(a, b) in &pairs {
            assert!(a < b);
            assert!(!tile_edges.contains(&(a, b)), "pair {a}-{b} is a tile edge");
            assert_eq!(instance.j()[edge_position(&instance, (a, b))], PAIR_MILLI);
            assert!(nodes.insert(a) && nodes.insert(b), "pairs share a node");
        }
        let paired = instance
            .edges()
            .iter()
            .zip(instance.j())
            .filter(|(edge, &j)| !tile_edges.contains(edge) && j == PAIR_MILLI)
            .count();
        assert_eq!(paired, pairs.len());
    }
}

#[test]
fn admission_codes_follow_fam_47() {
    let topology = lattice_topology(2, 2);
    let mixed = params(2, 0, 62, 8);
    assert_eq!(
        bounds(&admitted(&mixed).expect("admit"), &topology),
        Err(Code::TopologyIncompatible)
    );
    let cycle =
        BaseTopology::new(vec![0, 1, 2, 3], &[(0, 1), (1, 2), (2, 3), (3, 0)]).expect("cycle");
    let adjacency = params(0, 0, 62, 0);
    assert_eq!(
        bounds(&admitted(&adjacency).expect("admit"), &cycle),
        Err(Code::TopologyIncompatible)
    );
    let too_strong = params(2, 2, 1000, 8);
    assert_eq!(admitted(&too_strong).err(), Some(Code::ParamsOutOfBounds));
    let too_many = params(2, 2, 62, 65_537);
    assert_eq!(admitted(&too_many).err(), Some(Code::ParamsOutOfBounds));
    let star_edges: Vec<(u32, u32)> = (1..=33).map(|leaf| (0, leaf)).collect();
    let star = BaseTopology::new((0..34).collect(), &star_edges).expect("star");
    assert_eq!(
        bounds(&admitted(&adjacency).expect("admit"), &star),
        Err(Code::BoundsExceeded)
    );
}

/// A valid z(2,2) instance with 8 pairs: its parameter bytes and the instance.
fn valid_z22(nonce: u8) -> (Vec<u8>, Instance) {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 62, 8);
    let instance = build(&bytes, &topology, nonce_of(nonce));
    assert!(check(&bytes, &instance), "the base instance is valid");
    (bytes, instance)
}

#[test]
fn check_metadata_rejects_each_rule() {
    let (bytes, instance) = valid_z22(10);
    let metadata = instance.metadata().to_vec();
    let tile_edges = tile_edge_set(&instance);
    let pairs = pairs_of(&instance);

    // A tile edge with the wrong sign.
    let tile_edge = *tile_edges.iter().next().expect("a tile edge");
    let mut j = instance.j().to_vec();
    j[edge_position(&instance, tile_edge)] *= -1;
    assert!(
        !check(&bytes, &rebuilt(&instance, j, metadata.clone())),
        "tile J"
    );

    // A pair with a positive coupling.
    let mut j = instance.j().to_vec();
    j[edge_position(&instance, pairs[0])] = -PAIR_MILLI;
    assert!(
        !check(&bytes, &rebuilt(&instance, j, metadata.clone())),
        "pair J"
    );

    // A weak edge with the wrong magnitude.
    let weak_at = instance
        .edges()
        .iter()
        .position(|e| !tile_edges.contains(e) && !pairs.contains(e))
        .expect("a weak edge");
    let mut j = instance.j().to_vec();
    j[weak_at] = if j[weak_at] > 0 { 63 } else { -63 };
    assert!(
        !check(&bytes, &rebuilt(&instance, j, metadata.clone())),
        "weak J"
    );

    // A pair that is a tile edge: one pair, the tile edge listed as the pair.
    let topology = lattice_topology(2, 2);
    let one_bytes = params(2, 2, 62, 1);
    let one = build(&one_bytes, &topology, nonce_of(10));
    let [tiles, gauges, _] = split(&one);
    let mut on_tile = 1u32.to_le_bytes().to_vec();
    on_tile.extend_from_slice(&tile_edge.0.to_le_bytes());
    on_tile.extend_from_slice(&tile_edge.1.to_le_bytes());
    let bad = rebuilt(&one, one.j().to_vec(), join(&[tiles, gauges, on_tile]));
    assert!(!check(&one_bytes, &bad), "pair on a tile edge");

    // Two pairs that share a node, both with the pair coupling.
    let touching = instance
        .edges()
        .iter()
        .copied()
        .find(|&(a, b)| {
            !tile_edges.contains(&(a, b))
                && !pairs.contains(&(a, b))
                && [a, b].iter().any(|n| [pairs[0].0, pairs[0].1].contains(n))
        })
        .expect("an edge touching the first pair");
    let mut j = instance.j().to_vec();
    j[edge_position(&instance, pairs[1])] = 62;
    j[edge_position(&instance, touching)] = PAIR_MILLI;
    let mut two = vec![pairs[0], touching];
    two.sort_unstable();
    let mut payload = 2u32.to_le_bytes().to_vec();
    for (a, b) in &two {
        payload.extend_from_slice(&a.to_le_bytes());
        payload.extend_from_slice(&b.to_le_bytes());
    }
    let two_bytes = params(2, 2, 62, 2);
    let [tiles, gauges, _] = split(&instance);
    let overlap = rebuilt(&instance, j, join(&[tiles, gauges, payload]));
    assert!(!check(&two_bytes, &overlap), "overlapping pairs");

    // A pair count that differs from the parameter.
    assert!(!check(&params(2, 2, 62, 9), &instance), "pair count");

    // A gauge byte above one.
    let [tiles, mut gauges, pair_payload] = split(&instance);
    gauges[4] = 2;
    let bad = rebuilt(
        &instance,
        instance.j().to_vec(),
        join(&[tiles, gauges, pair_payload]),
    );
    assert!(!check(&bytes, &bad), "gauge byte 2");

    // One tile fewer in the gauge section.
    let [tiles, mut gauges, pair_payload] = split(&instance);
    let count = u32::from_le_bytes(gauges[..4].try_into().expect("four bytes")) - 1;
    gauges.truncate(gauges.len() - 4);
    gauges[..4].copy_from_slice(&count.to_le_bytes());
    let bad = rebuilt(
        &instance,
        instance.j().to_vec(),
        join(&[tiles, gauges, pair_payload]),
    );
    assert!(!check(&bytes, &bad), "gauge count");

    // A nonzero field.
    let mut h = instance.h().to_vec();
    h[0] = 1;
    let bad = rebuilt_with(&instance, h, instance.j().to_vec(), metadata);
    assert!(!check(&bytes, &bad), "nonzero h");
}

#[test]
fn oversized_counts_are_rejected_without_allocation() {
    let (bytes, instance) = valid_z22(11);
    for which in [1usize, 2] {
        let mut payloads = split(&instance);
        payloads[which][..4].copy_from_slice(&u32::MAX.to_le_bytes());
        let bad = rebuilt(&instance, instance.j().to_vec(), join(&payloads));
        assert!(!check(&bytes, &bad), "kind {} count", which + 1);
    }
}

#[test]
fn recipes_have_distinct_params_digests() {
    let expected: [(&str, &str, u16, u32); 3] = [
        ("x-w62-p1200", "ice-twisted-w62-dimer1200", 62, 1200),
        ("x-w62-p1350", "ice-twisted-w62-dimer1350", 62, 1350),
        ("x-w45-p1200", "ice-twisted-w45-dimer1200", 45, 1200),
    ];
    let mut digests = BTreeSet::new();
    for (recipe, (alias, cell, weak, pairs)) in RECIPES.iter().zip(expected) {
        assert_eq!(recipe.alias, alias);
        assert_eq!(recipe.cell, cell);
        let digest = params_digest(recipe.params);
        let admitted = admit(
            CONTRACTS,
            b"quip/twisted-ice-dimer",
            1,
            recipe.params,
            &digest,
        )
        .expect("a recipe is admitted");
        let get = |tag| admitted.params().get(tag);
        assert_eq!(get(1), Some(Value::Unsigned(12)));
        assert_eq!(get(2), Some(Value::Unsigned(4)));
        assert_eq!(get(3), Some(Value::Unsigned(u64::from(weak))));
        assert_eq!(get(4), Some(Value::Unsigned(u64::from(pairs))));
        assert_eq!(recipe.params, params(12, 4, weak, pairs));
        assert!(digests.insert(digest), "digest repeated");
    }
    assert_eq!(digests.len(), 3);
}

#[test]
fn live_recipes_reach_their_pair_counts() {
    let topology = common::live_topology();
    let n = topology.node_ids().len();
    for recipe in &RECIPES {
        let want = {
            let admitted = admitted(recipe.params).expect("admit");
            match admitted.params().get(4) {
                Some(Value::Unsigned(pairs)) => usize::try_from(pairs).expect("fits"),
                other => panic!("pairs parameter: {other:?}"),
            }
        };
        for i in 0..0x20u8 {
            let instance = build(recipe.params, &topology, nonce_of(i));
            let (tiles, gauges) = local_gauges(&instance).expect("local gauges");
            assert_eq!(tiles.len(), 1785, "{} nonce {i}", recipe.alias);
            assert_eq!(
                pairs_of(&instance).len(),
                want,
                "{} nonce {i}",
                recipe.alias
            );
            assert!(
                check(recipe.params, &instance),
                "{} nonce {i}",
                recipe.alias
            );
            assert!(gauge_obstructions(&tiles, &gauges, n).is_ok());
        }
    }
}
