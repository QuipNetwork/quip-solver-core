//! The `quip/flow-static` family, revision 1: a `hard` family for the adapted
//! active-flow task on a square-link lattice.
//!
//! The lattice is the native layout of [`crate::polariton`]: each vertex is
//! the K4 of one Zephyr cell and each link is one segment qubit. Every link
//! carries one unit of flow. Spin `+1` sends it from the tail to the head, and
//! spin `-1` sends it back. Every vertex is labeled. The charge of a vertex is
//! the arrows in minus the arrows out, so a vertex of label `+1` is a sink and
//! a vertex of label `-1` is a source.
//!
//! Interior vertices and the four corner vertices have label 0. The corners
//! have degree 2 and the interior vertices have degree 4, so a label of 0 has
//! the parity of the degree. Each other boundary vertex has degree 3 and an
//! odd label. A shuffle of those `4 (L - 1)` vertices, in vertex order, gives
//! the first `2 (L - 1)` the label `+1` and the rest the label `-1`. The
//! labels sum to 0.
//!
//! Each link also has a requested direction `t_e` (`+1` or `-1`) and an even
//! cost `c_e`. The objective is the cost of the links that point against
//! their request, `O(s) = sum c_e [s_e != t_e]`. The instance energy is the
//! objective plus the charge penalty `(U / 2) * sum (Q_v - q_v)^2`. A link
//! with cost 0 never changes the objective, but its target stays in the
//! metadata.
//!
//! The objective lies between 0 and `R = sum c_e`. A violated label changes a
//! charge by an even amount, so it costs at least `(U / 2) * 4 = 2 U`. The
//! weight is `U = R / 2 + 1`, raised by 1 when that is odd, so `2 U > R`. So
//! every infeasible state costs more than every feasible state, and the lowest
//! energy of the instance is a feasible state of the lowest objective.
//!
//! [`flow_network`] exposes the labels, the links, the targets, and the costs
//! for a min-cost circulation baseline. The module has no solver.

pub(crate) mod codec;

use alloc::vec;
use alloc::vec::Vec;
use core::num::NonZeroU32;

use crate::descriptor::{Contract, FieldSpec, Params, SizeBounds, TopologyStats, ValueType};
use crate::family::{BaseTopology, Construction, InstanceBody, Streams};
use crate::ice::build::{region, sections, Reader};
use crate::ice::twisted_dimer::Recipe;
use crate::instance::{Instance, InstanceLimits};
use crate::native::{
    charges_payload, dense_links, incidence_payload, is_zephyr_subgraph, link_spins, on_border,
    parse_incidence, place, DenseLinks, Energy, Expansion, Native, KIND_CHARGES, KIND_INCIDENCE,
};
use crate::rng::{shuffle, uniform, ChaCha8Rng};
use crate::routing::unsigned;
use crate::square::{charges, check_request, Boundary, Request, SquareLattice};
use crate::Code;

/// The parameters of `quip/flow-static@1`.
pub const FLOW_SCHEMA: &[FieldSpec] = &[
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
        max: 500,
    },
];

/// Tag of the parameter for the largest half cost, `c_e / 2`.
const TAG_COST_HALF_MAX: u16 = 4;
/// Metadata kind of the weight, the targets, and the costs.
const KIND_FLOW: u16 = 9;

/// The stream that draws the boundary labels, the targets, and the costs.
const FLOW_STREAM: &str = "flow";

/// The number of boundary draws before the salt gives code 14.
const DRAW_TRIES: usize = 4;
/// The largest degree of a link in the instance.
const LINK_DEGREE: u64 = 6;

// The limits are the smallest that hold for `side = 64` with
// `cost_half_max = 500`. A lattice of side `L` needs `L + 1 <= 2 m - 1` cells
// and the schema has `m <= 32`, so the largest side that builds is 62. Side 64
// never builds, so these limits are a safe upper bound. The weight grows with
// the side, so the bound at 64 covers every admitted side.
//
// - nodes: `2 L (L + 1) = 8320` links.
// - edges: one coupler for each pair of links at a vertex. Two links share at
//   most one vertex, so no pair cancels. That is `6 * 63^2 + 3 * 252 + 4 =
//   24574`: 3969 interior vertices with 6 pairs, 252 edge vertices with 3, and
//   4 corners with 1. The size bound `6 (L + 1)^2 = 25350` is larger. At side
//   64 it would exceed these limits, but side 64 never builds.
// - `R / 2 = sum u_e` is at most `8320 * 500 = 4160000`, so `U` is at most
//   `4160001` rounded up to even, 4160002.
// - coefficient: a link has two ends, each with label 0 or `+-1`, so the
//   charge penalty gives `|h| <= 2 U = 8320004`. The objective adds at most
//   `u_e = 500`, so `|h| <= 8320504`. A coupler comes from one vertex and has
//   magnitude `U`, so it is below that bound.
// - offset: the charge penalty gives `(U / 2) * sum (deg + q^2)`. The degrees
//   sum to `2 * 8320 = 16640` and the squared labels sum to `4 (L - 1) = 252`,
//   so the sum is 16892 and the penalty is `2080001 * 16892 = 35135376892`.
//   The objective adds `sum u_e <= 4160000`, so the offset is at most
//   `35139536892`.
// - metadata: 2 bytes for the count and 6 bytes of header for each of 3
//   sections, 20 bytes. Kind 4 has `2 + 4225` bytes, kind 5 has
//   `4 + 8320 * 4 = 33284` bytes, and kind 9 has `8 + 8320 * 5 = 41608`
//   bytes. That is 79139 bytes.
const LIMITS: InstanceLimits =
    match InstanceLimits::new(8_320, 24_574, 8_320_504, 35_139_536_892, 79_139) {
        Some(limits) => limits,
        None => InstanceLimits::FAM_37,
    };

/// The construction of `quip/flow-static@1`. Its only stream is `flow`. It has
/// a feasibility hook, so the family is `hard`.
pub static FLOW: Construction = Construction {
    labels: &[FLOW_STREAM],
    check_topology,
    generate,
    check_metadata,
    feasible: Some(feasible),
};

/// The contract of `quip/flow-static@1`: status `research`, constraint class
/// `hard` (the construction has a feasibility hook), and four boundary draws
/// before code 14.
pub const FLOW_V1: Contract = Contract {
    family_id: "quip/flow-static",
    revision: 1,
    schema: FLOW_SCHEMA,
    metadata_kinds: &[KIND_CHARGES, KIND_INCIDENCE, KIND_FLOW],
    limits: LIMITS,
    size_bounds: flow_size_bounds,
    construction: Some(&FLOW),
};

/// Canonical parameters of `l4`: `zephyr_m` 12, `zephyr_t` 4, `side` 4,
/// `cost_half_max` 500.
const PARAMS_L4: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 12, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 4, 0, 4, 0, 2, 0, 0, 0,
    244, 1,
];

/// Canonical parameters of `l6`: as `l4` with `side` 6.
const PARAMS_L6: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 12, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 6, 0, 4, 0, 2, 0, 0, 0,
    244, 1,
];

/// Canonical parameters of `l8`: as `l4` with `side` 8.
const PARAMS_L8: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 12, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 8, 0, 4, 0, 2, 0, 0, 0,
    244, 1,
];

/// Canonical parameters of `l12`: as `l4` with `side` 12.
const PARAMS_L12: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 12, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 12, 0, 4, 0, 2, 0, 0, 0,
    244, 1,
];

/// The registered parameter sets, on Zephyr z(12, 4) with
/// `cost_half_max = 500`. The live topology has no fully alive window above
/// side 9, so `l12` gives code 12 there.
pub const FLOW_RECIPES: [Recipe; 4] = [
    Recipe {
        alias: "l4",
        cell: "flow-l4",
        params: PARAMS_L4,
    },
    Recipe {
        alias: "l6",
        cell: "flow-l6",
        params: PARAMS_L6,
    },
    Recipe {
        alias: "l8",
        cell: "flow-l8",
        params: PARAMS_L8,
    },
    Recipe {
        alias: "l12",
        cell: "flow-l12",
        params: PARAMS_L12,
    },
];

/// The parameters of a revision.
#[derive(Clone, Copy)]
struct Config {
    native: Native,
    cost_half_max: u16,
}

impl Config {
    fn of(params: &Params<'_>) -> Option<Self> {
        Some(Self {
            native: Native::of(params)?,
            cost_half_max: u16::try_from(unsigned(params, TAG_COST_HALF_MAX)?).ok()?,
        })
    }

    /// The largest cost, `2 * cost_half_max`.
    fn cost_max(self) -> u32 {
        u32::from(self.cost_half_max).saturating_mul(2)
    }
}

/// The sizes of an instance: `N` is at most the links, `E` at most six
/// couplers per vertex of the `(L + 1)` by `(L + 1)` vertex grid, and `D` at
/// most six, because a link shares a vertex with at most six other links.
fn flow_size_bounds(params: &Params<'_>, stats: &TopologyStats) -> Result<SizeBounds, Code> {
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
/// parameters, the lattice fits the cell grid, and some window is fully alive
/// (code 12 otherwise).
fn check_topology(params: &Params<'_>, topology: &BaseTopology) -> bool {
    let Some(config) = Config::of(params) else {
        return false;
    };
    is_zephyr_subgraph(config.native, topology)
        && config.native.fits_grid()
        && place(config.native, topology).is_some()
}

/// Whether vertex `v` of an open lattice of side `side` is a corner.
fn is_corner(side: u16, v: u32) -> bool {
    let width = u32::from(side).saturating_add(1);
    let last = u32::from(side);
    let (x, y) = (v % width, v / width);
    (x == 0 || x == last) && (y == 0 || y == last)
}

/// The boundary vertices of degree 3, in vertex order.
fn edge_vertices(side: u16, vertices: u32) -> Vec<u32> {
    (0..vertices)
        .filter(|&v| on_border(side, v) && !is_corner(side, v))
        .collect()
}

/// The labels of one draw: `order` holds the edge vertices after the shuffle.
/// The first half are `+1` and the rest are `-1`. Every other vertex is 0.
/// `None` when a vertex is out of range.
fn labels_of(order: &[u32], vertices: usize) -> Option<Vec<i8>> {
    let mut labels = vec![0i8; vertices];
    let half = order.len() / 2;
    for (rank, &v) in order.iter().enumerate() {
        *labels.get_mut(usize::try_from(v).ok()?)? = if rank < half { 1 } else { -1 };
    }
    Some(labels)
}

/// The targets and the costs of a draw, in link order.
struct Links {
    targets: Vec<i8>,
    costs: Vec<u32>,
}

/// One draw of the request.
struct Draw {
    labels: Vec<i8>,
    links: Links,
}

/// The labels of the first passing try. Each try shuffles the edge vertices
/// and labels them, and `accepts` decides whether the labels pass. The salt
/// gives code 14 after [`DRAW_TRIES`] refused tries.
fn draw_labels(
    stream: &mut ChaCha8Rng,
    lattice: &SquareLattice,
    mut accepts: impl FnMut(&[i8]) -> bool,
) -> Result<Vec<i8>, Code> {
    let vertices = usize::try_from(lattice.vertex_count()).map_err(|_| Code::BoundsExceeded)?;
    let edge = edge_vertices(lattice.side(), lattice.vertex_count());
    for _ in 0..DRAW_TRIES {
        let mut order = edge.clone();
        shuffle(stream, &mut order)?;
        let drawn = labels_of(&order, vertices).ok_or(Code::TopologyIncompatible)?;
        if accepts(&drawn) {
            return Ok(drawn);
        }
    }
    Err(Code::NoInstance)
}

/// Draw the request on the `flow` stream. Each try shuffles the edge
/// vertices, in vertex order, and labels them. A try passes when
/// `check_request` passes for the labels. After [`DRAW_TRIES`] failed tries
/// the salt gives code 14. The targets and the costs follow on the same
/// stream after the first passing try. For each link in link order, one
/// `uniform(2)` word gives the target (`1` is `+1`, `0` is `-1`), and then one
/// `uniform(cost_half_max + 1)` word gives `u_e`, and `c_e = 2 u_e`.
///
/// A request is refused when a set of vertices has more net label than links
/// that leave it. At side 5 the labeling with every sink on the first eight
/// edge vertices is refused. Sides 2 to 4 have no refused labeling. So the
/// retry path runs, and code 14 needs four refused tries in a row.
fn draw(streams: &mut Streams<'_>, lattice: &SquareLattice, config: Config) -> Result<Draw, Code> {
    let overflow = Code::BoundsExceeded;
    let mut stream = streams.open(FLOW_STREAM).ok_or(Code::NoInstance)?;
    let labels = draw_labels(&mut stream, lattice, |labels| {
        let request = Request {
            charges: labels.iter().map(|&q| Some(q)).collect(),
            fixed: Vec::new(),
            fields: Vec::new(),
        };
        check_request(lattice, &request).is_ok()
    })?;
    let two = NonZeroU32::new(2).ok_or(overflow)?;
    let half_bound =
        NonZeroU32::new(u32::from(config.cost_half_max).saturating_add(1)).ok_or(overflow)?;
    let count = usize::try_from(lattice.link_count()).map_err(|_| overflow)?;
    let mut targets = Vec::with_capacity(count);
    let mut costs = Vec::with_capacity(count);
    for _ in 0..count {
        targets.push(if uniform(&mut stream, two) == 1 {
            1
        } else {
            -1
        });
        costs.push(
            uniform(&mut stream, half_bound)
                .checked_mul(2)
                .ok_or(overflow)?,
        );
    }
    Ok(Draw {
        labels,
        links: Links { targets, costs },
    })
}

/// The penalty weight `U`: `R / 2 + 1` for `R = sum c_e`, raised by 1 when
/// that is odd so that `U / 2` is exact. `None` on an overflow.
fn penalty(costs: &[u32]) -> Option<i64> {
    let mut weight = 1i64;
    for &cost in costs {
        weight = weight.checked_add(i64::from(cost / 2))?;
    }
    if weight % 2 == 1 {
        weight = weight.checked_add(1)?;
    }
    Some(weight)
}

/// What the energy reads: the labels, the layout, and the links.
struct Model<'a> {
    lattice: &'a SquareLattice,
    labels: &'a [i8],
    positions: &'a [u32],
    links: &'a Links,
    weight: i64,
}

/// The objective field and offset, then the charge penalty of every vertex.
/// `None` on an overflow, a position out of range, or a length that is not
/// one entry per vertex or per link.
fn energy(model: &Model<'_>, nodes: usize) -> Option<Energy> {
    let lattice = model.lattice;
    let links = usize::try_from(lattice.link_count()).ok()?;
    if model.links.targets.len() != links || model.links.costs.len() != links {
        return None;
    }
    let mut sums = Expansion::new(nodes);
    for (e, (&target, &cost)) in (0u32..).zip(model.links.targets.iter().zip(&model.links.costs)) {
        let half = i64::from(cost / 2);
        sums.add_field(
            model.positions,
            e,
            half.checked_mul(i64::from(target))?.checked_neg()?,
        )?;
        sums.add_offset(half)?;
    }
    let labels: Vec<Option<i8>> = model.labels.iter().map(|&q| Some(q)).collect();
    sums.add_charge_penalty(lattice, model.positions, &labels, model.weight)?;
    sums.finish()
}

/// The payload of the `flow` section: `U`, then for each link in link order
/// the target as `i8` and the cost as `u32`.
fn flow_payload(weight: u64, links: &Links) -> Vec<u8> {
    let mut payload = weight.to_le_bytes().to_vec();
    for (&target, &cost) in links.targets.iter().zip(&links.costs) {
        payload.extend_from_slice(&target.to_le_bytes());
        payload.extend_from_slice(&cost.to_le_bytes());
    }
    payload
}

fn generate(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let defect = Code::TopologyIncompatible;
    let config = Config::of(params).ok_or(Code::ParamsMalformed)?;
    if !config.native.fits_grid() {
        return Err(defect);
    }
    let lattice = SquareLattice::new(config.native.side, Boundary::Open).map_err(|_| defect)?;
    let (grid, placement) = place(config.native, topology).ok_or(defect)?;
    let DenseLinks {
        node_ids,
        positions,
    } = dense_links(&lattice, &grid, &placement, topology)?;
    let drawn = draw(streams, &lattice, config)?;
    let weight = penalty(&drawn.links.costs).ok_or(Code::BoundsExceeded)?;
    let model = Model {
        lattice: &lattice,
        labels: &drawn.labels,
        positions: &positions,
        links: &drawn.links,
        weight,
    };
    let built = energy(&model, node_ids.len()).ok_or(Code::BoundsExceeded)?;
    let shown: Vec<Option<i8>> = drawn.labels.iter().map(|&q| Some(q)).collect();
    let metadata = region(&[
        (KIND_CHARGES, charges_payload(config.native.side, &shown)),
        (
            KIND_INCIDENCE,
            incidence_payload(placement.c0, placement.r0, &positions),
        ),
        (
            KIND_FLOW,
            flow_payload(
                u64::try_from(weight).map_err(|_| Code::BoundsExceeded)?,
                &drawn.links,
            ),
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

/// The labels of a `charges` payload. `None` unless the side matches, there is
/// one byte per vertex, the corners and the interior vertices are 0, the edge
/// vertices are `+1` or `-1`, and there are as many of each.
fn parse_labels(payload: &[u8], config: Native, lattice: &SquareLattice) -> Option<Vec<i8>> {
    let mut reader = Reader::new(payload);
    if reader.u16()? != config.side {
        return None;
    }
    let raw = reader.rest();
    if raw.len() != usize::try_from(lattice.vertex_count()).ok()? {
        return None;
    }
    let (mut plus, mut minus) = (0usize, 0usize);
    let mut labels = Vec::with_capacity(raw.len());
    for (v, &byte) in (0u32..).zip(raw) {
        let value = i8::from_le_bytes([byte]);
        if on_border(config.side, v) && !is_corner(config.side, v) {
            match value {
                1 => plus = plus.checked_add(1)?,
                -1 => minus = minus.checked_add(1)?,
                _ => return None,
            }
        } else if value != 0 {
            return None;
        }
        labels.push(value);
    }
    (plus == minus).then_some(labels)
}

/// What a valid instance carries: its lattice, its labels, the dense position
/// of each link, and its targets and costs.
struct Parsed {
    lattice: SquareLattice,
    labels: Vec<i8>,
    positions: Vec<u32>,
    links: Links,
}

/// The instance read through its metadata. `None` unless every rule of
/// [`check_metadata`] holds.
fn parse(params: &Params<'_>, instance: &Instance) -> Option<Parsed> {
    let config = Config::of(params)?;
    let lattice = SquareLattice::new(config.native.side, Boundary::Open).ok()?;
    let [payload_charges, payload_incidence, payload_flow] = sections(
        instance.metadata(),
        [KIND_CHARGES, KIND_INCIDENCE, KIND_FLOW],
    )?;
    let labels = parse_labels(payload_charges, config.native, &lattice)?;
    let (_, _, positions) =
        parse_incidence(payload_incidence, config.native, instance.node_ids().len())?;

    let mut reader = Reader::new(payload_flow);
    let stored_weight = u64::from_le_bytes(reader.take(8)?.try_into().ok()?);
    let count = usize::try_from(lattice.link_count()).ok()?;
    let mut targets = Vec::with_capacity(count);
    let mut costs = Vec::with_capacity(count);
    for _ in 0..count {
        let target = i8::from_le_bytes([*reader.take(1)?.first()?]);
        let cost = u32::from_le_bytes(reader.take(4)?.try_into().ok()?);
        if (target != 1 && target != -1) || cost % 2 != 0 || cost > config.cost_max() {
            return None;
        }
        targets.push(target);
        costs.push(cost);
    }
    if !reader.rest().is_empty() {
        return None;
    }
    let weight = penalty(&costs)?;
    if i64::try_from(stored_weight).ok()? != weight {
        return None;
    }
    let links = Links { targets, costs };

    let model = Model {
        lattice: &lattice,
        labels: &labels,
        positions: &positions,
        links: &links,
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
        links,
    })
}

fn check_metadata(params: &Params<'_>, instance: &Instance) -> bool {
    parse(params, instance).is_some()
}

/// `true` when the metadata is valid, `spins` has one `+1` or `-1` entry per
/// instance node, and every label holds.
fn feasible(params: &Params<'_>, instance: &Instance, spins: &[i8]) -> bool {
    let Some(parsed) = parse(params, instance) else {
        return false;
    };
    let Some(links) = link_spins(&parsed.positions, spins, instance.node_ids().len()) else {
        return false;
    };
    charges(&parsed.lattice, &links).is_some_and(|got| got == parsed.labels)
}

/// The observables of a state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowObservables {
    /// The objective, the sum of the costs of the reversed links.
    pub objective: u64,
    /// The direct charge of every vertex, in vertex order.
    pub charges: Vec<i8>,
    /// The number of vertices whose charge differs from the label.
    pub violations: u32,
    /// The number of links whose spin differs from the target.
    pub reversed: u32,
}

/// The observables of `spins` on `instance`, or `None` when the metadata is
/// invalid or `spins` is not one `+1` or `-1` entry per instance node.
#[must_use]
pub fn flow_observables(
    params: &Params<'_>,
    instance: &Instance,
    spins: &[i8],
) -> Option<FlowObservables> {
    let parsed = parse(params, instance)?;
    let links = link_spins(&parsed.positions, spins, instance.node_ids().len())?;
    let got = charges(&parsed.lattice, &links)?;
    let mut violations = 0u32;
    for (&q, &label) in got.iter().zip(&parsed.labels) {
        if q != label {
            violations = violations.checked_add(1)?;
        }
    }
    let mut objective = 0u64;
    let mut reversed = 0u32;
    for ((&spin, &target), &cost) in links
        .iter()
        .zip(&parsed.links.targets)
        .zip(&parsed.links.costs)
    {
        if spin != target {
            objective = objective.checked_add(u64::from(cost))?;
            reversed = reversed.checked_add(1)?;
        }
    }
    Some(FlowObservables {
        objective,
        charges: got,
        violations,
        reversed,
    })
}

/// One link as an arc of the flow network. A flow along the arc goes from
/// `tail` to `head`, which is spin `+1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowArc {
    /// The vertex the arc leaves.
    pub tail: u32,
    /// The vertex the arc enters.
    pub head: u32,
    /// The requested direction: `+1` along the arc, `-1` against it.
    pub target: i8,
    /// The cost of a flow against the target.
    pub cost: u32,
}

/// The flow network of an instance for a min-cost circulation baseline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowNetwork {
    /// The supply of each vertex, in vertex order: the flow that must leave
    /// minus the flow that must enter. It is the negated label, so a source
    /// has `+1`, a sink has `-1`, and the supplies sum to 0.
    pub supplies: Vec<i8>,
    /// One arc per link, in link order. Every arc carries one unit of flow in
    /// one of its two directions.
    pub arcs: Vec<FlowArc>,
}

/// The flow network of `instance`, or `None` when the metadata is invalid.
#[must_use]
pub fn flow_network(params: &Params<'_>, instance: &Instance) -> Option<FlowNetwork> {
    let parsed = parse(params, instance)?;
    let supplies = parsed
        .labels
        .iter()
        .map(|&q| q.checked_neg())
        .collect::<Option<Vec<i8>>>()?;
    let mut arcs = Vec::with_capacity(parsed.links.costs.len());
    for (e, (&target, &cost)) in (0u32..).zip(parsed.links.targets.iter().zip(&parsed.links.costs))
    {
        let link = parsed.lattice.link(e)?;
        arcs.push(FlowArc {
            tail: link.tail,
            head: link.head,
            target,
            cost,
        });
    }
    Some(FlowNetwork { supplies, arcs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lattice() -> SquareLattice {
        match SquareLattice::new(3, Boundary::Open) {
            Ok(lattice) => lattice,
            Err(_) => unreachable!("side 3 is in range"),
        }
    }

    #[test]
    fn four_refusals_give_code_14() {
        let mut stream = ChaCha8Rng::from_seed([7; 32]);
        let mut calls = 0;
        let got = draw_labels(&mut stream, &lattice(), |_| {
            calls += 1;
            false
        });
        assert_eq!(got, Err(Code::NoInstance));
        assert_eq!(calls, DRAW_TRIES);
        assert_eq!(calls, 4);
    }

    #[test]
    fn the_fourth_draw_can_pass() {
        let mut stream = ChaCha8Rng::from_seed([7; 32]);
        let mut calls = 0;
        let mut seen = Vec::new();
        let got = draw_labels(&mut stream, &lattice(), |labels| {
            calls += 1;
            seen.push(labels.to_vec());
            calls == 4
        });
        assert_eq!(calls, 4);
        assert_eq!(got.as_ref().ok(), seen.last());
        assert_eq!(seen.len(), 4);
    }
}
