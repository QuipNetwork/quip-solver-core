//! The size bounds of an admitted descriptor, for tests that declare it with
//! `#[path = "common/size_bounds.rs"] mod size_bounds;`.

use ising_generators::descriptor::{Admitted, AdvertisedLimits, SizeBounds};
use ising_generators::family::{admit_topology, BaseTopology};
use ising_generators::Code;

/// The size bounds of an admitted descriptor on `topology`, with the
/// topology's own order digest and default advertised limits.
///
/// # Errors
///
/// The admission code.
pub(crate) fn bounds(admitted: &Admitted<'_>, topology: &BaseTopology) -> Result<SizeBounds, Code> {
    admit_topology(
        admitted,
        topology,
        topology.order_digest(),
        &AdvertisedLimits::default(),
    )
}
