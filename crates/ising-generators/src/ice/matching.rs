//! The two sides of the spine edges.

use alloc::collections::BTreeSet;
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use super::topology::{zephyr_coordinates, Candidates};
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
