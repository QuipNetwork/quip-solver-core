//! Disjoint soft constraints and their canonical rank stream.
use super::{
    bits::{byte_len, BitReader, BitWriter},
    check_spins, filled, CodecError,
};
use crate::instance::Instance;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Block {
    positions: [u32; 4],
    gauge: [i8; 4],
    len: usize,
    masks: &'static [u8],
    width: u8,
}
impl Block {
    pub(crate) fn pair(a: u32, b: u32) -> Self {
        Self {
            positions: [a, b, 0, 0],
            gauge: [1; 4],
            len: 2,
            masks: &[0, 3],
            width: 1,
        }
    }
    pub(crate) fn tile(
        positions: [u32; 4],
        gauge: [i8; 4],
        charge: i8,
    ) -> Result<Self, CodecError> {
        if gauge.iter().any(|&g| g != -1 && g != 1) {
            return Err(CodecError::InvalidInstance);
        }
        let (masks, width): (&'static [u8], u8) = match charge {
            0 => (&[3, 5, 6, 9, 10, 12], 3),
            2 => (&[7, 11, 13, 14], 2),
            -2 => (&[1, 2, 4, 8], 2),
            _ => return Err(CodecError::InvalidInstance),
        };
        Ok(Self {
            positions,
            gauge,
            len: 4,
            masks,
            width,
        })
    }
    fn positions(&self) -> &[u32] {
        self.positions.get(..self.len).unwrap_or(&[])
    }
    fn rank(&self, spins: &[i8]) -> Option<u8> {
        let mut mask = 0;
        for (i, (&p, &g)) in self.positions().iter().zip(&self.gauge).enumerate() {
            if *spins.get(p as usize)? * g == 1 {
                mask |= 1 << i;
            }
        }
        self.masks
            .iter()
            .position(|&m| m == mask)
            .and_then(|r| u8::try_from(r).ok())
    }
}
pub(crate) struct BlocksCodec {
    n: usize,
    selected: Vec<Block>,
    uncovered: Vec<usize>,
    bits: usize,
    max_len: usize,
}
impl BlocksCodec {
    pub(crate) fn new(n: usize, candidates: &[Block]) -> Result<Self, CodecError> {
        let max_len = byte_len(n.checked_add(1).ok_or(CodecError::BoundsExceeded)?)?;
        let mut used = filled(n, false)?;
        let mut selected = Vec::new();
        selected
            .try_reserve_exact(candidates.len().min(n / 2))
            .map_err(|_| CodecError::Allocation)?;
        let mut bits = 1usize;
        for block in candidates {
            for (i, &p) in block.positions().iter().enumerate() {
                if p as usize >= n
                    || block
                        .positions()
                        .get(..i)
                        .ok_or(CodecError::InvalidInstance)?
                        .contains(&p)
                {
                    return Err(CodecError::InvalidInstance);
                }
            }
            if block
                .positions()
                .iter()
                .any(|&p| used.get(p as usize) == Some(&true))
            {
                continue;
            }
            for &p in block.positions() {
                *used
                    .get_mut(p as usize)
                    .ok_or(CodecError::InvalidInstance)? = true;
            }
            bits = bits
                .checked_add(usize::from(block.width))
                .ok_or(CodecError::BoundsExceeded)?;
            selected.push(*block);
        }
        let mut uncovered = Vec::new();
        uncovered
            .try_reserve_exact(used.iter().filter(|&&u| !u).count())
            .map_err(|_| CodecError::Allocation)?;
        for (p, &u) in used.iter().enumerate() {
            if !u {
                uncovered.push(p);
            }
        }
        bits = bits
            .checked_add(uncovered.len())
            .ok_or(CodecError::BoundsExceeded)?;
        Ok(Self {
            n,
            selected,
            uncovered,
            bits,
            max_len,
        })
    }
    pub(crate) fn spin_count(&self) -> usize {
        self.n
    }
    pub(crate) fn max_encoded_len(&self) -> usize {
        self.max_len
    }
    fn eligible(&self, spins: &[i8]) -> bool {
        self.selected.iter().all(|b| b.rank(spins).is_some())
    }
    pub(crate) fn encode(&self, spins: &[i8]) -> Result<Vec<u8>, CodecError> {
        check_spins(spins, self.n)?;
        let structured = self.eligible(spins);
        let mut writer = BitWriter::new(if structured { self.bits } else { self.n + 1 })?;
        writer.push(u8::from(!structured), 1)?;
        if structured {
            for b in &self.selected {
                writer.push(b.rank(spins).ok_or(CodecError::InvalidSymbol)?, b.width)?;
            }
            for &p in &self.uncovered {
                writer.push(u8::from(spins.get(p) == Some(&1)), 1)?;
            }
        } else {
            for &s in spins {
                writer.push(u8::from(s == 1), 1)?;
            }
        }
        writer.finish()
    }
    pub(crate) fn decode(&self, bytes: &[u8]) -> Result<Vec<i8>, CodecError> {
        if bytes.len() > self.max_len {
            return Err(CodecError::Length);
        }
        let fallback = bytes.first().ok_or(CodecError::Length)? & 1 != 0;
        let mut reader = BitReader::new(bytes, if fallback { self.n + 1 } else { self.bits })?;
        let _ = reader.read(1)?;
        let mut spins = filled(self.n, -1)?;
        if fallback {
            for s in &mut spins {
                *s = if reader.read(1)? == 1 { 1 } else { -1 };
            }
            if self.eligible(&spins) {
                return Err(CodecError::NonCanonical);
            }
        } else {
            for b in &self.selected {
                let mask = b
                    .masks
                    .get(usize::from(reader.read(b.width)?))
                    .ok_or(CodecError::InvalidSymbol)?;
                for (i, (&p, &g)) in b.positions().iter().zip(&b.gauge).enumerate() {
                    *spins
                        .get_mut(p as usize)
                        .ok_or(CodecError::InvalidInstance)? =
                        g * if mask & (1 << i) != 0 { 1 } else { -1 };
                }
            }
            for &p in &self.uncovered {
                *spins.get_mut(p).ok_or(CodecError::InvalidInstance)? =
                    if reader.read(1)? == 1 { 1 } else { -1 };
            }
        }
        Ok(spins)
    }
}

/// Read global-gauge tiles without validating their model coefficients.
pub(crate) fn blocks(instance: &Instance) -> Result<Vec<Block>, CodecError> {
    use crate::ice::build::{sections, Reader};
    let [tiles, gauge] =
        sections(instance.metadata(), [1, 2]).ok_or(CodecError::InvalidInstance)?;
    let mut tiles = Reader::new(tiles);
    let mut gauge_reader = Reader::new(gauge);
    let count = tiles.u32().ok_or(CodecError::InvalidInstance)? as usize;
    let n = instance.node_ids().len();
    if gauge_reader.u32().ok_or(CodecError::InvalidInstance)? as usize != n
        || gauge_reader.rest().len() != n
        || gauge_reader.rest().iter().any(|&g| g > 1)
        || tiles.rest().len() != count.checked_mul(16).ok_or(CodecError::BoundsExceeded)?
    {
        return Err(CodecError::InvalidInstance);
    }
    let mut out = Vec::new();
    out.try_reserve_exact(count)
        .map_err(|_| CodecError::Allocation)?;
    let mut previous = None;
    for _ in 0..count {
        let mut positions = [0; 4];
        let mut signs = [1; 4];
        for (p, g) in positions.iter_mut().zip(&mut signs) {
            *p = tiles.u32().ok_or(CodecError::InvalidInstance)?;
            *g = if *gauge_reader
                .rest()
                .get(*p as usize)
                .ok_or(CodecError::InvalidInstance)?
                == 0
            {
                1
            } else {
                -1
            };
        }
        if positions.windows(2).any(|w| w.first() >= w.get(1))
            || previous.is_some_and(|p| p >= positions)
        {
            return Err(CodecError::InvalidInstance);
        }
        previous = Some(positions);
        out.push(Block::tile(positions, signs, 0)?);
    }
    Ok(out)
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions must fail independently of setup errors"
)]
mod tests {
    use super::*;
    #[test]
    fn basic_bytes() -> Result<(), CodecError> {
        let pair = BlocksCodec::new(2, &[Block::pair(0, 1)])?;
        assert_eq!(pair.encode(&[1, 1])?, [2]);
        assert_eq!(pair.encode(&[1, -1])?, [3]);
        assert_eq!(pair.decode(&[2])?, [1, 1]);
        assert_eq!(pair.decode(&[3])?, [1, -1]);
        assert_eq!(pair.decode(&[7]), Err(CodecError::NonCanonical));
        let tile = BlocksCodec::new(4, &[Block::tile([0, 1, 2, 3], [1; 4], 0)?])?;
        assert_eq!(tile.encode(&[1, 1, -1, -1])?, [0]);
        assert_eq!(tile.encode(&[1; 4])?, [31]);
        assert_eq!(tile.decode(&[0])?, [1, 1, -1, -1]);
        assert_eq!(tile.decode(&[31])?, [1; 4]);
        assert_eq!(tile.decode(&[12]), Err(CodecError::InvalidSymbol));
        assert_eq!(tile.decode(&[128]), Err(CodecError::Padding));
        assert_eq!(tile.decode(&[]), Err(CodecError::Length));
        Ok(())
    }
    #[test]
    fn exhaustive_overlap_and_charges() -> Result<(), CodecError> {
        for charge in [-2, 0, 2] {
            let c = BlocksCodec::new(
                5,
                &[
                    Block::tile([0, 1, 2, 3], [-1, 1, -1, 1], charge)?,
                    Block::pair(3, 4),
                ],
            )?;
            for mask in 0..32 {
                let spins: Vec<_> = (0..5)
                    .map(|i| if mask & (1 << i) != 0 { 1 } else { -1 })
                    .collect();
                assert_eq!(c.decode(&c.encode(&spins)?)?, spins);
            }
        }
        assert!(matches!(
            BlocksCodec::new(2, &[Block::pair(0, 0)]),
            Err(CodecError::InvalidInstance)
        ));
        assert!(matches!(
            BlocksCodec::new(2, &[Block::pair(0, 2)]),
            Err(CodecError::InvalidInstance)
        ));
        let empty = BlocksCodec::new(0, &[])?;
        assert_eq!(empty.encode(&[])?, [0]);
        assert_eq!(empty.decode(&[1]), Err(CodecError::NonCanonical));
        Ok(())
    }
}
