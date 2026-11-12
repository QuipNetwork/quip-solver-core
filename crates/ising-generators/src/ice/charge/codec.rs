//! Compact charged-ice wire adapters.
use crate::{
    codec::{
        bits::{BitReader, BitWriter},
        blocks::{Block, BlocksCodec},
        instance::{
            check_bit_array, decode_prefix, encode_prefix, preflight_prefix, read_column,
            write_column,
        },
        CodecError, WitnessCodec,
    },
    descriptor::Params,
    instance::Instance,
};
use alloc::vec::Vec;

pub(crate) fn prepare(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    // The registry has already run the parent's construction validation.
    let metadata = Metadata::parse(instance).map_err(|_| CodecError::InvalidInstance)?;
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(metadata.count)
        .map_err(|_| CodecError::Allocation)?;
    for i in 0..metadata.count {
        let tile = metadata.tile(i)?;
        let mut gauge = [1; 4];
        for (j, g) in gauge.iter_mut().enumerate() {
            *g = if metadata.gauges.get(4 * i + j) == Some(&1) {
                -1
            } else {
                1
            };
        }
        let charge =
            i8::from_le_bytes([*metadata.charges.get(i).ok_or(CodecError::InvalidInstance)?]);
        candidates.push(Block::tile(tile, gauge, charge)?);
    }
    Ok(BlocksCodec::new(instance.node_ids().len(), &candidates)?.into())
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, CodecError> {
    Ok(u32::from_le_bytes(
        data.get(at..at + 4)
            .ok_or(CodecError::Length)?
            .try_into()
            .map_err(|_| CodecError::Length)?,
    ))
}

fn metadata_len(count: usize) -> Result<usize, CodecError> {
    let size = count
        .checked_mul(21)
        .and_then(|n| n.checked_add(32))
        .ok_or(CodecError::BoundsExceeded)?;
    if size > super::LIMITS.max_metadata() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    Ok(size)
}

fn check_tile(
    tile: [u32; 4],
    previous: Option<[u32; 4]>,
    node_count: usize,
) -> Result<(), CodecError> {
    let [a, b, c, d] = tile;
    if a >= b || b >= c || c >= d || d as usize >= node_count || previous.is_some_and(|p| p >= tile)
    {
        return Err(CodecError::InvalidInstance);
    }
    Ok(())
}

struct Metadata<'a> {
    count: usize,
    tiles: &'a [u8],
    gauges: &'a [u8],
    charges: &'a [u8],
}
impl<'a> Metadata<'a> {
    fn parse(instance: &'a Instance) -> Result<Self, CodecError> {
        let invalid = CodecError::UnencodableInstance;
        let [tiles, gauges, charges] =
            super::sections(instance.metadata(), [1, 2, 6]).ok_or(invalid)?;
        let count = u32_at(tiles, 0).map_err(|_| invalid)? as usize;
        let _ = metadata_len(count)?;
        for (payload, width) in [(tiles, 16), (gauges, 4), (charges, 1)] {
            if u32_at(payload, 0).map_err(|_| invalid)? as usize != count
                || payload.len() != 4 + count * width
            {
                return Err(invalid);
            }
        }
        let result = Self {
            count,
            tiles: tiles.get(4..).ok_or(invalid)?,
            gauges: gauges.get(4..).ok_or(invalid)?,
            charges: charges.get(4..).ok_or(invalid)?,
        };
        if result.gauges.iter().any(|&g| g > 1)
            || result.charges.iter().any(|q| ![0, 2, 254].contains(q))
        {
            return Err(invalid);
        }
        let mut previous = None;
        for i in 0..count {
            let tile = result.tile(i)?;
            check_tile(tile, previous, instance.node_ids().len()).map_err(|_| invalid)?;
            previous = Some(tile);
        }
        Ok(result)
    }
    fn tile(&self, i: usize) -> Result<[u32; 4], CodecError> {
        Ok([
            u32_at(self.tiles, 16 * i)?,
            u32_at(self.tiles, 16 * i + 4)?,
            u32_at(self.tiles, 16 * i + 8)?,
            u32_at(self.tiles, 16 * i + 12)?,
        ])
    }
}

pub(crate) fn encode_instance(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    let metadata = Metadata::parse(instance)?;
    let mut positions = Vec::new();
    positions
        .try_reserve_exact(4 * metadata.count)
        .map_err(|_| CodecError::Allocation)?;
    for i in 0..metadata.count {
        positions.extend_from_slice(&metadata.tile(i)?);
    }
    let mut output = encode_prefix(instance)?;
    output
        .try_reserve_exact(5 + 17 * metadata.count)
        .map_err(|_| CodecError::Allocation)?;
    output.extend_from_slice(
        &u32::try_from(metadata.count)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    write_column(&positions, &mut output)?;
    let mut gauges = BitWriter::new(4 * metadata.count)?;
    for &g in metadata.gauges {
        gauges.push(g, 1)?;
    }
    output.extend_from_slice(&gauges.finish()?);
    let mut charges = BitWriter::new(2 * metadata.count)?;
    for q in metadata.charges {
        charges.push(
            match q {
                0 => 0,
                2 => 1,
                254 => 2,
                _ => return Err(CodecError::UnencodableInstance),
            },
            2,
        )?;
    }
    output.extend_from_slice(&charges.finish()?);
    Ok(output)
}

// Borrow and inspect the metadata column before calling the frozen reader, which
// checks minimality too early for the descriptor/structure error priority.
struct Positions<'a> {
    bytes: &'a [u8],
    width: usize,
}
impl Positions<'_> {
    fn get(&self, index: usize) -> Result<u32, CodecError> {
        let mut value = 0;
        for bit in 0..self.width {
            let at = index * self.width + bit;
            value |= u32::from((self.bytes.get(at / 8).ok_or(CodecError::Length)? >> (at % 8)) & 1)
                << bit;
        }
        Ok(value)
    }
    fn tile(&self, i: usize) -> Result<[u32; 4], CodecError> {
        Ok([
            self.get(4 * i)?,
            self.get(4 * i + 1)?,
            self.get(4 * i + 2)?,
            self.get(4 * i + 3)?,
        ])
    }
}

pub(crate) fn decode_instance(
    _params: &Params<'_>,
    descriptor_digest: &[u8; 32],
    bytes: &[u8],
) -> Result<Instance, CodecError> {
    let prefix = preflight_prefix(bytes, &super::LIMITS)?;
    let start = prefix.metadata_start();
    let count = u32_at(bytes, start)? as usize;
    let expanded = metadata_len(count)?;
    let column_start = start + 4;
    let width = usize::from(*bytes.get(column_start).ok_or(CodecError::Length)?);
    if width > 32 {
        return Err(CodecError::InvalidSymbol);
    }
    let bits = 4 * count * width;
    let end = column_start + 1 + bits.div_ceil(8);
    let positions = Positions {
        bytes: bytes.get(column_start + 1..end).ok_or(CodecError::Length)?,
        width,
    };
    check_bit_array(positions.bytes, bits)?;
    let gauge_end = end + (4 * count).div_ceil(8);
    let gauge_bytes = bytes.get(end..gauge_end).ok_or(CodecError::Length)?;
    check_bit_array(gauge_bytes, 4 * count)?;
    let charge_bytes = bytes.get(gauge_end..).ok_or(CodecError::Length)?;
    check_bit_array(charge_bytes, 2 * count)?;
    let mut charges = BitReader::new(charge_bytes, 2 * count)?;
    for _ in 0..count {
        if charges.read(2)? == 3 {
            return Err(CodecError::InvalidSymbol);
        }
    }
    if prefix.descriptor_digest() != descriptor_digest {
        return Err(CodecError::DescriptorMismatch);
    }
    let mut previous = None;
    for i in 0..count {
        let tile = positions.tile(i)?;
        check_tile(tile, previous, prefix.node_count())?;
        previous = Some(tile);
    }
    let mut metadata = Vec::new();
    metadata
        .try_reserve_exact(expanded)
        .map_err(|_| CodecError::Allocation)?;
    metadata.extend_from_slice(&3_u16.to_le_bytes());
    for (kind, width) in [(1_u16, 16), (2, 4), (6, 1)] {
        metadata.extend_from_slice(&kind.to_le_bytes());
        metadata.extend_from_slice(
            &u32::try_from(4 + width * count)
                .map_err(|_| CodecError::BoundsExceeded)?
                .to_le_bytes(),
        );
        metadata.extend_from_slice(
            &u32::try_from(count)
                .map_err(|_| CodecError::BoundsExceeded)?
                .to_le_bytes(),
        );
        match kind {
            1 => {
                for i in 0..4 * count {
                    metadata.extend_from_slice(&positions.get(i)?.to_le_bytes());
                }
            }
            2 => {
                let mut gauges = BitReader::new(gauge_bytes, 4 * count)?;
                for _ in 0..4 * count {
                    metadata.push(gauges.read(1)?);
                }
            }
            6 => {
                let mut charges = BitReader::new(charge_bytes, 2 * count)?;
                for _ in 0..count {
                    metadata.push(match charges.read(2)? {
                        0 => 0,
                        1 => 2,
                        2 => 254,
                        _ => return Err(CodecError::InvalidSymbol),
                    });
                }
            }
            _ => return Err(CodecError::InvalidInstance),
        }
    }
    let instance = decode_prefix(&prefix, metadata, &super::LIMITS, &[1, 2, 6])?;
    let (_, _) = read_column(
        bytes.get(column_start..end).ok_or(CodecError::Length)?,
        4 * count,
    )?;
    Ok(instance)
}
