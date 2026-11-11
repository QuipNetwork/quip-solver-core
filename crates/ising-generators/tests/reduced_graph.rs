//! Structured families on a graph that lost qubits and couplers: the tiles,
//! the pairs, and every instance edge exist on the graph the generator
//! receives, so a device that hosts that graph clamps nothing and drops
//! nothing.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;

use ising_generators::family::{generate_instance, BaseTopology};
use ising_generators::ice::{candidates, local_gauges, Lattice, RECIPES, TWISTED_DIMER_V1};
use ising_generators::instance::Instance;

#[path = "common/admit.rs"]
mod admit;
#[expect(dead_code, reason = "this test uses only the live topology loader")]
mod common;
#[path = "common/lattice.rs"]
mod lattice;
#[path = "common/nonce.rs"]
mod nonce;
#[path = "common/size_bounds.rs"]
mod size_bounds;

use admit::admitted;
use common::live_topology;
use lattice::lattice_topology;
use nonce::nonce_of;
use size_bounds::bounds;

/// An undirected native edge as `(min, max)`.
fn key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

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

fn build(what: &str, bytes: &[u8], topology: &BaseTopology, nonce: [u8; 32]) -> Instance {
    let admitted = admitted(&TWISTED_DIMER_V1, bytes).expect("admit");
    let bounds = bounds(&admitted, topology);
    assert!(bounds.is_ok(), "{what}: admit topology: {bounds:?}");
    let built = generate_instance(&admitted, topology, &bounds.expect("bounds"), nonce)
        .map_err(|fault| format!("{fault:?}"));
    assert!(built.is_ok(), "{what}: generate: {built:?}");
    built.expect("generate")
}

/// The native ids of `topology` minus `lost_nodes`, and its native edges
/// minus every edge at a lost node and minus `lost_edges`, both in the
/// original order.
fn without(
    topology: &BaseTopology,
    lost_nodes: &BTreeSet<u32>,
    lost_edges: &BTreeSet<(u32, u32)>,
) -> BaseTopology {
    let ids = topology.node_ids();
    let nodes: Vec<u32> = ids
        .iter()
        .copied()
        .filter(|id| !lost_nodes.contains(id))
        .collect();
    let edges: Vec<(u32, u32)> = topology
        .edges()
        .iter()
        .map(|&(u, v)| (ids[u as usize], ids[v as usize]))
        .filter(|&(a, b)| {
            !lost_nodes.contains(&a) && !lost_nodes.contains(&b) && !lost_edges.contains(&key(a, b))
        })
        .collect();
    BaseTopology::new(nodes, &edges).expect("reduced topology")
}

/// The tiles of an instance in native ids.
fn tiles_of(what: &str, instance: &Instance) -> Vec<[u32; 4]> {
    let (tiles, _) = local_gauges(instance).unwrap_or_else(|| panic!("{what}: local gauges"));
    let ids = instance.node_ids();
    tiles
        .into_iter()
        .map(|tile| tile.map(|pos| ids[pos as usize]))
        .collect()
}

/// The pairs of an instance in native ids, from the kind 3 section.
fn pairs_of(what: &str, instance: &Instance) -> Vec<(u32, u32)> {
    let region = instance.metadata();
    let word16 = |at: usize| u16::from_le_bytes([region[at], region[at + 1]]);
    let word32 = |at: usize| u32::from_le_bytes(region[at..at + 4].try_into().expect("four bytes"));
    assert_eq!(word16(0), 3, "{what}: three sections");
    let mut at = 2;
    let mut payload = None;
    for _ in 0..3 {
        let (kind, len) = (word16(at), word32(at + 2) as usize);
        if kind == 3 {
            payload = Some(&region[at + 6..at + 6 + len]);
        }
        at += 6 + len;
    }
    let payload = payload.unwrap_or_else(|| panic!("{what}: a pair section"));
    let count = u32::from_le_bytes(payload[..4].try_into().expect("four bytes")) as usize;
    let ids = instance.node_ids();
    (0..count)
        .map(|k| {
            let word =
                |at: usize| u32::from_le_bytes(payload[at..at + 4].try_into().expect("four bytes"));
            (ids[word(4 + 8 * k) as usize], ids[word(8 + 8 * k) as usize])
        })
        .collect()
}

/// Every tile, pair, and edge of `instance` exists on `graph`: the instance
/// nodes are the graph nodes, and the instance edges are the graph edges.
fn assert_hosted(what: &str, instance: &Instance, graph: &BaseTopology) {
    assert_eq!(instance.node_ids(), graph.node_ids(), "{what}: nodes");
    let ids = graph.node_ids();
    let hosted: BTreeSet<(u32, u32)> = graph
        .edges()
        .iter()
        .map(|&(u, v)| key(ids[u as usize], ids[v as usize]))
        .collect();
    let drawn: BTreeSet<(u32, u32)> = instance
        .edges()
        .iter()
        .map(|&(u, v)| key(ids[u as usize], ids[v as usize]))
        .collect();
    assert_eq!(drawn, hosted, "{what}: edges");
    assert_eq!(
        instance.edges().len(),
        hosted.len(),
        "{what}: no coupler is drawn twice"
    );
    let alive: BTreeSet<u32> = ids.iter().copied().collect();
    for tile in tiles_of(what, instance) {
        for (i, &a) in tile.iter().enumerate() {
            assert!(alive.contains(&a), "{what}: tile node {a} is alive");
            for &b in &tile[i + 1..] {
                assert!(hosted.contains(&key(a, b)), "{what}: tile coupler {a}-{b}");
            }
        }
    }
    for (a, b) in pairs_of(what, instance) {
        assert!(
            alive.contains(&a) && alive.contains(&b),
            "{what}: pair {a}-{b} is alive"
        );
        assert!(hosted.contains(&key(a, b)), "{what}: pair coupler {a}-{b}");
    }
}

/// The live z(12, 4) graph minus a drift of the shape seen on 2026-11-11:
/// 15 lost qubits and 2 lost couplers between live qubits, one of them a
/// tile coupler of the stale instance. The exact live drift list is not in
/// the repository, so the lost set is cut from the stale instance of the
/// first recipe: every tile that the drift breaks must disappear.
#[test]
fn twisted_dimer_recipes_build_on_the_reduced_live_graph() {
    let what = "reduced live graph";
    let live = live_topology();
    let stale = build(what, RECIPES[0].params, live, nonce_of(1));
    let stale_tiles = tiles_of(what, &stale);
    let step = stale_tiles.len() / 15;
    let lost_nodes: BTreeSet<u32> = (0..15).map(|k| stale_tiles[k * step][0]).collect();
    assert_eq!(lost_nodes.len(), 15, "fifteen distinct lost qubits");
    let alive = |tile: &[u32; 4]| tile.iter().all(|q| !lost_nodes.contains(q));
    let broken_tile = stale_tiles
        .iter()
        .find(|tile| alive(tile))
        .expect("a tile with four live qubits");
    let ids = live.node_ids();
    let weak = live
        .edges()
        .iter()
        .map(|&(u, v)| key(ids[u as usize], ids[v as usize]))
        .find(|&(a, b)| {
            !lost_nodes.contains(&a)
                && !lost_nodes.contains(&b)
                && !stale_tiles
                    .iter()
                    .any(|tile| tile.contains(&a) && tile.contains(&b))
        })
        .expect("a weak coupler between live qubits");
    let lost_edges: BTreeSet<(u32, u32)> = [key(broken_tile[0], broken_tile[1]), weak]
        .into_iter()
        .collect();
    let reduced = without(live, &lost_nodes, &lost_edges);
    assert_eq!(reduced.node_ids().len(), live.node_ids().len() - 15);
    assert!(reduced.edges().len() + 2 < live.edges().len());

    for recipe in RECIPES {
        let what = recipe.alias;
        let instance = build(what, recipe.params, &reduced, nonce_of(1));
        assert_hosted(what, &instance, &reduced);
        let tiles = tiles_of(what, &instance);
        assert!(!tiles.is_empty(), "{what}: tiles");
        assert!(
            !tiles
                .iter()
                .any(|tile| tile.contains(&broken_tile[0]) && tile.contains(&broken_tile[1])),
            "{what}: the tile with the lost coupler is gone"
        );
    }
    // The stale instance did use the lost part, so the reduction mattered.
    assert!(stale_tiles.iter().any(|tile| !alive(tile)));
    assert!(stale_tiles.contains(broken_tile));
}

/// A tile that lost one of its four qubits is not a tile: no candidate and no
/// selected tile holds the lost qubit, and the rest still builds.
#[test]
fn a_tile_with_a_lost_qubit_is_not_a_tile() {
    let full = lattice_topology(2, 2);
    let lattice = Lattice::Zephyr { m: 2, t: 2 };
    let found = candidates(lattice, &full).expect("full candidates");
    let lost = full.node_ids()[found.tiles[0].nodes[0] as usize];
    let reduced = without(&full, &[lost].into_iter().collect(), &BTreeSet::new());
    let ids = reduced.node_ids();
    assert!(!ids.contains(&lost));
    let kept = candidates(lattice, &reduced).expect("reduced candidates");
    assert!(
        kept.tiles.len() < found.tiles.len(),
        "the lost qubit took its tiles"
    );
    assert!(kept
        .tiles
        .iter()
        .all(|tile| tile.nodes.iter().all(|&pos| ids[pos as usize] != lost)));
    let instance = build("lost qubit", &params(2, 2, 62, 4), &reduced, nonce_of(2));
    assert_hosted("lost qubit", &instance, &reduced);
    assert!(tiles_of("lost qubit", &instance)
        .iter()
        .all(|tile| !tile.contains(&lost)));
}

/// A tile that lost any one of its six couplers is not a tile, whichever
/// coupler it is: an odd (spine) coupler or an internal one.
#[test]
fn a_tile_with_a_lost_coupler_is_not_a_tile() {
    let full = lattice_topology(2, 2);
    let lattice = Lattice::Zephyr { m: 2, t: 2 };
    let found = candidates(lattice, &full).expect("full candidates");
    let tile = found.tiles[0]
        .nodes
        .map(|pos| full.node_ids()[pos as usize]);
    for (i, &a) in tile.iter().enumerate() {
        for &b in &tile[i + 1..] {
            let what = format!("lost coupler {a}-{b}");
            let reduced = without(&full, &BTreeSet::new(), &[key(a, b)].into_iter().collect());
            let ids = reduced.node_ids();
            let kept = candidates(lattice, &reduced).expect("reduced candidates");
            assert!(
                kept.tiles.len() < found.tiles.len(),
                "{what}: took its tiles"
            );
            let holds = |nodes: &[u32]| nodes.contains(&a) && nodes.contains(&b);
            assert!(!kept
                .tiles
                .iter()
                .any(|tile| holds(&tile.nodes.map(|pos| ids[pos as usize]))));
            let instance = build(&what, &params(2, 2, 62, 4), &reduced, nonce_of(3));
            assert_hosted(&what, &instance, &reduced);
            assert!(!tiles_of(&what, &instance).iter().any(|tile| holds(tile)));
        }
    }
}

/// A pair needs both qubits and their coupler: without the coupler no pair
/// joins the two, and without a qubit no pair uses it.
#[test]
fn a_pair_needs_both_qubits_and_their_coupler() {
    let full = lattice_topology(2, 2);
    let bytes = params(2, 2, 62, 8);
    let instance = build("full", &bytes, &full, nonce_of(4));
    let pairs = pairs_of("full", &instance);
    assert_eq!(pairs.len(), 8);
    let (u, v) = pairs[0];

    let what = "lost pair coupler";
    let reduced = without(&full, &BTreeSet::new(), &[key(u, v)].into_iter().collect());
    let instance = build(what, &bytes, &reduced, nonce_of(4));
    assert_hosted(what, &instance, &reduced);
    let pairs = pairs_of(what, &instance);
    assert_eq!(pairs.len(), 8, "{what}: the fixed pair count holds");
    assert!(!pairs.iter().any(|&pair| key(pair.0, pair.1) == key(u, v)));

    let what = "lost pair qubit";
    let reduced = without(&full, &[u].into_iter().collect(), &BTreeSet::new());
    let instance = build(what, &bytes, &reduced, nonce_of(4));
    assert_hosted(what, &instance, &reduced);
    let pairs = pairs_of(what, &instance);
    assert_eq!(pairs.len(), 8, "{what}: the fixed pair count holds");
    assert!(!pairs.iter().any(|&(a, b)| a == u || b == u));
}
