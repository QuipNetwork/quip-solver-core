//! The z(`m`, `t`) Zephyr lattice as a base topology, for tests that declare
//! it with `#[path = "common/lattice.rs"] mod lattice;`.

use ising_generators::family::BaseTopology;
use ising_generators::ice::zephyr_lattice;

/// The z(`m`, `t`) Zephyr lattice as a base topology.
///
/// # Panics
///
/// When the shape is not a valid lattice.
pub(crate) fn lattice_topology(m: u16, t: u8) -> BaseTopology {
    let (nodes, edges) = zephyr_lattice(m, t).expect("lattice");
    BaseTopology::new(nodes, &edges).expect("topology")
}
