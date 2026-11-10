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

use alloc::vec;
use alloc::vec::Vec;

use crate::descriptor::Params;
use crate::family::BaseTopology;
use crate::routing::{best_choice, unsigned, Cell, Grid, Shape};
use crate::square::{Axis, SquareLattice};

/// Tag of the Zephyr grid size parameter.
pub(crate) const TAG_M: u16 = 1;
/// Tag of the Zephyr tile size parameter.
pub(crate) const TAG_T: u16 = 2;
/// Tag of the lattice side parameter.
pub(crate) const TAG_SIDE: u16 = 3;

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
