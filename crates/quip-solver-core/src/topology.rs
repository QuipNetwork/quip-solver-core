//! The working graph of a device, and whether a session `Topology` fits on it.

use std::collections::BTreeSet;

use ising_generators::descriptor::topology_order_digest;

/// The working graph of a device in native ids: the qubits it exposes and the
/// couplers between them. A backend returns it from
/// [`Sampler::native_topology`](crate::Sampler::native_topology).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTopology {
    nodes: BTreeSet<u32>,
    edges: BTreeSet<(u32, u32)>,
    hash: Option<[u8; 32]>,
}

/// How a session `Topology` fits on a native graph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TopologyFit {
    /// Qubits of the session topology that the device lacks.
    pub missing_nodes: usize,
    /// Couplers between two live qubits of the session topology that the
    /// device lacks. A coupler at a missing qubit counts under
    /// `missing_nodes` only.
    pub missing_edges: usize,
}

impl TopologyFit {
    /// Whether the device hosts every qubit and coupler of the topology.
    #[must_use]
    pub const fn hosted(self) -> bool {
        self.missing_nodes == 0 && self.missing_edges == 0
    }
}

fn canonical(u: u32, v: u32) -> (u32, u32) {
    (u.min(v), u.max(v))
}

impl NativeTopology {
    /// A working graph from its qubits and couplers, in any order. A coupler
    /// is undirected, and repeats collapse.
    #[must_use]
    pub fn new(
        nodes: impl IntoIterator<Item = u32>,
        edges: impl IntoIterator<Item = (u32, u32)>,
    ) -> Self {
        let nodes: BTreeSet<u32> = nodes.into_iter().collect();
        let edges: BTreeSet<(u32, u32)> = edges.into_iter().map(|(u, v)| canonical(u, v)).collect();
        let sorted_nodes: Vec<u32> = nodes.iter().copied().collect();
        let sorted_edges: Vec<(u32, u32)> = edges.iter().copied().collect();
        let hash = topology_order_digest(&sorted_nodes, &sorted_edges);
        Self { nodes, edges, hash }
    }

    /// The hash the session advertises as `Capabilities.native_topology_hash`:
    /// the FAM-19 topology order digest of the graph with the qubits
    /// ascending and the couplers `(min, max)` ascending. `None` when a count
    /// is above `u32::MAX`.
    #[must_use]
    pub const fn hash(&self) -> Option<[u8; 32]> {
        self.hash
    }

    /// How the session topology `nodes` and `edges` fit on this graph.
    #[must_use]
    pub fn fit(&self, nodes: &[u32], edges: &[(u32, u32)]) -> TopologyFit {
        let mut fit = TopologyFit::default();
        for node in nodes {
            if !self.nodes.contains(node) {
                fit.missing_nodes = fit.missing_nodes.saturating_add(1);
            }
        }
        for &(u, v) in edges {
            let live = self.nodes.contains(&u) && self.nodes.contains(&v);
            if live && !self.edges.contains(&canonical(u, v)) {
                fit.missing_edges = fit.missing_edges.saturating_add(1);
            }
        }
        fit
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeTopology, TopologyFit};
    use ising_generators::descriptor::topology_order_digest;

    #[test]
    fn the_hash_ignores_node_order_edge_order_and_edge_direction() {
        let a = NativeTopology::new(vec![10, 20, 30], vec![(10, 20), (20, 30)]);
        let b = NativeTopology::new(vec![30, 10, 20], vec![(30, 20), (10, 20)]);
        assert_eq!(a.hash(), b.hash());
        assert_eq!(
            a.hash(),
            topology_order_digest(&[10, 20, 30], &[(10, 20), (20, 30)])
        );
        let c = NativeTopology::new(vec![10, 20, 30], vec![(10, 20)]);
        assert_ne!(a.hash(), c.hash());
    }

    #[test]
    fn the_fit_counts_lost_qubits_and_lost_couplers_between_live_qubits() {
        let native = NativeTopology::new(vec![10, 30, 40], vec![(10, 30)]);
        // Qubit 20 is lost with its two couplers; coupler 30-40 joins two
        // live qubits and is lost on its own.
        let fit = native.fit(&[10, 20, 30, 40], &[(10, 20), (20, 30), (10, 30), (40, 30)]);
        assert_eq!(
            fit,
            TopologyFit {
                missing_nodes: 1,
                missing_edges: 1,
            }
        );
        assert!(!fit.hosted());
        assert!(native.fit(&[10, 30], &[(30, 10)]).hosted());
        assert!(native.fit(&[], &[]).hosted());
    }
}
