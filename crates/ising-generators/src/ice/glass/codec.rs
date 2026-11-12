//! Lossless glass metadata and globally gauged tile witnesses.
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
    ice::build::sections,
    instance::Instance,
};
use alloc::vec::Vec;

pub(crate) fn prepare(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    Ok(BlocksCodec::new(instance.node_ids().len(), &blocks(instance)?)?.into())
}

fn word(bytes: &[u8], at: usize) -> Result<u32, CodecError> {
    let end = at.checked_add(4).ok_or(CodecError::BoundsExceeded)?;
    Ok(u32::from_le_bytes(
        bytes
            .get(at..end)
            .ok_or(CodecError::Length)?
            .try_into()
            .map_err(|_| CodecError::Length)?,
    ))
}

fn metadata_len(n: usize, count: usize) -> Result<usize, CodecError> {
    let size = count
        .checked_mul(16)
        .and_then(|s| s.checked_add(n))
        .and_then(|s| s.checked_add(22))
        .ok_or(CodecError::BoundsExceeded)?;
    if size > super::LIMITS.max_metadata() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    Ok(size)
}

pub(crate) fn encode_instance(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    let invalid = CodecError::UnencodableInstance;
    let [tiles, gauge] = sections(instance.metadata(), [1, 2]).ok_or(invalid)?;
    let count = word(tiles, 0).map_err(|_| invalid)? as usize;
    let n = instance.node_ids().len();
    let size = metadata_len(n, count)?;
    if tiles.len() != 4 + count * 16
        || word(gauge, 0).map_err(|_| invalid)? as usize != n
        || gauge.len() != 4 + n
        || instance.metadata().len() != size
    {
        return Err(invalid);
    }
    let gauge = gauge.get(4..).ok_or(invalid)?;
    if gauge.iter().any(|&g| g > 1) {
        return Err(invalid);
    }
    // Validate before reserving the column, including positions in overlapping tiles.
    for i in 0..count * 4 {
        if word(tiles, 4 + 4 * i)? as usize >= n {
            return Err(invalid);
        }
    }
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(count * 4)
        .map_err(|_| CodecError::Allocation)?;
    for i in 0..count * 4 {
        positions.push(word(tiles, 4 + 4 * i)?);
    }
    let mut out = encode_prefix(instance)?;
    out.try_reserve(4).map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(
        &u32::try_from(count)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    write_column(&positions, &mut out)?;
    out.try_reserve(n.div_ceil(8))
        .map_err(|_| CodecError::Allocation)?;
    for chunk in gauge.chunks(8) {
        let mut packed = 0;
        for (i, &sign) in chunk.iter().enumerate() {
            packed |= sign << i;
        }
        out.push(packed);
    }
    Ok(out)
}

// Borrow the column before shared read_column, whose minimality check is eager.
// This keeps descriptor and structural errors ahead of nonminimal widths.
fn position(data: &[u8], index: usize, width: u8) -> Result<u32, CodecError> {
    let mut value = 0;
    for bit in 0..usize::from(width) {
        let at = index * usize::from(width) + bit;
        value |= u32::from((data.get(at / 8).ok_or(CodecError::Length)? >> (at % 8)) & 1) << bit;
    }
    Ok(value)
}

pub(crate) fn decode_instance(
    _params: &Params<'_>,
    descriptor_digest: &[u8; 32],
    bytes: &[u8],
) -> Result<Instance, CodecError> {
    let prefix = preflight_prefix(bytes, &super::LIMITS)?;
    let n = prefix.node_count();
    let at = prefix.metadata_start();
    let count = word(bytes, at)? as usize;
    let size = metadata_len(n, count)?;
    let column = bytes.get(at + 4..).ok_or(CodecError::Length)?;
    let width = *column.first().ok_or(CodecError::Length)?;
    if width > 32 {
        return Err(CodecError::InvalidSymbol);
    }
    let bits = count
        .checked_mul(4)
        .and_then(|c| c.checked_mul(usize::from(width)))
        .ok_or(CodecError::BoundsExceeded)?;
    let end = 1 + bits.div_ceil(8);
    let data = column.get(1..end).ok_or(CodecError::Length)?;
    check_bit_array(data, bits)?;
    let gauge = column.get(end..).ok_or(CodecError::Length)?;
    check_bit_array(gauge, n)?;
    if prefix.descriptor_digest() != descriptor_digest {
        return Err(CodecError::DescriptorMismatch);
    }
    for i in 0..count * 4 {
        if position(data, i, width)? as usize >= n {
            return Err(CodecError::InvalidInstance);
        }
    }
    let mut metadata = Vec::new();
    metadata
        .try_reserve_exact(size)
        .map_err(|_| CodecError::Allocation)?;
    metadata.extend_from_slice(&[2, 0, 1, 0]);
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
        metadata.extend_from_slice(&position(data, i, width)?.to_le_bytes());
    }
    metadata.extend_from_slice(&[2, 0]);
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
    let _ = read_column(column, count * 4)?;
    Ok(instance)
}
