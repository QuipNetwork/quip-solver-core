//! The `quip/ice-fields` family, revision 1: the ice-glass with a sparse
//! random field on spins that sit in two tiles.
//!
//! The base is the globally gauged glass of `quip/ice-glass@1`. The field
//! lands on `n / field_divisor` of the nodes that are in exactly two selected
//! tiles, with a drawn sign times the gauge. The metadata is the ice-glass
//! metadata. A verifier cannot check the field sign, because a drawn sign
//! times the gauge can be either sign.

use alloc::vec;
use alloc::vec::Vec;
use core::num::NonZeroU32;

use crate::descriptor::{Contract, FieldSpec, Params, SizeBounds, TopologyStats, Value, ValueType};
use crate::family::{BaseTopology, Construction, InstanceBody, Streams};
use crate::ice::build::lattice;
use crate::ice::glass::{check_glass, glass_base, metadata_region, GlassStreams};
use crate::ice::topology::two_slot_nodes;
use crate::ice::twisted_dimer::Recipe;
use crate::ice::{candidates, StructureError};
use crate::instance::{Instance, InstanceLimits};
use crate::rng::{shuffle, uniform};
use crate::Code;

/// The parameters of `quip/ice-fields@1`.
pub const ICE_FIELDS_SCHEMA: &[FieldSpec] = &[
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
    FieldSpec {
        tag: 4,
        kind: ValueType::U16,
        min: 1,
        max: 1000,
    },
    FieldSpec {
        tag: 5,
        kind: ValueType::U16,
        min: 1,
        max: 65535,
    },
];

/// Tag of the field magnitude parameter.
const TAG_FIELD: u16 = 4;
/// Tag of the field divisor parameter.
const TAG_DIVISOR: u16 = 5;
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
/// The stream that shuffles the eligible nodes.
const FIELD: &str = "field";
/// The stream that draws the field signs.
const FIELD_SIGN: &str = "field_sign";

const LIMITS: InstanceLimits = match InstanceLimits::new(131_072, 2_097_152, 1_000, 0, 8_388_608) {
    Some(limits) => limits,
    None => InstanceLimits::FAM_37,
};

/// The construction of `quip/ice-fields@1`. Its streams are `left`, `right`,
/// `glass`, `gauge`, `field`, and `field_sign`.
pub static ICE_FIELDS: Construction = Construction {
    labels: &[LEFT, RIGHT, GLASS, GAUGE, FIELD, FIELD_SIGN],
    check_topology,
    generate,
    check_metadata,
    feasible: None,
};

/// The contract of `quip/ice-fields@1`.
pub const ICE_FIELDS_V1: Contract = Contract {
    family_id: "quip/ice-fields",
    revision: 1,
    schema: ICE_FIELDS_SCHEMA,
    metadata_kinds: &[KIND_TILES, KIND_GAUGE],
    limits: LIMITS,
    size_bounds: ice_fields_size_bounds,
    construction: Some(&ICE_FIELDS),
};

/// Canonical parameters of `h500`: `zephyr_m` 12, `zephyr_t` 4,
/// `glass_milli` 125 (0x7d), `field_milli` 500 (0x01f4), `field_divisor` 16.
const PARAMS_H500: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 12, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 0x7d, 0x00, 4, 0, 2, 0,
    0, 0, 0xf4, 0x01, 5, 0, 2, 0, 0, 0, 16, 0,
];

/// Canonical parameters of `h1000`: as `h500` with `field_milli` 1000
/// (0x03e8).
const PARAMS_H1000: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 12, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 0x7d, 0x00, 4, 0, 2, 0,
    0, 0, 0xe8, 0x03, 5, 0, 2, 0, 0, 0, 16, 0,
];

/// The two registered parameter sets, both on Zephyr z(12, 4).
pub const ICE_FIELDS_RECIPES: [Recipe; 2] = [
    Recipe {
        alias: "h500",
        cell: "ice-fields-h500",
        params: PARAMS_H500,
    },
    Recipe {
        alias: "h1000",
        cell: "ice-fields-h1000",
        params: PARAMS_H1000,
    },
];

/// The sizes of an instance: the topology itself.
fn ice_fields_size_bounds(_: &Params<'_>, stats: &TopologyStats) -> Result<SizeBounds, Code> {
    crate::ice::build::topology_bounds(stats)
}

/// An unsigned parameter, `None` when it is missing or out of range.
fn unsigned(params: &Params<'_>, tag: u16) -> Option<u32> {
    match params.get(tag) {
        Some(Value::Unsigned(value)) => u32::try_from(value).ok(),
        _ => None,
    }
}

/// The number of field nodes: `n / field_divisor`.
fn field_count(params: &Params<'_>, n: usize) -> Option<usize> {
    let divisor = usize::try_from(unsigned(params, TAG_DIVISOR)?).ok()?;
    n.checked_div(divisor)
}

/// Admission: the parameters name a lattice, the topology carries tiles, and
/// the field count is within the nodes that can hold two tile slots. A degree
/// above the limit is left to the size bounds (code 13).
fn check_topology(params: &Params<'_>, topology: &BaseTopology) -> bool {
    let Ok(lattice) = lattice(params) else {
        return false;
    };
    match candidates(lattice, topology) {
        Ok(found) => {
            let Some(count) = field_count(params, topology.node_ids().len()) else {
                return false;
            };
            two_slot_nodes(topology, &found).is_ok_and(|slots| count <= slots)
        }
        Err(StructureError::DegreeAboveLimit) => true,
        Err(_) => false,
    }
}

fn generate(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let field_value = unsigned(params, TAG_FIELD)
        .map(i64::from)
        .ok_or(Code::ParamsMalformed)?;
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
    let n = base.node_ids.len();
    let count = field_count(params, n).ok_or(Code::ParamsMalformed)?;

    let mut memberships = vec![0u8; n];
    for tile in &base.tiles {
        for pos in tile.nodes {
            let slot = usize::try_from(pos)
                .ok()
                .and_then(|pos| memberships.get_mut(pos))
                .ok_or(Code::TopologyIncompatible)?;
            *slot = slot.checked_add(1).ok_or(Code::BoundsExceeded)?;
        }
    }
    let mut eligible: Vec<u32> = Vec::new();
    for (pos, &seen) in (0u32..).zip(&memberships) {
        if seen == 2 {
            eligible.push(pos);
        }
    }
    if eligible.len() < count {
        return Err(Code::NoInstance);
    }

    let mut field = streams.open(FIELD).ok_or(Code::NoInstance)?;
    shuffle(&mut field, &mut eligible)?;
    let mut chosen: Vec<u32> = eligible.into_iter().take(count).collect();
    chosen.sort_unstable();

    let two = NonZeroU32::MIN.saturating_add(1);
    let mut sign_stream = streams.open(FIELD_SIGN).ok_or(Code::NoInstance)?;
    let mut h = vec![0i32; n];
    for pos in chosen {
        let at = usize::try_from(pos).map_err(|_| Code::BoundsExceeded)?;
        let drawn = if uniform(&mut sign_stream, two) == 0 {
            1i64
        } else {
            -1
        };
        let gauge = base
            .gauge
            .get(at)
            .copied()
            .ok_or(Code::TopologyIncompatible)?;
        let value = drawn
            .checked_mul(field_value)
            .and_then(|value| value.checked_mul(gauge))
            .and_then(|value| i32::try_from(value).ok())
            .ok_or(Code::BoundsExceeded)?;
        *h.get_mut(at).ok_or(Code::TopologyIncompatible)? = value;
    }

    let metadata = metadata_region(&base.tiles, &base.gauge)?;
    Ok(InstanceBody {
        node_ids: base.node_ids,
        edges: base.edges,
        h,
        j: base.j,
        offset_milli: 0,
        metadata,
    })
}

fn check_metadata(params: &Params<'_>, instance: &Instance) -> bool {
    check_instance(params, instance).is_some()
}

/// `Some` when the instance is a valid ice-fields instance.
fn check_instance(params: &Params<'_>, instance: &Instance) -> Option<()> {
    let magnitude = i64::from(unsigned(params, TAG_FIELD)?);
    let tiles = check_glass(params, instance)?;
    let n = instance.node_ids().len();
    if instance.offset_milli() != 0 || instance.h().len() != n {
        return None;
    }
    let mut memberships = vec![0u8; n];
    for tile in &tiles {
        for pos in tile {
            let slot = memberships.get_mut(usize::try_from(*pos).ok()?)?;
            *slot = slot.checked_add(1)?;
        }
    }
    let mut fields = 0usize;
    for (&h, &seen) in instance.h().iter().zip(&memberships) {
        if h == 0 {
            continue;
        }
        if seen != 2 || i64::from(h).abs() != magnitude {
            return None;
        }
        fields = fields.checked_add(1)?;
    }
    (fields == field_count(params, n)?).then_some(())
}
