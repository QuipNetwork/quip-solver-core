//! Lossless polariton instance metadata and forest witness adapter.
use super::{parse, Config, LIMITS, POLARITON_V1};
use crate::{
    codec::{
        bits::{BitReader, BitWriter},
        filled,
        instance::{
            decode_prefix, encode_prefix, preflight_prefix, read_column, write_column, Column,
        },
        CodecError, WitnessCodec,
    },
    descriptor::Params,
    instance::Instance,
    native::codec::ForestCodec,
};
use alloc::vec::Vec;

pub(crate) fn prepare(
    params: &Params<'_>,
    instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    let parsed = parse(params, instance).ok_or(CodecError::InvalidInstance)?;
    Ok(ForestCodec::new(&parsed.lattice, &parsed.labels, &parsed.positions, &[])?.into())
}
fn config(params: &Params<'_>) -> Result<Config, CodecError> {
    Config::of(params)
        .filter(|c| c.fits())
        .ok_or(CodecError::InvalidInstance)
}
fn take<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N], CodecError> {
    data.get(at..at.checked_add(N).ok_or(CodecError::BoundsExceeded)?)
        .ok_or(CodecError::Length)?
        .try_into()
        .map_err(|_| CodecError::Length)
}
fn window(config: Config, c: u16, r: u16) -> Result<(), CodecError> {
    let grid = config
        .native
        .grid_side()
        .ok_or(CodecError::InvalidInstance)?;
    if c == 0
        || r == 0
        || c.checked_add(config.native.side).is_none_or(|v| v > grid)
        || r.checked_add(config.native.side).is_none_or(|v| v > grid)
    {
        return Err(CodecError::InvalidInstance);
    }
    Ok(())
}
fn sizes(config: Config) -> Result<(usize, usize, usize), CodecError> {
    let width = usize::from(config.native.side) + 1;
    let vertices = width.checked_mul(width).ok_or(CodecError::BoundsExceeded)?;
    let metadata = config
        .native
        .links()
        .checked_mul(4)
        .and_then(|n| n.checked_add(vertices))
        .and_then(|n| n.checked_add(20))
        .ok_or(CodecError::BoundsExceeded)?;
    if metadata > LIMITS.max_metadata() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    Ok((width, vertices, metadata))
}
fn interior(v: usize, width: usize) -> bool {
    !v.is_multiple_of(width) && v % width != width - 1 && v / width != 0 && v / width != width - 1
}
fn encode_metadata(config: Config, instance: &Instance) -> Result<Vec<u8>, CodecError> {
    let invalid = CodecError::UnencodableInstance;
    let (width, vertices, _) = sizes(config)?;
    if instance.node_ids().len() != config.native.links() {
        return Err(invalid);
    }
    let [charges, incidence] = super::sections(instance.metadata(), [4, 5]).ok_or(invalid)?;
    if charges.len() != vertices + 2
        || u16::from_le_bytes(take(charges, 0)?) != config.native.side
        || incidence.len() != 4 + 4 * config.native.links()
    {
        return Err(invalid);
    }
    window(
        config,
        u16::from_le_bytes(take(incidence, 0)?),
        u16::from_le_bytes(take(incidence, 2)?),
    )
    .map_err(|_| invalid)?;
    let mut writer = BitWriter::new(2 * config.native.interior())?;
    for (v, &label) in charges.get(2..).ok_or(invalid)?.iter().enumerate() {
        if interior(v, width) {
            let rank = match label {
                254 => 0,
                0 => 1,
                2 => 2,
                _ => return Err(invalid),
            };
            writer.push(rank, 2)?;
        } else if label != 127 {
            return Err(invalid);
        }
    }
    let mut positions = filled(config.native.links(), 0)?;
    let mut seen = filled(config.native.links(), false)?;
    for (i, p) in positions.iter_mut().enumerate() {
        *p = u32::from_le_bytes(take(incidence, 4 + 4 * i)?);
        let slot = seen.get_mut(*p as usize).ok_or(invalid)?;
        if core::mem::replace(slot, true) {
            return Err(invalid);
        }
    }
    let labels = writer.finish()?;
    let mut out = Vec::new();
    out.try_reserve(4 + labels.len())
        .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(incidence.get(..4).ok_or(invalid)?);
    out.extend_from_slice(&labels);
    write_column(&positions, &mut out)?;
    Ok(out)
}
pub(crate) fn encode_instance(
    params: &Params<'_>,
    instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    let config = config(params).map_err(|_| CodecError::UnencodableInstance)?;
    let metadata = encode_metadata(config, instance)?;
    let mut out = encode_prefix(instance)?;
    out.try_reserve(metadata.len())
        .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(&metadata);
    Ok(out)
}
pub(crate) fn decode_instance(
    params: &Params<'_>,
    descriptor_digest: &[u8; 32],
    bytes: &[u8],
) -> Result<Instance, CodecError> {
    let prefix = preflight_prefix(bytes, &LIMITS)?;
    let config = config(params)?;
    let _ = sizes(config)?;
    let data = bytes
        .get(prefix.metadata_start()..)
        .ok_or(CodecError::Length)?;
    let c = u16::from_le_bytes(take(data, 0)?);
    let r = u16::from_le_bytes(take(data, 2)?);
    let bits = 2 * config.native.interior();
    let end = 4 + bits.div_ceil(8);
    let mut reader = BitReader::new(data.get(4..end).ok_or(CodecError::Length)?, bits)?;
    for _ in 0..config.native.interior() {
        if reader.read(2)? == 3 {
            return Err(CodecError::InvalidSymbol);
        }
    }
    let (positions, used) = read_column(
        data.get(end..).ok_or(CodecError::Length)?,
        config.native.links(),
    )?;
    if end + used != data.len() {
        return Err(CodecError::Length);
    }
    if prefix.descriptor_digest() != descriptor_digest {
        return Err(CodecError::DescriptorMismatch);
    }
    if prefix.node_count() != config.native.links() {
        return Err(CodecError::InvalidInstance);
    }
    window(config, c, r)?;
    let mut seen = filled(config.native.links(), false)?;
    for i in 0..config.native.links() {
        let position = positions.get(i)?;
        let slot = seen
            .get_mut(position as usize)
            .ok_or(CodecError::InvalidInstance)?;
        if core::mem::replace(slot, true) {
            return Err(CodecError::InvalidInstance);
        }
    }
    let metadata = rebuild_metadata(
        config,
        data.get(4..end).ok_or(CodecError::Length)?,
        c,
        r,
        &positions,
    )?;
    let instance = decode_prefix(&prefix, metadata, &LIMITS, POLARITON_V1.metadata_kinds)?;
    positions.minimal()?;
    Ok(instance)
}
fn rebuild_metadata(
    config: Config,
    labels: &[u8],
    c: u16,
    r: u16,
    positions: &Column<'_>,
) -> Result<Vec<u8>, CodecError> {
    let (width, vertices, metadata_size) = sizes(config)?;
    let mut metadata = Vec::new();
    metadata
        .try_reserve_exact(metadata_size)
        .map_err(|_| CodecError::Allocation)?;
    metadata.extend_from_slice(&[2, 0, 4, 0]);
    metadata.extend_from_slice(
        &u32::try_from(vertices + 2)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    metadata.extend_from_slice(&config.native.side.to_le_bytes());
    let mut reader = BitReader::new(labels, 2 * config.native.interior())?;
    for v in 0..vertices {
        metadata.push(if interior(v, width) {
            match reader.read(2)? {
                0 => 254,
                1 => 0,
                2 => 2,
                _ => return Err(CodecError::InvalidSymbol),
            }
        } else {
            127
        });
    }
    metadata.extend_from_slice(&[5, 0]);
    metadata.extend_from_slice(
        &u32::try_from(4 + 4 * config.native.links())
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    metadata.extend_from_slice(&c.to_le_bytes());
    metadata.extend_from_slice(&r.to_le_bytes());
    for i in 0..config.native.links() {
        metadata.extend_from_slice(&positions.get(i)?.to_le_bytes());
    }
    Ok(metadata)
}
