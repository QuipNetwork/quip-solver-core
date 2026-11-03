//! Zephyr coordinates and candidate ice tiles.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec;
use alloc::vec::Vec;

use super::matching::sides;
use super::{Lattice, StructureError, MAX_DEGREE};
use crate::family::BaseTopology;

/// The node count `4 * t * m * (2m + 1)` of `z(m, t)`.
fn node_count(m: u16, t: u8) -> Option<u64> {
    let m = u64::from(m);
    let side = m.checked_mul(2)?.checked_add(1)?;
    4u64.checked_mul(u64::from(t))?
        .checked_mul(m)?
        .checked_mul(side)
}

/// Zephyr index (u, w, k, j, z) of a linear id:
/// `q = (((u*(2m+1)+w)*t+k)*2+j)*m+z`.
///
/// `None` when `m` or `t` is zero, or when `q` is not a node of `z(m, t)`.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "u, w, k, j, z are the Zephyr coordinate names"
)]
pub fn zephyr_coordinates(m: u16, t: u8, q: u32) -> Option<[u32; 5]> {
    if m == 0 || t == 0 || u64::from(q) >= node_count(m, t)? {
        return None;
    }
    let (m, t, q) = (u32::from(m), u32::from(t), u64::from(q));
    let side = u64::from(2 * m + 1);
    let z = q % u64::from(m);
    let rest = q / u64::from(m);
    let j = rest % 2;
    let rest = rest / 2;
    let k = rest % u64::from(t);
    let rest = rest / u64::from(t);
    let w = rest % side;
    let u = rest / side;
    Some([
        u32::try_from(u).ok()?,
        u32::try_from(w).ok()?,
        u32::try_from(k).ok()?,
        u32::try_from(j).ok()?,
        u32::try_from(z).ok()?,
    ])
}

/// The linear id of Zephyr coordinates, or `None` when out of range.
#[expect(
    clippy::many_single_char_names,
    reason = "u, w, k, j, z are the Zephyr coordinate names"
)]
fn zephyr_id(m: u16, t: u8, c: [i64; 5]) -> Option<u32> {
    let (m, t) = (i64::from(m), i64::from(t));
    let [u, w, k, j, z] = c;
    if !(0..2).contains(&u)
        || !(0..=m.checked_mul(2)?).contains(&w)
        || !(0..t).contains(&k)
        || !(0..2).contains(&j)
        || !(0..m).contains(&z)
    {
        return None;
    }
    let side = m.checked_mul(2)?.checked_add(1)?;
    let q = u
        .checked_mul(side)?
        .checked_add(w)?
        .checked_mul(t)?
        .checked_add(k)?
        .checked_mul(2)?
        .checked_add(j)?
        .checked_mul(m)?
        .checked_add(z)?;
    u32::try_from(q).ok()
}

/// Whether `ca ~ cb` is a coupler in this order: external, odd, or the
/// internal coupler from a `u = 0` node to a `u = 1` node.
fn directed_coupler(ca: [u32; 5], cb: [u32; 5]) -> bool {
    let [ua, wa, ka, ja, za] = ca;
    let [ub, wb, kb, jb, zb] = cb;
    if ua == ub {
        if wa != wb || ka != kb {
            return false;
        }
        if ja == jb {
            // External: z and z + 1.
            return zb == za.saturating_add(1);
        }
        // Odd: (.., 0, z) ~ (.., 1, z - a) for a in {0, 1}.
        return ja == 0 && (zb == za || zb.saturating_add(1) == za);
    }
    if ua != 0 || ub != 1 {
        return false;
    }
    // Internal: (0, 2w+1+a(2i-1), k, j, z) ~ (1, 2z+1+b(2j-1), h, i, w),
    // where `w` is `zb`, `z` is `za`, `i` is `jb` and `j` is `ja`.
    let reaches = |index: u32, centre: u32, bit: u32| -> bool {
        // index = 2*centre + 1 + s*(2*bit - 1) for s in {0, 1}.
        let index = u64::from(index);
        let Some(base) = u64::from(centre)
            .checked_mul(2)
            .and_then(|doubled| doubled.checked_add(1))
        else {
            return false;
        };
        index == base
            || (bit == 1 && base.checked_add(1) == Some(index))
            || (bit == 0 && index.checked_add(1) == Some(base))
    };
    reaches(wa, zb, jb) && reaches(wb, za, ja)
}

/// Whether native ids a and b are a Zephyr z(m,t) coupler (external, odd, or
/// internal).
#[must_use]
pub fn zephyr_coupler(m: u16, t: u8, a: u32, b: u32) -> bool {
    match (zephyr_coordinates(m, t, a), zephyr_coordinates(m, t, b)) {
        (Some(ca), Some(cb)) => directed_coupler(ca, cb) || directed_coupler(cb, ca),
        _ => false,
    }
}

/// The full z(m,t) lattice: node ids ascending, edges (a < b) ascending.
///
/// `None` when `m` or `t` is zero, when `m` is above 32 or `t` is above 8
/// (the `quip/ice-glass@1` bounds), or when the node count does not fit
/// `u32`.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "u, w, k, j, z are the Zephyr coordinate names"
)]
#[expect(
    clippy::type_complexity,
    reason = "the return type is the pinned interface"
)]
pub fn zephyr_lattice(m: u16, t: u8) -> Option<(Vec<u32>, Vec<(u32, u32)>)> {
    if m == 0 || t == 0 || m > 32 || t > 8 {
        return None;
    }
    let count = u32::try_from(node_count(m, t)?).ok()?;
    let nodes: Vec<u32> = (0..count).collect();
    let mut edges = Vec::new();
    for &a in &nodes {
        let [u, w, k, _, z] = zephyr_coordinates(m, t, a)?;
        let (w, k, z) = (i64::from(w), i64::from(k), i64::from(z));
        let mut partners = Vec::new();
        // Same (u, w, k): external and odd partners.
        for j in 0..2 {
            for zz in 0..i64::from(m) {
                partners.extend(zephyr_id(m, t, [i64::from(u), w, k, j, zz]));
            }
        }
        // The other u: the internal partners.
        for ww in 2 * z..=2 * z + 2 {
            for kk in 0..i64::from(t) {
                for j in 0..2 {
                    for zz in 0..i64::from(m) {
                        partners.extend(zephyr_id(m, t, [1 - i64::from(u), ww, kk, j, zz]));
                    }
                }
            }
        }
        partners.sort_unstable();
        partners.dedup();
        edges.extend(
            partners
                .into_iter()
                .filter(|&b| b > a && zephyr_coupler(m, t, a, b))
                .map(|b| (a, b)),
        );
    }
    Some((nodes, edges))
}

/// One candidate tile, in dense positions of the `BaseTopology`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tile {
    /// The four nodes, ascending.
    pub nodes: [u32; 4],
}

/// Every candidate tile of a topology and its spine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidates {
    /// Ascending by `nodes`.
    pub tiles: Vec<Tile>,
    /// Parallel to `tiles`: the two spine edges (each (a, b) with a < b, the
    /// pair ascending), or `None` for a free tile.
    pub spines: Vec<Option<[(u32, u32); 2]>>,
}

fn at<T: Copy>(items: &[T], pos: u32) -> Result<T, StructureError> {
    usize::try_from(pos)
        .ok()
        .and_then(|i| items.get(i))
        .copied()
        .ok_or(StructureError::Overflow)
}

/// Ascending neighbour lists over dense positions.
fn neighbour_lists(
    n: usize,
    edges: &BTreeSet<(u32, u32)>,
) -> Result<Vec<Vec<u32>>, StructureError> {
    let mut lists: Vec<Vec<u32>> = vec![Vec::new(); n];
    for &(a, b) in edges {
        for (from, to) in [(a, b), (b, a)] {
            usize::try_from(from)
                .ok()
                .and_then(|i| lists.get_mut(i))
                .ok_or(StructureError::Overflow)?
                .push(to);
        }
    }
    for list in &mut lists {
        list.sort_unstable();
    }
    Ok(lists)
}

fn neighbours(lists: &[Vec<u32>], node: u32) -> &[u32] {
    usize::try_from(node)
        .ok()
        .and_then(|i| lists.get(i))
        .map_or(&[], Vec::as_slice)
}

/// The Zephyr coordinates of every dense node, after the node and coupler
/// checks.
fn zephyr_checked(m: u16, t: u8, topology: &BaseTopology) -> Result<Vec<[u32; 5]>, StructureError> {
    let mut coordinates = Vec::with_capacity(topology.node_ids().len());
    for &id in topology.node_ids() {
        coordinates.push(zephyr_coordinates(m, t, id).ok_or(StructureError::NotZephyrNode(id))?);
    }
    for &(a, b) in topology.edges() {
        let (a, b) = (at(topology.node_ids(), a)?, at(topology.node_ids(), b)?);
        if !zephyr_coupler(m, t, a, b) {
            return Err(StructureError::NotZephyrCoupler(a.min(b), a.max(b)));
        }
    }
    Ok(coordinates)
}

/// A same-(u, w, k) pair with different j: an odd coupler.
fn odd_pair(coordinates: &[[u32; 5]], a: u32, b: u32, u: u32) -> Result<bool, StructureError> {
    let (ca, cb) = (at(coordinates, a)?, at(coordinates, b)?);
    Ok(ca[0] == u && cb[0] == u && ca[1] == cb[1] && ca[2] == cb[2] && ca[3] != cb[3])
}

/// The two spine edges of a tile.
type Spine = [(u32, u32); 2];

type Found = Vec<(Tile, Option<Spine>)>;

/// Zephyr mode: a vertical odd coupler and a horizontal odd coupler that are
/// fully joined.
fn zephyr_tiles(
    coordinates: &[[u32; 5]],
    edges: &BTreeSet<(u32, u32)>,
    lists: &[Vec<u32>],
) -> Result<Found, StructureError> {
    let mut found = Found::new();
    for &(a, b) in edges {
        if !odd_pair(coordinates, a, b, 0)? {
            continue;
        }
        let common: Vec<u32> = neighbours(lists, a)
            .iter()
            .copied()
            .filter(|c| neighbours(lists, b).binary_search(c).is_ok())
            .collect();
        for (i, &c) in common.iter().enumerate() {
            for &d in common.iter().skip(i + 1) {
                if edges.contains(&(c, d)) && odd_pair(coordinates, c, d, 1)? {
                    let mut nodes = [a, b, c, d];
                    nodes.sort_unstable();
                    let spine = if (a, b) < (c, d) {
                        [(a, b), (c, d)]
                    } else {
                        [(c, d), (a, b)]
                    };
                    found.push((Tile { nodes }, Some(spine)));
                }
            }
        }
    }
    Ok(found)
}

/// Adjacency mode: every 4-clique `a < b < c < d`.
///
/// The search stops with [`StructureError::SpineConflict`] once it has found
/// more than `edge_count / 4` tiles. Without a spine conflict, each tile
/// shares at most its 2 spine edges with other tiles, so it owns at least 4 of
/// its 6 edges alone and `4 * tiles <= edges`. A larger count must therefore
/// end in a spine conflict, and the order of checks makes `NoCandidates`
/// impossible here.
fn adjacency_tiles(lists: &[Vec<u32>], edge_count: usize) -> Result<Vec<Tile>, StructureError> {
    let cap = edge_count / 4;
    let mut tiles = Vec::new();
    for (a, na) in (0u32..).zip(lists) {
        for &b in na.iter().filter(|&&b| b > a) {
            let above: Vec<u32> = na
                .iter()
                .copied()
                .filter(|&c| c > b && neighbours(lists, b).binary_search(&c).is_ok())
                .collect();
            for (i, &c) in above.iter().enumerate() {
                for &d in above.iter().skip(i + 1) {
                    if neighbours(lists, c).binary_search(&d).is_ok() {
                        if tiles.len() >= cap {
                            return Err(StructureError::SpineConflict);
                        }
                        tiles.push(Tile {
                            nodes: [a, b, c, d],
                        });
                    }
                }
            }
        }
    }
    Ok(tiles)
}

/// The six edges of a tile and its three perfect matchings.
fn tile_edges(nodes: [u32; 4]) -> ([(u32, u32); 6], [Spine; 3]) {
    let [a, b, c, d] = nodes;
    (
        [(a, b), (a, c), (a, d), (b, c), (b, d), (c, d)],
        [[(a, b), (c, d)], [(a, c), (b, d)], [(a, d), (b, c)]],
    )
}

/// Adjacency spines: the first perfect matching that holds every edge shared
/// with another tile, or `None` for a free tile.
fn adjacency_spines(tiles: &[Tile]) -> Result<Vec<Option<Spine>>, StructureError> {
    let mut count: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for tile in tiles {
        for edge in tile_edges(tile.nodes).0 {
            let slot = count.entry(edge).or_insert(0);
            *slot = slot.saturating_add(1);
        }
    }
    let shared = |edge: &(u32, u32)| count.get(edge).is_some_and(|&n| n >= 2);
    let mut spines = Vec::with_capacity(tiles.len());
    for tile in tiles {
        let (edges, matchings) = tile_edges(tile.nodes);
        let held: Vec<(u32, u32)> = edges.into_iter().filter(shared).collect();
        if held.is_empty() {
            spines.push(None);
            continue;
        }
        let spine = matchings
            .into_iter()
            .find(|matching| held.iter().all(|edge| matching.contains(edge)))
            .ok_or(StructureError::SpineConflict)?;
        spines.push(Some(spine));
    }
    Ok(spines)
}

/// Validate the topology for `lattice` and find every candidate tile.
///
/// The checks run in this order and the first failure wins: degree,
/// duplicate edges, Zephyr nodes, Zephyr couplers, no tile, spine conflict,
/// membership, and bipartiteness of the spine graph.
///
/// # Errors
///
/// A [`StructureError`] for the first failed check.
pub fn candidates(lattice: Lattice, topology: &BaseTopology) -> Result<Candidates, StructureError> {
    if topology.stats().max_degree > MAX_DEGREE {
        return Err(StructureError::DegreeAboveLimit);
    }
    let mut edges = BTreeSet::new();
    for &(a, b) in topology.edges() {
        if a == b || !edges.insert((a.min(b), a.max(b))) {
            return Err(StructureError::DuplicateEdge);
        }
    }
    let coordinates = match lattice {
        Lattice::Zephyr { m, t } => Some(zephyr_checked(m, t, topology)?),
        Lattice::Adjacency => None,
    };
    let lists = neighbour_lists(topology.node_ids().len(), &edges)?;
    let mut found: Found = match &coordinates {
        Some(coordinates) => zephyr_tiles(coordinates, &edges, &lists)?,
        None => adjacency_tiles(&lists, edges.len())?
            .into_iter()
            .map(|tile| (tile, None))
            .collect(),
    };
    found.sort_unstable();
    found.dedup();
    if found.is_empty() {
        return Err(StructureError::NoCandidates);
    }
    let (tiles, mut spines): (Vec<Tile>, Vec<_>) = found.into_iter().unzip();
    if coordinates.is_none() {
        spines = adjacency_spines(&tiles)?;
    }
    check_membership(topology, &tiles, &spines)?;
    let result = Candidates { tiles, spines };
    let _ = sides(lattice, topology, &result)?;
    Ok(result)
}

/// Each node is in at most two slots: a distinct spine edge or a free tile.
fn check_membership(
    topology: &BaseTopology,
    tiles: &[Tile],
    spines: &[Option<[(u32, u32); 2]>],
) -> Result<(), StructureError> {
    let mut slots = vec![0u32; topology.node_ids().len()];
    let mut spine_edges = BTreeSet::new();
    let mut add = |node: u32| -> Result<(), StructureError> {
        let slot = usize::try_from(node)
            .ok()
            .and_then(|i| slots.get_mut(i))
            .ok_or(StructureError::Overflow)?;
        *slot = slot.saturating_add(1);
        Ok(())
    };
    for (tile, spine) in tiles.iter().zip(spines) {
        match spine {
            Some(pair) => spine_edges.extend(pair.iter().copied()),
            None => {
                for node in tile.nodes {
                    add(node)?;
                }
            }
        }
    }
    for (a, b) in spine_edges {
        add(a)?;
        add(b)?;
    }
    for (&id, &slot) in topology.node_ids().iter().zip(&slots) {
        if slot > 2 {
            return Err(StructureError::MembershipAboveTwo(id));
        }
    }
    Ok(())
}
