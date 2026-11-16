//! The nonce-independent tile structure of a base topology, kept with the
//! topology so that each salt of a lease reuses it.
//!
//! [`candidates`], [`sides`], the select graph, and the sorted edge list
//! depend only on the lattice and the topology. Every ice construction needs
//! them for every salt, and they cost far more than the salt's own draws on a
//! topology with thousands of nodes. With the `std` feature, [`IceCache`]
//! holds one result per lattice behind a mutex. Without it, the cache holds
//! nothing and each call computes the structure again. The results are the
//! same either way.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

use super::build::{tile_edges, EdgeIndex};
use super::matching::{sides, SelectGraph};
use super::topology::{candidates, Candidates};
use super::{Lattice, StructureError};
use crate::family::BaseTopology;

/// The tile structure of one lattice on one topology.
#[derive(Debug)]
pub(crate) struct IceTiles {
    /// The candidate tiles, as [`candidates`] returns them.
    pub(crate) candidates: Candidates,
    /// The select graph of the candidates and their sides.
    pub(crate) graph: Result<SelectGraph, StructureError>,
    /// The edges as `(min, max)` pairs, sorted ascending.
    pub(crate) edges: Vec<(u32, u32)>,
    /// Parallel to the candidate tiles: the positions in `edges` of the six
    /// tile edges in `tile_edges` order, or `None` when one is not an edge.
    pub(crate) positions: Vec<Option<[usize; 6]>>,
}

/// The cached result for one lattice. `Err` is the [`candidates`] error.
pub(crate) type Cached = Arc<Result<IceTiles, StructureError>>;

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

fn compute(lattice: Lattice, topology: &BaseTopology) -> Result<IceTiles, StructureError> {
    let found = candidates(lattice, topology)?;
    let graph = sides(lattice, topology, &found).and_then(|split| SelectGraph::new(&found, &split));
    let edges = sorted_edges(topology);
    let index = EdgeIndex::new(&edges, topology.node_ids().len());
    let positions = found
        .tiles
        .iter()
        .map(|tile| {
            let pairs = tile_edges(tile.nodes);
            let mut out = [0usize; 6];
            for (slot, pair) in out.iter_mut().zip(pairs) {
                *slot = index.position(pair)?;
            }
            Some(out)
        })
        .collect();
    Ok(IceTiles {
        candidates: found,
        graph,
        edges,
        positions,
    })
}

/// The tile structure of `lattice` on `topology`, from the cache when the
/// topology already holds it.
pub(crate) fn ice_tiles(lattice: Lattice, topology: &BaseTopology) -> Cached {
    topology
        .ice_cache()
        .get_or_insert(lattice, || compute(lattice, topology))
}

/// One cached structure per lattice. A clone copies the entries. Equality
/// ignores the cache, because it is a pure function of the topology.
#[derive(Default)]
pub(crate) struct IceCache {
    #[cfg(feature = "std")]
    entries: std::sync::Mutex<Vec<(Lattice, Cached)>>,
}

impl IceCache {
    /// The entry of `lattice`, computed with `make` when absent. The lock is
    /// not held while `make` runs, so two threads may both compute a missing
    /// entry; the first to finish is kept.
    #[cfg(feature = "std")]
    fn get_or_insert(
        &self,
        lattice: Lattice,
        make: impl FnOnce() -> Result<IceTiles, StructureError>,
    ) -> Cached {
        let find = |entries: &[(Lattice, Cached)]| {
            entries
                .iter()
                .find(|(key, _)| *key == lattice)
                .map(|(_, value)| Arc::clone(value))
        };
        if let Some(hit) = self.entries.lock().ok().and_then(|e| find(&e)) {
            return hit;
        }
        let made = Arc::new(make());
        // A poisoned lock only loses the cache entry, never the result.
        let Ok(mut entries) = self.entries.lock() else {
            return made;
        };
        if let Some(hit) = find(&entries) {
            return hit;
        }
        entries.push((lattice, Arc::clone(&made)));
        made
    }

    #[cfg(not(feature = "std"))]
    #[expect(
        clippy::unused_self,
        reason = "the std build reads the entries, and both builds share the call"
    )]
    fn get_or_insert(
        &self,
        _lattice: Lattice,
        make: impl FnOnce() -> Result<IceTiles, StructureError>,
    ) -> Cached {
        Arc::new(make())
    }
}

impl Clone for IceCache {
    fn clone(&self) -> Self {
        #[cfg(feature = "std")]
        {
            let entries = self
                .entries
                .lock()
                .map(|entries| entries.clone())
                .unwrap_or_default();
            Self {
                entries: std::sync::Mutex::new(entries),
            }
        }
        #[cfg(not(feature = "std"))]
        Self {}
    }
}

impl PartialEq for IceCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for IceCache {}

impl fmt::Debug for IceCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IceCache").finish_non_exhaustive()
    }
}
