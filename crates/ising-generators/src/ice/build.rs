//! Helpers shared by the ice families: the tile pipeline, the metadata
//! region codec, and the checks every ice family applies to its tiles.

use alloc::vec;
use alloc::vec::Vec;

use crate::family::{BaseTopology, Streams};
use crate::ice::{candidates, select, sides, Lattice, Tile};
use crate::rng::shuffle;
use crate::Code;

/// A shuffled `0..len`, drawn from the stream `label`.
pub(crate) fn shuffled_order(
    streams: &mut Streams<'_>,
    label: &str,
    len: usize,
) -> Result<Vec<u32>, Code> {
    let len = u32::try_from(len).map_err(|_| Code::BoundsExceeded)?;
    let mut order: Vec<u32> = (0..len).collect();
    let mut rng = streams.open(label).ok_or(Code::NoInstance)?;
    shuffle(&mut rng, &mut order)?;
    Ok(order)
}

/// The six edges of a tile, ascending pairs.
pub(crate) const fn tile_edges(nodes: [u32; 4]) -> [(u32, u32); 6] {
    let [a, b, c, d] = nodes;
    [(a, b), (a, c), (a, d), (b, c), (b, d), (c, d)]
}

/// Candidates, sides, both shuffles, and select. A structure error maps to
/// [`Code::TopologyIncompatible`]. The `left` and `right` stream labels order
/// the two sides.
pub(crate) fn select_tiles(
    lattice: Lattice,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
    left: &str,
    right: &str,
) -> Result<Vec<Tile>, Code> {
    let defect = Code::TopologyIncompatible;
    let found = candidates(lattice, topology).map_err(|_| defect)?;
    let split = sides(lattice, topology, &found).map_err(|_| defect)?;
    let left_order = shuffled_order(streams, left, split.left.len())?;
    let right_order = shuffled_order(streams, right, split.right.len())?;
    select(&found, &split, &left_order, &right_order).map_err(|_| defect)
}

/// The edges of the topology as `(min, max)` pairs, sorted ascending.
pub(crate) fn sorted_edges(topology: &BaseTopology) -> Vec<(u32, u32)> {
    let mut edges: Vec<(u32, u32)> = topology
        .edges()
        .iter()
        .map(|&(u, v)| (u.min(v), u.max(v)))
        .collect();
    edges.sort_unstable();
    edges
}

/// A reader over a byte slice that never panics.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// A reader at the start of `bytes`.
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// The bytes not read yet.
    pub(crate) const fn rest(&self) -> &'a [u8] {
        self.bytes
    }

    /// The next `len` bytes, `None` when fewer remain.
    pub(crate) fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let head = self.bytes.get(..len)?;
        self.bytes = self.bytes.get(len..)?;
        Some(head)
    }

    /// The next little-endian `u16`.
    pub(crate) fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }

    /// The next little-endian `u32`.
    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

/// The payloads of a region that holds exactly `kinds` in order. `None` for
/// any other region.
pub(crate) fn sections<const K: usize>(region: &[u8], kinds: [u16; K]) -> Option<[&[u8]; K]> {
    let mut reader = Reader::new(region);
    if usize::from(reader.u16()?) != K {
        return None;
    }
    let mut payloads = [&[][..]; K];
    for (slot, kind) in payloads.iter_mut().zip(kinds) {
        if reader.u16()? != kind {
            return None;
        }
        let len = usize::try_from(reader.u32()?).ok()?;
        *slot = reader.take(len)?;
    }
    reader.rest().is_empty().then_some(())?;
    Some(payloads)
}

/// The tiles of a tile payload: four positions each, ascending, tiles
/// ascending and distinct.
pub(crate) fn parse_tiles(payload: &[u8], n: usize) -> Option<Vec<[u32; 4]>> {
    let mut reader = Reader::new(payload);
    let count = usize::try_from(reader.u32()?).ok()?;
    if reader.rest().len() != count.checked_mul(16)? {
        return None;
    }
    let mut tiles: Vec<[u32; 4]> = Vec::with_capacity(count);
    for _ in 0..count {
        let tile = [reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?];
        let ascending = tile.windows(2).all(|w| matches!(w, [a, b] if a < b));
        let last = usize::try_from(tile[3]).ok()?;
        if !ascending || last >= n || tiles.last().is_some_and(|prev| *prev >= tile) {
            return None;
        }
        tiles.push(tile);
    }
    Some(tiles)
}

/// Mark the tile edges (flags indexed by edge position in `edges`, which is
/// sorted ascending). `None` when a node is in more than two tiles, a tile
/// edge is missing from `edges`, or two tiles share an edge.
pub(crate) fn mark_tiles(tiles: &[[u32; 4]], edges: &[(u32, u32)], n: usize) -> Option<Vec<bool>> {
    let mut strong = vec![false; edges.len()];
    let mut slots = vec![0u8; n];
    for tile in tiles {
        for pos in *tile {
            let slot = slots.get_mut(usize::try_from(pos).ok()?)?;
            *slot = slot.checked_add(1).filter(|count| *count <= 2)?;
        }
        for pair in tile_edges(*tile) {
            let at = edges.binary_search(&pair).ok()?;
            let seen = strong.get_mut(at)?;
            if *seen {
                return None;
            }
            *seen = true;
        }
    }
    Some(strong)
}

/// A metadata region from `(kind, payload)` sections, in order.
pub(crate) fn region(sections: &[(u16, Vec<u8>)]) -> Result<Vec<u8>, Code> {
    let overflow = Code::BoundsExceeded;
    let count = u16::try_from(sections.len()).map_err(|_| overflow)?;
    let mut region = count.to_le_bytes().to_vec();
    for (kind, payload) in sections {
        region.extend_from_slice(&kind.to_le_bytes());
        region.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_| overflow)?
                .to_le_bytes(),
        );
        region.extend_from_slice(payload);
    }
    Ok(region)
}

/// The payload of a tile list: the count, then four positions per tile.
pub(crate) fn tiles_payload(tiles: &[Tile]) -> Result<Vec<u8>, Code> {
    let mut payload = u32::try_from(tiles.len())
        .map_err(|_| Code::BoundsExceeded)?
        .to_le_bytes()
        .to_vec();
    for tile in tiles {
        for pos in tile.nodes {
            payload.extend_from_slice(&pos.to_le_bytes());
        }
    }
    Ok(payload)
}

/// The payload of local gauges: the tile count, then four bytes per tile,
/// `0x00` for +1 and `0x01` for -1.
pub(crate) fn gauges_payload(gauges: &[[i8; 4]]) -> Result<Vec<u8>, Code> {
    let mut payload = u32::try_from(gauges.len())
        .map_err(|_| Code::BoundsExceeded)?
        .to_le_bytes()
        .to_vec();
    for gauge in gauges {
        payload.extend(gauge.iter().map(|&g| u8::from(g < 0)));
    }
    Ok(payload)
}

/// The local gauges of a kind 2 payload: four signs per tile, `tiles` tiles.
pub(crate) fn parse_gauges(payload: &[u8], tiles: usize) -> Option<Vec<[i8; 4]>> {
    let mut reader = Reader::new(payload);
    let count = usize::try_from(reader.u32()?).ok()?;
    if count != tiles || reader.rest().len() != count.checked_mul(4)? {
        return None;
    }
    let sign = |byte: u8| match byte {
        0 => Some(1),
        1 => Some(-1),
        _ => None,
    };
    reader
        .rest()
        .chunks_exact(4)
        .map(|chunk| match *chunk {
            [a, b, c, d] => Some([sign(a)?, sign(b)?, sign(c)?, sign(d)?]),
            _ => None,
        })
        .collect()
}
