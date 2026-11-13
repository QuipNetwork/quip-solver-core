//! The `quip/ice-glass` family, revision 1: a spin glass with a hidden ice
//! planted inside it.
//!
//! The construction picks a set of ice tiles, gives every tile edge the strong
//! coupling [`STRONG_MILLI`] and every other edge a random coupling of
//! magnitude `glass_milli`, then hides the structure with a random gauge. The
//! metadata lets a verifier recover the tiles and the gauge.

pub(crate) mod codec;

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;
use core::num::NonZeroU32;

use crate::descriptor::{Contract, FieldSpec, Params, SizeBounds, TopologyStats, Value, ValueType};
use crate::family::{BaseTopology, Construction, InstanceBody, Streams};
use crate::ice::build::{
    lattice, mark_tiles, parse_tiles, region, sections, select_tiles, sorted_edges, tile_edges,
    tiles_payload, topology_bounds, Reader,
};
use crate::ice::{candidates, StructureError, Tile};
use crate::instance::{Instance, InstanceLimits};
use crate::rng::uniform;
use crate::Code;

/// The strong tile coupling, in milli.
pub const STRONG_MILLI: i32 = 1000;

/// The parameters of `quip/ice-glass@1`.
pub const ICE_GLASS_SCHEMA: &[FieldSpec] = &[
    FieldSpec {
        tag: 1,
        kind: ValueType::U16,
        min: 0,
        max: 32,
    },
    FieldSpec {
        tag: 2,
        kind: ValueType::U8,
        min: 0,
        max: 8,
    },
    FieldSpec {
        tag: 3,
        kind: ValueType::U16,
        min: 1,
        max: 999,
    },
];

/// Tag of the glass coupling magnitude parameter.
const TAG_GLASS: u16 = 3;
/// Metadata kind of the tile list.
const KIND_TILES: u16 = 1;
/// Metadata kind of the gauge.
const KIND_GAUGE: u16 = 2;

/// The stream that orders the left spine edges.
const LEFT: &str = "left";
/// The stream that orders the right spine edges.
const RIGHT: &str = "right";
/// The stream that draws the glass signs.
const GLASS: &str = "glass";
/// The stream that draws the gauge.
const GAUGE: &str = "gauge";

const LIMITS: InstanceLimits = match InstanceLimits::new(131_072, 2_097_152, 1_000, 0, 8_388_608) {
    Some(limits) => limits,
    None => InstanceLimits::FAM_37,
};

/// The construction of `quip/ice-glass@1`. Its streams are `left`, `right`,
/// `glass`, and `gauge`.
pub static ICE_GLASS: Construction = Construction {
    labels: &[LEFT, RIGHT, GLASS, GAUGE],
    check_topology,
    generate,
    check_metadata,
    feasible: None,
};

/// The contract of `quip/ice-glass@1`.
pub const ICE_GLASS_V1: Contract = Contract {
    family_id: "quip/ice-glass",
    revision: 1,
    schema: ICE_GLASS_SCHEMA,
    metadata_kinds: &[KIND_TILES, KIND_GAUGE],
    limits: LIMITS,
    size_bounds: ice_glass_size_bounds,
    construction: Some(&ICE_GLASS),
};

/// The sizes of an instance: the topology itself.
fn ice_glass_size_bounds(_: &Params<'_>, stats: &TopologyStats) -> Result<SizeBounds, Code> {
    topology_bounds(stats)
}

/// The glass magnitude, `None` when the parameter is missing or out of range.
fn glass_milli(params: &Params<'_>) -> Option<i32> {
    match params.get(TAG_GLASS) {
        Some(Value::Unsigned(glass)) => i32::try_from(glass).ok(),
        _ => None,
    }
}

/// Admission: the parameters name a lattice and the topology carries tiles.
/// A degree above the limit is left to the size bounds (code 13).
fn check_topology(params: &Params<'_>, topology: &BaseTopology) -> bool {
    let Ok(lattice) = lattice(params) else {
        return false;
    };
    match candidates(lattice, topology) {
        Ok(_) | Err(StructureError::DegreeAboveLimit) => true,
        Err(_) => false,
    }
}

/// The sign of the gauge at `pos`: `1` or `-1`.
fn gauge_at(gauge: &[i64], pos: u32) -> Result<i64, Code> {
    usize::try_from(pos)
        .ok()
        .and_then(|pos| gauge.get(pos))
        .copied()
        .ok_or(Code::TopologyIncompatible)
}

fn generate(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let base = glass_base(
        params,
        topology,
        streams,
        &GlassStreams {
            left: LEFT,
            right: RIGHT,
            glass: GLASS,
            gauge: GAUGE,
        },
    )?;
    let metadata = metadata_region(&base.tiles, &base.gauge)?;
    Ok(InstanceBody {
        h: vec![0; base.node_ids.len()],
        node_ids: base.node_ids,
        edges: base.edges,
        j: base.j,
        offset_milli: 0,
        metadata,
    })
}

/// The labels of the four glass streams. Each family that builds on the glass
/// registers its own labels.
pub(crate) struct GlassStreams<'a> {
    /// Orders the left spine edges.
    pub(crate) left: &'a str,
    /// Orders the right spine edges.
    pub(crate) right: &'a str,
    /// Draws the glass signs.
    pub(crate) glass: &'a str,
    /// Draws the gauge.
    pub(crate) gauge: &'a str,
}

/// The globally gauged glass: steps 1 to 5 of the ice-glass construction.
pub(crate) struct GlassBase {
    /// The selected tiles.
    pub(crate) tiles: Vec<Tile>,
    /// Node ids in output order.
    pub(crate) node_ids: Vec<u32>,
    /// Edges as dense positions, sorted ascending.
    pub(crate) edges: Vec<(u32, u32)>,
    /// The gauged couplings, in milli.
    pub(crate) j: Vec<i32>,
    /// The gauge sign of every node.
    pub(crate) gauge: Vec<i64>,
}

/// Select the tiles, give the tile edges the strong coupling and every other
/// edge a glass coupling, then multiply a global gauge into every coupling.
pub(crate) fn glass_base(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
    labels: &GlassStreams<'_>,
) -> Result<GlassBase, Code> {
    let defect = Code::TopologyIncompatible;
    let glass_value = glass_milli(params).ok_or(Code::ParamsMalformed)?;
    let lattice = lattice(params).map_err(|_| defect)?;
    let tiles = select_tiles(lattice, topology, streams, labels.left, labels.right)?;

    let n = u32::try_from(topology.node_ids().len()).map_err(|_| Code::BoundsExceeded)?;
    let node_ids = topology.node_ids().to_vec();
    let edges = sorted_edges(topology);
    let strong: BTreeSet<(u32, u32)> = tiles
        .iter()
        .flat_map(|tile| tile_edges(tile.nodes))
        .collect();

    let two = NonZeroU32::MIN.saturating_add(1);
    let mut glass = streams.open(labels.glass).ok_or(Code::NoInstance)?;
    let mut j: Vec<i32> = Vec::with_capacity(edges.len());
    for edge in &edges {
        if strong.contains(edge) {
            j.push(STRONG_MILLI);
        } else if uniform(&mut glass, two) == 0 {
            j.push(glass_value);
        } else {
            j.push(-glass_value);
        }
    }

    let mut gauge_stream = streams.open(labels.gauge).ok_or(Code::NoInstance)?;
    let gauge: Vec<i64> = (0..n)
        .map(|_| {
            if uniform(&mut gauge_stream, two) == 0 {
                1
            } else {
                -1
            }
        })
        .collect();
    for (&(u, v), coupling) in edges.iter().zip(j.iter_mut()) {
        let sign = gauge_at(&gauge, u)?
            .checked_mul(gauge_at(&gauge, v)?)
            .ok_or(Code::BoundsExceeded)?;
        let signed = i64::from(*coupling)
            .checked_mul(sign)
            .ok_or(Code::BoundsExceeded)?;
        *coupling = i32::try_from(signed).map_err(|_| Code::BoundsExceeded)?;
    }
    Ok(GlassBase {
        tiles,
        node_ids,
        edges,
        j,
        gauge,
    })
}

/// The metadata region: tiles (kind 1) then gauge (kind 2).
pub(crate) fn metadata_region(tiles: &[Tile], gauge: &[i64]) -> Result<Vec<u8>, Code> {
    let mut gauge_payload = u32::try_from(gauge.len())
        .map_err(|_| Code::BoundsExceeded)?
        .to_le_bytes()
        .to_vec();
    gauge_payload.extend(gauge.iter().map(|&g| u8::from(g < 0)));
    region(&[
        (KIND_TILES, tiles_payload(tiles)?),
        (KIND_GAUGE, gauge_payload),
    ])
}

/// The gauge of a kind 2 payload: one sign per node.
fn parse_gauge(payload: &[u8], n: usize) -> Option<Vec<i64>> {
    let mut reader = Reader::new(payload);
    if usize::try_from(reader.u32()?).ok()? != n || reader.rest().len() != n {
        return None;
    }
    reader
        .rest()
        .iter()
        .map(|byte| match byte {
            0 => Some(1),
            1 => Some(-1),
            _ => None,
        })
        .collect()
}

fn check_metadata(params: &Params<'_>, instance: &Instance) -> bool {
    check_instance(params, instance).is_some()
}

/// `Some` when the instance is a valid ice-glass instance.
fn check_instance(params: &Params<'_>, instance: &Instance) -> Option<()> {
    let all_zero = instance.h().iter().all(|&h| h == 0) && instance.offset_milli() == 0;
    (check_glass(params, instance).is_some() && all_zero).then_some(())
}

/// The tiles of an instance whose layout, tile list, gauge, and coupling
/// magnitudes are those of a globally gauged glass. `None` otherwise. The
/// fields and the offset are left to the caller.
pub(crate) fn check_glass(params: &Params<'_>, instance: &Instance) -> Option<Vec<[u32; 4]>> {
    let glass = i64::from(glass_milli(params)?);
    let n = instance.node_ids().len();
    let [tile_payload, gauge_payload] = sections(instance.metadata(), [KIND_TILES, KIND_GAUGE])?;
    let tiles = parse_tiles(tile_payload, n)?;
    let gauge = parse_gauge(gauge_payload, n)?;
    let edges = instance.edges();

    let strong = mark_tiles(&tiles, edges, n)?;

    if instance.j().len() != edges.len() {
        return None;
    }
    for ((edge, &coupling), &is_strong) in edges.iter().zip(instance.j()).zip(&strong) {
        let (u, v) = *edge;
        let hidden = i64::from(coupling)
            .checked_mul(*gauge.get(usize::try_from(u).ok()?)?)?
            .checked_mul(*gauge.get(usize::try_from(v).ok()?)?)?;
        let fits = if is_strong {
            hidden == i64::from(STRONG_MILLI)
        } else {
            hidden == glass || hidden == -glass
        };
        if !fits {
            return None;
        }
    }
    Some(tiles)
}
