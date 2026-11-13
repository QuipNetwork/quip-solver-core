//! Lossless routing metadata and chain-pair witness codecs.
use super::{KIND_ROUTING, LIMITS};
use crate::{
    codec::{
        bits::{BitReader, BitWriter},
        blocks::{Block, BlocksCodec},
        filled,
        instance::{
            check_bit_array, decode_prefix, encode_prefix, preflight_prefix, read_column,
            write_column, Column,
        },
        CodecError, WitnessCodec,
    },
    descriptor::Params,
    instance::Instance,
};
use alloc::vec::Vec;

fn take<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], CodecError> {
    bytes
        .get(at..at.checked_add(N).ok_or(CodecError::BoundsExceeded)?)
        .ok_or(CodecError::Length)?
        .try_into()
        .map_err(|_| CodecError::Length)
}
fn word(bytes: &[u8], at: usize) -> Result<usize, CodecError> {
    usize::try_from(u32::from_le_bytes(take(bytes, at)?)).map_err(|_| CodecError::BoundsExceeded)
}
fn expanded(cells: usize, arcs: usize) -> Result<usize, CodecError> {
    let size = cells
        .checked_mul(6)
        .and_then(|v| arcs.checked_mul(24).and_then(|a| v.checked_add(a)))
        .and_then(|v| v.checked_add(18))
        .ok_or(CodecError::BoundsExceeded)?;
    if size > LIMITS.max_metadata() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    Ok(size)
}
struct Metadata<'a> {
    cells: &'a [u8],
    arcs: &'a [u8],
    strength: [u8; 2],
}
fn metadata(instance: &Instance) -> Result<Metadata<'_>, CodecError> {
    let bytes = instance.metadata();
    if take::<4>(bytes, 0)? != [1, 0, 7, 0] || word(bytes, 4)? != bytes.len().saturating_sub(8) {
        return Err(CodecError::InvalidInstance);
    }
    let cells = word(bytes, 8)?;
    let _ = expanded(cells, 0)?;
    let end = 12 + cells * 6;
    let cell_bytes = bytes.get(12..end).ok_or(CodecError::Length)?;
    let arcs = word(bytes, end)?;
    let size = expanded(cells, arcs)?;
    if size != bytes.len() {
        return Err(CodecError::Length);
    }
    if arcs.checked_mul(3) != Some(instance.node_ids().len()) {
        return Err(CodecError::InvalidInstance);
    }
    for cell in cell_bytes.chunks_exact(6) {
        for byte in cell.get(4..).ok_or(CodecError::Length)? {
            if ![255, 0, 1].contains(byte) {
                return Err(CodecError::InvalidInstance);
            }
        }
    }
    let arc_bytes = bytes.get(end + 4..size - 2).ok_or(CodecError::Length)?;
    let mut used = filled(instance.node_ids().len(), false)?;
    for arc in arc_bytes.chunks_exact(24) {
        if word(arc, 0)? >= cells || word(arc, 4)? >= cells {
            return Err(CodecError::InvalidInstance);
        }
        for at in [8, 12, 16] {
            let slot = used
                .get_mut(word(arc, at)?)
                .ok_or(CodecError::InvalidInstance)?;
            if *slot {
                return Err(CodecError::InvalidInstance);
            }
            *slot = true;
        }
    }
    Ok(Metadata {
        cells: cell_bytes,
        arcs: arc_bytes,
        strength: take(bytes, size - 2)?,
    })
}
pub(crate) fn prepare(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    let meta = metadata(instance)?;
    let mut pairs = Vec::new();
    pairs
        .try_reserve_exact(meta.arcs.len() / 24)
        .map_err(|_| CodecError::Allocation)?;
    for arc in meta.arcs.chunks_exact(24) {
        pairs.push(Block::pair(
            u32::from_le_bytes(take(arc, 12)?),
            u32::from_le_bytes(take(arc, 16)?),
        ));
    }
    Ok(BlocksCodec::new(instance.node_ids().len(), &pairs)?.into())
}
pub(crate) fn encode_instance(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    let meta = metadata(instance).map_err(|error| match error {
        CodecError::Allocation | CodecError::BoundsExceeded => error,
        _ => CodecError::UnencodableInstance,
    })?;
    let cells = meta.cells.len() / 6;
    let arcs = meta.arcs.len() / 24;
    let mut out = encode_prefix(instance)?;
    out.try_reserve(instance.metadata().len())
        .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(
        &u32::try_from(cells)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    let mut values = filled(cells * 2, 0)?;
    let mut supplies = BitWriter::new(cells * 4)?;
    for (i, cell) in meta.cells.chunks_exact(6).enumerate() {
        for k in 0..2 {
            *values.get_mut(2 * i + k).ok_or(CodecError::Length)? =
                u32::from(u16::from_le_bytes(take(cell, 2 * k)?));
            let sign = i8::from_le_bytes(take(cell, 4 + k)?);
            supplies.push(
                u8::try_from(i16::from(sign) + 1).map_err(|_| CodecError::InvalidInstance)?,
                2,
            )?;
        }
    }
    write_column(&values, &mut out)?;
    out.extend_from_slice(&supplies.finish()?);
    out.extend_from_slice(
        &u32::try_from(arcs)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    let mut positions = filled(arcs * 5, 0)?;
    let mut costs = filled(arcs * 2, 0)?;
    for (i, arc) in meta.arcs.chunks_exact(24).enumerate() {
        for k in 0..5 {
            *positions.get_mut(5 * i + k).ok_or(CodecError::Length)? =
                u32::from_le_bytes(take(arc, 4 * k)?);
        }
        for k in 0..2 {
            *costs.get_mut(2 * i + k).ok_or(CodecError::Length)? =
                u32::from(u16::from_le_bytes(take(arc, 20 + 2 * k)?));
        }
    }
    write_column(&positions, &mut out)?;
    write_column(&costs, &mut out)?;
    out.extend_from_slice(&meta.strength);
    Ok(out)
}

pub(crate) fn decode_instance(
    _params: &Params<'_>,
    descriptor_digest: &[u8; 32],
    bytes: &[u8],
) -> Result<Instance, CodecError> {
    let prefix = preflight_prefix(bytes, &LIMITS)?;
    let mut at = prefix.metadata_start();
    let cells = word(bytes, at)?;
    let _ = expanded(cells, 0)?;
    at += 4;
    let (coordinates, len) = read_column(bytes.get(at..).ok_or(CodecError::Length)?, cells * 2)?;
    at += len;
    let supply_len = (cells * 4).div_ceil(8);
    let supply = bytes.get(at..at + supply_len).ok_or(CodecError::Length)?;
    check_bit_array(supply, cells * 4)?;
    let mut ranks = BitReader::new(supply, cells * 4)?;
    for _ in 0..cells * 2 {
        if ranks.read(2)? == 3 {
            return Err(CodecError::InvalidSymbol);
        }
    }
    at += supply_len;
    let arcs = word(bytes, at)?;
    let size = expanded(cells, arcs)?;
    if arcs > LIMITS.max_nodes() as usize / 3 {
        return Err(CodecError::BoundsExceeded);
    }
    at += 4;
    let (positions, len) = read_column(bytes.get(at..).ok_or(CodecError::Length)?, arcs * 5)?;
    at += len;
    let (costs, len) = read_column(bytes.get(at..).ok_or(CodecError::Length)?, arcs * 2)?;
    at += len;
    let strength = take::<2>(bytes, at)?;
    if at + 2 != bytes.len() {
        return Err(CodecError::Length);
    }
    if prefix.descriptor_digest() != descriptor_digest {
        return Err(CodecError::DescriptorMismatch);
    }
    validate_columns(
        cells,
        arcs,
        prefix.node_count(),
        &coordinates,
        &positions,
        &costs,
    )?;
    let mut metadata = Vec::new();
    metadata
        .try_reserve_exact(size)
        .map_err(|_| CodecError::Allocation)?;
    metadata.extend_from_slice(&[1, 0]);
    metadata.extend_from_slice(&KIND_ROUTING.to_le_bytes());
    metadata.extend_from_slice(
        &u32::try_from(size - 8)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    metadata.extend_from_slice(
        &u32::try_from(cells)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    let mut ranks = BitReader::new(supply, cells * 4)?;
    for i in 0..cells {
        for k in 0..2 {
            metadata.extend_from_slice(
                &u16::try_from(coordinates.get(2 * i + k)?)
                    .map_err(|_| CodecError::InvalidInstance)?
                    .to_le_bytes(),
            );
        }
        for _ in 0..2 {
            metadata.push(
                i8::try_from(i16::from(ranks.read(2)?) - 1)
                    .map_err(|_| CodecError::InvalidSymbol)?
                    .to_le_bytes()[0],
            );
        }
    }
    metadata.extend_from_slice(
        &u32::try_from(arcs)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    for i in 0..arcs {
        for k in 0..5 {
            metadata.extend_from_slice(&positions.get(5 * i + k)?.to_le_bytes());
        }
        for k in 0..2 {
            metadata.extend_from_slice(
                &u16::try_from(costs.get(2 * i + k)?)
                    .map_err(|_| CodecError::InvalidInstance)?
                    .to_le_bytes(),
            );
        }
    }
    metadata.extend_from_slice(&strength);
    let instance = decode_prefix(&prefix, metadata, &LIMITS, &[KIND_ROUTING])?;
    coordinates.minimal()?;
    positions.minimal()?;
    costs.minimal()?;
    Ok(instance)
}

fn validate_columns(
    cells: usize,
    arcs: usize,
    nodes: usize,
    coordinates: &Column<'_>,
    positions: &Column<'_>,
    costs: &Column<'_>,
) -> Result<(), CodecError> {
    if arcs * 3 != nodes {
        return Err(CodecError::InvalidInstance);
    }
    for i in 0..cells * 2 {
        if coordinates.get(i)? > u32::from(u16::MAX) {
            return Err(CodecError::InvalidInstance);
        }
    }
    for i in 0..arcs * 2 {
        if costs.get(i)? > u32::from(u16::MAX) {
            return Err(CodecError::InvalidInstance);
        }
    }
    let mut used = filled(nodes, false)?;
    for i in 0..arcs {
        for k in 0..5 {
            let value = positions.get(i * 5 + k)? as usize;
            if k < 2 {
                if value >= cells {
                    return Err(CodecError::InvalidInstance);
                }
            } else {
                let slot = used.get_mut(value).ok_or(CodecError::InvalidInstance)?;
                if *slot {
                    return Err(CodecError::InvalidInstance);
                }
                *slot = true;
            }
        }
    }
    Ok(())
}
