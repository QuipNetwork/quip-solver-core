//! The `quip/ice-fields@1` construction: the globally gauged glass base, the
//! sparse field, admission codes, and the metadata check.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use ising_generators::descriptor::{admit, params_digest, Admitted, AdvertisedLimits, SizeBounds};
use ising_generators::family::{
    admit_topology, generate_instance, BaseTopology, SaltFault, Streams,
};
use ising_generators::ice::{
    zephyr_lattice, ICE_FIELDS, ICE_FIELDS_RECIPES, ICE_FIELDS_V1, STRONG_MILLI,
};
use ising_generators::instance::{Instance, InstanceParts};
use ising_generators::rng::{shuffle, uniform};
use ising_generators::Code;

#[expect(dead_code, reason = "this test uses only the live topology loader")]
mod common;

const CONTRACTS: &[ising_generators::descriptor::Contract] = &[ICE_FIELDS_V1];

/// Canonical parameter bytes: format, then `(tag, len, payload)` per field.
fn params(m: u16, t: u8, glass: u16, field: u16, divisor: u16) -> Vec<u8> {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(&[1, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&m.to_le_bytes());
    bytes.extend_from_slice(&[2, 0, 1, 0, 0, 0, t]);
    for (tag, value) in [(3u8, glass), (4, field), (5, divisor)] {
        bytes.extend_from_slice(&[tag, 0, 2, 0, 0, 0]);
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn admitted(bytes: &[u8]) -> Result<Admitted<'_>, Code> {
    admit(
        CONTRACTS,
        b"quip/ice-fields",
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

fn small_topology(nodes: Vec<u32>, edges: &[(u32, u32)]) -> BaseTopology {
    BaseTopology::new(nodes, edges).expect("test topology is valid")
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

/// The tile membership count of every node.
fn memberships(tiles: &[[u32; 4]], n: usize) -> Vec<u8> {
    let mut seen = vec![0u8; n];
    for tile in tiles {
        for &pos in tile {
            seen[pos as usize] += 1;
        }
    }
    seen
}

/// A copy of `instance` with new fields and metadata.
fn rebuilt(instance: &Instance, h: Vec<i32>, metadata: Vec<u8>) -> Instance {
    let parts = InstanceParts {
        descriptor_digest: *instance.descriptor_digest(),
        topology_order_digest: *instance.topology_order_digest(),
        nonce: instance.nonce().copied(),
        node_ids: instance.node_ids().to_vec(),
        edges: instance.edges().to_vec(),
        h,
        j: instance.j().to_vec(),
        offset_milli: instance.offset_milli(),
        metadata,
    };
    Instance::new(
        parts,
        &ICE_FIELDS_V1.limits,
        ICE_FIELDS_V1.metadata_kinds,
        None,
    )
    .expect("rebuilt instance is well formed")
}

fn check(bytes: &[u8], instance: &Instance) -> bool {
    let admitted = admitted(bytes).expect("admit");
    let construction = ICE_FIELDS_V1.construction.expect("construction");
    (construction.check_metadata)(admitted.params(), instance)
}

/// Run the generator directly with a fixed descriptor digest, so that two
/// parameter sets share every stream.
fn raw(bytes: &[u8], topology: &BaseTopology, nonce: [u8; 32]) -> Result<Instance, Code> {
    let admitted = admitted(bytes).expect("admit");
    let mut streams = Streams::new(nonce, [7; 32], *topology.order_digest(), ICE_FIELDS.labels);
    let body = (ICE_FIELDS.generate)(admitted.params(), topology, &mut streams)?;
    let parts = InstanceParts {
        descriptor_digest: [7; 32],
        topology_order_digest: *topology.order_digest(),
        nonce: Some(nonce),
        node_ids: body.node_ids,
        edges: body.edges,
        h: body.h,
        j: body.j,
        offset_milli: body.offset_milli,
        metadata: body.metadata,
    };
    Ok(Instance::new(
        parts,
        &ICE_FIELDS_V1.limits,
        ICE_FIELDS_V1.metadata_kinds,
        None,
    )
    .expect("well formed"))
}

#[test]
fn the_contract_is_pinned() {
    let limits = ICE_FIELDS_V1.limits;
    assert_eq!(limits.max_nodes(), 131_072);
    assert_eq!(limits.max_edges(), 2_097_152);
    assert_eq!(limits.max_coefficient(), 1_000);
    assert_eq!(limits.max_offset(), 0);
    assert_eq!(limits.max_metadata(), 8_388_608);
    assert_eq!(ICE_FIELDS_V1.family_id, "quip/ice-fields");
    assert_eq!(ICE_FIELDS_V1.revision, 1);
    assert_eq!(ICE_FIELDS_V1.metadata_kinds, &[1, 2]);
    assert_eq!(
        ICE_FIELDS.labels,
        &["left", "right", "glass", "gauge", "field", "field_sign"]
    );
}

#[test]
fn field_streams_do_not_change_the_glass() {
    let topology = lattice_topology(2, 2);
    let base = raw(&params(2, 2, 125, 500, 16), &topology, nonce_of(1)).expect("base");
    for (field, divisor) in [(1000, 16), (500, 40), (1000, 40)] {
        let other = raw(&params(2, 2, 125, field, divisor), &topology, nonce_of(1)).expect("other");
        assert_eq!(base.j(), other.j(), "field {field} divisor {divisor}");
        assert_eq!(decode(&base).1, decode(&other).1);
        assert_eq!(base.metadata(), other.metadata());
    }
}

#[test]
fn tile_edges_follow_the_global_gauge() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125, 500, 16);
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
fn fields_sit_on_shared_spins_only() {
    let topology = lattice_topology(2, 2);
    let n = topology.node_ids().len();
    for (m, t) in [(2, 2), (0, 0)] {
        let bytes = params(m, t, 125, 500, 16);
        for i in 0..4 {
            let instance = build(&bytes, &topology, nonce_of(i));
            let (tiles, _) = decode(&instance);
            let seen = memberships(&tiles, n);
            let fields: Vec<usize> = (0..n).filter(|&k| instance.h()[k] != 0).collect();
            assert_eq!(fields.len(), n / 16, "mode ({m}, {t}) nonce {i}");
            for k in fields {
                assert_eq!(seen[k], 2, "field node {k} is in two tiles");
                assert_eq!(instance.h()[k].abs(), 500);
            }
            assert_eq!(instance.offset_milli(), 0);
            assert!(check(&bytes, &instance), "mode ({m}, {t}) nonce {i}");
        }
    }
}

#[test]
fn field_values_are_drawn_sign_times_gauge() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125, 500, 16);
    let nonce = nonce_of(5);
    let instance = build(&bytes, &topology, nonce);
    let (tiles, gauge) = decode(&instance);
    let n = topology.node_ids().len();
    let seen = memberships(&tiles, n);
    let mut eligible: Vec<u32> = (0..u32::try_from(n).expect("fits"))
        .filter(|&k| seen[k as usize] == 2)
        .collect();
    let mut streams = Streams::new(
        nonce,
        *instance.descriptor_digest(),
        *topology.order_digest(),
        ICE_FIELDS.labels,
    );
    let mut field = streams.open("field").expect("field stream");
    shuffle(&mut field, &mut eligible).expect("shuffle");
    let mut chosen: Vec<u32> = eligible.into_iter().take(n / 16).collect();
    chosen.sort_unstable();
    let mut sign = streams.open("field_sign").expect("sign stream");
    let two = NonZeroU32::new(2).expect("two");
    let mut expected = vec![0i32; n];
    for k in chosen {
        let drawn: i32 = if uniform(&mut sign, two) == 0 { 1 } else { -1 };
        expected[k as usize] = drawn * 500 * i32::try_from(gauge[k as usize]).expect("sign");
    }
    assert_eq!(instance.h(), expected.as_slice());
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

/// Three K4s on nine nodes. Tiles `{0,1,2,3}` and `{2,3,4,5}` share the edge
/// (2, 3), and a selection keeps only one of them. Tile `{0,6,7,8}` is free.
/// Node 0 is the only node with two slots, and it is in two selected tiles
/// only when `{0,1,2,3}` is the tile kept.
fn competing_tiles() -> BaseTopology {
    let mut edges: Vec<(u32, u32)> = [[0, 1, 2, 3], [2, 3, 4, 5], [0, 6, 7, 8]]
        .into_iter()
        .flat_map(clique)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    small_topology((0..9).collect(), &edges)
}

#[test]
fn a_shortfall_gives_no_instance() {
    let topology = competing_tiles();
    let bytes = params(0, 0, 125, 500, 9);
    let outcomes: Vec<Result<Instance, SaltFault>> = (0..16u8)
        .map(|i| try_build(&bytes, &topology, nonce_of(i)))
        .collect();
    let built = outcomes.iter().filter_map(|o| o.as_ref().ok()).count();
    let skipped = outcomes
        .iter()
        .filter(|o| matches!(o, Err(SaltFault::Skip(Code::NoInstance))))
        .count();
    assert!(built > 0, "some nonce keeps tile {{0,1,2,3}}");
    assert!(skipped > 0, "some nonce keeps tile {{2,3,4,5}}");
    assert_eq!(built + skipped, 16, "no other outcome");
    for instance in outcomes.iter().filter_map(|o| o.as_ref().ok()) {
        assert_eq!(instance.h().iter().filter(|&&h| h != 0).count(), 1);
        assert_ne!(instance.h()[0], 0, "the field is on node 0");
        assert!(check(&bytes, instance));
    }
}

#[test]
fn admission_codes() {
    let topology = lattice_topology(2, 2);
    assert_eq!(
        bounds(
            &admitted(&params(2, 0, 125, 500, 16)).expect("admit"),
            &topology
        ),
        Err(Code::TopologyIncompatible),
        "mixed shape"
    );
    let competing = competing_tiles();
    let admit_competing = |divisor| {
        bounds(
            &admitted(&params(0, 0, 125, 500, divisor)).expect("admit"),
            &competing,
        )
    };
    assert!(
        admit_competing(9).is_ok(),
        "count 1 equals the two-slot nodes"
    );
    assert_eq!(
        admit_competing(4),
        Err(Code::TopologyIncompatible),
        "count 2"
    );
    assert_eq!(
        admit_competing(1),
        Err(Code::TopologyIncompatible),
        "count 9"
    );
    assert!(admit_competing(10).is_ok(), "count 0 is valid");
    assert_eq!(
        admitted(&params(2, 2, 125, 1001, 16)).err(),
        Some(Code::ParamsOutOfBounds)
    );
    assert_eq!(
        admitted(&params(2, 2, 1000, 500, 16)).err(),
        Some(Code::ParamsOutOfBounds)
    );
    assert_eq!(
        admitted(&params(2, 2, 125, 500, 0)).err(),
        Some(Code::ParamsOutOfBounds)
    );
    let star_edges: Vec<(u32, u32)> = (1..=33).map(|leaf| (0, leaf)).collect();
    let star = small_topology((0..34).collect(), &star_edges);
    assert_eq!(
        bounds(
            &admitted(&params(0, 0, 125, 500, 16)).expect("admit"),
            &star
        ),
        Err(Code::BoundsExceeded),
        "degree 33"
    );
}

#[test]
fn a_large_divisor_gives_no_fields() {
    let topology = lattice_topology(2, 2);
    let n = topology.node_ids().len();
    assert!(n < 65_535);
    let bytes = params(2, 2, 125, 500, 65_535);
    let instance = build(&bytes, &topology, nonce_of(2));
    assert!(instance.h().iter().all(|&h| h == 0));
    assert!(check(&bytes, &instance));
}

#[test]
fn generation_is_repeatable() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125, 500, 16);
    let mut bodies = Vec::new();
    for nonce in [nonce_of(1), nonce_of(2)] {
        let first = build(&bytes, &topology, nonce);
        let again = build(&bytes, &topology, nonce);
        assert_eq!(first.canonical_bytes(), again.canonical_bytes());
        bodies.push((
            first.j().to_vec(),
            first.h().to_vec(),
            first.metadata().to_vec(),
        ));
    }
    assert_ne!(bodies[0], bodies[1], "the nonce changes the body");
}

#[test]
fn check_metadata_rejects_each_rule() {
    let topology = lattice_topology(2, 2);
    let n = topology.node_ids().len();
    let bytes = params(2, 2, 125, 500, 16);
    let instance = build(&bytes, &topology, nonce_of(6));
    assert!(check(&bytes, &instance), "positive control");
    let (tiles, _) = decode(&instance);
    let seen = memberships(&tiles, n);
    let metadata = instance.metadata().to_vec();
    let held: Vec<usize> = (0..n).filter(|&k| instance.h()[k] != 0).collect();
    let spare = (0..n)
        .find(|&k| seen[k] == 2 && instance.h()[k] == 0)
        .expect("an eligible node without a field");
    let single = (0..n).find(|&k| seen[k] == 1).expect("a node in one tile");

    let with = |edit: &dyn Fn(&mut Vec<i32>)| {
        let mut h = instance.h().to_vec();
        edit(&mut h);
        rebuilt(&instance, h, metadata.clone())
    };
    let moved = with(&|h| {
        h[single] = h[held[0]];
        h[held[0]] = 0;
    });
    assert!(!check(&bytes, &moved), "a field on a node in one tile");
    let wrong = with(&|h| h[held[0]] = h[held[0]].signum() * 501);
    assert!(!check(&bytes, &wrong), "a field of the wrong size");
    let many = with(&|h| h[spare] = 500);
    assert!(!check(&bytes, &many), "one field too many");
    let few = with(&|h| h[held[0]] = 0);
    assert!(!check(&bytes, &few), "one field too few");
    let flipped = with(&|h| h[held[0]] = -h[held[0]]);
    assert!(check(&bytes, &flipped), "the sign is not checked");
}

#[test]
fn oversized_counts_are_rejected_without_allocation() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 125, 500, 16);
    let instance = build(&bytes, &topology, nonce_of(11));
    assert!(check(&bytes, &instance));
    let region = instance.metadata().to_vec();
    let tile_len = u32::from_le_bytes(region[4..8].try_into().expect("four bytes")) as usize;
    let gauge_count_at = 8 + tile_len + 6;
    for at in [8, gauge_count_at] {
        let mut damaged = region.clone();
        damaged[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let bad = rebuilt(&instance, instance.h().to_vec(), damaged);
        assert!(!check(&bytes, &bad), "count at {at}");
    }
}

#[test]
fn recipes_are_canonical() {
    let expected = [
        ("h500", "ice-fields-h500", 500u16),
        ("h1000", "ice-fields-h1000", 1000),
    ];
    let mut digests = BTreeSet::new();
    for (recipe, (alias, cell, field)) in ICE_FIELDS_RECIPES.iter().zip(expected) {
        assert_eq!(recipe.alias, alias);
        assert_eq!(recipe.cell, cell);
        assert_eq!(recipe.params, params(12, 4, 125, field, 16));
        assert!(admitted(recipe.params).is_ok());
        assert!(digests.insert(params_digest(recipe.params)));
    }
}

#[test]
fn live_recipes_build() {
    let topology = common::live_topology();
    for recipe in &ICE_FIELDS_RECIPES {
        let instance = build(recipe.params, topology, nonce_of(9));
        let field = if recipe.alias == "h500" { 500 } else { 1000 };
        let nonzero: Vec<i32> = instance.h().iter().copied().filter(|&h| h != 0).collect();
        assert_eq!(nonzero.len(), 285, "{}", recipe.alias);
        assert!(nonzero.iter().all(|h| h.abs() == field), "{}", recipe.alias);
        assert!(check(recipe.params, &instance), "{}", recipe.alias);
    }
}
