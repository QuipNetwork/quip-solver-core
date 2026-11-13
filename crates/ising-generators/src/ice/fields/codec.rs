//! Compact fields witnesses and structurally lossless instances.
use crate::{
    codec::{
        blocks::{blocks, BlocksCodec},
        instance::{
            check_bit_array, decode_prefix, encode_prefix, preflight_prefix, read_column,
            write_column,
        },
        CodecError, WitnessCodec,
    },
    descriptor::Params,
    ice::build::{sections, Reader},
    instance::Instance,
};
use alloc::vec::Vec;

pub(crate) fn prepare(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    // The registry validates the fields construction before this adapter runs.
    Ok(BlocksCodec::new(instance.node_ids().len(), &blocks(instance)?)?.into())
}

fn metadata_len(n: usize, tiles: usize) -> Result<usize, CodecError> {
    let len = tiles
        .checked_mul(16)
        .and_then(|v| v.checked_add(n))
        .and_then(|v| v.checked_add(22))
        .ok_or(CodecError::BoundsExceeded)?;
    if len > super::LIMITS.max_metadata() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    Ok(len)
}

fn check_tiles(
    count: usize,
    n: usize,
    mut get: impl FnMut(usize) -> Result<u32, CodecError>,
) -> Result<(), CodecError> {
    let mut previous = None;
    for i in 0..count {
        let mut tile = [0; 4];
        for (j, p) in tile.iter_mut().enumerate() {
            *p = get(4 * i + j)?;
            if *p as usize >= n {
                return Err(CodecError::InvalidInstance);
            }
        }
        if tile.windows(2).any(|w| w.first() >= w.get(1)) || previous.is_some_and(|p| p >= tile) {
            return Err(CodecError::InvalidInstance);
        }
        previous = Some(tile);
    }
    Ok(())
}

pub(crate) fn encode_instance(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    let invalid = CodecError::UnencodableInstance;
    let [tiles, gauge] = sections(instance.metadata(), [1, 2]).ok_or(invalid)?;
    let mut tiles = Reader::new(tiles);
    let count = tiles.u32().ok_or(invalid)? as usize;
    let n = instance.node_ids().len();
    let _ = metadata_len(n, count)?;
    let positions = tiles.rest();
    let mut gauge = Reader::new(gauge);
    if positions.len() != count * 16
        || gauge.u32().ok_or(invalid)? as usize != n
        || gauge.rest().len() != n
        || gauge.rest().iter().any(|&g| g > 1)
    {
        return Err(invalid);
    }
    let get = |i: usize| -> Result<u32, CodecError> {
        Ok(u32::from_le_bytes(
            positions
                .get(i * 4..i * 4 + 4)
                .ok_or(invalid)?
                .try_into()
                .map_err(|_| invalid)?,
        ))
    };
    check_tiles(count, n, get).map_err(|_| invalid)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(count * 4)
        .map_err(|_| CodecError::Allocation)?;
    for i in 0..count * 4 {
        values.push(get(i)?);
    }
    let mut out = encode_prefix(instance)?;
    out.try_reserve_exact(4)
        .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(
        &u32::try_from(count)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    write_column(&values, &mut out)?;
    let start = out.len();
    let bytes = n.div_ceil(8);
    out.try_reserve_exact(bytes)
        .map_err(|_| CodecError::Allocation)?;
    out.resize(start + bytes, 0);
    for (i, &g) in gauge.rest().iter().enumerate() {
        *out.get_mut(start + i / 8).ok_or(invalid)? |= g << (i % 8);
    }
    Ok(out)
}

pub(crate) fn decode_instance(
    _params: &Params<'_>,
    descriptor_digest: &[u8; 32],
    bytes: &[u8],
) -> Result<Instance, CodecError> {
    let prefix = preflight_prefix(bytes, &super::LIMITS)?;
    let n = prefix.node_count();
    let data = bytes
        .get(prefix.metadata_start()..)
        .ok_or(CodecError::Length)?;
    let count = u32::from_le_bytes(
        data.get(..4)
            .ok_or(CodecError::Length)?
            .try_into()
            .map_err(|_| CodecError::Length)?,
    ) as usize;
    let len = metadata_len(n, count)?;
    let column = data.get(4..).ok_or(CodecError::Length)?;
    let (positions, used) = read_column(column, count * 4)?;
    let gauge = column.get(used..).ok_or(CodecError::Length)?;
    check_bit_array(gauge, n)?;
    if prefix.descriptor_digest() != descriptor_digest {
        return Err(CodecError::DescriptorMismatch);
    }
    check_tiles(count, n, |i| positions.get(i))?;
    let mut metadata = Vec::new();
    metadata
        .try_reserve_exact(len)
        .map_err(|_| CodecError::Allocation)?;
    metadata.extend_from_slice(&2u16.to_le_bytes());
    metadata.extend_from_slice(&1u16.to_le_bytes());
    metadata.extend_from_slice(
        &u32::try_from(4 + count * 16)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    metadata.extend_from_slice(
        &u32::try_from(count)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    for i in 0..count * 4 {
        let position = positions.get(i)?;
        metadata.extend_from_slice(&position.to_le_bytes());
    }
    metadata.extend_from_slice(&2u16.to_le_bytes());
    metadata.extend_from_slice(
        &u32::try_from(4 + n)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    metadata.extend_from_slice(
        &u32::try_from(n)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    for i in 0..n {
        metadata.push((gauge.get(i / 8).ok_or(CodecError::Length)? >> (i % 8)) & 1);
    }
    let instance = decode_prefix(&prefix, metadata, &super::LIMITS, &[1, 2])?;
    positions.minimal()?;
    Ok(instance)
}
