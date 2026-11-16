//! Helpers shared by the ice families: the tile pipeline, the tile and
//! gauge payloads, and the checks every ice family applies to its tiles.

use alloc::vec;
use alloc::vec::Vec;

use crate::descriptor::{Params, SizeBounds, TopologyStats, Value};
use crate::family::{BaseTopology, Streams};
use crate::ice::cache::ice_tiles;
use crate::ice::{Candidates, Lattice, StructureError, Tile, MAX_DEGREE};
use crate::region::Reader;
use crate::rng::shuffle;
use crate::Code;

/// Tag of the Zephyr grid size parameter.
const TAG_M: u16 = 1;
/// Tag of the Zephyr tile size parameter.
const TAG_T: u16 = 2;

/// The lattice the parameters name.
pub(crate) fn lattice(params: &Params<'_>) -> Result<Lattice, StructureError> {
    let m = match params.get(TAG_M) {
        Some(Value::Unsigned(m)) => u16::try_from(m).ok(),
        _ => None,
    };
    let t = match params.get(TAG_T) {
        Some(Value::Unsigned(t)) => u8::try_from(t).ok(),
        _ => None,
    };
    match (m, t) {
        (Some(m), Some(t)) => Lattice::from_shape(m, t),
        _ => Err(StructureError::Overflow),
    }
}

/// The sizes of an instance: the topology itself. A degree above the limit
/// gives [`Code::BoundsExceeded`].
pub(crate) fn topology_bounds(stats: &TopologyStats) -> Result<SizeBounds, Code> {
    if stats.max_degree > MAX_DEGREE {
        return Err(Code::BoundsExceeded);
    }
    Ok(SizeBounds {
        nodes: stats.n_nodes,
        edges: stats.n_edges,
        degree: stats.max_degree,
    })
}

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

/// The outcome of [`select_tiles`].
pub(crate) struct Selected {
    /// The selected tiles, ascending.
    pub(crate) tiles: Vec<Tile>,
    /// Parallel to `tiles`: the positions in `edges` of the six tile edges in
    /// [`tile_edges`] order, or `None` when one is not an edge.
    pub(crate) positions: Vec<Option<[usize; 6]>>,
    /// The edges of the topology as `(min, max)` pairs, sorted ascending.
    pub(crate) edges: Vec<(u32, u32)>,
}

/// Candidates, sides, both shuffles, and select. A structure error maps to
/// [`Code::TopologyIncompatible`]. The `left` and `right` stream labels order
/// the two sides.
///
/// Everything but the shuffles and the matching comes from the topology's
/// cache (see [`ice_tiles`]).
pub(crate) fn select_tiles(
    lattice: Lattice,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
    left: &str,
    right: &str,
) -> Result<Selected, Code> {
    let defect = Code::TopologyIncompatible;
    let cached = ice_tiles(lattice, topology);
    let structure = cached.as_ref().as_ref().map_err(|_| defect)?;
    let graph = structure.graph.as_ref().map_err(|_| defect)?;
    let left_order = shuffled_order(streams, left, graph.left_len())?;
    let right_order = shuffled_order(streams, right, graph.right_len())?;
    let chosen = graph
        .select(&left_order, &right_order)
        .map_err(|_| defect)?;
    let mut tiles = Vec::with_capacity(chosen.len());
    let mut positions = Vec::with_capacity(chosen.len());
    for at in chosen {
        tiles.push(*structure.candidates.tiles.get(at).ok_or(defect)?);
        positions.push(*structure.positions.get(at).ok_or(defect)?);
    }
    Ok(Selected {
        tiles,
        positions,
        edges: structure.edges.clone(),
    })
}

/// The positions of the edges that `strong` does not mark, ascending.
pub(crate) fn non_tile_edges(strong: &[bool]) -> Result<Vec<u32>, Code> {
    let len = u32::try_from(strong.len()).map_err(|_| Code::BoundsExceeded)?;
    // Write every position and advance past the non-tile ones. A branch on
    // the flag would mispredict about as often as the tiles alternate.
    let mut free = vec![0u32; strong.len()];
    let mut count = 0usize;
    for (at, &is_tile) in (0..len).zip(strong) {
        if let Some(slot) = free.get_mut(count) {
            *slot = at;
        }
        count += usize::from(!is_tile);
    }
    free.truncate(count);
    Ok(free)
}

/// The structure check of every ice admission: `check` of the candidates when
/// the topology carries tiles for `lattice`, `true` when only the degree limit
/// fails (code 13 then comes from the size bounds), and `false` for any other
/// structure error.
pub(crate) fn admit_tiles(
    lattice: Lattice,
    topology: &BaseTopology,
    check: impl FnOnce(&Candidates) -> bool,
) -> bool {
    match ice_tiles(lattice, topology).as_ref() {
        Ok(structure) => check(&structure.candidates),
        Err(StructureError::DegreeAboveLimit) => true,
        Err(_) => false,
    }
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

/// The position of each edge in a list of distinct edges sorted ascending,
/// found by first end. [`EdgeIndex::position`] gives the same answer as a
/// binary search of the whole list, from a run of at most the node degree.
pub(crate) struct EdgeIndex<'a> {
    edges: &'a [(u32, u32)],
    /// `starts[a]` is the position of the first edge whose first end is at
    /// least `a`, for `a` in `0..=n`.
    starts: Vec<usize>,
}

impl<'a> EdgeIndex<'a> {
    /// Index `edges`, which must be sorted ascending, for first ends below `n`.
    pub(crate) fn new(edges: &'a [(u32, u32)], n: usize) -> Self {
        let mut starts = Vec::with_capacity(n.saturating_add(1));
        let mut at = 0usize;
        for a in 0..=n {
            while edges
                .get(at)
                .is_some_and(|&(u, _)| usize::try_from(u).is_ok_and(|u| u < a))
            {
                at = at.saturating_add(1);
            }
            starts.push(at);
        }
        Self { edges, starts }
    }

    /// The indexed edges.
    pub(crate) const fn edges(&self) -> &'a [(u32, u32)] {
        self.edges
    }

    /// The position of `pair`, `None` when it is not an edge.
    pub(crate) fn position(&self, pair: (u32, u32)) -> Option<usize> {
        let first = usize::try_from(pair.0).ok();
        let run =
            first.and_then(|a| Some((*self.starts.get(a)?, *self.starts.get(a.checked_add(1)?)?)));
        match run {
            Some((start, end)) => {
                let offset = self
                    .edges
                    .get(start..end)?
                    .binary_search_by_key(&pair.1, |&(_, v)| v)
                    .ok()?;
                start.checked_add(offset)
            }
            // A first end outside the index: search the whole list.
            None => self.edges.binary_search(&pair).ok(),
        }
    }
}

/// The tile edges of an instance, from [`mark_tiles`].
pub(crate) struct TileMarks {
    /// Indexed by edge position: `true` for a tile edge.
    pub(crate) strong: Vec<bool>,
    /// Parallel to the tiles: the positions of the six tile edges in
    /// [`tile_edges`] order.
    pub(crate) positions: Vec<[usize; 6]>,
}

/// Mark the tile edges (flags indexed by edge position in `edges`, which is
/// sorted ascending). `None` when a node is in more than two tiles, a tile
/// edge is missing from `edges`, or two tiles share an edge.
pub(crate) fn mark_tiles(tiles: &[[u32; 4]], edges: &EdgeIndex<'_>, n: usize) -> Option<TileMarks> {
    let mut strong = vec![false; edges.edges().len()];
    let mut positions = Vec::with_capacity(tiles.len());
    let mut slots = vec![0u8; n];
    for tile in tiles {
        for pos in *tile {
            let slot = slots.get_mut(usize::try_from(pos).ok()?)?;
            *slot = slot.checked_add(1).filter(|count| *count <= 2)?;
        }
        let mut found = [0usize; 6];
        for (pair, slot) in tile_edges(*tile).into_iter().zip(&mut found) {
            let at = edges.position(pair)?;
            let seen = strong.get_mut(at)?;
            if *seen {
                return None;
            }
            *seen = true;
            *slot = at;
        }
        positions.push(found);
    }
    Some(TileMarks { strong, positions })
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
