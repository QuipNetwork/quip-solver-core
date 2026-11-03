//! Gauge obstructions: whether the local tile gauges admit one common node
//! gauge. The result describes an instance only. It is never an acceptance
//! rule.

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use super::StructureError;

/// Whether the local tile gauges admit one common node gauge (diagnostic
/// only, never an acceptance rule).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Obstructions {
    /// The connected components of the tile link graph.
    pub components: u32,
    /// `links - tiles + components`.
    pub cycle_rank: u32,
    /// The links whose relation disagrees with the breadth-first potentials.
    pub frustrated_cycles: u32,
    /// `frustrated_cycles == 0`.
    pub gauge_equivalent: bool,
}

/// Test whether the local gauges admit one common node gauge.
///
/// Two tiles that share a node are linked by the product of their signs at
/// that node. Breadth-first potentials from each unvisited tile, in ascending
/// order, assign each tile a sign. A link is frustrated when the potentials
/// disagree with its relation.
///
/// # Errors
///
/// [`StructureError::MembershipAboveTwo`] with the lowest dense position that
/// is in more than two tiles. [`StructureError::Overflow`] for mismatched
/// lengths, a position at or above `n`, a sign other than `1` or `-1`, or a
/// count that does not fit `u32`.
pub fn gauge_obstructions(
    tiles: &[[u32; 4]],
    gauges: &[[i8; 4]],
    n: usize,
) -> Result<Obstructions, StructureError> {
    if tiles.len() != gauges.len() {
        return Err(StructureError::Overflow);
    }
    let tile_count = u32::try_from(tiles.len()).map_err(|_| StructureError::Overflow)?;
    let mut membership: Vec<Vec<(u32, i8)>> = vec![Vec::new(); n];
    for (tile, (positions, signs)) in (0..tile_count).zip(tiles.iter().zip(gauges)) {
        for (&position, &sign) in positions.iter().zip(signs) {
            if sign != 1 && sign != -1 {
                return Err(StructureError::Overflow);
            }
            let slot = usize::try_from(position)
                .ok()
                .and_then(|index| membership.get_mut(index))
                .ok_or(StructureError::Overflow)?;
            slot.push((tile, sign));
        }
    }

    let mut links: Vec<(u32, u32, i8)> = Vec::new();
    let mut neighbours: Vec<Vec<(u32, i8)>> = vec![Vec::new(); tiles.len()];
    for (position, group) in (0_u32..).zip(&membership) {
        match group.as_slice() {
            [] | [_] => {}
            &[(a, sign_a), (b, sign_b)] => {
                let relation = sign_a.checked_mul(sign_b).ok_or(StructureError::Overflow)?;
                links.push((a, b, relation));
                for (from, to) in [(a, b), (b, a)] {
                    neighbours
                        .get_mut(usize::try_from(from).map_err(|_| StructureError::Overflow)?)
                        .ok_or(StructureError::Overflow)?
                        .push((to, relation));
                }
            }
            _ => return Err(StructureError::MembershipAboveTwo(position)),
        }
    }
    for list in &mut neighbours {
        list.sort_unstable();
    }

    // Potential 0 marks an unvisited tile.
    let mut potential: Vec<i8> = vec![0; tiles.len()];
    let mut components: u32 = 0;
    for start in 0..tiles.len() {
        if potential.get(start).copied() != Some(0) {
            continue;
        }
        components = components.checked_add(1).ok_or(StructureError::Overflow)?;
        *potential.get_mut(start).ok_or(StructureError::Overflow)? = 1;
        let mut pending: VecDeque<usize> = VecDeque::new();
        pending.push_back(start);
        while let Some(current) = pending.pop_front() {
            let current_potential = potential
                .get(current)
                .copied()
                .ok_or(StructureError::Overflow)?;
            let list = neighbours.get(current).ok_or(StructureError::Overflow)?;
            for &(neighbour, relation) in list {
                let index = usize::try_from(neighbour).map_err(|_| StructureError::Overflow)?;
                let slot = potential.get_mut(index).ok_or(StructureError::Overflow)?;
                if *slot == 0 {
                    *slot = current_potential
                        .checked_mul(relation)
                        .ok_or(StructureError::Overflow)?;
                    pending.push_back(index);
                }
            }
        }
    }

    let mut frustrated: u32 = 0;
    for &(a, b, relation) in &links {
        let at = |tile: u32| {
            usize::try_from(tile)
                .ok()
                .and_then(|index| potential.get(index).copied())
                .ok_or(StructureError::Overflow)
        };
        let product = at(a)?.checked_mul(at(b)?).ok_or(StructureError::Overflow)?;
        if product != relation {
            frustrated = frustrated.checked_add(1).ok_or(StructureError::Overflow)?;
        }
    }
    let link_count = u32::try_from(links.len()).map_err(|_| StructureError::Overflow)?;
    let cycle_rank = link_count
        .checked_add(components)
        .and_then(|sum| sum.checked_sub(tile_count))
        .ok_or(StructureError::Overflow)?;
    Ok(Obstructions {
        components,
        cycle_rank,
        frustrated_cycles: frustrated,
        gauge_equivalent: frustrated == 0,
    })
}
