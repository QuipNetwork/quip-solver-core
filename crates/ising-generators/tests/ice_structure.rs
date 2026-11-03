//! Ice tiles: topology, candidates, matching, and fixtures.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    clippy::panic,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;

use ising_generators::family::BaseTopology;
use ising_generators::ice::{
    candidates, select, sides, zephyr_coordinates, zephyr_coupler, zephyr_lattice, Candidates,
    Lattice, Sides, StructureError, Tile, MAX_DEGREE,
};
use ising_generators::rng::{shuffle, ChaCha8Rng};
use serde_json::Value;

mod common;
use common::{array, field, number, pairs, text, u32s};

fn topology(nodes: Vec<u32>, edges: &[(u32, u32)]) -> BaseTopology {
    BaseTopology::new(nodes, edges).expect("test topology is valid")
}

fn lattice_topology(m: u16, t: u8) -> BaseTopology {
    let (nodes, edges) = zephyr_lattice(m, t).expect("lattice");
    topology(nodes, &edges)
}

fn adjacency_graph(edges: &[(u32, u32)]) -> BaseTopology {
    let n = edges
        .iter()
        .map(|&(a, b)| a.max(b))
        .max()
        .map_or(0, |x| x + 1);
    topology((0..n).collect(), edges)
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

/// A ring of `len` spine edges, consecutive pairs fully joined: one tile per
/// consecutive pair, and the spine graph is a cycle of `len`.
fn tile_ring(len: u32) -> Vec<(u32, u32)> {
    let tiles: Vec<_> = (0..len)
        .map(|i| {
            let next = (i + 1) % len;
            clique([2 * i, 2 * i + 1, 2 * next, 2 * next + 1])
        })
        .collect();
    union(&tiles)
}

#[test]
fn zephyr_lattice_matches_the_edge_count_formula() {
    for m in 1u16..=3 {
        for t in 1u8..=4 {
            let (nodes, edges) = zephyr_lattice(m, t).expect("lattice");
            let (mm, tt) = (usize::from(m), usize::from(t));
            assert_eq!(nodes.len(), 4 * tt * mm * (2 * mm + 1));
            let expected = if m == 1 {
                2 * tt * (8 * tt + 3)
            } else {
                2 * tt * ((8 * tt + 8) * mm * mm - 2 * mm - 3)
            };
            assert_eq!(edges.len(), expected, "z({m},{t})");
            assert!(nodes.windows(2).all(|w| w[0] < w[1]));
            assert!(edges.iter().all(|&(a, b)| a < b));
            assert!(edges.windows(2).all(|w| w[0] < w[1]));
        }
    }
}

#[test]
fn zephyr_coupler_accepts_exactly_the_lattice_edges() {
    let (nodes, edges) = zephyr_lattice(2, 2).expect("lattice");
    let set: BTreeSet<_> = edges.iter().copied().collect();
    for &a in &nodes {
        for &b in &nodes {
            let want = set.contains(&(a.min(b), a.max(b)));
            assert_eq!(zephyr_coupler(2, 2, a, b), want, "({a},{b})");
        }
    }
    assert!(!zephyr_coupler(2, 2, 0, 4 * 2 * 2 * 5));
}

#[test]
fn zephyr_coordinates_round_trip() {
    let (m, t) = (2u32, 3u32);
    let count = 4 * t * m * (2 * m + 1);
    for q in 0..count {
        let [u, w, k, j, z] = zephyr_coordinates(2, 3, q).expect("in range");
        assert_eq!(((((u * (2 * m + 1) + w) * t + k) * 2 + j) * m) + z, q);
        assert!(u < 2 && w < 2 * m + 1 && k < t && j < 2 && z < m);
    }
    assert_eq!(zephyr_coordinates(2, 3, count), None);
}

#[test]
fn both_modes_find_the_same_tiles_on_a_lattice() {
    for (m, t) in [(1u16, 2u8), (2, 1), (2, 2)] {
        let base = lattice_topology(m, t);
        let adjacency = candidates(Lattice::Adjacency, &base).expect("adjacency");
        let zephyr = candidates(Lattice::Zephyr { m, t }, &base).expect("zephyr");
        assert_eq!(adjacency.tiles, zephyr.tiles, "z({m},{t})");
        for (tile, spine) in zephyr.tiles.iter().zip(&zephyr.spines) {
            let [(a, b), (c, d)] = spine.expect("no Zephyr tile is free");
            let co = |q| zephyr_coordinates(m, t, q).expect("node");
            for ((x, y), u) in [((a, b), 0), ((c, d), 1)] {
                let (cx, cy) = (co(x), co(y));
                assert!(x < y && cx[0] == u && cy[0] == u);
                assert!(cx[1] == cy[1] && cx[2] == cy[2] && cx[3] != cy[3]);
                assert!(zephyr_coupler(m, t, x, y));
            }
            for p in [a, b] {
                for q in [c, d] {
                    assert!(zephyr_coupler(m, t, p, q));
                }
            }
            let mut nodes = [a, b, c, d];
            nodes.sort_unstable();
            assert_eq!(nodes, tile.nodes);
        }
    }
}

#[test]
fn missing_edge_removes_its_candidates() {
    let (nodes, edges) = zephyr_lattice(2, 2).expect("lattice");
    let full = lattice_topology(2, 2);
    let base = candidates(Lattice::Zephyr { m: 2, t: 2 }, &full).expect("full");
    let tile = base.tiles[0];
    let [(a, _), (c, _)] = base.spines[0].expect("spine");
    let removed = (a.min(c), a.max(c));
    assert!(edges.contains(&removed), "an internal edge of the tile");
    let kept: Vec<_> = edges.iter().copied().filter(|e| *e != removed).collect();
    let cut = topology(nodes, &kept);
    let expected: Vec<_> = base
        .tiles
        .iter()
        .copied()
        .filter(|t| !(t.nodes.contains(&removed.0) && t.nodes.contains(&removed.1)))
        .collect();
    assert!(expected.len() < base.tiles.len());
    assert!(!expected.contains(&tile));
    for lattice in [Lattice::Adjacency, Lattice::Zephyr { m: 2, t: 2 }] {
        let got = candidates(lattice, &cut).expect("cut");
        assert_eq!(got.tiles, expected, "{lattice:?}");
    }
}

#[test]
fn shuffled_and_flipped_edge_insertion_gives_identical_structure() {
    let (nodes, edges) = zephyr_lattice(2, 2).expect("lattice");
    let mut scrambled: Vec<_> = edges.iter().rev().copied().collect();
    for (i, edge) in scrambled.iter_mut().enumerate() {
        if i % 2 == 1 {
            *edge = (edge.1, edge.0);
        }
    }
    let plain = topology(nodes.clone(), &edges);
    let mixed = topology(nodes, &scrambled);
    for lattice in [Lattice::Adjacency, Lattice::Zephyr { m: 2, t: 2 }] {
        let one: Candidates = candidates(lattice, &plain).expect("plain");
        let two: Candidates = candidates(lattice, &mixed).expect("mixed");
        assert_eq!(one, two, "{lattice:?}");
    }
}

#[test]
fn even_tile_rings_are_accepted() {
    let base = adjacency_graph(&tile_ring(4));
    let found = candidates(Lattice::Adjacency, &base).expect("even ring");
    assert_eq!(found.tiles.len(), 4);
    assert!(found.spines.iter().all(Option::is_some));
}

#[test]
fn invalid_topologies_return_stable_errors() {
    // Three tile pairs around node 0, each pair sharing the spine edge
    // (0, p): node 0 is on three spine edges.
    let mut around = Vec::new();
    let mut next = 4;
    for p in 1..=3 {
        for _ in 0..2 {
            around.push(clique([0, p, next, next + 1]));
            next += 2;
        }
    }
    // Two shared edges (0,1) and (0,2) of one clique meet at node 0.
    let conflict = union(&[
        clique([0, 1, 2, 3]),
        clique([0, 1, 4, 5]),
        clique([0, 2, 6, 7]),
    ]);
    let star: Vec<_> = (1..=33).map(|leaf| (0, leaf)).collect();

    let table: Vec<(&str, StructureError, Lattice, BaseTopology)> = vec![
        (
            "membership-above-two",
            StructureError::MembershipAboveTwo(0),
            Lattice::Adjacency,
            adjacency_graph(&union(&around)),
        ),
        (
            "spine-conflict",
            StructureError::SpineConflict,
            Lattice::Adjacency,
            adjacency_graph(&conflict),
        ),
        (
            "not-bipartite",
            StructureError::NotBipartite,
            Lattice::Adjacency,
            adjacency_graph(&tile_ring(5)),
        ),
        (
            "degree-above-limit",
            StructureError::DegreeAboveLimit,
            Lattice::Adjacency,
            adjacency_graph(&star),
        ),
        (
            "no-candidates",
            StructureError::NoCandidates,
            Lattice::Adjacency,
            adjacency_graph(&[(0, 1), (1, 2), (2, 3), (3, 0)]),
        ),
        (
            "duplicate-edge",
            StructureError::DuplicateEdge,
            Lattice::Adjacency,
            adjacency_graph(&[(0, 1), (1, 0)]),
        ),
        (
            "not-zephyr-node",
            StructureError::NotZephyrNode(12),
            Lattice::Zephyr { m: 1, t: 1 },
            topology(vec![0, 12], &[(0, 12)]),
        ),
        (
            "not-zephyr-coupler",
            StructureError::NotZephyrCoupler(0, 3),
            Lattice::Zephyr { m: 1, t: 1 },
            topology(vec![0, 3], &[(3, 0)]),
        ),
    ];
    assert_eq!(MAX_DEGREE, 32);
    for (name, want, lattice, base) in &table {
        let got = candidates(*lattice, base).expect_err(name);
        assert_eq!(got, *want, "{name}");
        assert_eq!(got.name(), *name);
    }
    assert_eq!(Lattice::from_shape(1, 0), Err(StructureError::MixedShape));
    assert_eq!(Lattice::from_shape(0, 2), Err(StructureError::MixedShape));
    assert_eq!(StructureError::MixedShape.name(), "mixed-shape");
    assert_eq!(StructureError::BadOrder.name(), "bad-order");
    assert_eq!(StructureError::Overflow.name(), "overflow");
    assert_eq!(Lattice::from_shape(0, 0), Ok(Lattice::Adjacency));
    assert_eq!(
        Lattice::from_shape(2, 3),
        Ok(Lattice::Zephyr { m: 2, t: 3 })
    );
}

#[test]
fn the_first_failing_check_names_the_error() {
    // Nodes 0 and 1 are a coupler, the pair repeats, and (0, 3) is not a
    // coupler. The duplicate check runs first.
    assert!(zephyr_coupler(1, 1, 0, 1));
    let base = topology(vec![0, 1, 3], &[(0, 1), (1, 0), (0, 3)]);
    let got = candidates(Lattice::Zephyr { m: 1, t: 1 }, &base);
    assert_eq!(got, Err(StructureError::DuplicateEdge));

    // Node 12 is not a z(1, 1) node and (0, 3) is not a z(1, 1) coupler. The
    // node check runs before the coupler check.
    let base = topology(vec![0, 3, 12], &[(0, 3), (0, 12)]);
    let got = candidates(Lattice::Zephyr { m: 1, t: 1 }, &base);
    assert_eq!(got, Err(StructureError::NotZephyrNode(12)));
}

/// Tiles for spine-graph edges over spine indices: spine `i` is the edge
/// `(2i, 2i + 1)` and each pair is the clique of its two spines.
fn spine_graph(pairs: &[(u32, u32)]) -> BaseTopology {
    let tiles: Vec<_> = pairs
        .iter()
        .map(|&(a, b)| clique([2 * a, 2 * a + 1, 2 * b, 2 * b + 1]))
        .collect();
    adjacency_graph(&union(&tiles))
}

fn adjacency_parts(base: &BaseTopology) -> (Candidates, Sides) {
    let found = candidates(Lattice::Adjacency, base).expect("candidates");
    let split = sides(Lattice::Adjacency, base, &found).expect("sides");
    (found, split)
}

fn permutations(len: u32) -> Vec<Vec<u32>> {
    if len == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for rest in permutations(len - 1) {
        for at in 0..=rest.len() {
            let mut one = rest.clone();
            one.insert(at, len - 1);
            out.push(one);
        }
    }
    out
}

fn identity(len: usize) -> Vec<u32> {
    (0..u32::try_from(len).expect("small")).collect()
}

/// The size of a maximum set of spine tiles with pairwise distinct spine
/// edges, by trying every subset.
fn brute_force_maximum(found: &Candidates) -> usize {
    let spine_tiles: Vec<[(u32, u32); 2]> = found.spines.iter().flatten().copied().collect();
    assert!(spine_tiles.len() <= 16);
    let mut best = 0;
    for mask in 0u32..(1 << spine_tiles.len()) {
        let mut used = BTreeSet::new();
        let mut ok = true;
        for (i, pair) in spine_tiles.iter().enumerate() {
            if mask >> i & 1 == 1 {
                ok &= used.insert(pair[0]) && used.insert(pair[1]);
            }
        }
        if ok {
            best = best.max(mask.count_ones() as usize);
        }
    }
    best
}

#[test]
fn tiny_exhaustive_matching_is_maximum() {
    let fixtures: [&[(u32, u32)]; 3] = [
        &[(0, 1), (0, 3), (2, 1), (2, 3)],
        &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5)],
        &[
            (0, 1),
            (0, 3),
            (0, 5),
            (2, 1),
            (2, 5),
            (4, 3),
            (4, 5),
            (4, 7),
            (6, 7),
        ],
    ];
    for pairs in fixtures {
        let base = spine_graph(pairs);
        let (found, split) = adjacency_parts(&base);
        assert!(split.left.len() <= 4 && split.right.len() <= 4, "{pairs:?}");
        let maximum = brute_force_maximum(&found);
        let spine_count = found.spines.iter().flatten().count();
        assert_eq!(spine_count, pairs.len());
        let lefts = permutations(u32::try_from(split.left.len()).expect("small"));
        let rights = permutations(u32::try_from(split.right.len()).expect("small"));
        for left in &lefts {
            for right in &rights {
                let chosen = select(&found, &split, left, right).expect("select");
                assert_eq!(chosen.len(), maximum, "{pairs:?} {left:?} {right:?}");
                let mut used = BTreeSet::new();
                for tile in &chosen {
                    let at = found.tiles.binary_search(tile).expect("a candidate");
                    let [e, f] = found.spines[at].expect("spine tile");
                    assert!(used.insert(e) && used.insert(f), "edge reused");
                }
                for node in 0..=found.tiles.iter().flat_map(|t| t.nodes).max().unwrap_or(0) {
                    let count = chosen.iter().filter(|t| t.nodes.contains(&node)).count();
                    assert!(count <= 2, "node {node} in {count} slots");
                }
                assert!(chosen.windows(2).all(|w| w[0] < w[1]));
            }
        }
    }
}

#[test]
fn matching_ties_follow_the_orders() {
    let base = adjacency_graph(&tile_ring(4));
    let (found, split) = adjacency_parts(&base);
    assert_eq!(split.left, [(0, 1), (4, 5)]);
    assert_eq!(split.right, [(2, 3), (6, 7)]);
    let tile = |nodes| Tile { nodes };
    let identity_orders = select(&found, &split, &[0, 1], &[0, 1]).expect("identity");
    assert_eq!(identity_orders, [tile([0, 1, 6, 7]), tile([2, 3, 4, 5])]);
    let reversed_left = select(&found, &split, &[1, 0], &[0, 1]).expect("reversed");
    assert_eq!(reversed_left, [tile([0, 1, 2, 3]), tile([4, 5, 6, 7])]);
}

#[test]
fn free_tiles_are_always_selected() {
    let free = clique([20, 21, 22, 23]);
    let base = adjacency_graph(&union(&[tile_ring(4), free]));
    let (found, split) = adjacency_parts(&base);
    let free_tile = Tile {
        nodes: [20, 21, 22, 23],
    };
    let at = found.tiles.binary_search(&free_tile).expect("free tile");
    assert_eq!(found.spines[at], None);
    for left in permutations(2) {
        for right in permutations(2) {
            let chosen = select(&found, &split, &left, &right).expect("select");
            assert!(chosen.contains(&free_tile));
            assert_eq!(chosen.len(), 3);
        }
    }
}

#[test]
fn a_bad_order_is_rejected() {
    let base = adjacency_graph(&tile_ring(4));
    let (found, split) = adjacency_parts(&base);
    let bad = Err(StructureError::BadOrder);
    assert_eq!(select(&found, &split, &[0], &[0, 1]), bad);
    assert_eq!(select(&found, &split, &[0, 1, 1], &[0, 1]), bad);
    assert_eq!(select(&found, &split, &[0, 0], &[0, 1]), bad);
    assert_eq!(select(&found, &split, &[0, 2], &[0, 1]), bad);
    assert_eq!(select(&found, &split, &[0, 1], &[1, 1]), bad);
    assert_eq!(select(&found, &split, &[0, 1], &[]), bad);
}

#[test]
fn zephyr_sides_split_vertical_and_horizontal() {
    let base = lattice_topology(2, 2);
    let lattice = Lattice::Zephyr { m: 2, t: 2 };
    let found = candidates(lattice, &base).expect("candidates");
    let split = sides(lattice, &base, &found).expect("sides");
    let spine_edges: BTreeSet<_> = found.spines.iter().flatten().flatten().copied().collect();
    assert_eq!(split.left.len() + split.right.len(), spine_edges.len());
    assert!(!split.left.is_empty() && !split.right.is_empty());
    assert!(split.left.windows(2).all(|w| w[0] < w[1]));
    assert!(split.right.windows(2).all(|w| w[0] < w[1]));
    let u_of = |dense: u32| {
        let id = base.node_ids()[dense as usize];
        zephyr_coordinates(2, 2, id).expect("node")[0]
    };
    assert!(split
        .left
        .iter()
        .all(|&(a, b)| u_of(a) == 0 && u_of(b) == 0));
    assert!(split
        .right
        .iter()
        .all(|&(a, b)| u_of(a) == 1 && u_of(b) == 1));
    for [e, f] in found.spines.iter().flatten() {
        assert_ne!(split.left.contains(e), split.left.contains(f));
    }
    let chosen = select(
        &found,
        &split,
        &identity(split.left.len()),
        &identity(split.right.len()),
    );
    assert!(chosen.expect("select").len() <= split.left.len().min(split.right.len()));
}

// ---- fixtures: the live topology, archived tiles, and structure vectors ----

/// The orders of a selection: the identity of each side shuffled from one
/// stream, left first.
fn seeded_orders(seed: &str, left: usize, right: usize) -> (Vec<u32>, Vec<u32>) {
    let mut rng = ChaCha8Rng::from_seed(common::seed(seed));
    let mut left_order = identity(left);
    shuffle(&mut rng, &mut left_order).expect("shuffle left");
    let mut right_order = identity(right);
    shuffle(&mut rng, &mut right_order).expect("shuffle right");
    (left_order, right_order)
}

fn live_lattice(mode: &str) -> Lattice {
    match mode {
        "adjacency" => Lattice::Adjacency,
        "zephyr" => Lattice::Zephyr { m: 12, t: 4 },
        other => panic!("unknown mode {other}"),
    }
}

/// The tiles as `u32` little-endian words, four per tile.
fn tile_bytes(tiles: &[Tile]) -> Vec<u8> {
    tiles
        .iter()
        .flat_map(|tile| tile.nodes)
        .flat_map(u32::to_le_bytes)
        .collect()
}

fn tiles_of(value: &Value) -> Vec<Tile> {
    value
        .as_array()
        .expect("tiles")
        .iter()
        .map(|tile| {
            let nodes: [u32; 4] = u32s(tile).try_into().expect("four nodes");
            Tile { nodes }
        })
        .collect()
}

#[test]
fn live_topology_rebuilds_with_the_pinned_digest() {
    let fixture = common::parse(common::LIVE_JSON);
    let base = common::live_topology();
    assert_eq!(base.node_ids().len() as u64, number(&fixture, "nodes"));
    assert_eq!(base.edges().len() as u64, number(&fixture, "edges"));
    assert_eq!(base.node_ids().len(), 4575);
    assert_eq!(base.edges().len(), 41474);
    assert_eq!(
        common::hex(base.order_digest()),
        text(&fixture, "order_digest")
    );
    assert!(base.node_ids().windows(2).all(|w| w[0] < w[1]));
    assert_eq!(text(&fixture, "provenance"), "archived-research");
}

#[test]
fn live_candidates_match_the_research_counts() {
    let base = common::live_topology();
    let adjacency = candidates(Lattice::Adjacency, &base).expect("adjacency");
    let zephyr = candidates(live_lattice("zephyr"), &base).expect("zephyr");
    assert_eq!(adjacency.tiles.len(), 6822);
    assert_eq!(zephyr.tiles.len(), 6822);
    assert_eq!(adjacency.tiles, zephyr.tiles);
}

#[test]
fn live_selection_has_1785_tiles() {
    let base = common::live_topology();
    let doc = common::parse(common::STRUCTURE_JSON);
    for case in array(&doc, "live") {
        let lattice = live_lattice(text(case, "name"));
        let found = candidates(lattice, &base).expect("candidates");
        let split = sides(lattice, &base, &found).expect("sides");
        assert_eq!(found.tiles.len() as u64, number(case, "candidates"));
        assert_eq!(
            found.spines.iter().filter(|s| s.is_none()).count() as u64,
            number(case, "free")
        );
        assert_eq!(split.left.len() as u64, number(case, "left"));
        assert_eq!(split.right.len() as u64, number(case, "right"));
        assert_eq!(array(case, "selections").len(), 2);
        for selection in array(case, "selections") {
            let (left, right) =
                seeded_orders(text(selection, "seed"), split.left.len(), split.right.len());
            let chosen = select(&found, &split, &left, &right).expect("select");
            assert_eq!(chosen.len(), 1785);
            assert_eq!(chosen.len() as u64, number(selection, "selected"));
            let digest = blake3::hash(&tile_bytes(&chosen));
            assert_eq!(digest.to_hex().as_str(), text(selection, "tiles_blake3"));
        }
    }
}

#[test]
fn archived_tiles_are_valid_candidates() {
    let sidecar = common::parse(common::ARCHIVED_JSON);
    assert_eq!(text(&sidecar, "provenance"), "archived-research");
    assert_eq!(
        blake3::hash(common::ARCHIVED_BIN).to_hex().as_str(),
        text(&sidecar, "bin_blake3")
    );
    let base = common::live_topology();
    let found = candidates(live_lattice("zephyr"), &base).expect("candidates");
    let models = common::archived_models();
    assert_eq!(models.len(), 2);
    let total: u64 = array(&sidecar, "models")
        .iter()
        .map(|m| number(m, "length_bytes"))
        .sum();
    assert_eq!(total, common::ARCHIVED_BIN.len() as u64);
    for model in &models {
        assert_eq!(model.tiles.len(), 1785, "{}", model.nonce);
        let mut used = BTreeSet::new();
        for nodes in &model.tiles {
            assert!(nodes.windows(2).all(|w| w[0] < w[1]), "ascending");
            let tile = Tile { nodes: *nodes };
            assert!(found.tiles.binary_search(&tile).is_ok(), "{nodes:?}");
            for edge in clique(*nodes) {
                assert!(used.insert(edge), "edge {edge:?} used twice");
            }
        }
    }
}

/// The native ids carried by an error variant.
fn error_detail(error: StructureError) -> Vec<u32> {
    match error {
        StructureError::NotZephyrNode(id) | StructureError::MembershipAboveTwo(id) => vec![id],
        StructureError::NotZephyrCoupler(a, b) => vec![a, b],
        _ => Vec::new(),
    }
}

/// Check one structure case against the library.
fn check_structure_case(case: &Value) {
    let name = text(case, "name");
    let lattice = field(case, "lattice");
    let shape = Lattice::from_shape(
        u16::try_from(number(lattice, "m")).expect("m"),
        u8::try_from(number(lattice, "t")).expect("t"),
    );
    let topology_json = field(case, "topology");
    let base = topology(
        u32s(field(topology_json, "nodes")),
        &pairs(field(topology_json, "edges")),
    );
    let result = shape.and_then(|lattice| {
        let found = candidates(lattice, &base)?;
        let split = sides(lattice, &base, &found)?;
        Ok((lattice, found, split))
    });
    if let Some(error) = case.get("error") {
        let got = result.expect_err(name);
        assert_eq!(got.name(), error.as_str().expect("error name"), "{name}");
        assert_eq!(
            error_detail(got),
            u32s(field(case, "error_detail")),
            "{name}: error detail"
        );
        return;
    }
    let (_, found, split) = result.unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert_eq!(found.tiles, tiles_of(field(case, "tiles")), "{name}: tiles");
    let spines: Vec<Option<[(u32, u32); 2]>> = array(case, "spines")
        .iter()
        .map(|spine| {
            (!spine.is_null()).then(|| {
                let two = pairs(spine);
                [two[0], two[1]]
            })
        })
        .collect();
    assert_eq!(found.spines, spines, "{name}: spines");
    assert_eq!(split.left, pairs(field(case, "left")), "{name}: left");
    assert_eq!(split.right, pairs(field(case, "right")), "{name}: right");
    let selections = array(case, "selections");
    assert_eq!(selections.len(), 2, "{name}: two seeds");
    for selection in selections {
        let seed = text(selection, "seed");
        let (left, right) = seeded_orders(seed, split.left.len(), split.right.len());
        assert_eq!(
            left,
            u32s(field(selection, "left_order")),
            "{name}: left order"
        );
        assert_eq!(
            right,
            u32s(field(selection, "right_order")),
            "{name}: right order"
        );
        let chosen = select(&found, &split, &left, &right).expect("select");
        assert_eq!(
            chosen,
            tiles_of(field(selection, "tiles")),
            "{name}: {seed}"
        );
    }
}

/// Forty disjoint complete graphs on 33 nodes (degree 32) hold 21120 edges and
/// far more than 5280 four-cliques, so the tile count is above `edges / 4` and
/// the search stops with a spine conflict.
#[test]
fn dense_cliques_fail_fast_with_spine_conflict() {
    let mut edges = Vec::new();
    for copy in 0..40u32 {
        let base = 33 * copy;
        for a in 0..33 {
            for b in (a + 1)..33 {
                edges.push((base + a, base + b));
            }
        }
    }
    let base = adjacency_graph(&edges);
    assert_eq!(
        candidates(Lattice::Adjacency, &base).unwrap_err(),
        StructureError::SpineConflict
    );
}

#[test]
fn structure_vectors_match() {
    let doc = common::parse(common::STRUCTURE_JSON);
    let cases = array(&doc, "structure");
    assert!(cases.len() >= 18, "every planned case is present");
    let errors: BTreeSet<&str> = cases
        .iter()
        .filter_map(|c| c.get("error").and_then(Value::as_str))
        .collect();
    for name in [
        "mixed-shape",
        "degree-above-limit",
        "duplicate-edge",
        "not-zephyr-node",
        "not-zephyr-coupler",
        "no-candidates",
        "spine-conflict",
        "membership-above-two",
        "not-bipartite",
    ] {
        assert!(errors.contains(name), "missing error case {name}");
    }
    for case in cases {
        check_structure_case(case);
    }
}

#[test]
fn repeated_structure_is_identical() {
    let base = common::live_topology();
    for mode in ["adjacency", "zephyr"] {
        let lattice = live_lattice(mode);
        let run = || {
            let found = candidates(lattice, &base).expect("candidates");
            let split = sides(lattice, &base, &found).expect("sides");
            let (left, right) =
                seeded_orders(&common::hex(&[7; 32]), split.left.len(), split.right.len());
            let chosen = select(&found, &split, &left, &right).expect("select");
            (found, split, chosen)
        };
        assert_eq!(run(), run(), "{mode}");
    }
}
