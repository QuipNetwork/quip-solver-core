//! The `quip/polariton-static` family, revision 1: the first `hard` family.
//!
//! An open `L` by `L` square-link lattice sits natively on Zephyr cells. Each
//! lattice vertex is the K4 of one cell, and each link is one segment qubit
//! shared by the two neighbouring cells. Every coupler is a hardware coupler.
//!
//! The interior vertices carry balanced charge labels `+2`, `-2`, and `0`.
//! The boundary vertices are free. The energy is the charge penalty
//! `(U / 2) * sum (Q_v - q_v)^2` over the labeled vertices, with `U = 250`
//! milli, so a witness has energy 0 exactly when every label holds. A
//! witness is valid only then: the family is `hard` because its construction
//! has a feasibility hook.
//!
//! A static pattern shows no optical preparation, retention, or transport. The
//! observables are the direct charges, the number of violated labels, and the
//! charge loss.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::descriptor::{Contract, FieldSpec, Params, SizeBounds, TopologyStats, ValueType};
use crate::family::{BaseTopology, Construction, InstanceBody, Streams};
use crate::ice::build::{region, sections, Reader};
use crate::ice::topology::{zephyr_coordinates, zephyr_coupler};
use crate::ice::twisted_dimer::Recipe;
use crate::instance::{Instance, InstanceLimits};
use crate::rng::shuffle;
use crate::routing::{best_choice, unsigned, Cell, Grid, Shape};
use crate::square::{charges, check_request, Axis, Boundary, Request, SquareLattice};
use crate::Code;

/// The parameters of `quip/polariton-static@1`.
pub const POLARITON_SCHEMA: &[FieldSpec] = &[
    FieldSpec {
        tag: 1,
        kind: ValueType::U16,
        min: 1,
        max: 32,
    },
    FieldSpec {
        tag: 2,
        kind: ValueType::U8,
        min: 1,
        max: 8,
    },
    FieldSpec {
        tag: 3,
        kind: ValueType::U16,
        min: 2,
        max: 64,
    },
    FieldSpec {
        tag: 4,
        kind: ValueType::U16,
        min: 0,
        max: 2048,
    },
];

/// Tag of the Zephyr grid size parameter.
const TAG_M: u16 = 1;
/// Tag of the Zephyr tile size parameter.
const TAG_T: u16 = 2;
/// Tag of the lattice side parameter.
const TAG_SIDE: u16 = 3;
/// Tag of the parameter for the number of `+2` labels, and of `-2` labels.
const TAG_PAIRS: u16 = 4;
/// Metadata kind of the vertex labels.
const KIND_CHARGES: u16 = 4;
/// Metadata kind of the window and the link positions.
const KIND_INCIDENCE: u16 = 5;

/// The stream that shuffles the interior vertices.
const CHARGES: &str = "charges";

/// The penalty weight `U`, in milli.
const PENALTY: i64 = 250;
/// The number of charge draws before the salt gives code 14.
const CHARGE_TRIES: usize = 4;
/// The label value of a free vertex in the metadata.
const FREE: i8 = 127;
/// The largest degree of a link in the instance.
const LINK_DEGREE: u64 = 6;

const LIMITS: InstanceLimits =
    match InstanceLimits::new(131_072, 2_097_152, 1_000, 1_099_511_627_776, 1_048_576) {
        Some(limits) => limits,
        None => InstanceLimits::FAM_37,
    };

/// The construction of `quip/polariton-static@1`. Its only stream is
/// `charges`. It has a feasibility hook, so the family is `hard`.
pub static POLARITON: Construction = Construction {
    labels: &[CHARGES],
    check_topology,
    generate,
    check_metadata,
    feasible: Some(feasible),
};

/// The contract of `quip/polariton-static@1`: status `research`, constraint
/// class `hard` (the construction has a feasibility hook), and four charge
/// draws before code 14.
pub const POLARITON_V1: Contract = Contract {
    family_id: "quip/polariton-static",
    revision: 1,
    schema: POLARITON_SCHEMA,
    metadata_kinds: &[KIND_CHARGES, KIND_INCIDENCE],
    limits: LIMITS,
    size_bounds: polariton_size_bounds,
    construction: Some(&POLARITON),
};

/// Canonical parameters of `l4`: `zephyr_m` 12, `zephyr_t` 4, `side` 4,
/// `pairs` 1.
const PARAMS_L4: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 12, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 4, 0, 4, 0, 2, 0, 0, 0,
    1, 0,
];

/// Canonical parameters of `l8`: as `l4` with `side` 8 and `pairs` 6.
const PARAMS_L8: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 12, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 8, 0, 4, 0, 2, 0, 0, 0,
    6, 0,
];

/// Canonical parameters of `l9`: as `l4` with `side` 9 and `pairs` 8.
const PARAMS_L9: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 12, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 9, 0, 4, 0, 2, 0, 0, 0,
    8, 0,
];

/// The registered parameter sets, on Zephyr z(12, 4), with
/// `pairs = floor((side - 1)^2 / 8)`.
pub const POLARITON_RECIPES: [Recipe; 3] = [
    Recipe {
        alias: "l4",
        cell: "polariton-l4",
        params: PARAMS_L4,
    },
    Recipe {
        alias: "l8",
        cell: "polariton-l8",
        params: PARAMS_L8,
    },
    Recipe {
        alias: "l9",
        cell: "polariton-l9",
        params: PARAMS_L9,
    },
];

/// The parameters of a revision.
#[derive(Clone, Copy)]
struct Config {
    m: u16,
    t: u8,
    side: u16,
    pairs: usize,
}

impl Config {
    fn of(params: &Params<'_>) -> Option<Self> {
        let m = u16::try_from(unsigned(params, TAG_M)?).ok()?;
        let t = u8::try_from(unsigned(params, TAG_T)?).ok()?;
        let side = u16::try_from(unsigned(params, TAG_SIDE)?).ok()?;
        let pairs = usize::try_from(unsigned(params, TAG_PAIRS)?).ok()?;
        (m >= 1 && t >= 1 && side >= 2).then_some(Self { m, t, side, pairs })
    }

    /// The Zephyr cell grid side `2m - 1`.
    fn grid_side(self) -> Option<u16> {
        self.m.checked_mul(2)?.checked_sub(1)
    }

    /// The number of interior vertices, `(L - 1)^2`.
    fn interior(self) -> usize {
        let inner = usize::from(self.side.saturating_sub(1));
        inner.saturating_mul(inner)
    }

    /// The links of the lattice, `2 L (L + 1)`.
    fn links(self) -> usize {
        let l = usize::from(self.side);
        l.saturating_mul(l.saturating_add(1)).saturating_mul(2)
    }

    /// Whether the charge counts and the lattice fit the Zephyr grid.
    fn fits(self) -> bool {
        let wide = self.side.checked_add(1).zip(self.grid_side());
        wide.is_some_and(|(cells, grid)| cells <= grid)
            && self
                .pairs
                .checked_mul(2)
                .is_some_and(|n| n <= self.interior())
    }
}

/// The sizes of an instance: `N` is at most the links, `E` at most six
/// couplers per interior vertex, and `D` at most six, because a link is in at
/// most two labeled vertices.
fn polariton_size_bounds(params: &Params<'_>, stats: &TopologyStats) -> Result<SizeBounds, Code> {
    let config = Config::of(params).ok_or(Code::ParamsMalformed)?;
    let overflow = Code::BoundsExceeded;
    let nodes = stats
        .n_nodes
        .min(u64::try_from(config.links()).map_err(|_| overflow)?);
    let edges = stats.n_edges.min(
        u64::try_from(config.interior())
            .map_err(|_| overflow)?
            .checked_mul(6)
            .ok_or(overflow)?,
    );
    let degree = stats
        .max_degree
        .min(LINK_DEGREE)
        .min(nodes.saturating_sub(1));
    Ok(SizeBounds {
        nodes,
        edges,
        degree,
    })
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
struct Placement {
    rows: Vec<u8>,
    cols: Vec<u8>,
    c0: u16,
    r0: u16,
}

/// The layout and the first window of `(L + 1)^2` alive cells, in order of
/// `c0`, then `r0`.
fn place(config: Config, topology: &BaseTopology) -> Option<(Grid, Placement)> {
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
fn link_qubits(lattice: &SquareLattice, grid: &Grid, placement: &Placement) -> Option<Vec<u32>> {
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

/// Admission: the topology is a subgraph of the Zephyr lattice named by the
/// parameters, the charge counts fit the interior, the lattice fits the cell
/// grid, and some window is fully alive (code 12 otherwise).
fn check_topology(params: &Params<'_>, topology: &BaseTopology) -> bool {
    let Some(config) = Config::of(params) else {
        return false;
    };
    let ids = topology.node_ids();
    let zephyr_nodes = ids
        .iter()
        .all(|&id| zephyr_coordinates(config.m, config.t, id).is_some());
    let zephyr_edges = topology.edges().iter().all(|&(u, v)| {
        let at = |pos: u32| usize::try_from(pos).ok().and_then(|pos| ids.get(pos));
        matches!((at(u), at(v)), (Some(&a), Some(&b)) if zephyr_coupler(config.m, config.t, a, b))
    });
    zephyr_nodes && zephyr_edges && config.fits() && place(config, topology).is_some()
}

/// The charge labels of one draw: `pairs` vertices of `+2`, then `pairs` of
/// `-2`, then `0` on the rest of `order`. Every other vertex is free.
fn labels_of(order: &[u32], pairs: usize, vertices: usize) -> Vec<Option<i8>> {
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

/// The interior vertices of the open lattice, in vertex order.
fn interior_vertices(side: u16) -> Vec<u32> {
    let side = u32::from(side);
    let width = side.saturating_add(1);
    (1..side)
        .flat_map(|y| (1..side).map(move |x| y * width + x))
        .collect()
}

/// Draw the labels: shuffle the interior list on the `charges` stream and
/// label it, then keep the draw when `check_request` passes. After
/// [`CHARGE_TRIES`] failed draws the salt gives code 14.
///
/// A balanced request can fail the feasibility check, so admission does not
/// make every draw pass. A block of `k` interior vertices that all carry `+2`
/// needs a net charge of `2k`, but only the links that leave the block can
/// supply it. A 2 by 3 block has `k = 6` and 10 such links, so charge 12
/// cannot hold. Each draw starts from the interior list in vertex order and
/// continues the same stream.
fn draw_labels(
    streams: &mut Streams<'_>,
    lattice: &SquareLattice,
    pairs: usize,
) -> Result<Vec<Option<i8>>, Code> {
    let interior = interior_vertices(lattice.side());
    let vertices = usize::try_from(lattice.vertex_count()).map_err(|_| Code::BoundsExceeded)?;
    let mut stream = streams.open(CHARGES).ok_or(Code::NoInstance)?;
    for _ in 0..CHARGE_TRIES {
        let mut order = interior.clone();
        shuffle(&mut stream, &mut order)?;
        let request = Request {
            charges: labels_of(&order, pairs, vertices),
            fixed: Vec::new(),
            fields: Vec::new(),
        };
        if check_request(lattice, &request).is_ok() {
            return Ok(request.charges);
        }
    }
    Err(Code::NoInstance)
}

/// The Ising form of the charge penalty over the link positions.
struct Energy {
    h: Vec<i32>,
    edges: Vec<(u32, u32)>,
    j: Vec<i32>,
    offset: i64,
}

/// `(U / 2) * sum (Q_v - q_v)^2` over the labeled vertices, in vertex order.
/// `positions` holds the dense instance position of each link. `None` on an
/// overflow, a position out of range, or a label count that is not one per
/// vertex.
fn energy(
    lattice: &SquareLattice,
    labels: &[Option<i8>],
    positions: &[u32],
    nodes: usize,
) -> Option<Energy> {
    if labels.len() != usize::try_from(lattice.vertex_count()).ok()? {
        return None;
    }
    let mut h = vec![0i64; nodes];
    let mut couplers: BTreeMap<(u32, u32), i64> = BTreeMap::new();
    let mut offset = 0i64;
    for (v, label) in (0u32..).zip(labels) {
        let Some(q) = label.map(i64::from) else {
            continue;
        };
        let at = lattice.incident(v);
        let degree = i64::try_from(at.len()).ok()?;
        offset = offset.checked_add((PENALTY / 2).checked_mul(degree.checked_add(q * q)?)?)?;
        for (rank, &(e, inc_e)) in at.iter().enumerate() {
            let pe = *positions.get(usize::try_from(e).ok()?)?;
            let slot = h.get_mut(usize::try_from(pe).ok()?)?;
            *slot = slot.checked_sub(PENALTY.checked_mul(q)?.checked_mul(i64::from(inc_e))?)?;
            for &(f, inc_f) in at.get(rank.checked_add(1)?..)? {
                let pf = *positions.get(usize::try_from(f).ok()?)?;
                let weight = PENALTY
                    .checked_mul(i64::from(inc_e))?
                    .checked_mul(i64::from(inc_f))?;
                let entry = couplers.entry((pe.min(pf), pe.max(pf))).or_insert(0);
                *entry = entry.checked_add(weight)?;
            }
        }
    }
    let mut edges = Vec::new();
    let mut j = Vec::new();
    for (&pair, &value) in &couplers {
        if value != 0 {
            edges.push(pair);
            j.push(i32::try_from(value).ok()?);
        }
    }
    let h = h
        .into_iter()
        .map(|value| i32::try_from(value).ok())
        .collect::<Option<Vec<i32>>>()?;
    Some(Energy {
        h,
        edges,
        j,
        offset,
    })
}

/// The metadata region: the labels (kind 4) and the window with the link
/// positions (kind 5).
fn metadata_region(
    side: u16,
    labels: &[Option<i8>],
    c0: u16,
    r0: u16,
    positions: &[u32],
) -> Result<Vec<u8>, Code> {
    let mut charges_payload = side.to_le_bytes().to_vec();
    charges_payload.extend(
        labels
            .iter()
            .map(|label| label.unwrap_or(FREE).to_le_bytes()[0]),
    );
    let mut incidence = c0.to_le_bytes().to_vec();
    incidence.extend_from_slice(&r0.to_le_bytes());
    for position in positions {
        incidence.extend_from_slice(&position.to_le_bytes());
    }
    region(&[(KIND_CHARGES, charges_payload), (KIND_INCIDENCE, incidence)])
}

fn generate(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let defect = Code::TopologyIncompatible;
    let config = Config::of(params).ok_or(Code::ParamsMalformed)?;
    if !config.fits() {
        return Err(defect);
    }
    let lattice = SquareLattice::new(config.side, Boundary::Open).map_err(|_| defect)?;
    let (grid, placement) = place(config, topology).ok_or(defect)?;
    let qubits = link_qubits(&lattice, &grid, &placement).ok_or(defect)?;
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
    let labels = draw_labels(streams, &lattice, config.pairs)?;
    let built =
        energy(&lattice, &labels, &positions, node_ids.len()).ok_or(Code::BoundsExceeded)?;
    let metadata = metadata_region(config.side, &labels, placement.c0, placement.r0, &positions)?;
    Ok(InstanceBody {
        node_ids,
        edges: built.edges,
        h: built.h,
        j: built.j,
        offset_milli: built.offset,
        metadata,
    })
}

/// What a valid instance carries: its lattice, its labels, and the dense
/// position of each link.
struct Parsed {
    lattice: SquareLattice,
    labels: Vec<Option<i8>>,
    positions: Vec<u32>,
}

/// The instance read through its metadata. `None` unless every rule of
/// [`check_metadata`] holds.
fn parse(params: &Params<'_>, instance: &Instance) -> Option<Parsed> {
    let config = Config::of(params)?;
    let lattice = SquareLattice::new(config.side, Boundary::Open).ok()?;
    let [payload_charges, payload_incidence] =
        sections(instance.metadata(), [KIND_CHARGES, KIND_INCIDENCE])?;

    let mut reader = Reader::new(payload_charges);
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
    if plus != config.pairs || minus != config.pairs {
        return None;
    }

    let mut reader = Reader::new(payload_incidence);
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
    if reader.rest().len() != links.checked_mul(4)? || instance.node_ids().len() != links {
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

    let rebuilt = energy(&lattice, &labels, &positions, links)?;
    let same = rebuilt.h == instance.h()
        && rebuilt.edges == instance.edges()
        && rebuilt.j == instance.j()
        && rebuilt.offset == instance.offset_milli();
    same.then_some(Parsed {
        lattice,
        labels,
        positions,
    })
}

fn check_metadata(params: &Params<'_>, instance: &Instance) -> bool {
    parse(params, instance).is_some()
}

/// The direct charges of `spins` (one per instance node) on the lattice of
/// `parsed`.
fn link_charges(parsed: &Parsed, spins: &[i8]) -> Option<Vec<i8>> {
    let link_spins = parsed
        .positions
        .iter()
        .map(|&p| spins.get(usize::try_from(p).ok()?).copied())
        .collect::<Option<Vec<i8>>>()?;
    charges(&parsed.lattice, &link_spins)
}

/// `true` when the metadata is valid, `spins` has one `+1` or `-1` entry per
/// instance node, and every labeled vertex has its charge.
fn feasible(params: &Params<'_>, instance: &Instance, spins: &[i8]) -> bool {
    let Some(parsed) = parse(params, instance) else {
        return false;
    };
    if spins.len() != instance.node_ids().len() {
        return false;
    }
    link_charges(&parsed, spins).is_some_and(|got| {
        got.iter()
            .zip(&parsed.labels)
            .all(|(&q, label)| label.is_none_or(|want| want == q))
    })
}

/// The static observables of a witness. A static pattern shows no optical
/// preparation, retention, or transport, so there is no optical field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolaritonObservables {
    /// The direct charge of every vertex, in vertex order.
    pub charges: Vec<i8>,
    /// The number of labeled vertices whose charge differs from the label.
    pub violations: u32,
    /// The charge loss `(U / 2) * sum (Q_v - q_v)^2` in milli.
    pub loss_milli: u64,
}

/// The observables of `spins` on `instance`, or `None` when the metadata is
/// invalid or `spins` is not one `+1` or `-1` entry per instance node.
#[must_use]
pub fn polariton_observables(
    params: &Params<'_>,
    instance: &Instance,
    spins: &[i8],
) -> Option<PolaritonObservables> {
    let parsed = parse(params, instance)?;
    if spins.len() != instance.node_ids().len() {
        return None;
    }
    let got = link_charges(&parsed, spins)?;
    let mut violations = 0u32;
    let mut loss = 0u64;
    for (&q, label) in got.iter().zip(&parsed.labels) {
        let Some(want) = *label else {
            continue;
        };
        let gap = u64::from(i16::from(q).abs_diff(i16::from(want)));
        if gap != 0 {
            violations = violations.checked_add(1)?;
            loss = loss.checked_add(gap.checked_mul(gap)?.checked_mul(125)?)?;
        }
    }
    Some(PolaritonObservables {
        charges: got,
        violations,
        loss_milli: loss,
    })
}
