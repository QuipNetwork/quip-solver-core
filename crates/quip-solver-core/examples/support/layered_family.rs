//! The layered test construction shared by the family tests and examples.
//! Not an example target: Cargo discovers only `examples/*.rs` and
//! `examples/*/main.rs`. Every includer must use every `pub(crate)` item.

use core::num::NonZeroU32;

use quip_solver_core::ising_generators::descriptor::{
    FieldSpec, Params, SizeBounds, TopologyStats, Value as ParamValue, ValueType,
};
use quip_solver_core::ising_generators::family::{
    BaseTopology, Construction, InstanceBody, Streams,
};
use quip_solver_core::ising_generators::instance::Instance;
use quip_solver_core::ising_generators::rng::uniform;
use quip_solver_core::ising_generators::Code;

pub(crate) const UNIFORM_FIELD: &[FieldSpec] = &[
    FieldSpec {
        tag: 1,
        kind: ValueType::U16,
        min: 1,
        max: 16,
    },
    FieldSpec {
        tag: 2,
        kind: ValueType::I32,
        min: -4000,
        max: 4000,
    },
];

pub(crate) fn example_size_bounds(
    params: &Params<'_>,
    stats: &TopologyStats,
) -> Result<SizeBounds, Code> {
    let Some(ParamValue::Unsigned(layers)) = params.get(1) else {
        return Err(Code::ParamsMalformed);
    };
    let overflow = || Code::BoundsExceeded;
    let nodes = layers.checked_mul(stats.n_nodes).ok_or_else(overflow)?;
    let edges = layers
        .checked_mul(stats.n_edges)
        .and_then(|e| e.checked_add(layers.saturating_sub(1).checked_mul(stats.n_nodes)?))
        .ok_or_else(overflow)?;
    let degree = stats
        .max_degree
        .checked_add(layers.saturating_sub(1).min(2))
        .ok_or_else(overflow)?;
    Ok(SizeBounds {
        nodes,
        edges,
        degree,
    })
}

pub(crate) static LAYERED: Construction = Construction {
    labels: &["salt", "couplings"],
    check_topology: layered_check_topology,
    generate: layered_generate,
    check_metadata: layered_check_metadata,
};

fn nz(n: u32) -> Result<NonZeroU32, Code> {
    NonZeroU32::new(n).ok_or(Code::BoundsExceeded)
}

fn layered_check_topology(_params: &Params<'_>, topology: &BaseTopology) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    topology
        .edges()
        .iter()
        .all(|&(u, v)| seen.insert((u.min(v), u.max(v))))
}

fn layered_params(params: &Params<'_>) -> Result<(u64, i32), Code> {
    let Some(ParamValue::Unsigned(layers)) = params.get(1) else {
        return Err(Code::ParamsMalformed);
    };
    let Some(ParamValue::Signed(field)) = params.get(2) else {
        return Err(Code::ParamsMalformed);
    };
    let field = i32::try_from(field).map_err(|_| Code::ParamsMalformed)?;
    Ok((layers, field))
}

fn layered_generate(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let overflow = Code::BoundsExceeded;
    let (layers, field) = layered_params(params)?;
    let layers = u32::try_from(layers).map_err(|_| overflow)?;
    let n = u32::try_from(topology.node_ids().len()).map_err(|_| overflow)?;
    let mut salt = streams.open("salt").ok_or(Code::NoInstance)?;
    let w = uniform(&mut salt, nz(8)?);
    if w == 0 {
        return Err(Code::NoInstance);
    }
    if w == 2 {
        let _wide = field.checked_mul(1 << 21).ok_or(overflow)?;
    }
    let scale = if w == 1 { 2 } else { 1 };
    let h_value = field.checked_mul(scale).ok_or(overflow)?;
    let mut node_ids = Vec::new();
    for k in 0..layers {
        let base = k.checked_mul(n).ok_or(overflow)?;
        for pos in 0..n {
            node_ids.push(base.checked_add(pos).ok_or(overflow)?);
        }
    }
    let mut edges = Vec::new();
    for k in 0..layers {
        let base = k.checked_mul(n).ok_or(overflow)?;
        for &(a, b) in topology.edges() {
            edges.push((
                base.checked_add(a.min(b)).ok_or(overflow)?,
                base.checked_add(a.max(b)).ok_or(overflow)?,
            ));
        }
        if k.checked_add(1).ok_or(overflow)? < layers {
            let next = base.checked_add(n).ok_or(overflow)?;
            for pos in 0..n {
                edges.push((
                    base.checked_add(pos).ok_or(overflow)?,
                    next.checked_add(pos).ok_or(overflow)?,
                ));
            }
        }
    }
    edges.sort_unstable();
    let mut couplings = streams.open("couplings").ok_or(Code::NoInstance)?;
    let three = nz(3)?;
    let j = edges
        .iter()
        .map(|_| {
            i32::try_from(uniform(&mut couplings, three))
                .ok()
                .and_then(|d| d.checked_sub(1))
                .and_then(|d| d.checked_mul(1000))
                .ok_or(overflow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let h = vec![h_value; node_ids.len()];
    let offset_milli = 250_i64.checked_mul(i64::from(layers)).ok_or(overflow)?;
    Ok(InstanceBody {
        node_ids,
        edges,
        h,
        j,
        offset_milli,
        metadata: layered_region(layers, n),
    })
}

fn layered_region(layers: u32, n: u32) -> Vec<u8> {
    let mut region = vec![1, 0, 1, 0, 6, 0, 0, 0];
    region.extend_from_slice(&u16::try_from(layers).unwrap_or(u16::MAX).to_le_bytes());
    region.extend_from_slice(&n.to_le_bytes());
    region
}

fn layered_check_metadata(params: &Params<'_>, instance: &Instance) -> bool {
    let Ok((layers, _)) = layered_params(params) else {
        return false;
    };
    let (Ok(layers32), Ok(len)) = (
        u32::try_from(layers),
        u32::try_from(instance.node_ids().len()),
    ) else {
        return false;
    };
    let Some(n) = len.checked_div(layers32) else {
        return false;
    };
    instance.metadata() == layered_region(layers32, n) && n.checked_mul(layers32) == Some(len)
}
