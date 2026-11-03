//! Deliberately faulty layered generators for the instance checker tests.
//! Each one runs [`LAYERED`] first and changes one item. Each one keeps the
//! all-+1 energy of the honest instance, or fails earlier on size, so a
//! reported energy alone cannot expose the fault.

use ising_generators::descriptor::{Contract, Params};
use ising_generators::family::{BaseTopology, Construction, InstanceBody, Streams};
use ising_generators::Code;
use quip_solver_conformance::families::layered::{LAYERED, UNIFORM_V1};

/// Byte offset of the `u16` tiles field `L` in the metadata region.
const TILES_AT: usize = 8;

fn layered_body(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    (LAYERED.generate)(params, topology, streams)
}

fn faulty_j_generate(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let mut body = layered_body(params, topology, streams)?;
    body.j.rotate_left(1);
    Ok(body)
}

fn faulty_metadata_generate(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let mut body = layered_body(params, topology, streams)?;
    let field = body
        .metadata
        .get_mut(TILES_AT..TILES_AT + 2)
        .ok_or(Code::BoundsExceeded)?;
    let tiles = field
        .first_chunk::<2>()
        .map(|bytes| u16::from_le_bytes(*bytes))
        .ok_or(Code::BoundsExceeded)?;
    let bumped = tiles.checked_add(1).ok_or(Code::BoundsExceeded)?;
    field.copy_from_slice(&bumped.to_le_bytes());
    Ok(body)
}

fn faulty_oversize_generate(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let mut body = layered_body(params, topology, streams)?;
    let id = u32::try_from(body.node_ids.len()).map_err(|_| Code::BoundsExceeded)?;
    body.node_ids.push(id);
    body.h.push(0);
    Ok(body)
}

/// The metadata check of a generator that accepts any region.
fn accept_any_metadata(
    _params: &Params<'_>,
    _instance: &ising_generators::instance::Instance,
) -> bool {
    true
}

/// LAYERED, then `j.rotate_left(1)`. The sum of J, and so the all-+1 energy, is unchanged.
pub(crate) static FAULTY_J: Construction = Construction {
    generate: faulty_j_generate,
    ..LAYERED
};

/// LAYERED, then the tiles field `L` in the region becomes `L + 1`. `check_metadata` accepts any region.
pub(crate) static FAULTY_METADATA: Construction = Construction {
    generate: faulty_metadata_generate,
    check_metadata: accept_any_metadata,
    ..LAYERED
};

/// LAYERED, then one more node: id `n_nodes`, `h = 0`, no edge. Over the size bound N.
pub(crate) static FAULTY_OVERSIZE: Construction = Construction {
    generate: faulty_oversize_generate,
    check_metadata: accept_any_metadata,
    ..LAYERED
};

/// [`UNIFORM_V1`] bound to [`FAULTY_J`].
pub(crate) const FAULTY_J_V1: Contract = Contract {
    construction: Some(&FAULTY_J),
    ..UNIFORM_V1
};

/// [`UNIFORM_V1`] bound to [`FAULTY_METADATA`].
pub(crate) const FAULTY_METADATA_V1: Contract = Contract {
    construction: Some(&FAULTY_METADATA),
    ..UNIFORM_V1
};

/// [`UNIFORM_V1`] bound to [`FAULTY_OVERSIZE`].
pub(crate) const FAULTY_OVERSIZE_V1: Contract = Contract {
    construction: Some(&FAULTY_OVERSIZE),
    ..UNIFORM_V1
};
