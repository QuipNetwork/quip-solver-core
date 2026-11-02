//! Family constructions: the base topology a lease builds on, the seeded
//! substreams, and the one function that turns a nonce into a canonical
//! instance (FAM-21 to FAM-27, FAM-30 to FAM-38, FAM-47, FAM-52).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::descriptor::{
    check_size_bounds, topology_order_digest, Admitted, AdvertisedLimits, Params, SizeBounds,
    TopologyStats,
};
use crate::instance::{Instance, InstanceParts};
use crate::rng::{stream_seed, ChaCha8Rng};
use crate::Code;

/// The largest degree over `edges`, each edge counted once at each end.
///
/// An edge end at or past `n_nodes` counts for no node.
fn max_degree(n_nodes: usize, edges: &[(u32, u32)]) -> u64 {
    let mut degree = vec![0u64; n_nodes];
    for &(u, v) in edges {
        for end in [u, v] {
            let slot = usize::try_from(end)
                .ok()
                .and_then(|pos| degree.get_mut(pos));
            if let Some(slot) = slot {
                *slot = slot.saturating_add(1);
            }
        }
    }
    degree.into_iter().max().unwrap_or(0)
}

/// A base topology in registration order (FAM-19, FAM-38).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseTopology {
    node_ids: Vec<u32>,
    edges: Vec<(u32, u32)>,
    stats: TopologyStats,
    order_digest: [u8; 32],
}

/// Why a wire topology cannot be a base topology.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopologyFault {
    /// A node id appears twice.
    DuplicateNode(u32),
    /// An edge names a node that is not in the node list.
    UnknownNode(u32),
    /// A node or edge count is above `u32::MAX`.
    TooLarge,
}

impl BaseTopology {
    /// Build a base topology. `native_edges` holds native ids in
    /// registration order, not normalized.
    ///
    /// # Errors
    ///
    /// A repeated node, an edge to an unknown node, or a count above
    /// `u32::MAX`.
    pub fn new(node_ids: Vec<u32>, native_edges: &[(u32, u32)]) -> Result<Self, TopologyFault> {
        let too_large = TopologyFault::TooLarge;
        let n_nodes = u32::try_from(node_ids.len()).map_err(|_| too_large)?;
        if u32::try_from(native_edges.len()).is_err() {
            return Err(too_large);
        }
        let mut position: BTreeMap<u32, u32> = BTreeMap::new();
        for (pos, id) in (0..n_nodes).zip(&node_ids) {
            if position.insert(*id, pos).is_some() {
                return Err(TopologyFault::DuplicateNode(*id));
            }
        }
        let lookup = |id: u32| {
            position
                .get(&id)
                .copied()
                .ok_or(TopologyFault::UnknownNode(id))
        };
        let mut edges = Vec::with_capacity(native_edges.len());
        for &(u, v) in native_edges {
            edges.push((lookup(u)?, lookup(v)?));
        }
        let order_digest = topology_order_digest(&node_ids, native_edges).ok_or(too_large)?;
        let stats = TopologyStats {
            n_nodes: u64::from(n_nodes),
            n_edges: u64::try_from(edges.len()).map_err(|_| too_large)?,
            max_degree: max_degree(node_ids.len(), &edges),
        };
        Ok(Self {
            node_ids,
            edges,
            stats,
            order_digest,
        })
    }

    /// The native node ids in registration order.
    #[must_use]
    pub fn node_ids(&self) -> &[u32] {
        &self.node_ids
    }

    /// Dense positions into `node_ids`, in registration order, not normalized.
    #[must_use]
    pub fn edges(&self) -> &[(u32, u32)] {
        &self.edges
    }

    /// The statistics a size bound reads. `max_degree` counts each
    /// registered edge.
    #[must_use]
    pub const fn stats(&self) -> &TopologyStats {
        &self.stats
    }

    /// The topology order digest (FAM-19).
    #[must_use]
    pub const fn order_digest(&self) -> &[u8; 32] {
        &self.order_digest
    }
}

/// The substreams of one salt (FAM-21, FAM-22).
#[derive(Clone, Debug)]
pub struct Streams<'a> {
    nonce: [u8; 32],
    descriptor_digest: [u8; 32],
    order_digest: [u8; 32],
    labels: &'a [&'a str],
    fault: bool,
}

impl<'a> Streams<'a> {
    /// The substreams of the salt with `nonce`. `labels` are the labels the
    /// construction lists.
    #[must_use]
    pub const fn new(
        nonce: [u8; 32],
        descriptor_digest: [u8; 32],
        topology_order_digest: [u8; 32],
        labels: &'a [&'a str],
    ) -> Self {
        Self {
            nonce,
            descriptor_digest,
            order_digest: topology_order_digest,
            labels,
            fault: false,
        }
    }

    /// The stream for `label`, at keystream word 0. `None` when the
    /// construction does not list `label`. The salt is then a generator
    /// defect, whatever the construction returns. A label that is listed but
    /// invalid sets the same fault and reports [`Defect::UnlistedLabel`]; a
    /// declaration that passed `Declaration::validate` never reaches that
    /// case, because validation refuses invalid labels.
    pub fn open(&mut self, label: &str) -> Option<ChaCha8Rng> {
        let seed = self
            .labels
            .contains(&label)
            .then(|| {
                stream_seed(
                    &self.nonce,
                    &self.descriptor_digest,
                    &self.order_digest,
                    label.as_bytes(),
                )
                .ok()
            })
            .flatten();
        if seed.is_none() {
            self.fault = true;
        }
        seed.map(ChaCha8Rng::from_seed)
    }
}

/// FAM-12 code 12 at admission: `true` when the revision can build on
/// `topology`.
pub type CheckTopologyFn = fn(&Params<'_>, &BaseTopology) -> bool;

/// Build the body of one instance. `Err(BoundsExceeded)` for an overflow or
/// `Err(NoInstance)` for retry exhaustion skips the salt. Any other error is a
/// defect.
pub type GenerateFn =
    fn(&Params<'_>, &BaseTopology, &mut Streams<'_>) -> Result<InstanceBody, Code>;

/// FAM-36: `true` when the metadata of `instance` is valid against it.
pub type CheckMetadataFn = fn(&Params<'_>, &Instance) -> bool;

/// The construction items of a family contract (FAM-38).
#[derive(Clone, Copy, Debug)]
pub struct Construction {
    /// Every substream label the construction reads.
    pub labels: &'static [&'static str],
    /// The topology check of admission.
    pub check_topology: CheckTopologyFn,
    /// The instance builder.
    pub generate: GenerateFn,
    /// The metadata check of FAM-36.
    pub check_metadata: CheckMetadataFn,
}

/// An instance before the digests and nonce are added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceBody {
    /// Node ids in output order.
    pub node_ids: Vec<u32>,
    /// Edges as dense positions into `node_ids`, sorted ascending.
    pub edges: Vec<(u32, u32)>,
    /// The linear coefficients in milli.
    pub h: Vec<i32>,
    /// The quadratic coefficients in milli, in edge order.
    pub j: Vec<i32>,
    /// The offset in milli.
    pub offset_milli: i64,
    /// The metadata region, from the `u16` section count.
    pub metadata: Vec<u8>,
}

/// Why one salt gives no instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaltFault {
    /// Code 13 or 14 during generation: skip the salt, continue the lease
    /// (FAM-26, FAM-27, FAM-52).
    Skip(Code),
    /// The construction broke its contract. Every verifier rejects a `Result`
    /// for this salt.
    Defect(Defect),
}

/// How a construction broke its contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Defect {
    /// The construction opened a label its contract does not list.
    UnlistedLabel,
    /// The construction returned a code other than 13 or 14.
    ConstructionCode(Code),
    /// The instance failed validation with code 15 or 16.
    InstanceInvalid(Code),
    /// The instance is beyond the size bounds `N`, `E`, or `D`.
    SizeBound,
    /// The construction rejected the metadata of its own instance.
    MetadataRejected,
    /// The contract has no construction.
    NoConstruction,
}

/// Codes 11, 12, 13 of FAM-47 step 5, in order. Returns the admitted size
/// bounds.
///
/// # Errors
///
/// 11 when `sent_order_digest` differs from `topology.order_digest()`;
/// 12 for a self-loop, a contract without a construction, or `check_topology`
/// false;
/// 13 from the size bounds (overflow, contract limits, advertised limits).
pub fn admit_topology(
    admitted: &Admitted<'_>,
    topology: &BaseTopology,
    sent_order_digest: &[u8],
    advertised: &AdvertisedLimits,
) -> Result<SizeBounds, Code> {
    if sent_order_digest != topology.order_digest().as_slice() {
        return Err(Code::TopologyOrderMismatch);
    }
    let contract = admitted.contract();
    let construction = contract.construction.ok_or(Code::TopologyIncompatible)?;
    if topology.edges().iter().any(|&(u, v)| u == v)
        || !(construction.check_topology)(admitted.params(), topology)
    {
        return Err(Code::TopologyIncompatible);
    }
    let bounds = admitted.size_bounds(topology.stats())?;
    check_size_bounds(&bounds, &contract.limits, advertised)?;
    Ok(bounds)
}

/// Whether `instance` is beyond `bounds` in nodes, edges, or degree.
fn beyond(instance: &Instance, bounds: &SizeBounds) -> bool {
    let over = |len: usize, bound: u64| u64::try_from(len).map_or(true, |len| len > bound);
    over(instance.node_ids().len(), bounds.nodes)
        || over(instance.edges().len(), bounds.edges)
        || max_degree(instance.node_ids().len(), instance.edges()) > bounds.degree
}

/// Build the canonical instance of `nonce`: streams, body, `Instance::new`
/// with the contract limits and metadata kinds, the size bounds, then
/// `check_metadata`.
///
/// # Errors
///
/// `Skip(13 | 14)` from the construction, or `Skip(13)` from `Instance::new`;
/// `Defect` for an unlisted label, any other construction code, code 15 or 16
/// from `Instance::new`, a count or degree above `bounds`, or failed
/// metadata.
pub fn generate_instance(
    admitted: &Admitted<'_>,
    topology: &BaseTopology,
    bounds: &SizeBounds,
    nonce: [u8; 32],
) -> Result<Instance, SaltFault> {
    let contract = admitted.contract();
    let construction = contract
        .construction
        .ok_or(SaltFault::Defect(Defect::NoConstruction))?;
    let descriptor_digest = admitted.descriptor().digest();
    let mut streams = Streams::new(
        nonce,
        descriptor_digest,
        *topology.order_digest(),
        construction.labels,
    );
    let built = (construction.generate)(admitted.params(), topology, &mut streams);
    if streams.fault {
        return Err(SaltFault::Defect(Defect::UnlistedLabel));
    }
    let body = match built {
        Ok(body) => body,
        Err(code @ (Code::BoundsExceeded | Code::NoInstance)) => {
            return Err(SaltFault::Skip(code));
        }
        Err(code) => return Err(SaltFault::Defect(Defect::ConstructionCode(code))),
    };
    let parts = InstanceParts {
        descriptor_digest,
        topology_order_digest: *topology.order_digest(),
        nonce: Some(nonce),
        node_ids: body.node_ids,
        edges: body.edges,
        h: body.h,
        j: body.j,
        offset_milli: body.offset_milli,
        metadata: body.metadata,
    };
    let instance =
        Instance::new(parts, &contract.limits, contract.metadata_kinds, None).map_err(|code| {
            match code {
                Code::BoundsExceeded => SaltFault::Skip(code),
                other => SaltFault::Defect(Defect::InstanceInvalid(other)),
            }
        })?;
    if beyond(&instance, bounds) {
        return Err(SaltFault::Defect(Defect::SizeBound));
    }
    if !(construction.check_metadata)(admitted.params(), &instance) {
        return Err(SaltFault::Defect(Defect::MetadataRejected));
    }
    Ok(instance)
}
