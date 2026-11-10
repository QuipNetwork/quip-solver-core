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

use alloc::vec::Vec;

use crate::descriptor::{Contract, FieldSpec, Params, SizeBounds, TopologyStats, ValueType};
use crate::family::{BaseTopology, Construction, InstanceBody, Streams};
use crate::ice::build::{region, sections};
use crate::ice::twisted_dimer::Recipe;
use crate::instance::{Instance, InstanceLimits};
use crate::native::{
    charges_payload, dense_links, incidence_payload, interior_vertices, is_zephyr_subgraph,
    labels_of, parse_charges, parse_incidence, place, DenseLinks, Energy, Expansion, Native,
    KIND_CHARGES, KIND_INCIDENCE,
};
use crate::rng::shuffle;
use crate::routing::unsigned;
use crate::square::{charges, check_request, Boundary, Request, SquareLattice};
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

/// Tag of the parameter for the number of `+2` labels, and of `-2` labels.
const TAG_PAIRS: u16 = 4;

/// The stream that shuffles the interior vertices.
const CHARGES: &str = "charges";

/// The penalty weight `U`, in milli.
const PENALTY: i64 = 250;
/// The number of charge draws before the salt gives code 14.
const CHARGE_TRIES: usize = 4;
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
    native: Native,
    pairs: usize,
}

impl Config {
    fn of(params: &Params<'_>) -> Option<Self> {
        let native = Native::of(params)?;
        let pairs = usize::try_from(unsigned(params, TAG_PAIRS)?).ok()?;
        Some(Self { native, pairs })
    }

    /// Whether the charge counts and the lattice fit the Zephyr grid.
    fn fits(self) -> bool {
        self.native.fits_grid()
            && self
                .pairs
                .checked_mul(2)
                .is_some_and(|n| n <= self.native.interior())
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
        .min(u64::try_from(config.native.links()).map_err(|_| overflow)?);
    let edges = stats.n_edges.min(
        u64::try_from(config.native.interior())
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

/// Admission: the topology is a subgraph of the Zephyr lattice named by the
/// parameters, the charge counts fit the interior, the lattice fits the cell
/// grid, and some window is fully alive (code 12 otherwise).
fn check_topology(params: &Params<'_>, topology: &BaseTopology) -> bool {
    let Some(config) = Config::of(params) else {
        return false;
    };
    is_zephyr_subgraph(config.native, topology)
        && config.fits()
        && place(config.native, topology).is_some()
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
    let mut sums = Expansion::new(nodes);
    sums.add_charge_penalty(lattice, positions, labels, PENALTY)?;
    sums.finish()
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
    region(&[
        (KIND_CHARGES, charges_payload(side, labels)),
        (KIND_INCIDENCE, incidence_payload(c0, r0, positions)),
    ])
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
    let labels = draw_labels(streams, &lattice, config.pairs)?;
    let built =
        energy(&lattice, &labels, &positions, node_ids.len()).ok_or(Code::BoundsExceeded)?;
    let metadata = metadata_region(
        config.native.side,
        &labels,
        placement.c0,
        placement.r0,
        &positions,
    )?;
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
    let lattice = SquareLattice::new(config.native.side, Boundary::Open).ok()?;
    let [payload_charges, payload_incidence] =
        sections(instance.metadata(), [KIND_CHARGES, KIND_INCIDENCE])?;

    let labels = parse_charges(payload_charges, config.native, &lattice, config.pairs)?;
    let (_, _, positions) =
        parse_incidence(payload_incidence, config.native, instance.node_ids().len())?;
    let links = config.native.links();

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
