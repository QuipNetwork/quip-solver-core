//! The native square-link layout on Zephyr cells, shared by the link-based
//! families.
//!
//! An open `L` by `L` square-link lattice sits on Zephyr cells. Each lattice
//! vertex is the K4 of one cell, and each link is one segment qubit shared by
//! the two neighbouring cells. Every coupler between links that share a vertex
//! is a hardware coupler.
//!
//! The layout picks one line per row and per column of the cell grid so that
//! the most cells keep a K4. The window is the first block of `(L + 1)^2` alive
//! cells, in order of `c0`, then `r0`.

pub(crate) mod codec;

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::descriptor::Params;
use crate::family::BaseTopology;
use crate::ice::build::Reader;
use crate::ice::topology::{zephyr_coordinates, zephyr_coupler};
use crate::routing::{best_choice, unsigned, Cell, Grid, Shape};
use crate::square::{Axis, SquareLattice};
use crate::Code;

/// Tag of the Zephyr grid size parameter.
pub(crate) const TAG_M: u16 = 1;
/// Tag of the Zephyr tile size parameter.
pub(crate) const TAG_T: u16 = 2;
/// Tag of the lattice side parameter.
pub(crate) const TAG_SIDE: u16 = 3;

/// Metadata kind of the vertex labels.
pub(crate) const KIND_CHARGES: u16 = 4;
/// Metadata kind of the window and the link positions.
pub(crate) const KIND_INCIDENCE: u16 = 5;
/// The label value of a free vertex in the metadata.
pub(crate) const FREE: i8 = 127;

/// The Zephyr parameters of a native layout.
#[derive(Clone, Copy)]
pub(crate) struct Native {
    pub(crate) m: u16,
    pub(crate) t: u8,
    pub(crate) side: u16,
}

impl Native {
    /// The grid size, tile size, and lattice side of `params`. `None` when a
    /// value is out of range.
    pub(crate) fn of(params: &Params<'_>) -> Option<Self> {
        let m = u16::try_from(unsigned(params, TAG_M)?).ok()?;
        let t = u8::try_from(unsigned(params, TAG_T)?).ok()?;
        let side = u16::try_from(unsigned(params, TAG_SIDE)?).ok()?;
        (m >= 1 && t >= 1 && side >= 2).then_some(Self { m, t, side })
    }

    /// The Zephyr cell grid side `2m - 1`.
    pub(crate) fn grid_side(self) -> Option<u16> {
        self.m.checked_mul(2)?.checked_sub(1)
    }

    /// The number of interior vertices, `(L - 1)^2`.
    pub(crate) fn interior(self) -> usize {
        let inner = usize::from(self.side.saturating_sub(1));
        inner.saturating_mul(inner)
    }

    /// The links of the lattice, `2 L (L + 1)`.
    pub(crate) fn links(self) -> usize {
        let l = usize::from(self.side);
        l.saturating_mul(l.saturating_add(1)).saturating_mul(2)
    }

    /// Whether the lattice of `(L + 1)^2` cells fits the cell grid.
    pub(crate) fn fits_grid(self) -> bool {
        self.side
            .checked_add(1)
            .zip(self.grid_side())
            .is_some_and(|(cells, grid)| cells <= grid)
    }
}

/// The lines of every row and column that keep the most cells alive.
/// `rows[r]` and `cols[c]` are indexed by the coordinate, so entry 0 is
/// unused.
fn layout(grid: &Grid) -> Option<(Vec<u8>, Vec<u8>)> {
    let side = grid.shape.side;
    let lines: Vec<u8> = (0..grid.shape.t).collect();
    let mut rows = vec![0u8; usize::from(side) + 1];
    let mut cols = rows.clone();
    loop {
        let mut changed = false;
        for r in 1..=side {
            let score = |k2: u8| -> usize {
                (1..=side)
                    .filter(|&c| {
                        cols.get(usize::from(c))
                            .is_some_and(|&k| grid.has_k4((c, r), k, k2))
                    })
                    .count()
            };
            let current = *rows.get(usize::from(r))?;
            if let Some(best) = best_choice(&lines, score(current), score) {
                *rows.get_mut(usize::from(r))? = best;
                changed = true;
            }
        }
        for c in 1..=side {
            let score = |k: u8| -> usize {
                (1..=side)
                    .filter(|&r| {
                        rows.get(usize::from(r))
                            .is_some_and(|&k2| grid.has_k4((c, r), k, k2))
                    })
                    .count()
            };
            let current = *cols.get(usize::from(c))?;
            if let Some(best) = best_choice(&lines, score(current), score) {
                *cols.get_mut(usize::from(c))? = best;
                changed = true;
            }
        }
        if !changed {
            return Some((rows, cols));
        }
    }
}

/// The lines and the window origin of a topology.
pub(crate) struct Placement {
    pub(crate) rows: Vec<u8>,
    pub(crate) cols: Vec<u8>,
    pub(crate) c0: u16,
    pub(crate) r0: u16,
}

/// The layout and the first window of `(L + 1)^2` alive cells, in order of
/// `c0`, then `r0`.
pub(crate) fn place(config: Native, topology: &BaseTopology) -> Option<(Grid, Placement)> {
    let side = config.grid_side()?;
    let shape = Shape {
        m: config.m,
        t: config.t,
        // The divisor belongs to the routing family. Nothing here reads it.
        divisor: 1,
        side,
    };
    let grid = Grid::new(shape, topology);
    let (rows, cols) = layout(&grid)?;
    let width = usize::from(side);
    let mut alive = vec![false; width.checked_mul(width)?];
    for c in 1..=side {
        for r in 1..=side {
            let cell: Cell = (c, r);
            let k = *cols.get(usize::from(c))?;
            let k2 = *rows.get(usize::from(r))?;
            let at = usize::from(c - 1) * width + usize::from(r - 1);
            *alive.get_mut(at)? = grid.has_k4(cell, k, k2);
        }
    }
    let is_alive = |c: u16, r: u16| {
        let at = usize::from(c - 1) * width + usize::from(r - 1);
        alive.get(at).copied().unwrap_or(false)
    };
    let last = side.checked_sub(config.side)?;
    for c0 in 1..=last {
        for r0 in 1..=last {
            let whole =
                (0..=config.side).all(|x| (0..=config.side).all(|y| is_alive(c0 + x, r0 + y)));
            if whole {
                return Some((grid, Placement { rows, cols, c0, r0 }));
            }
        }
    }
    None
}

/// The topology positions of the link qubits, in the link order of the
/// lattice. The link along `+x` is the right horizontal corner of its tail
/// cell and the link along `+y` is the high vertical corner.
pub(crate) fn link_qubits(
    lattice: &SquareLattice,
    grid: &Grid,
    placement: &Placement,
) -> Option<Vec<u32>> {
    let width = u32::from(lattice.side()).checked_add(1)?;
    (0..lattice.link_count())
        .map(|e| {
            let link = lattice.link(e)?;
            let x = i64::from(link.tail % width);
            let y = i64::from(link.tail / width);
            let c = i64::from(placement.c0) + x;
            let r = i64::from(placement.r0) + y;
            match link.axis {
                Axis::X => grid.seg(1, r, *placement.rows.get(usize::try_from(r).ok()?)?, c),
                Axis::Y => grid.seg(0, c, *placement.cols.get(usize::try_from(c).ok()?)?, r),
            }
        })
        .collect()
}

/// Whether the topology is a subgraph of the Zephyr lattice `z(m, t)`: every
/// node id and every edge exists there.
pub(crate) fn is_zephyr_subgraph(config: Native, topology: &BaseTopology) -> bool {
    let ids = topology.node_ids();
    let nodes = ids
        .iter()
        .all(|&id| zephyr_coordinates(config.m, config.t, id).is_some());
    let edges = topology.edges().iter().all(|&(u, v)| {
        let at = |pos: u32| usize::try_from(pos).ok().and_then(|pos| ids.get(pos));
        matches!((at(u), at(v)), (Some(&a), Some(&b)) if zephyr_coupler(config.m, config.t, a, b))
    });
    nodes && edges
}

/// The interior vertices of the open lattice, in vertex order.
pub(crate) fn interior_vertices(side: u16) -> Vec<u32> {
    let side = u32::from(side);
    let width = side.saturating_add(1);
    (1..side)
        .flat_map(|y| (1..side).map(move |x| y * width + x))
        .collect()
}

/// Whether vertex `v` of an open lattice of side `side` is on the outer
/// border.
pub(crate) fn on_border(side: u16, v: u32) -> bool {
    let width = u32::from(side).saturating_add(1);
    let (x, y) = (v % width, v / width);
    let last = u32::from(side);
    x == 0 || y == 0 || x == last || y == last
}

/// The charge labels of one draw: `pairs` vertices of `+2`, then `pairs` of
/// `-2`, then `0` on the rest of `order`. Every other vertex is free.
pub(crate) fn labels_of(order: &[u32], pairs: usize, vertices: usize) -> Vec<Option<i8>> {
    let mut labels = vec![None; vertices];
    for (rank, &v) in order.iter().enumerate() {
        let label = if rank < pairs {
            2
        } else if rank < pairs.saturating_mul(2) {
            -2
        } else {
            0
        };
        if let Some(slot) = usize::try_from(v).ok().and_then(|v| labels.get_mut(v)) {
            *slot = Some(label);
        }
    }
    labels
}

/// The instance nodes of a layout: the qubit of each link, sorted, and the
/// dense instance position of each link.
pub(crate) struct DenseLinks {
    /// The topology node id of each instance node, ascending.
    pub(crate) node_ids: Vec<u32>,
    /// The instance position of each link, in link order.
    pub(crate) positions: Vec<u32>,
}

/// The dense link positions of a placement. `TopologyIncompatible` when two
/// links share a qubit or a qubit is missing, and `BoundsExceeded` when a
/// position does not fit a `u32`.
pub(crate) fn dense_links(
    lattice: &SquareLattice,
    grid: &Grid,
    placement: &Placement,
    topology: &BaseTopology,
) -> Result<DenseLinks, Code> {
    let defect = Code::TopologyIncompatible;
    let qubits = link_qubits(lattice, grid, placement).ok_or(defect)?;
    let mut sorted = qubits.clone();
    sorted.sort_unstable();
    if sorted
        .windows(2)
        .any(|pair| matches!(pair, [a, b] if a == b))
    {
        return Err(defect);
    }
    let mut positions = Vec::with_capacity(qubits.len());
    for qubit in &qubits {
        let at = sorted.binary_search(qubit).map_err(|_| defect)?;
        positions.push(u32::try_from(at).map_err(|_| Code::BoundsExceeded)?);
    }
    let mut node_ids = Vec::with_capacity(sorted.len());
    for &qubit in &sorted {
        let at = usize::try_from(qubit).map_err(|_| defect)?;
        node_ids.push(*topology.node_ids().get(at).ok_or(defect)?);
    }
    Ok(DenseLinks {
        node_ids,
        positions,
    })
}

/// The payload of the `charges` section: the side, then one byte per vertex.
pub(crate) fn charges_payload(side: u16, labels: &[Option<i8>]) -> Vec<u8> {
    let mut payload = side.to_le_bytes().to_vec();
    payload.extend(
        labels
            .iter()
            .map(|label| label.unwrap_or(FREE).to_le_bytes()[0]),
    );
    payload
}

/// The payload of the `incidence` section: the window origin, then the dense
/// position of each link.
pub(crate) fn incidence_payload(c0: u16, r0: u16, positions: &[u32]) -> Vec<u8> {
    let mut payload = c0.to_le_bytes().to_vec();
    payload.extend_from_slice(&r0.to_le_bytes());
    for position in positions {
        payload.extend_from_slice(&position.to_le_bytes());
    }
    payload
}

/// The labels of a `charges` payload. `None` unless the side matches, there is
/// one byte per vertex, boundary vertices are free, interior labels are `-2`,
/// `0`, or `2`, and there are `pairs` of each of `+2` and `-2`.
pub(crate) fn parse_charges(
    payload: &[u8],
    config: Native,
    lattice: &SquareLattice,
    pairs: usize,
) -> Option<Vec<Option<i8>>> {
    let mut reader = Reader::new(payload);
    if reader.u16()? != config.side {
        return None;
    }
    let raw = reader.rest();
    if raw.len() != usize::try_from(lattice.vertex_count()).ok()? {
        return None;
    }
    let width = usize::from(config.side).checked_add(1)?;
    let mut labels = Vec::with_capacity(raw.len());
    let (mut plus, mut minus) = (0usize, 0usize);
    for (v, &byte) in raw.iter().enumerate() {
        let value = i8::from_le_bytes([byte]);
        let (x, y) = (v % width, v / width);
        let boundary =
            x == 0 || y == 0 || x == usize::from(config.side) || y == usize::from(config.side);
        if boundary {
            if value != FREE {
                return None;
            }
            labels.push(None);
            continue;
        }
        match value {
            2 => plus = plus.checked_add(1)?,
            -2 => minus = minus.checked_add(1)?,
            0 => {}
            _ => return None,
        }
        labels.push(Some(value));
    }
    (plus == pairs && minus == pairs).then_some(labels)
}

/// The window origin and the link positions of an `incidence` payload.
/// `None` unless the window lies in the cell grid and the positions are a
/// permutation of `0..links`.
pub(crate) fn parse_incidence(
    payload: &[u8],
    config: Native,
    nodes: usize,
) -> Option<(u16, u16, Vec<u32>)> {
    let mut reader = Reader::new(payload);
    let (c0, r0) = (reader.u16()?, reader.u16()?);
    let grid = config.grid_side()?;
    if c0 < 1
        || r0 < 1
        || c0.checked_add(config.side)? > grid
        || r0.checked_add(config.side)? > grid
    {
        return None;
    }
    let links = config.links();
    if reader.rest().len() != links.checked_mul(4)? || nodes != links {
        return None;
    }
    let mut seen = vec![false; links];
    let mut positions = Vec::with_capacity(links);
    for _ in 0..links {
        let position = reader.u32()?;
        let slot = seen.get_mut(usize::try_from(position).ok()?)?;
        if core::mem::replace(slot, true) {
            return None;
        }
        positions.push(position);
    }
    Some((c0, r0, positions))
}

/// The spin of each link, in link order, from the spins of the instance
/// nodes. `None` unless `spins` has one entry per instance node and every
/// position is in range.
pub(crate) fn link_spins(positions: &[u32], spins: &[i8], nodes: usize) -> Option<Vec<i8>> {
    if spins.len() != nodes {
        return None;
    }
    positions
        .iter()
        .map(|&p| spins.get(usize::try_from(p).ok()?).copied())
        .collect()
}

/// The Ising form of an energy over the link positions.
pub(crate) struct Energy {
    pub(crate) h: Vec<i32>,
    pub(crate) edges: Vec<(u32, u32)>,
    pub(crate) j: Vec<i32>,
    pub(crate) offset: i64,
}

/// The sums of an energy in progress. The couplers are ordered by position
/// pair, so the output order does not depend on the order of the additions.
pub(crate) struct Expansion {
    h: Vec<i64>,
    couplers: BTreeMap<(u32, u32), i64>,
    offset: i64,
}

impl Expansion {
    /// An empty energy over `nodes` instance nodes.
    pub(crate) fn new(nodes: usize) -> Self {
        Self {
            h: vec![0; nodes],
            couplers: BTreeMap::new(),
            offset: 0,
        }
    }

    /// Adds `value` to the field of link `e`. `None` on an overflow or a
    /// position out of range.
    pub(crate) fn add_field(&mut self, positions: &[u32], e: u32, value: i64) -> Option<()> {
        let at = usize::try_from(*positions.get(usize::try_from(e).ok()?)?).ok()?;
        let slot = self.h.get_mut(at)?;
        *slot = slot.checked_add(value)?;
        Some(())
    }

    /// Adds `value` to the offset. `None` on an overflow.
    pub(crate) fn add_offset(&mut self, value: i64) -> Option<()> {
        self.offset = self.offset.checked_add(value)?;
        Some(())
    }

    /// Adds `value` to the coupling of the positions `a` and `b`. `None` on
    /// an overflow.
    pub(crate) fn add_coupling(&mut self, a: u32, b: u32, value: i64) -> Option<()> {
        let slot = self.couplers.entry((a.min(b), a.max(b))).or_insert(0);
        *slot = slot.checked_add(value)?;
        Some(())
    }

    /// `(U / 2) * sum (Q_v - q_v)^2` over the labeled vertices, in vertex
    /// order, with `U = weight`. For each vertex it adds `(U / 2) (deg + q^2)`
    /// to the offset, `-U q inc` to the field of each link, and `U inc inc'`
    /// to each pair of links. `positions` holds the dense instance position
    /// of each link. `None` on an overflow, a position out of range, or a
    /// label count that is not one per vertex.
    pub(crate) fn add_charge_penalty(
        &mut self,
        lattice: &SquareLattice,
        positions: &[u32],
        labels: &[Option<i8>],
        weight: i64,
    ) -> Option<()> {
        if labels.len() != usize::try_from(lattice.vertex_count()).ok()? {
            return None;
        }
        for (v, label) in (0u32..).zip(labels) {
            let Some(q) = label.map(i64::from) else {
                continue;
            };
            let at = lattice.incident(v);
            let degree = i64::try_from(at.len()).ok()?;
            self.add_offset((weight / 2).checked_mul(degree.checked_add(q.checked_mul(q)?)?)?)?;
            for (rank, &(e, inc_e)) in at.iter().enumerate() {
                self.add_field(
                    positions,
                    e,
                    weight
                        .checked_mul(q)?
                        .checked_mul(i64::from(inc_e))?
                        .checked_neg()?,
                )?;
                let pe = *positions.get(usize::try_from(e).ok()?)?;
                for &(f, inc_f) in at.get(rank.checked_add(1)?..)? {
                    let pf = *positions.get(usize::try_from(f).ok()?)?;
                    self.add_coupling(
                        pe,
                        pf,
                        weight
                            .checked_mul(i64::from(inc_e))?
                            .checked_mul(i64::from(inc_f))?,
                    )?;
                }
            }
        }
        Some(())
    }

    /// The Ising form: the nonzero couplings in ascending pair order. `None`
    /// when a coefficient does not fit an `i32`.
    pub(crate) fn finish(self) -> Option<Energy> {
        let mut edges = Vec::new();
        let mut j = Vec::new();
        for (&pair, &value) in &self.couplers {
            if value != 0 {
                edges.push(pair);
                j.push(i32::try_from(value).ok()?);
            }
        }
        let h = self
            .h
            .into_iter()
            .map(|value| i32::try_from(value).ok())
            .collect::<Option<Vec<i32>>>()?;
        Some(Energy {
            h,
            edges,
            j,
            offset: self.offset,
        })
    }
}
