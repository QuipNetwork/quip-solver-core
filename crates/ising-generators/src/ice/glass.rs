//! The `quip/ice-glass` family, revision 1: a spin glass with a hidden ice
//! planted inside it.
//!
//! The construction picks a set of ice tiles, gives every tile edge the strong
//! coupling [`STRONG_MILLI`] and every other edge a random coupling of
//! magnitude `glass_milli`, then hides the structure with a random gauge. The
//! metadata lets a verifier recover the tiles and the gauge.

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;
use core::num::NonZeroU32;

use crate::descriptor::{Contract, FieldSpec, Params, SizeBounds, TopologyStats, Value, ValueType};
use crate::family::{BaseTopology, Construction, InstanceBody, Streams};
use crate::ice::{candidates, select, sides, Lattice, StructureError, MAX_DEGREE};
use crate::instance::{Instance, InstanceLimits};
use crate::rng::{shuffle, uniform};
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

/// Tag of the Zephyr grid size parameter.
const TAG_M: u16 = 1;
/// Tag of the Zephyr tile size parameter.
const TAG_T: u16 = 2;
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
    if stats.max_degree > MAX_DEGREE {
        return Err(Code::BoundsExceeded);
    }
    Ok(SizeBounds {
        nodes: stats.n_nodes,
        edges: stats.n_edges,
        degree: stats.max_degree,
    })
}

/// The lattice the parameters name.
fn lattice(params: &Params<'_>) -> Result<Lattice, StructureError> {
    let m = match params.get(TAG_M) {
        Some(Value::Unsigned(m)) => u16::try_from(m).ok(),
        _ => None,
    };
    let t = match params.get(TAG_T) {
        Some(Value::Unsigned(t)) => u8::try_from(t).ok(),
        _ => None,
    };
    match (m, t) {
        (Some(m), Some(t)) => Lattice::from_shape(m, t),
        _ => Err(StructureError::Overflow),
    }
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

/// A shuffled `0..len`.
fn shuffled_order(streams: &mut Streams<'_>, label: &str, len: usize) -> Result<Vec<u32>, Code> {
    let len = u32::try_from(len).map_err(|_| Code::BoundsExceeded)?;
    let mut order: Vec<u32> = (0..len).collect();
    let mut rng = streams.open(label).ok_or(Code::NoInstance)?;
    shuffle(&mut rng, &mut order)?;
    Ok(order)
}

/// The six edges of a tile, ascending pairs.
const fn tile_edges(nodes: [u32; 4]) -> [(u32, u32); 6] {
    let [a, b, c, d] = nodes;
    [(a, b), (a, c), (a, d), (b, c), (b, d), (c, d)]
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
    let defect = Code::TopologyIncompatible;
    let glass_value = glass_milli(params).ok_or(Code::ParamsMalformed)?;
    let lattice = lattice(params).map_err(|_| defect)?;
    let found = candidates(lattice, topology).map_err(|_| defect)?;
    let split = sides(lattice, topology, &found).map_err(|_| defect)?;
    let left_order = shuffled_order(streams, LEFT, split.left.len())?;
    let right_order = shuffled_order(streams, RIGHT, split.right.len())?;
    let tiles = select(&found, &split, &left_order, &right_order).map_err(|_| defect)?;

    let n = u32::try_from(topology.node_ids().len()).map_err(|_| Code::BoundsExceeded)?;
    let node_ids = topology.node_ids().to_vec();
    let mut edges: Vec<(u32, u32)> = topology
        .edges()
        .iter()
        .map(|&(u, v)| (u.min(v), u.max(v)))
        .collect();
    edges.sort_unstable();
    let strong: BTreeSet<(u32, u32)> = tiles
        .iter()
        .flat_map(|tile| tile_edges(tile.nodes))
        .collect();

    let two = NonZeroU32::MIN.saturating_add(1);
    let mut glass = streams.open(GLASS).ok_or(Code::NoInstance)?;
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

    let mut gauge_stream = streams.open(GAUGE).ok_or(Code::NoInstance)?;
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

    let metadata = metadata_region(&tiles, &gauge)?;
    Ok(InstanceBody {
        node_ids,
        edges,
        h: vec![0; topology.node_ids().len()],
        j,
        offset_milli: 0,
        metadata,
    })
}

/// The metadata region: tiles (kind 1) then gauge (kind 2).
fn metadata_region(tiles: &[crate::ice::Tile], gauge: &[i64]) -> Result<Vec<u8>, Code> {
    let overflow = Code::BoundsExceeded;
    let mut tile_payload = u32::try_from(tiles.len())
        .map_err(|_| overflow)?
        .to_le_bytes()
        .to_vec();
    for tile in tiles {
        for pos in tile.nodes {
            tile_payload.extend_from_slice(&pos.to_le_bytes());
        }
    }
    let mut gauge_payload = u32::try_from(gauge.len())
        .map_err(|_| overflow)?
        .to_le_bytes()
        .to_vec();
    gauge_payload.extend(gauge.iter().map(|&g| u8::from(g < 0)));
    let mut region = 2u16.to_le_bytes().to_vec();
    for (kind, payload) in [(KIND_TILES, tile_payload), (KIND_GAUGE, gauge_payload)] {
        region.extend_from_slice(&kind.to_le_bytes());
        region.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_| overflow)?
                .to_le_bytes(),
        );
        region.extend_from_slice(&payload);
    }
    Ok(region)
}

/// A reader over a byte slice that never panics.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let head = self.bytes.get(..len)?;
        self.bytes = self.bytes.get(len..)?;
        Some(head)
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

/// The two payloads of the region: tiles and gauge. `None` unless the region
/// holds exactly the kinds 1 and 2 in that order.
fn sections(region: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut reader = Reader { bytes: region };
    if reader.u16()? != 2 {
        return None;
    }
    let mut payloads = [&[][..]; 2];
    for (slot, kind) in payloads.iter_mut().zip([KIND_TILES, KIND_GAUGE]) {
        if reader.u16()? != kind {
            return None;
        }
        let len = usize::try_from(reader.u32()?).ok()?;
        *slot = reader.take(len)?;
    }
    reader.bytes.is_empty().then_some(())?;
    let [tiles, gauge] = payloads;
    Some((tiles, gauge))
}

/// The tiles of a kind 1 payload: four positions each, ascending, tiles
/// ascending and distinct.
fn parse_tiles(payload: &[u8], n: usize) -> Option<Vec<[u32; 4]>> {
    let mut reader = Reader { bytes: payload };
    let count = usize::try_from(reader.u32()?).ok()?;
    if reader.bytes.len() != count.checked_mul(16)? {
        return None;
    }
    let mut tiles: Vec<[u32; 4]> = Vec::with_capacity(count);
    for _ in 0..count {
        let tile = [reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?];
        let ascending = tile.windows(2).all(|w| matches!(w, [a, b] if a < b));
        let last = usize::try_from(tile[3]).ok()?;
        if !ascending || last >= n || tiles.last().is_some_and(|prev| *prev >= tile) {
            return None;
        }
        tiles.push(tile);
    }
    Some(tiles)
}

/// The gauge of a kind 2 payload: one sign per node.
fn parse_gauge(payload: &[u8], n: usize) -> Option<Vec<i64>> {
    let mut reader = Reader { bytes: payload };
    if usize::try_from(reader.u32()?).ok()? != n || reader.bytes.len() != n {
        return None;
    }
    reader
        .bytes
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
    let glass = i64::from(glass_milli(params)?);
    let n = instance.node_ids().len();
    let (tile_payload, gauge_payload) = sections(instance.metadata())?;
    let tiles = parse_tiles(tile_payload, n)?;
    let gauge = parse_gauge(gauge_payload, n)?;
    let edges = instance.edges();

    let mut strong = vec![false; edges.len()];
    let mut slots = vec![0u8; n];
    for tile in &tiles {
        for pos in *tile {
            let slot = slots.get_mut(usize::try_from(pos).ok()?)?;
            *slot = slot.checked_add(1).filter(|count| *count <= 2)?;
        }
        for pair in tile_edges(*tile) {
            let at = edges.binary_search(&pair).ok()?;
            let seen = strong.get_mut(at)?;
            if *seen {
                return None;
            }
            *seen = true;
        }
    }

    let all_zero = instance.h().iter().all(|&h| h == 0) && instance.offset_milli() == 0;
    if !all_zero || instance.j().len() != edges.len() {
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
    Some(())
}
