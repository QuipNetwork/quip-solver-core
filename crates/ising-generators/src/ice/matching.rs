//! The two sides of the spine edges and the order-driven matching.

use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use super::topology::{zephyr_coordinates, Candidates, Tile};
use super::{Lattice, StructureError};
use crate::family::BaseTopology;

/// The spine edges, split into two sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sides {
    /// Spine edges on the left, ascending. Zephyr mode: vertical (u = 0).
    pub left: Vec<(u32, u32)>,
    /// Spine edges on the right, ascending.
    pub right: Vec<(u32, u32)>,
}

/// Split the spine edges of `candidates` into two sides so that the two
/// spine edges of each spine tile are on different sides.
///
/// Adjacency mode colours the spine graph, which has the spine edges as
/// vertices and one edge per spine tile. Components are visited from the
/// lowest spine edge, which is left, with neighbours in ascending order.
/// Zephyr mode puts the spine edges with u = 0 on the left. Free tiles are
/// not in the spine graph.
///
/// # Errors
///
/// [`StructureError::NotBipartite`] when a spine tile has both spine edges on
/// one side. [`StructureError::NotZephyrNode`] when a spine end is not a
/// Zephyr node.
pub fn sides(
    lattice: Lattice,
    topology: &BaseTopology,
    candidates: &Candidates,
) -> Result<Sides, StructureError> {
    let pairs: Vec<[(u32, u32); 2]> = candidates.spines.iter().flatten().copied().collect();
    let spine_edges: Vec<(u32, u32)> = pairs
        .iter()
        .flatten()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let side_of = match lattice {
        Lattice::Zephyr { m, t } => zephyr_sides(m, t, topology, &spine_edges)?,
        Lattice::Adjacency => adjacency_sides(&spine_edges, &pairs)?,
    };
    if side_of.is_empty() {
        return Ok(Sides {
            left: Vec::new(),
            right: Vec::new(),
        });
    }
    let side_at = |edge: &(u32, u32)| -> Option<bool> {
        let at = spine_edges.binary_search(edge).ok()?;
        side_of.get(at).copied()
    };
    if lattice != Lattice::Adjacency {
        for [e, f] in &pairs {
            if side_at(e) == side_at(f) {
                return Err(StructureError::NotBipartite);
            }
        }
    }
    let mut out = Sides {
        left: Vec::new(),
        right: Vec::new(),
    };
    for (edge, &right) in spine_edges.iter().zip(&side_of) {
        if right {
            out.right.push(*edge);
        } else {
            out.left.push(*edge);
        }
    }
    Ok(out)
}

/// `true` (right) or `false` (left) per spine edge: left when the first end
/// has u = 0.
fn zephyr_sides(
    m: u16,
    t: u8,
    topology: &BaseTopology,
    spine_edges: &[(u32, u32)],
) -> Result<Vec<bool>, StructureError> {
    let mut out = Vec::with_capacity(spine_edges.len());
    for &(a, _) in spine_edges {
        let id = usize::try_from(a)
            .ok()
            .and_then(|pos| topology.node_ids().get(pos))
            .copied()
            .ok_or(StructureError::Overflow)?;
        let coordinates = zephyr_coordinates(m, t, id).ok_or(StructureError::NotZephyrNode(id))?;
        out.push(coordinates[0] != 0);
    }
    Ok(out)
}

/// Two-colour the spine graph. `true` is right.
fn adjacency_sides(
    spine_edges: &[(u32, u32)],
    pairs: &[[(u32, u32); 2]],
) -> Result<Vec<bool>, StructureError> {
    let mut graph: Vec<Vec<usize>> = vec![Vec::new(); spine_edges.len()];
    for [e, f] in pairs {
        let (Ok(i), Ok(j)) = (spine_edges.binary_search(e), spine_edges.binary_search(f)) else {
            return Err(StructureError::Overflow);
        };
        if i == j {
            return Err(StructureError::NotBipartite);
        }
        for (from, to) in [(i, j), (j, i)] {
            if let Some(list) = graph.get_mut(from) {
                list.push(to);
            }
        }
    }
    for list in &mut graph {
        list.sort_unstable();
        list.dedup();
    }
    let mut colour: Vec<Option<bool>> = vec![None; spine_edges.len()];
    for root in 0..spine_edges.len() {
        if colour.get(root).copied().flatten().is_some() {
            continue;
        }
        if let Some(slot) = colour.get_mut(root) {
            *slot = Some(false);
        }
        let mut queue = VecDeque::from([root]);
        while let Some(node) = queue.pop_front() {
            let mine = colour
                .get(node)
                .copied()
                .flatten()
                .ok_or(StructureError::Overflow)?;
            for &next in graph.get(node).map_or(&[][..], Vec::as_slice) {
                match colour.get(next).copied().flatten() {
                    Some(theirs) if theirs == mine => return Err(StructureError::NotBipartite),
                    Some(_) => {}
                    None => {
                        if let Some(slot) = colour.get_mut(next) {
                            *slot = Some(!mine);
                        }
                        queue.push_back(next);
                    }
                }
            }
        }
    }
    Ok(colour.into_iter().map(|c| c.unwrap_or(false)).collect())
}

/// Check that `order` is a permutation of `0..len` and return the position of
/// each index in it.
fn ranks(order: &[u32], len: usize) -> Result<Vec<usize>, StructureError> {
    if order.len() != len {
        return Err(StructureError::BadOrder);
    }
    let mut rank: Vec<Option<usize>> = vec![None; len];
    for (position, &index) in order.iter().enumerate() {
        let slot = usize::try_from(index)
            .ok()
            .and_then(|i| rank.get_mut(i))
            .ok_or(StructureError::BadOrder)?;
        if slot.is_some() {
            return Err(StructureError::BadOrder);
        }
        *slot = Some(position);
    }
    rank.into_iter()
        .map(|r| r.ok_or(StructureError::BadOrder))
        .collect()
}

/// Check that `order` is a permutation of `0..len`.
fn check_order(order: &[u32], len: usize) -> Result<(), StructureError> {
    ranks(order, len).map(drop)
}

/// Try to match left spine `root`, following augmenting paths in the order
/// of `adj`. This is the recursive `try(l)` with an explicit stack: each
/// frame is a left spine and the next position in its list.
///
/// A right spine counts as visited when its `visited` entry equals `stamp`,
/// so one buffer serves every root without a reset.
///
/// `stack` is scratch space that the caller reuses across roots.
fn augment(
    root: usize,
    adj: &Adjacency,
    visited: &mut [usize],
    stamp: usize,
    matched: &mut [Option<usize>],
    stack: &mut Vec<(usize, usize)>,
) -> Result<(), StructureError> {
    stack.clear();
    stack.push((root, 0));
    while let Some(&(left, position)) = stack.last() {
        let list = adj.list(left)?;
        let Some(&right) = list.get(position) else {
            let _ = stack.pop();
            continue;
        };
        if let Some(top) = stack.last_mut() {
            top.1 = position.saturating_add(1);
        }
        let seen = visited.get_mut(right).ok_or(StructureError::Overflow)?;
        if *seen == stamp {
            continue;
        }
        *seen = stamp;
        let owner = matched
            .get(right)
            .copied()
            .ok_or(StructureError::Overflow)?;
        if let Some(next) = owner {
            stack.push((next, 0));
            continue;
        }
        // Success: every frame takes the right spine it just tried.
        for &(frame_left, next_position) in stack.iter() {
            let chosen = adj
                .list(frame_left)?
                .get(next_position.saturating_sub(1))
                .copied()
                .ok_or(StructureError::Overflow)?;
            *matched.get_mut(chosen).ok_or(StructureError::Overflow)? = Some(frame_left);
        }
        return Ok(());
    }
    Ok(())
}

/// The right spines of each left spine, in one flat array.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Adjacency {
    /// `starts[l]..starts[l + 1]` is the range of left spine `l` in `right`.
    starts: Vec<usize>,
    /// Right spine indices.
    right: Vec<usize>,
}

impl Adjacency {
    fn list(&self, left: usize) -> Result<&[usize], StructureError> {
        let start = *self.starts.get(left).ok_or(StructureError::Overflow)?;
        let end = *self
            .starts
            .get(left.saturating_add(1))
            .ok_or(StructureError::Overflow)?;
        self.right.get(start..end).ok_or(StructureError::Overflow)
    }
}

/// The nonce-independent part of [`select`]: the free tiles and the spine
/// tile of each `(left, right)` spine pair, as indices into the candidate
/// tiles. Build it once per topology and call [`SelectGraph::select`] for each
/// pair of orders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectGraph {
    /// The number of candidate tiles.
    candidate_count: usize,
    /// Free tiles, in candidate order.
    free: Vec<usize>,
    /// The right spines of each left spine, ascending.
    adj: Adjacency,
    /// Parallel to `adj.right`: the tile of each `(left, right)` pair.
    tiles: Vec<usize>,
    /// The number of right spines.
    right_len: usize,
}

impl SelectGraph {
    /// The graph of `candidates` split by `sides`.
    ///
    /// # Errors
    ///
    /// [`StructureError::Overflow`] when `sides` does not match `candidates`.
    pub(crate) fn new(candidates: &Candidates, sides: &Sides) -> Result<Self, StructureError> {
        let mut tile_of: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let mut free: Vec<usize> = Vec::new();
        for (at, spine) in candidates.spines.iter().enumerate() {
            let Some([e, f]) = spine else {
                free.push(at);
                continue;
            };
            let straight = (sides.left.binary_search(e), sides.right.binary_search(f));
            let swapped = (sides.left.binary_search(f), sides.right.binary_search(e));
            let (Ok(l), Ok(r)) = (if straight.0.is_ok() {
                straight
            } else {
                swapped
            }) else {
                return Err(StructureError::Overflow);
            };
            // Two distinct candidate tiles cannot share both spine edges, since
            // the two edges name all four nodes.
            if tile_of.insert((l, r), at).is_some() {
                return Err(StructureError::Overflow);
            }
        }
        let mut starts = vec![0usize; sides.left.len().saturating_add(1)];
        for &(l, _) in tile_of.keys() {
            let slot = starts
                .get_mut(l.saturating_add(1))
                .ok_or(StructureError::Overflow)?;
            *slot = slot.saturating_add(1);
        }
        for i in 1..starts.len() {
            let before = starts.get(i - 1).copied().unwrap_or(0);
            if let Some(slot) = starts.get_mut(i) {
                *slot = slot.saturating_add(before);
            }
        }
        // `tile_of` iterates in `(l, r)` order, so each left run is contiguous.
        let (right, tiles): (Vec<usize>, Vec<usize>) =
            tile_of.into_iter().map(|((_, r), at)| (r, at)).unzip();
        Ok(Self {
            candidate_count: candidates.tiles.len(),
            free,
            adj: Adjacency { starts, right },
            tiles,
            right_len: sides.right.len(),
        })
    }

    /// The number of left spines.
    pub(crate) fn left_len(&self) -> usize {
        self.adj.starts.len().saturating_sub(1)
    }

    /// The number of right spines.
    pub(crate) const fn right_len(&self) -> usize {
        self.right_len
    }

    /// The selection for one pair of orders, as ascending indices into the
    /// candidate tiles. See [`select`].
    ///
    /// # Errors
    ///
    /// [`StructureError::BadOrder`] when an order has the wrong length or
    /// repeats or exceeds an index.
    pub(crate) fn select(
        &self,
        left_order: &[u32],
        right_order: &[u32],
    ) -> Result<Vec<usize>, StructureError> {
        check_order(left_order, self.left_len())?;
        let rank = ranks(right_order, self.right_len)?;
        // Each left list, ascending by rank. The ranks are distinct, so the
        // result does not depend on the order the list starts in.
        let mut pairs: Vec<(usize, usize)> = self
            .adj
            .right
            .iter()
            .copied()
            .zip(self.tiles.iter().copied())
            .collect();
        for l in 0..self.left_len() {
            let start = self.adj.starts.get(l).copied().unwrap_or(0);
            let end = self
                .adj
                .starts
                .get(l.saturating_add(1))
                .copied()
                .unwrap_or(0);
            if let Some(list) = pairs.get_mut(start..end) {
                list.sort_by_key(|(r, _)| rank.get(*r).copied().unwrap_or(usize::MAX));
            }
        }
        let (right, tiles): (Vec<usize>, Vec<usize>) = pairs.into_iter().unzip();
        let adj = Adjacency {
            starts: self.adj.starts.clone(),
            right,
        };
        let mut matched: Vec<Option<usize>> = vec![None; self.right_len];
        // Stamps start at 1, so the zeroed buffer reads as unvisited.
        let mut visited = vec![0usize; self.right_len];
        let mut stack = Vec::new();
        for (stamp, &l) in (1usize..).zip(left_order) {
            let root = usize::try_from(l).map_err(|_| StructureError::BadOrder)?;
            augment(root, &adj, &mut visited, stamp, &mut matched, &mut stack)?;
        }
        // The tile of `(l, r)`: its position in the list of `l`.
        let tile_at = |l: usize, r: usize| -> Option<usize> {
            let start = adj.starts.get(l).copied()?;
            let list = adj.list(l).ok()?;
            let offset = list.iter().position(|&x| x == r)?;
            tiles.get(start.checked_add(offset)?).copied()
        };
        // Marking the chosen candidates and reading them back in index order
        // sorts them, because the candidates are ascending.
        let mut chosen = vec![false; self.candidate_count];
        let mut mark = |at: usize| -> Result<(), StructureError> {
            *chosen.get_mut(at).ok_or(StructureError::Overflow)? = true;
            Ok(())
        };
        for &at in &self.free {
            mark(at)?;
        }
        for (r, owner) in matched.iter().enumerate() {
            if let Some(l) = owner {
                mark(tile_at(*l, r).ok_or(StructureError::Overflow)?)?;
            }
        }
        Ok((0..self.candidate_count)
            .zip(chosen)
            .filter_map(|(at, on)| on.then_some(at))
            .collect())
    }
}

/// Select the tiles: free tiles plus a maximum matching of spine tiles.
///
/// A spine tile is an edge between its left and its right spine edge. The
/// matching takes each left spine in `left_order` and searches for an
/// augmenting path over right spines in ascending position in
/// `right_order`. `left_order` and `right_order` are permutations of
/// `0..sides.left.len()` and `0..sides.right.len()`. The result is ascending
/// by `nodes`.
///
/// # Errors
///
/// [`StructureError::BadOrder`] when an order has the wrong length or repeats
/// or exceeds an index. [`StructureError::Overflow`] when `sides` does not
/// match `candidates`.
pub fn select(
    candidates: &Candidates,
    sides: &Sides,
    left_order: &[u32],
    right_order: &[u32],
) -> Result<Vec<Tile>, StructureError> {
    check_order(left_order, sides.left.len())?;
    let _ = ranks(right_order, sides.right.len())?;
    SelectGraph::new(candidates, sides)?
        .select(left_order, right_order)?
        .into_iter()
        .map(|at| {
            candidates
                .tiles
                .get(at)
                .copied()
                .ok_or(StructureError::Overflow)
        })
        .collect()
}
