//! Compact flow cycles and lossless structural instance metadata.
use super::{is_corner, on_border, Config, KIND_FLOW, LIMITS};
use crate::{
    codec::{
        bits::{BitReader, BitWriter},
        instance::{
            check_bit_array, decode_prefix, encode_prefix, preflight_prefix, read_column,
            write_column,
        },
        CodecError, WitnessCodec,
    },
    descriptor::Params,
    ice::build::sections,
    instance::Instance,
    native::{codec::ForestCodec, KIND_CHARGES, KIND_INCIDENCE},
    square::{Boundary, SquareLattice},
};
use alloc::vec::Vec;

pub(crate) fn prepare(
    params: &Params<'_>,
    instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    // Registry preparation already validates the canonical family model once.
    let config = config(params)?;
    let lattice = SquareLattice::new(config.native.side, Boundary::Open)
        .map_err(|_| CodecError::InvalidInstance)?;
    let [charges, incidence, _] = sections(
        instance.metadata(),
        [KIND_CHARGES, KIND_INCIDENCE, KIND_FLOW],
    )
    .ok_or(CodecError::InvalidInstance)?;
    let raw = charges.get(2..).ok_or(CodecError::InvalidInstance)?;
    let mut labels = Vec::new();
    labels
        .try_reserve_exact(raw.len())
        .map_err(|_| CodecError::Allocation)?;
    labels.extend(raw.iter().map(|&q| Some(i8::from_le_bytes([q]))));
    let positions = values(config.native.links(), |i| {
        Ok(u32::from_le_bytes(take(incidence, 4 + 4 * i)?))
    })?;
    Ok(ForestCodec::new(&lattice, &labels, &positions, &[])?.into())
}

fn take<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], CodecError> {
    bytes
        .get(at..at.checked_add(N).ok_or(CodecError::BoundsExceeded)?)
        .ok_or(CodecError::Length)?
        .try_into()
        .map_err(|_| CodecError::Length)
}
fn config(params: &Params<'_>) -> Result<Config, CodecError> {
    Config::of(params).ok_or(CodecError::InvalidInstance)
}
fn boundary(side: u16, vertex: u32) -> bool {
    on_border(side, vertex) && !is_corner(side, vertex)
}
fn origin(config: Config, c0: u16, r0: u16) -> Result<(), CodecError> {
    let grid = config
        .native
        .grid_side()
        .ok_or(CodecError::InvalidInstance)?;
    if c0 == 0
        || r0 == 0
        || c0
            .checked_add(config.native.side)
            .is_none_or(|end| end > grid)
        || r0
            .checked_add(config.native.side)
            .is_none_or(|end| end > grid)
    {
        return Err(CodecError::InvalidInstance);
    }
    Ok(())
}
fn permutation(
    count: usize,
    mut value: impl FnMut(usize) -> Result<u32, CodecError>,
) -> Result<(), CodecError> {
    // The admitted family has at most 8320 links. This fixed bitmap keeps preflight allocation-free.
    let mut seen = [0u64; 130];
    if count > 8320 {
        return Err(CodecError::BoundsExceeded);
    }
    for i in 0..count {
        let p = usize::try_from(value(i)?).map_err(|_| CodecError::BoundsExceeded)?;
        if p >= count {
            return Err(CodecError::InvalidInstance);
        }
        let word = seen.get_mut(p / 64).ok_or(CodecError::InvalidInstance)?;
        let bit = 1u64 << (p % 64);
        if *word & bit != 0 {
            return Err(CodecError::InvalidInstance);
        }
        *word |= bit;
    }
    Ok(())
}
fn values(
    count: usize,
    mut value: impl FnMut(usize) -> Result<u32, CodecError>,
) -> Result<Vec<u32>, CodecError> {
    let mut out = Vec::new();
    out.try_reserve_exact(count)
        .map_err(|_| CodecError::Allocation)?;
    for i in 0..count {
        out.push(value(i)?);
    }
    Ok(out)
}

fn encode_metadata(config: Config, instance: &Instance) -> Result<Vec<u8>, CodecError> {
    let invalid = CodecError::UnencodableInstance;
    let side = config.native.side;
    let n = config.native.links();
    let vertices = (usize::from(side) + 1).pow(2);
    let [charges, incidence, flow] = sections(
        instance.metadata(),
        [KIND_CHARGES, KIND_INCIDENCE, KIND_FLOW],
    )
    .ok_or(invalid)?;
    if instance.node_ids().len() != n
        || charges.len() != 2 + vertices
        || incidence.len() != 4 + 4 * n
        || flow.len() != 8 + 5 * n
        || u16::from_le_bytes(take(charges, 0)?) != side
    {
        return Err(invalid);
    }
    origin(
        config,
        u16::from_le_bytes(take(incidence, 0)?),
        u16::from_le_bytes(take(incidence, 2)?),
    )
    .map_err(|_| invalid)?;
    permutation(n, |i| Ok(u32::from_le_bytes(take(incidence, 4 + 4 * i)?))).map_err(|_| invalid)?;
    let mut signs = BitWriter::new(4 * (usize::from(side) - 1))?;
    for (v, &byte) in (0u32..).zip(charges.get(2..).ok_or(invalid)?) {
        let q = i8::from_le_bytes([byte]);
        if boundary(side, v) {
            if q != -1 && q != 1 {
                return Err(invalid);
            }
            signs.push(u8::from(q == 1), 1)?;
        } else if q != 0 {
            return Err(invalid);
        }
    }
    let mut targets = BitWriter::new(n)?;
    for i in 0..n {
        let t = *flow.get(8 + 5 * i).ok_or(invalid)?;
        if t != 1 && t != 255 {
            return Err(invalid);
        }
        targets.push(u8::from(t == 1), 1)?;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(instance.metadata().len())
        .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(incidence.get(..4).ok_or(invalid)?);
    out.extend_from_slice(&signs.finish()?);
    write_column(
        &values(n, |i| Ok(u32::from_le_bytes(take(incidence, 4 + 4 * i)?)))?,
        &mut out,
    )?;
    out.extend_from_slice(flow.get(..8).ok_or(invalid)?);
    out.extend_from_slice(&targets.finish()?);
    write_column(
        &values(n, |i| Ok(u32::from_le_bytes(take(flow, 9 + 5 * i)?)))?,
        &mut out,
    )?;
    Ok(out)
}
pub(crate) fn encode_instance(
    params: &Params<'_>,
    instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    let metadata = encode_metadata(config(params)?, instance)?;
    let mut out = encode_prefix(instance)?;
    out.try_reserve_exact(metadata.len())
        .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(&metadata);
    Ok(out)
}
fn section(out: &mut Vec<u8>, kind: u16, len: usize) -> Result<(), CodecError> {
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(len)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    Ok(())
}

// The frozen reader checks minimality immediately. Borrow raw columns first so
// descriptor and structural errors retain their specified priority.
fn column_bytes(data: &[u8], count: usize) -> Result<(&[u8], usize), CodecError> {
    let width = usize::from(*data.first().ok_or(CodecError::Length)?);
    if width > 32 {
        return Err(CodecError::InvalidSymbol);
    }
    let bits = count.checked_mul(width).ok_or(CodecError::BoundsExceeded)?;
    let end = 1 + bits.div_ceil(8);
    check_bit_array(data.get(1..end).ok_or(CodecError::Length)?, bits)?;
    Ok((data.get(..end).ok_or(CodecError::Length)?, end))
}
fn column_value(data: &[u8], index: usize) -> Result<u32, CodecError> {
    let width = usize::from(*data.first().ok_or(CodecError::Length)?);
    let mut value = 0;
    for bit in 0..width {
        let at = index * width + bit;
        value |=
            u32::from((data.get(1 + at / 8).ok_or(CodecError::Length)? >> (at % 8)) & 1) << bit;
    }
    Ok(value)
}

pub(crate) fn decode_instance(
    params: &Params<'_>,
    descriptor_digest: &[u8; 32],
    bytes: &[u8],
) -> Result<Instance, CodecError> {
    let prefix = preflight_prefix(bytes, &LIMITS)?;
    let config = config(params)?;
    let side = config.native.side;
    let n = config.native.links();
    let vertices = (usize::from(side) + 1).pow(2);
    let expanded = 34 + vertices + 9 * n;
    if expanded > LIMITS.max_metadata() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    let data = bytes
        .get(prefix.metadata_start()..)
        .ok_or(CodecError::Length)?;
    let c0 = u16::from_le_bytes(take(data, 0)?);
    let r0 = u16::from_le_bytes(take(data, 2)?);
    let bits = 4 * (usize::from(side) - 1);
    let end = 4 + bits.div_ceil(8);
    let signs = data.get(4..end).ok_or(CodecError::Length)?;
    check_bit_array(signs, bits)?;
    let (positions, used) = column_bytes(data.get(end..).ok_or(CodecError::Length)?, n)?;
    let at = end + used;
    let weight: [u8; 8] = take(data, at)?;
    let at = at + 8;
    let end = at + n.div_ceil(8);
    let targets = data.get(at..end).ok_or(CodecError::Length)?;
    check_bit_array(targets, n)?;
    let (costs, used) = column_bytes(data.get(end..).ok_or(CodecError::Length)?, n)?;
    if end + used != data.len() {
        return Err(CodecError::Length);
    }
    if prefix.descriptor_digest() != descriptor_digest {
        return Err(CodecError::DescriptorMismatch);
    }
    if prefix.node_count() != n {
        return Err(CodecError::InvalidInstance);
    }
    origin(config, c0, r0)?;
    permutation(n, |i| column_value(positions, i))?;
    // All metadata lengths, signs, and positions are checked before the first allocation.
    let mut metadata = Vec::new();
    metadata
        .try_reserve_exact(expanded)
        .map_err(|_| CodecError::Allocation)?;
    metadata.extend_from_slice(&3u16.to_le_bytes());
    section(&mut metadata, KIND_CHARGES, 2 + vertices)?;
    metadata.extend_from_slice(&side.to_le_bytes());
    let mut reader = BitReader::new(signs, bits)?;
    for v in 0..u32::try_from(vertices).map_err(|_| CodecError::BoundsExceeded)? {
        metadata.push(if boundary(side, v) {
            if reader.read(1)? == 1 {
                1
            } else {
                255
            }
        } else {
            0
        });
    }
    section(&mut metadata, KIND_INCIDENCE, 4 + 4 * n)?;
    metadata.extend_from_slice(&c0.to_le_bytes());
    metadata.extend_from_slice(&r0.to_le_bytes());
    for i in 0..n {
        metadata.extend_from_slice(&column_value(positions, i)?.to_le_bytes());
    }
    section(&mut metadata, KIND_FLOW, 8 + 5 * n)?;
    metadata.extend_from_slice(&weight);
    let mut reader = BitReader::new(targets, n)?;
    for i in 0..n {
        metadata.push(if reader.read(1)? == 1 { 1 } else { 255 });
        metadata.extend_from_slice(&column_value(costs, i)?.to_le_bytes());
    }
    let instance = decode_prefix(
        &prefix,
        metadata,
        &LIMITS,
        &[KIND_CHARGES, KIND_INCIDENCE, KIND_FLOW],
    )?;
    let _ = read_column(positions, n)?;
    let _ = read_column(costs, n)?;
    Ok(instance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inconsistent_root_cannot_decode() {
        let lattice = SquareLattice::new(2, Boundary::Open).expect("lattice");
        let spins = alloc::vec![1; 12];
        let labels = crate::square::charges(&lattice, &spins).expect("charges");
        let mut labels: Vec<Option<i8>> = labels.into_iter().map(Some).collect();
        let positions: Vec<u32> = (0..12).collect();
        let codec = ForestCodec::new(&lattice, &labels, &positions, &[]).expect("forest");
        let payload = codec.encode(&spins).expect("consistent root");
        *labels.first_mut().expect("root") = Some(0);
        let inconsistent = ForestCodec::new(&lattice, &labels, &positions, &[]).expect("forest");
        assert_eq!(inconsistent.decode(&payload), Err(CodecError::Infeasible));
    }
}
