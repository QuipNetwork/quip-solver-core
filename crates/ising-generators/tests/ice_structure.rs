//! Ice tiles: topology, candidates, matching, and fixtures.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;

use ising_generators::family::BaseTopology;
use ising_generators::ice::{
    candidates, select, sides, zephyr_coordinates, zephyr_coupler, zephyr_lattice, Candidates,
    Lattice, Sides, StructureError, Tile, MAX_DEGREE,
};

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
    let [(a, b), (c, _)] = base.spines[0].expect("spine");
    let _ = b;
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
    let z11 = |nodes: Vec<u32>, edges: &[(u32, u32)]| topology(nodes, edges);
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
            z11(vec![0, 12], &[(0, 12)]),
        ),
        (
            "not-zephyr-coupler",
            StructureError::NotZephyrCoupler(0, 3),
            Lattice::Zephyr { m: 1, t: 1 },
            z11(vec![0, 3], &[(3, 0)]),
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
                let mut slots = BTreeSet::new();
                for tile in &chosen {
                    let at = found.tiles.binary_search(tile).expect("a candidate");
                    let [e, f] = found.spines[at].expect("spine tile");
                    assert!(used.insert(e) && used.insert(f), "edge reused");
                    let _ = slots.insert(*tile);
                }
                assert_eq!(slots.len(), chosen.len());
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
