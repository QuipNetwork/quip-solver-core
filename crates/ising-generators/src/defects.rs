//! The `quip/defects-static` family, revision 1: a `hard` family that selects
//! a low physical energy state of a square-link lattice under requested
//! charges, pins, and obstacles.
//!
//! The lattice is the native layout of [`crate::polariton`]: each vertex is
//! the K4 of one Zephyr cell and each link is one segment qubit. Link `e` of
//! the open lattice is an island of a dipolar geometry. A link along `+x` from
//! vertex `(x, y)` sits at `(2x + 1, 2y)` with axis `+x`, and a link along
//! `+y` sits at `(2x, 2y + 1)` with axis `+y`. Every moment is 1, `D` is 1,
//! the scale `S` is 1000, and `R2` is 4. So a table entry `q` is the coupling
//! in milli of `D`.
//!
//! The table comes from [`crate::dipolar`] with the oracle calibration. Only
//! the entries whose two links share a lattice vertex are kept, because only
//! those pairs are couplers of the native layout. The other entries with
//! `K <= 4` are parallel pairs. Their summed magnitude is stored as `dropped`,
//! and the table truncation bound covers the pairs beyond `R2`. A table with
//! the oracle calibration supports no physical claim.
//!
//! The reference state has every link spin `+1`. Each interior vertex then has
//! charge 0. The request has three parts. The interior vertices carry balanced
//! charge labels `+2`, `-2`, and `0`. Some zero-labeled vertices are
//! obstacles that a route may not enter. Some boundary links are pinned to the
//! reference spin `+1`.
//!
//! The instance energy is the physical energy plus a penalty. The physical
//! energy is `sum J s s + sum h s` with `J` from the kept entries and a field
//! term `h = -field_x` on links along `+x` and `h = -field_y` on links along
//! `+y`. The penalty is `(U / 2) * sum (Q_v - q_v)^2` over the labeled
//! vertices plus `U * (1 - s)` for each pinned link. The weight `U` is
//! derived from the table and the field, see the penalty function below. It makes every
//! infeasible state cost more than every feasible state, so the lowest energy
//! of the instance is a feasible state with the lowest physical energy.
//!
//! A static pattern shows no propagation or switching. A route witness, see
//! [`check_route`], is combinatorial evidence about a state and never changes
//! feasibility.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec;
use alloc::vec::Vec;

use crate::descriptor::{Contract, FieldSpec, Params, SizeBounds, TopologyStats, Value, ValueType};
use crate::dipolar::{
    physical_status, Axis as DipolarAxis, Calibration, Entry, Geometry, Island, PhysicalStatus,
    Table,
};
use crate::family::{BaseTopology, Construction, InstanceBody, Streams};
use crate::ice::build::{region, sections, Reader};
use crate::instance::{Instance, InstanceLimits};
use crate::native::{
    charges_payload, dense_links, incidence_payload, interior_vertices, is_zephyr_subgraph,
    labels_of, on_border, parse_charges, parse_incidence, place, DenseLinks, Native, KIND_CHARGES,
    KIND_INCIDENCE,
};
use crate::rng::shuffle;
use crate::routing::unsigned;
use crate::square::{charges, check_request, Axis, Boundary, Request, SquareLattice};
use crate::Code;

/// The parameters of `quip/defects-static@1`.
pub const DEFECTS_SCHEMA: &[FieldSpec] = &[
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
        max: 3,
    },
    FieldSpec {
        tag: 4,
        kind: ValueType::U16,
        min: 0,
        max: 2,
    },
    FieldSpec {
        tag: 5,
        kind: ValueType::U16,
        min: 0,
        max: 4,
    },
    FieldSpec {
        tag: 6,
        kind: ValueType::U16,
        min: 0,
        max: 12,
    },
    FieldSpec {
        tag: 7,
        kind: ValueType::I32,
        min: -1000,
        max: 1000,
    },
    FieldSpec {
        tag: 8,
        kind: ValueType::I32,
        min: -1000,
        max: 1000,
    },
];

/// Tag of the parameter for the number of `+2` labels, and of `-2` labels.
const TAG_PAIRS: u16 = 4;
/// Tag of the parameter for the number of obstacles.
const TAG_OBSTACLES: u16 = 5;
/// Tag of the parameter for the number of pinned links.
const TAG_PINS: u16 = 6;
/// Tag of the field along `+x`, in milli of `D`.
const TAG_FIELD_X: u16 = 7;
/// Tag of the field along `+y`, in milli of `D`.
const TAG_FIELD_Y: u16 = 8;
/// Metadata kind of the obstacles, the pins, and the table summary.
const KIND_DEFECTS: u16 = 8;

/// The stream that draws the labels, the obstacles, and the pins.
const CHARGES: &str = "charges";

/// The number of draws before the salt gives code 14.
const DRAW_TRIES: usize = 4;
/// The dipolar constant `D`.
const DIPOLAR_D: i32 = 1;
/// The dipolar scale `S`, so a table entry is the coupling in milli of `D`.
const DIPOLAR_SCALE: u32 = 1000;
/// The dipolar radius `R2`: pairs with `K <= 4` are in the table.
const DIPOLAR_R2: u32 = 4;
/// The largest degree of a link in the instance.
const LINK_DEGREE: u64 = 6;

// The limits are the smallest that hold for `side = 3` with `|field| = 1000`.
// The weight `U` depends only on the table and the field, so side 3 with both
// fields at 1000 gives the largest `U`.
//
// The table has 52 kept entries at side 3, one for each pair of links at a
// vertex: the 4 corner vertices have 1 pair, the 8 edge vertices have 3, and
// the 4 interior vertices have 6. Of these, 16 pairs are collinear and have
// magnitude 250, and 36 pairs are perpendicular and have magnitude 530. So
// `sum |J| = 16 * 250 + 36 * 530 = 23080`. The 24 links each have `|h| = 1000`,
// so `sum |h| = 24000`. Then `R = 2 * 47080`, `R / 2 + 1 = 47081` is odd, and
// `U = 47082`.
//
// - nodes: 24 links, `2 L (L + 1)`.
// - edges: 96, the size bound `6 (L + 1)^2`. The 52 pairs at a vertex are the
//   most that a coupler can join.
// - coefficient: a link between two interior vertices with charges `+2` and
//   `-2` has `|h| = 4 U` from the charge penalty. The field adds at most 1000.
//   So `4 * 47082 + 1000 = 189328`. A coupler has at most `530 + U`.
// - offset: the 4 interior vertices carry at most `(U / 2) * (4 + 4)` each,
//   which is `16 U`, and 12 pins add `12 U`. So `28 * 47082 = 1318296`.
// - metadata: 2 bytes for the count, then the sections with 6 bytes of header
//   each. Kind 4 has 2 + 16 bytes, kind 5 has 4 + 24 * 4 bytes, and kind 8 has
//   8 + 8 + 32 + 2 + 4 * 4 + 2 + 12 * 4 bytes. That is 254 bytes.
const LIMITS: InstanceLimits = match InstanceLimits::new(24, 96, 189_328, 1_318_296, 254) {
    Some(limits) => limits,
    None => InstanceLimits::FAM_37,
};

/// The construction of `quip/defects-static@1`. Its only stream is `charges`.
/// It has a feasibility hook, so the family is `hard`.
pub static DEFECTS: Construction = Construction {
    labels: &[CHARGES],
    check_topology,
    generate,
    check_metadata,
    feasible: Some(feasible),
};

/// The contract of `quip/defects-static@1`: status `research`, constraint
/// class `hard` (the construction has a feasibility hook), and four draws
/// before code 14.
pub const DEFECTS_V1: Contract = Contract {
    family_id: "quip/defects-static",
    revision: 1,
    schema: DEFECTS_SCHEMA,
    metadata_kinds: &[KIND_CHARGES, KIND_INCIDENCE, KIND_DEFECTS],
    limits: LIMITS,
    size_bounds: defects_size_bounds,
    construction: Some(&DEFECTS),
};

/// A signed parameter, `None` when it is missing or out of range.
fn signed(params: &Params<'_>, tag: u16) -> Option<i32> {
    match params.get(tag) {
        Some(Value::Signed(value)) => i32::try_from(value).ok(),
        _ => None,
    }
}

/// The parameters of a revision.
#[derive(Clone, Copy)]
struct Config {
    native: Native,
    pairs: usize,
    obstacles: usize,
    pins: usize,
    field_x: i32,
    field_y: i32,
}

impl Config {
    fn of(params: &Params<'_>) -> Option<Self> {
        let count = |tag| usize::try_from(unsigned(params, tag)?).ok();
        Some(Self {
            native: Native::of(params)?,
            pairs: count(TAG_PAIRS)?,
            obstacles: count(TAG_OBSTACLES)?,
            pins: count(TAG_PINS)?,
            field_x: signed(params, TAG_FIELD_X)?,
            field_y: signed(params, TAG_FIELD_Y)?,
        })
    }

    /// The boundary links of the lattice, `4 L`.
    fn boundary_links(self) -> usize {
        usize::from(self.native.side).saturating_mul(4)
    }

    /// Whether the counts and the lattice fit: the labels and the obstacles
    /// fit the interior, the pins fit the boundary links, and the lattice fits
    /// the cell grid.
    fn fits(self) -> bool {
        self.native.fits_grid()
            && self
                .pairs
                .checked_mul(2)
                .and_then(|labels| labels.checked_add(self.obstacles))
                .is_some_and(|used| used <= self.native.interior())
            && self.pins <= self.boundary_links()
    }
}

/// The sizes of an instance: `N` is at most the links, `E` at most six
/// couplers per vertex of the `(L + 1)` by `(L + 1)` vertex grid, and `D` at
/// most six, because a link shares a vertex with at most six other links.
fn defects_size_bounds(params: &Params<'_>, stats: &TopologyStats) -> Result<SizeBounds, Code> {
    let config = Config::of(params).ok_or(Code::ParamsMalformed)?;
    let overflow = Code::BoundsExceeded;
    let nodes = stats
        .n_nodes
        .min(u64::try_from(config.native.links()).map_err(|_| overflow)?);
    let vertices = u64::from(config.native.side)
        .checked_add(1)
        .and_then(|width| width.checked_mul(width))
        .ok_or(overflow)?;
    let edges = stats.n_edges.min(vertices.checked_mul(6).ok_or(overflow)?);
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

/// Admission: the topology is a subgraph of the Zephyr lattice named by the
/// parameters, the counts fit the lattice, the lattice fits the cell grid, and
/// some window is fully alive (code 12 otherwise).
fn check_topology(params: &Params<'_>, topology: &BaseTopology) -> bool {
    let Some(config) = Config::of(params) else {
        return false;
    };
    is_zephyr_subgraph(config.native, topology)
        && config.fits()
        && place(config.native, topology).is_some()
}

/// The links with both ends on the outer border, in link order.
fn boundary_links(lattice: &SquareLattice) -> Vec<u32> {
    (0..lattice.link_count())
        .filter(|&e| {
            lattice.link(e).is_some_and(|link| {
                on_border(lattice.side(), link.tail) && on_border(lattice.side(), link.head)
            })
        })
        .collect()
}

/// The couplings of the table that the native layout can carry.
struct Couplings {
    table: Table,
    /// The entries whose two links share a vertex, in table order.
    kept: Vec<Entry>,
    /// The sum of `|q|` over the other entries.
    dropped: u64,
    /// The sum of `|q|` over the kept entries.
    kept_magnitude: u64,
}

/// Whether links `i` and `j` of the lattice share a vertex.
fn share_vertex(lattice: &SquareLattice, i: u32, j: u32) -> Option<bool> {
    let (a, b) = (lattice.link(i)?, lattice.link(j)?);
    Some(
        [a.tail, a.head]
            .iter()
            .any(|v| *v == b.tail || *v == b.head),
    )
}

/// The table over the link midpoints, split into the kept and the dropped
/// entries. `None` on an arithmetic fault.
fn couplings(lattice: &SquareLattice) -> Option<Couplings> {
    let width = u32::from(lattice.side()).checked_add(1)?;
    let mut islands = Vec::new();
    for e in 0..lattice.link_count() {
        let link = lattice.link(e)?;
        let x = u16::try_from(link.tail % width).ok()?.checked_mul(2)?;
        let y = u16::try_from(link.tail / width).ok()?.checked_mul(2)?;
        islands.push(match link.axis {
            Axis::X => Island {
                x: x.checked_add(1)?,
                y,
                axis: DipolarAxis::PlusX,
                moment: 1,
            },
            Axis::Y => Island {
                x,
                y: y.checked_add(1)?,
                axis: DipolarAxis::PlusY,
                moment: 1,
            },
        });
    }
    let geometry = Geometry::new(islands, DIPOLAR_D, DIPOLAR_SCALE, DIPOLAR_R2).ok()?;
    let entries = crate::dipolar::quantize(&geometry).ok()?;
    let table = Table::new(geometry, entries, Calibration::Oracle).ok()?;
    let mut kept = Vec::new();
    let mut dropped = 0u64;
    let mut kept_magnitude = 0u64;
    for entry in table.entries() {
        let magnitude = u64::from(entry.q.unsigned_abs());
        if share_vertex(lattice, entry.i, entry.j)? {
            kept.push(*entry);
            kept_magnitude = kept_magnitude.checked_add(magnitude)?;
        } else {
            dropped = dropped.checked_add(magnitude)?;
        }
    }
    Some(Couplings {
        table,
        kept,
        dropped,
        kept_magnitude,
    })
}

/// The penalty weight `U` in milli.
///
/// `R = 2 * (sum |J| + sum |h|)` bounds the range of the physical energy,
/// because each term changes by at most twice its magnitude. `U` is
/// `R / 2 + 1`, raised by 1 when that is odd so that `U / 2` is exact. So
/// `2 U > R`.
///
/// A charge `Q_v` differs from its label `q_v` by an even number, because the
/// degree and the label have the same parity. A violated label then costs at
/// least `(U / 2) * 4 = 2 U`, and a violated pin costs `2 U`. So the energy of
/// every infeasible state is above the lowest physical energy by more than
/// `R`, and above the energy of every feasible state.
fn penalty(config: Config, couplings: &Couplings) -> Option<i64> {
    let per_axis = u64::try_from(config.native.links() / 2).ok()?;
    let field = u64::from(config.field_x.unsigned_abs())
        .checked_add(u64::from(config.field_y.unsigned_abs()))?;
    let magnitude = couplings
        .kept_magnitude
        .checked_add(per_axis.checked_mul(field)?)?;
    let range = magnitude.checked_mul(2)?;
    let mut weight = (range / 2).checked_add(1)?;
    if weight % 2 == 1 {
        weight = weight.checked_add(1)?;
    }
    i64::try_from(weight).ok()
}

/// What the energy reads: the request, the layout, and the table.
struct Model<'a> {
    lattice: &'a SquareLattice,
    labels: &'a [Option<i8>],
    positions: &'a [u32],
    pins: &'a [u32],
    kept: &'a [Entry],
    field_x: i32,
    field_y: i32,
    weight: i64,
}

/// The Ising form of the energy over the link positions.
struct Energy {
    h: Vec<i32>,
    edges: Vec<(u32, u32)>,
    j: Vec<i32>,
    offset: i64,
}

/// The position of link `e` in the instance.
fn position(positions: &[u32], e: u32) -> Option<u32> {
    positions.get(usize::try_from(e).ok()?).copied()
}

/// The field, the table couplings, the charge penalty, and the pin penalty.
/// `None` on an overflow, a position out of range, or a label count that is
/// not one per vertex.
fn energy(model: &Model<'_>, nodes: usize) -> Option<Energy> {
    let lattice = model.lattice;
    if model.labels.len() != usize::try_from(lattice.vertex_count()).ok()? {
        return None;
    }
    let weight = model.weight;
    let mut h = vec![0i64; nodes];
    let mut couplers: BTreeMap<(u32, u32), i64> = BTreeMap::new();
    let mut offset = 0i64;
    let mut add_field = |e: u32, value: i64| -> Option<()> {
        let slot = h.get_mut(usize::try_from(position(model.positions, e)?).ok()?)?;
        *slot = slot.checked_add(value)?;
        Some(())
    };
    for e in 0..lattice.link_count() {
        let field = match lattice.link(e)?.axis {
            Axis::X => model.field_x,
            Axis::Y => model.field_y,
        };
        add_field(e, -i64::from(field))?;
    }
    for entry in model.kept {
        let (pi, pj) = (
            position(model.positions, entry.i)?,
            position(model.positions, entry.j)?,
        );
        let slot = couplers.entry((pi.min(pj), pi.max(pj))).or_insert(0);
        *slot = slot.checked_add(i64::from(entry.q))?;
    }
    for (v, label) in (0u32..).zip(model.labels) {
        let Some(q) = label.map(i64::from) else {
            continue;
        };
        let at = lattice.incident(v);
        let degree = i64::try_from(at.len()).ok()?;
        offset = offset.checked_add((weight / 2).checked_mul(degree.checked_add(q * q)?)?)?;
        for (rank, &(e, inc_e)) in at.iter().enumerate() {
            add_field(
                e,
                weight
                    .checked_mul(q)?
                    .checked_mul(i64::from(inc_e))?
                    .checked_neg()?,
            )?;
            let pe = position(model.positions, e)?;
            for &(f, inc_f) in at.get(rank.checked_add(1)?..)? {
                let pf = position(model.positions, f)?;
                let pair = weight
                    .checked_mul(i64::from(inc_e))?
                    .checked_mul(i64::from(inc_f))?;
                let slot = couplers.entry((pe.min(pf), pe.max(pf))).or_insert(0);
                *slot = slot.checked_add(pair)?;
            }
        }
    }
    for &e in model.pins {
        add_field(e, weight.checked_neg()?)?;
        offset = offset.checked_add(weight)?;
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

/// One draw of the request.
struct Draw {
    labels: Vec<Option<i8>>,
    /// The obstacle vertices, ascending.
    obstacles: Vec<u32>,
    /// The pinned links, ascending.
    pins: Vec<u32>,
}

/// Draw the request on the `charges` stream. Each try shuffles the interior
/// vertices and labels them, shuffles the zero-labeled vertices in vertex
/// order and takes the obstacles, then shuffles the boundary links in link
/// order and takes the pins. A try passes when `check_request` passes for the
/// labels and the pins. After [`DRAW_TRIES`] failed tries the salt gives code
/// 14. Each try continues the same stream.
///
/// Every admitted request passes `check_request` on the first try, so code 14
/// does not happen for admitted parameters. Boundary links join two boundary
/// vertices, so a pin never touches a labeled vertex. Every interior vertex
/// has degree 4. The interior links of the `(L - 1)` by `(L - 1)` block can
/// form cycles that give each interior vertex one arrow in and one arrow out,
/// which adds charge 0. Each interior vertex also has free links to the
/// boundary vertices, and the boundary vertices are free. At `L = 2` the one
/// interior vertex has four free links. At `L = 3` each interior vertex has
/// two. Those links give charge `+2` with both arrows in, `-2` with both
/// arrows out, and 0 with one of each. So any labels hold.
fn draw(streams: &mut Streams<'_>, lattice: &SquareLattice, config: Config) -> Result<Draw, Code> {
    let defect = Code::TopologyIncompatible;
    let interior = interior_vertices(lattice.side());
    let boundary = boundary_links(lattice);
    let vertices = usize::try_from(lattice.vertex_count()).map_err(|_| Code::BoundsExceeded)?;
    let mut stream = streams.open(CHARGES).ok_or(Code::NoInstance)?;
    for _ in 0..DRAW_TRIES {
        let mut order = interior.clone();
        shuffle(&mut stream, &mut order)?;
        let labels = labels_of(&order, config.pairs, vertices);

        let mut zeros: Vec<u32> = (0..lattice.vertex_count())
            .filter(|&v| {
                usize::try_from(v)
                    .ok()
                    .and_then(|v| labels.get(v))
                    .is_some_and(|label| *label == Some(0))
            })
            .collect();
        shuffle(&mut stream, &mut zeros)?;
        let mut obstacles = zeros.get(..config.obstacles).ok_or(defect)?.to_vec();
        obstacles.sort_unstable();

        let mut links = boundary.clone();
        shuffle(&mut stream, &mut links)?;
        let mut pins = links.get(..config.pins).ok_or(defect)?.to_vec();
        pins.sort_unstable();

        let request = Request {
            charges: labels.clone(),
            fixed: pins.iter().map(|&e| (e, 1)).collect(),
            fields: Vec::new(),
        };
        if check_request(lattice, &request).is_ok() {
            return Ok(Draw {
                labels,
                obstacles,
                pins,
            });
        }
    }
    Err(Code::NoInstance)
}

/// The payload of the `defects` section: `U`, `dropped`, the table
/// commitment, the obstacles, then the pins.
fn defects_payload(
    weight: u64,
    couplings: &Couplings,
    obstacles: &[u32],
    pins: &[u32],
) -> Result<Vec<u8>, Code> {
    let overflow = Code::BoundsExceeded;
    let mut payload = weight.to_le_bytes().to_vec();
    payload.extend_from_slice(&couplings.dropped.to_le_bytes());
    payload.extend_from_slice(&couplings.table.commitment());
    for list in [obstacles, pins] {
        let count = u16::try_from(list.len()).map_err(|_| overflow)?;
        payload.extend_from_slice(&count.to_le_bytes());
        for id in list {
            payload.extend_from_slice(&id.to_le_bytes());
        }
    }
    Ok(payload)
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
    let lattice = SquareLattice::new(config.native.side, Boundary::Open).map_err(|_| defect)?;
    let (grid, placement) = place(config.native, topology).ok_or(defect)?;
    let DenseLinks {
        node_ids,
        positions,
    } = dense_links(&lattice, &grid, &placement, topology)?;
    let drawn = draw(streams, &lattice, config)?;
    let couplings = couplings(&lattice).ok_or(Code::BoundsExceeded)?;
    let weight = penalty(config, &couplings).ok_or(Code::BoundsExceeded)?;
    let model = Model {
        lattice: &lattice,
        labels: &drawn.labels,
        positions: &positions,
        pins: &drawn.pins,
        kept: &couplings.kept,
        field_x: config.field_x,
        field_y: config.field_y,
        weight,
    };
    let built = energy(&model, node_ids.len()).ok_or(Code::BoundsExceeded)?;
    let metadata = region(&[
        (
            KIND_CHARGES,
            charges_payload(config.native.side, &drawn.labels),
        ),
        (
            KIND_INCIDENCE,
            incidence_payload(placement.c0, placement.r0, &positions),
        ),
        (
            KIND_DEFECTS,
            defects_payload(
                u64::try_from(weight).map_err(|_| Code::BoundsExceeded)?,
                &couplings,
                &drawn.obstacles,
                &drawn.pins,
            )?,
        ),
    ])?;
    Ok(InstanceBody {
        node_ids,
        edges: built.edges,
        h: built.h,
        j: built.j,
        offset_milli: built.offset,
        metadata,
    })
}

/// What a valid instance carries: its lattice, its labels, the dense position
/// of each link, its obstacles and pins, its table, and its field.
pub(crate) struct Parsed {
    pub(crate) lattice: SquareLattice,
    pub(crate) labels: Vec<Option<i8>>,
    pub(crate) positions: Vec<u32>,
    pub(crate) obstacles: Vec<u32>,
    pub(crate) pins: Vec<u32>,
    couplings: Couplings,
    field_x: i32,
    field_y: i32,
}

/// A strictly ascending list of `u32` that follows a `u16` count. `None` for
/// a count that differs from `expected`.
fn id_list(reader: &mut Reader<'_>, expected: usize) -> Option<Vec<u32>> {
    let count = usize::from(reader.u16()?);
    if count != expected {
        return None;
    }
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let id = reader.u32()?;
        if ids.last().is_some_and(|&last| last >= id) {
            return None;
        }
        ids.push(id);
    }
    Some(ids)
}

/// The instance read through its metadata. `None` unless every rule of
/// [`check_metadata`] holds.
fn parse(params: &Params<'_>, instance: &Instance) -> Option<Parsed> {
    let config = Config::of(params)?;
    let lattice = SquareLattice::new(config.native.side, Boundary::Open).ok()?;
    let [payload_charges, payload_incidence, payload_defects] = sections(
        instance.metadata(),
        [KIND_CHARGES, KIND_INCIDENCE, KIND_DEFECTS],
    )?;
    let labels = parse_charges(payload_charges, config.native, &lattice, config.pairs)?;
    let (_, _, positions) =
        parse_incidence(payload_incidence, config.native, instance.node_ids().len())?;

    let couplings = couplings(&lattice)?;
    let weight = penalty(config, &couplings)?;
    let mut reader = Reader::new(payload_defects);
    let stored_weight = u64::from_le_bytes(reader.take(8)?.try_into().ok()?);
    let stored_dropped = u64::from_le_bytes(reader.take(8)?.try_into().ok()?);
    let stored_commitment: [u8; 32] = reader.take(32)?.try_into().ok()?;
    if i64::try_from(stored_weight).ok()? != weight
        || stored_dropped != couplings.dropped
        || stored_commitment != couplings.table.commitment()
    {
        return None;
    }
    let obstacles = id_list(&mut reader, config.obstacles)?;
    let obstacle_ok = obstacles.iter().all(|&v| {
        let interior = !on_border(config.native.side, v);
        let zero = usize::try_from(v)
            .ok()
            .and_then(|v| labels.get(v))
            .is_some_and(|label| *label == Some(0));
        interior && zero
    });
    let pins = id_list(&mut reader, config.pins)?;
    let boundary = boundary_links(&lattice);
    let pins_ok = pins.iter().all(|e| boundary.binary_search(e).is_ok());
    if !obstacle_ok || !pins_ok || !reader.rest().is_empty() {
        return None;
    }

    let model = Model {
        lattice: &lattice,
        labels: &labels,
        positions: &positions,
        pins: &pins,
        kept: &couplings.kept,
        field_x: config.field_x,
        field_y: config.field_y,
        weight,
    };
    let rebuilt = energy(&model, instance.node_ids().len())?;
    let same = rebuilt.h == instance.h()
        && rebuilt.edges == instance.edges()
        && rebuilt.j == instance.j()
        && rebuilt.offset == instance.offset_milli();
    same.then_some(Parsed {
        lattice,
        labels,
        positions,
        obstacles,
        pins,
        couplings,
        field_x: config.field_x,
        field_y: config.field_y,
    })
}

fn check_metadata(params: &Params<'_>, instance: &Instance) -> bool {
    parse(params, instance).is_some()
}

/// The spin of each link, in link order, from the spins of the instance
/// nodes. `None` unless `spins` has one entry per instance node.
fn link_spins(parsed: &Parsed, spins: &[i8], nodes: usize) -> Option<Vec<i8>> {
    if spins.len() != nodes {
        return None;
    }
    parsed
        .positions
        .iter()
        .map(|&p| spins.get(usize::try_from(p).ok()?).copied())
        .collect()
}

/// `true` when the metadata is valid, `spins` has one `+1` or `-1` entry per
/// instance node, every label holds, and every pinned link has spin `+1`.
fn feasible(params: &Params<'_>, instance: &Instance, spins: &[i8]) -> bool {
    let Some(parsed) = parse(params, instance) else {
        return false;
    };
    let Some(links) = link_spins(&parsed, spins, instance.node_ids().len()) else {
        return false;
    };
    let labels_hold = charges(&parsed.lattice, &links).is_some_and(|got| {
        got.iter()
            .zip(&parsed.labels)
            .all(|(&q, label)| label.is_none_or(|want| want == q))
    });
    let pins_hold = parsed
        .pins
        .iter()
        .all(|&e| usize::try_from(e).ok().and_then(|e| links.get(e)) == Some(&1));
    labels_hold && pins_hold
}

/// A route: a list of paths. A path is a sequence of lattice vertices, with
/// at least two vertices, and each step joins two vertices that share a link.
pub type Route = Vec<Vec<u32>>;

/// Why a route is refused. The checks run in the order of the variants and the
/// first failure wins. Each check covers the whole route before the next one
/// starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RouteError {
    /// The instance metadata is not valid for the parameters.
    Metadata,
    /// `spins` is not one `+1` or `-1` entry per instance node.
    Spins,
    /// A path has fewer than two vertices, or a vertex is not in the lattice.
    Shape,
    /// A step of a path joins two vertices that share no link.
    NotAdjacent,
    /// A path visits a vertex twice.
    RepeatedVertex,
    /// Two paths use the same link.
    RepeatedLink,
    /// A path visits an obstacle.
    Obstacle,
    /// The endpoints of a path are not one `+2` and one `-2` labeled vertex,
    /// or some `+2` or `-2` vertex is not the endpoint of exactly one path.
    Endpoints,
    /// The links with spin `-1` are not exactly the links of the paths.
    Difference,
}

/// The link that joins vertices `u` and `v`, if there is one.
fn link_between(lattice: &SquareLattice, u: u32, v: u32) -> Option<u32> {
    lattice.incident(u).iter().find_map(|&(e, _)| {
        let link = lattice.link(e)?;
        (u != v && (link.tail == v || link.head == v)).then_some(e)
    })
}

/// The label of vertex `v`, if it is a labeled vertex.
fn label_of(parsed: &Parsed, v: u32) -> Option<i8> {
    usize::try_from(v)
        .ok()
        .and_then(|v| parsed.labels.get(v))
        .copied()
        .flatten()
}

/// Checks `route` against the state `spins`, relative to the reference state
/// with every link spin `+1`.
///
/// A route is combinatorial evidence about the state. It does not change
/// the `feasible` hook, and a valid route does not make a state feasible. The checks
/// run in the order of [`RouteError`], and the first failure wins.
///
/// # Errors
///
/// The first failed check, see [`RouteError`].
pub fn check_route(
    params: &Params<'_>,
    instance: &Instance,
    spins: &[i8],
    route: &[Vec<u32>],
) -> Result<(), RouteError> {
    let parsed = parse(params, instance).ok_or(RouteError::Metadata)?;
    let links = link_spins(&parsed, spins, instance.node_ids().len())
        .filter(|links| links.iter().all(|&s| s == 1 || s == -1))
        .ok_or(RouteError::Spins)?;
    let lattice = &parsed.lattice;
    let vertices = lattice.vertex_count();
    if route
        .iter()
        .any(|path| path.len() < 2 || path.iter().any(|&v| v >= vertices))
    {
        return Err(RouteError::Shape);
    }
    let mut steps: Vec<Vec<u32>> = Vec::with_capacity(route.len());
    for path in route {
        let step_links: Option<Vec<u32>> = path
            .windows(2)
            .map(|pair| match pair {
                [u, v] => link_between(lattice, *u, *v),
                _ => None,
            })
            .collect();
        steps.push(step_links.ok_or(RouteError::NotAdjacent)?);
    }
    for path in route {
        let mut seen = path.clone();
        seen.sort_unstable();
        if seen.windows(2).any(|pair| matches!(pair, [a, b] if a == b)) {
            return Err(RouteError::RepeatedVertex);
        }
    }
    let mut used = BTreeSet::new();
    for &e in steps.iter().flatten() {
        if !used.insert(e) {
            return Err(RouteError::RepeatedLink);
        }
    }
    if route
        .iter()
        .flatten()
        .any(|v| parsed.obstacles.binary_search(v).is_ok())
    {
        return Err(RouteError::Obstacle);
    }
    let mut ends = BTreeSet::new();
    for path in route {
        let (first, last) = (path.first().copied(), path.last().copied());
        let labels = (
            first.and_then(|v| label_of(&parsed, v)),
            last.and_then(|v| label_of(&parsed, v)),
        );
        let opposite = matches!(labels, (Some(2), Some(-2)) | (Some(-2), Some(2)));
        let fresh = first.is_some_and(|v| ends.insert(v)) && last.is_some_and(|v| ends.insert(v));
        if !opposite || !fresh {
            return Err(RouteError::Endpoints);
        }
    }
    let charged = parsed
        .labels
        .iter()
        .filter(|label| matches!(label, Some(2 | -2)))
        .count();
    if Some(charged) != route.len().checked_mul(2) {
        return Err(RouteError::Endpoints);
    }
    let flipped: BTreeSet<u32> = (0u32..)
        .zip(&links)
        .filter(|&(_, &spin)| spin == -1)
        .map(|(e, _)| e)
        .collect();
    if flipped != used {
        return Err(RouteError::Difference);
    }
    Ok(())
}

/// The observables of a state. The table has the oracle calibration, so
/// `physical_status` is [`PhysicalStatus::InsufficientPhysicalEvidence`] and
/// the physical energy supports no claim about a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefectObservables {
    /// The physical energy in milli: the kept couplings and the field, without
    /// any penalty.
    pub physical_energy: i64,
    /// The direct charge of every vertex, in vertex order.
    pub charges: Vec<i8>,
    /// The number of labeled vertices whose charge differs from the label.
    pub violated_labels: u32,
    /// The number of pinned links whose spin is not `+1`.
    pub violated_pins: u32,
    /// The bound of the table on the magnitude of the pairs beyond `R2`, in
    /// units of the table.
    pub truncation_bound: u128,
    /// The sum of `|q|` over the table entries that the native layout cannot
    /// carry.
    pub dropped: u64,
    /// What the table may claim about physics.
    pub physical_status: PhysicalStatus,
}

/// The observables of `spins` on `instance`, or `None` when the metadata is
/// invalid or `spins` is not one `+1` or `-1` entry per instance node.
#[must_use]
pub fn defect_observables(
    params: &Params<'_>,
    instance: &Instance,
    spins: &[i8],
) -> Option<DefectObservables> {
    let parsed = parse(params, instance)?;
    let links = link_spins(&parsed, spins, instance.node_ids().len())?;
    let got = charges(&parsed.lattice, &links)?;
    let mut violated_labels = 0u32;
    for (&q, label) in got.iter().zip(&parsed.labels) {
        if label.is_some_and(|want| want != q) {
            violated_labels = violated_labels.checked_add(1)?;
        }
    }
    let mut violated_pins = 0u32;
    for &e in &parsed.pins {
        if links.get(usize::try_from(e).ok()?) != Some(&1) {
            violated_pins = violated_pins.checked_add(1)?;
        }
    }
    let mut physical = 0i64;
    for entry in &parsed.couplings.kept {
        let si = i64::from(*links.get(usize::try_from(entry.i).ok()?)?);
        let sj = i64::from(*links.get(usize::try_from(entry.j).ok()?)?);
        physical = physical.checked_add(i64::from(entry.q).checked_mul(si)?.checked_mul(sj)?)?;
    }
    for (e, &spin) in (0u32..).zip(&links) {
        let field = match parsed.lattice.link(e)?.axis {
            Axis::X => parsed.field_x,
            Axis::Y => parsed.field_y,
        };
        physical = physical.checked_sub(i64::from(field).checked_mul(i64::from(spin))?)?;
    }
    Some(DefectObservables {
        physical_energy: physical,
        charges: got,
        violated_labels,
        violated_pins,
        truncation_bound: parsed.couplings.table.truncation_bound(),
        dropped: parsed.couplings.dropped,
        physical_status: physical_status(&parsed.couplings.table),
    })
}
