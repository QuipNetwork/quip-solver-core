//! Lossless twisted metadata columns and pair-first soft witness blocks.
use super::{sections, KIND_GAUGES, KIND_PAIRS, KIND_TILES, LIMITS};
use crate::{
    codec::{
        blocks::{Block, BlocksCodec},
        filled,
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

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, CodecError> {
    let end = at.checked_add(4).ok_or(CodecError::BoundsExceeded)?;
    Ok(u32::from_le_bytes(
        bytes
            .get(at..end)
            .ok_or(CodecError::Length)?
            .try_into()
            .map_err(|_| CodecError::Length)?,
    ))
}

fn metadata_size(tiles: usize, pairs: usize) -> Result<usize, CodecError> {
    let size = tiles
        .checked_mul(20)
        .and_then(|t| pairs.checked_mul(8).and_then(|p| t.checked_add(p)))
        .and_then(|s| s.checked_add(32))
        .ok_or(CodecError::BoundsExceeded)?;
    if size > LIMITS.max_metadata() as usize {
        return Err(CodecError::BoundsExceeded);
    }
    Ok(size)
}

fn positions<const K: usize>(
    count: usize,
    n: usize,
    get: impl Fn(usize) -> Result<u32, CodecError>,
) -> Result<(), CodecError> {
    let mut previous = None;
    for i in 0..count {
        let mut row = [0; K];
        for (j, value) in row.iter_mut().enumerate() {
            *value = get(i * K + j)?;
        }
        if row.windows(2).any(|w| w.first() >= w.get(1))
            || row.last().is_none_or(|&p| p as usize >= n)
            || previous.is_some_and(|p| p >= row)
        {
            return Err(CodecError::InvalidInstance);
        }
        previous = Some(row);
    }
    Ok(())
}

struct Metadata<'a> {
    tiles: &'a [u8],
    gauges: &'a [u8],
    pairs: &'a [u8],
    t: usize,
    d: usize,
}
impl<'a> Metadata<'a> {
    fn parse(instance: &'a Instance) -> Result<Self, CodecError> {
        let invalid = CodecError::UnencodableInstance;
        let [tiles, gauges, pairs] =
            sections(instance.metadata(), [KIND_TILES, KIND_GAUGES, KIND_PAIRS]).ok_or(invalid)?;
        let t = u32_at(tiles, 0).map_err(|_| invalid)? as usize;
        let d = u32_at(pairs, 0).map_err(|_| invalid)? as usize;
        let _ = metadata_size(t, d)?;
        if tiles.len() != 4 + 16 * t
            || pairs.len() != 4 + 8 * d
            || gauges.len() != 4 + 4 * t
            || u32_at(gauges, 0).map_err(|_| invalid)? as usize != t
        {
            return Err(invalid);
        }
        let tiles = tiles.get(4..).ok_or(invalid)?;
        let gauges = gauges.get(4..).ok_or(invalid)?;
        let pairs = pairs.get(4..).ok_or(invalid)?;
        if gauges.iter().any(|&g| g > 1) {
            return Err(invalid);
        }
        let n = instance.node_ids().len();
        positions::<4>(t, n, |i| u32_at(tiles, 4 * i)).map_err(|_| invalid)?;
        positions::<2>(d, n, |i| u32_at(pairs, 4 * i)).map_err(|_| invalid)?;
        Ok(Self {
            tiles,
            gauges,
            pairs,
            t,
            d,
        })
    }
}

pub(crate) fn prepare(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    let meta = Metadata::parse(instance).map_err(|e| {
        if e == CodecError::UnencodableInstance {
            CodecError::InvalidInstance
        } else {
            e
        }
    })?;
    let mut candidates = Vec::new();
    candidates
        .try_reserve_exact(meta.t + meta.d)
        .map_err(|_| CodecError::Allocation)?;
    for i in 0..meta.d {
        candidates.push(Block::pair(
            u32_at(meta.pairs, 8 * i)?,
            u32_at(meta.pairs, 8 * i + 4)?,
        ));
    }
    for i in 0..meta.t {
        let mut tile = [0; 4];
        let mut gauge = [1; 4];
        for (j, (p, g)) in tile.iter_mut().zip(&mut gauge).enumerate() {
            *p = u32_at(meta.tiles, 16 * i + 4 * j)?;
            *g = if meta.gauges.get(4 * i + j) == Some(&1) {
                -1
            } else {
                1
            };
        }
        candidates.push(Block::tile(tile, gauge, 0)?);
    }
    Ok(BlocksCodec::new(instance.node_ids().len(), &candidates)?.into())
}

pub(crate) fn encode_instance(
    _params: &Params<'_>,
    instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    let meta = Metadata::parse(instance)?;
    let mut out = encode_prefix(instance)?;
    // This upper bound includes uncompressed columns and both scalar counts.
    out.try_reserve(10 + 16 * meta.t + (4 * meta.t).div_ceil(8) + 8 * meta.d)
        .map_err(|_| CodecError::Allocation)?;
    out.extend_from_slice(
        &u32::try_from(meta.t)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    let mut values = filled(4 * meta.t, 0)?;
    for (i, v) in values.iter_mut().enumerate() {
        *v = u32_at(meta.tiles, 4 * i)?;
    }
    write_column(&values, &mut out)?;
    let start = out.len();
    out.resize(start + (4 * meta.t).div_ceil(8), 0);
    for (i, &g) in meta.gauges.iter().enumerate() {
        *out.get_mut(start + i / 8).ok_or(CodecError::Length)? |= g << (i % 8);
    }
    out.extend_from_slice(
        &u32::try_from(meta.d)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    let mut values = filled(2 * meta.d, 0)?;
    for (i, v) in values.iter_mut().enumerate() {
        *v = u32_at(meta.pairs, 4 * i)?;
    }
    write_column(&values, &mut out)?;
    Ok(out)
}

fn section(out: &mut Vec<u8>, kind: u16, len: usize, count: usize) -> Result<(), CodecError> {
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(len)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    out.extend_from_slice(
        &u32::try_from(count)
            .map_err(|_| CodecError::BoundsExceeded)?
            .to_le_bytes(),
    );
    Ok(())
}

pub(crate) fn decode_instance(
    _params: &Params<'_>,
    descriptor_digest: &[u8; 32],
    bytes: &[u8],
) -> Result<Instance, CodecError> {
    let prefix = preflight_prefix(bytes, &LIMITS)?;
    let mut at = prefix.metadata_start();
    let t = u32_at(bytes, at)? as usize;
    let _ = metadata_size(t, 0)?;
    at += 4;
    let (tiles, len) = read_column(bytes.get(at..).ok_or(CodecError::Length)?, 4 * t)?;
    at += len;
    let end = at + (4 * t).div_ceil(8);
    let gauges = bytes.get(at..end).ok_or(CodecError::Length)?;
    check_bit_array(gauges, 4 * t)?;
    at = end;
    let d = u32_at(bytes, at)? as usize;
    let size = metadata_size(t, d)?;
    at += 4;
    let (pairs, len) = read_column(bytes.get(at..).ok_or(CodecError::Length)?, 2 * d)?;
    if at + len != bytes.len() {
        return Err(CodecError::Length);
    }
    if prefix.descriptor_digest() != descriptor_digest {
        return Err(CodecError::DescriptorMismatch);
    }
    positions::<4>(t, prefix.node_count(), |i| tiles.get(i))?;
    positions::<2>(d, prefix.node_count(), |i| pairs.get(i))?;
    let mut metadata = Vec::new();
    metadata
        .try_reserve_exact(size)
        .map_err(|_| CodecError::Allocation)?;
    metadata.extend_from_slice(&3u16.to_le_bytes());
    section(&mut metadata, KIND_TILES, 4 + 16 * t, t)?;
    for i in 0..4 * t {
        metadata.extend_from_slice(&tiles.get(i)?.to_le_bytes());
    }
    section(&mut metadata, KIND_GAUGES, 4 + 4 * t, t)?;
    for i in 0..4 * t {
        metadata.push((gauges.get(i / 8).ok_or(CodecError::Length)? >> (i % 8)) & 1);
    }
    section(&mut metadata, KIND_PAIRS, 4 + 8 * d, d)?;
    for i in 0..2 * d {
        metadata.extend_from_slice(&pairs.get(i)?.to_le_bytes());
    }
    let instance = decode_prefix(
        &prefix,
        metadata,
        &LIMITS,
        &[KIND_TILES, KIND_GAUGES, KIND_PAIRS],
    )?;
    tiles.minimal()?;
    pairs.minimal()?;
    Ok(instance)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test fixture failures must abort the test"
)]
mod tests {
    use super::*;
    use crate::instance::InstanceParts;
    use alloc::vec;

    fn codec(n: u32, tiles: &[[u32; 4]], gauges: &[[i8; 4]], pairs: &[(u32, u32)]) -> WitnessCodec {
        let mut tile_bytes = u32::try_from(tiles.len())
            .expect("count")
            .to_le_bytes()
            .to_vec();
        for tile in tiles {
            for p in tile {
                tile_bytes.extend_from_slice(&p.to_le_bytes());
            }
        }
        let metadata = super::super::metadata_region(&tile_bytes, gauges, pairs).expect("metadata");
        let instance = Instance::new(
            InstanceParts {
                descriptor_digest: [0; 32],
                topology_order_digest: [0; 32],
                nonce: None,
                node_ids: (0..n).collect(),
                edges: vec![],
                h: vec![0; n as usize],
                j: vec![],
                offset_milli: 0,
                metadata,
            },
            &LIMITS,
            &[1, 2, 3],
            None,
        )
        .expect("instance");
        prepare(&Params::parse(&[], &[1]).expect("params"), &instance).expect("prepare")
    }

    #[test]
    fn blocks_have_exact_bytes() {
        let pair = codec(3, &[], &[], &[(0, 1)]);
        assert_eq!(pair.encode(&[1, 1, -1]).expect("encode"), [2]);
        assert_eq!(pair.encode(&[1, -1, -1]).expect("encode"), [3]);
        let tile = codec(4, &[[0, 1, 2, 3]], &[[1; 4]], &[]);
        assert_eq!(tile.encode(&[1, 1, -1, -1]).expect("encode"), [0]);
        assert_eq!(tile.encode(&[1; 4]).expect("encode"), [31]);
        assert_eq!(tile.decode(&[7]), Err(CodecError::NonCanonical));
        for rank in [12, 14] {
            assert_eq!(tile.decode(&[rank]), Err(CodecError::InvalidSymbol));
        }
        for bit in 4..8 {
            assert_eq!(tile.decode(&[1 << bit]), Err(CodecError::Padding));
        }
        for bit in 5..8 {
            assert_eq!(tile.decode(&[31 | (1 << bit)]), Err(CodecError::Padding));
        }
        assert_eq!(tile.encode(&[0; 4]), Err(CodecError::SpinValue));
        assert_eq!(tile.encode(&[1; 3]), Err(CodecError::Length));
        assert_eq!(tile.decode(&[]), Err(CodecError::Length));
        assert_eq!(tile.decode(&[0; 32]), Err(CodecError::Length));
        let empty = codec(0, &[], &[], &[]);
        assert_eq!(empty.encode(&[]).expect("empty"), [0]);
        assert_eq!(empty.decode(&[1]), Err(CodecError::NonCanonical));
    }

    #[test]
    fn twisted_overlap_and_priority() {
        let pair_first = codec(6, &[[0, 1, 2, 3]], &[[1; 4]], &[(0, 4)]);
        // Tile is skipped, and the uncovered positions 1,2,3,5 retain their bits.
        assert_eq!(
            pair_first.encode(&[1, 1, -1, 1, 1, -1]).expect("encode"),
            [22]
        );
        assert_eq!(
            pair_first.decode(&[22]).expect("decode"),
            [1, 1, -1, 1, 1, -1]
        );
        let tile_first = codec(6, &[[0, 1, 2, 3], [2, 3, 4, 5]], &[[1; 4]; 2], &[]);
        // The second tile violates its constraint but overlaps the selected first tile.
        assert_eq!(
            tile_first.encode(&[1, 1, -1, -1, -1, -1]).expect("encode"),
            [0]
        );
        assert_eq!(
            tile_first.decode(&[0]).expect("decode"),
            [1, 1, -1, -1, -1, -1]
        );
    }

    #[test]
    fn twisted_every_spin_is_encodable() {
        for prepared in [
            codec(6, &[[0, 1, 2, 3]], &[[-1, 1, -1, 1]], &[(0, 4)]),
            codec(6, &[[0, 1, 2, 3], [2, 3, 4, 5]], &[[1; 4], [-1; 4]], &[]),
            codec(6, &[], &[], &[]),
        ] {
            for mask in 0..64 {
                let spins: Vec<_> = (0..6)
                    .map(|i| if mask & (1 << i) == 0 { -1 } else { 1 })
                    .collect();
                assert_eq!(
                    prepared
                        .decode(&prepared.encode(&spins).expect("encode"))
                        .expect("decode"),
                    spins
                );
            }
        }
    }

    #[test]
    fn blocks_malformed_properties() {
        let prepared = codec(6, &[[0, 1, 2, 3]], &[[-1, 1, -1, 1]], &[]);
        let mut state = 2311u64;
        for _ in 0..1024 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let spins: Vec<_> = (0..6)
                .map(|i| if state & (1 << (i + 32)) == 0 { -1 } else { 1 })
                .collect();
            assert_eq!(
                prepared
                    .decode(&prepared.encode(&spins).expect("encode"))
                    .expect("decode"),
                spins
            );
            let bytes = state.to_le_bytes();
            let payload = &bytes[..usize::from(bytes[7] % 8)];
            if let Ok(decoded) = prepared.decode(payload) {
                assert_eq!(prepared.encode(&decoded).expect("encode"), payload);
            }
        }
    }
}
