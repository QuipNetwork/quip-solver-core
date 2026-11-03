//! Golden vectors, embedded from this crate's own manifest directory.
//!
//! Resolving against `CARGO_MANIFEST_DIR` here, rather than in each consumer,
//! is what lets a solver repository delete its vendored copy. The bytes travel
//! with the version pin.

use ising_generators::blob;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Target and diversity vectors generated from the validator's
/// `quantum-validation` crate.
pub const GOLDEN_TARGET: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/golden_target.json"
));

/// Lease salt, nonce, and draw vectors generated from the validator's
/// `quantum-validation` crate.
pub const GOLDEN_LEASE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/golden_lease.json"
));

/// Raw `golden_adapt.json`: adaptive-parameter parity with the Python source.
pub const GOLDEN_ADAPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/golden_adapt.json"
));

/// Raw `golden_vectors.json`: consensus parity for `ChaCha8`, nonce derivation,
/// energy, energy rounding, the non-finite sentinel, diversity, the Ising
/// draw, and truncation.
pub const GOLDEN_VECTORS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/golden_vectors.json"
));

/// Raw `golden_wire.json`: byte-level protobuf encodings and enum numbers.
pub const GOLDEN_WIRE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/golden_wire.json"
));

/// The `golden_wire.json` fixture: the canonical bytes of one fully-populated
/// instance of each message, plus the numeric value of every enum variant.
///
/// The other fixtures pin *semantics* — what a solver must compute. This one
/// pins *representation*: which field lands on which tag, and which number a
/// variant carries. Renumbering a field or reordering an enum is a silent,
/// cross-language break that no behavioural test sees, because both sides of a
/// single-language round trip change together.
#[derive(Debug, Clone, Deserialize)]
pub struct GoldenWire {
    /// Lower-case hex of the canonical encoding of each named message.
    pub messages: BTreeMap<String, String>,
    /// Variant name to wire number, keyed by proto enum name.
    pub enums: BTreeMap<String, BTreeMap<String, i32>>,
}

/// The parsed `golden_wire.json` fixture.
///
/// # Panics
///
/// Panics when the embedded JSON does not match the schema. That is a
/// build-time fixture error, not a runtime condition.
#[must_use]
pub fn golden_wire() -> GoldenWire {
    #[expect(
        clippy::expect_used,
        reason = "the fixture is embedded at build time; a schema mismatch is a build error"
    )]
    serde_json::from_str(GOLDEN_WIRE).expect("golden_wire.json matches the fixture schema")
}

/// One `energy_to_difficulty` case: a difficulty target and the graph it is
/// measured against. Field names are the verbatim JSON keys.
#[derive(Debug, Clone, Deserialize)]
pub struct EnergyToDifficultyCase {
    /// Acceptance ceiling in milli-energy.
    pub target_milli: i64,
    /// Variables in the graph.
    pub num_nodes: usize,
    /// Edges in the graph.
    pub num_edges: usize,
    /// The h-field values the topology advertises, in milli units.
    pub allowed_h_milli: Vec<i32>,
    /// Normalized difficulty the Python source produces for this case. This is
    /// the expected output.
    pub difficulty: f64,
    /// Ground-state-energy estimate at `c = 0.75`, carried for diagnostics when
    /// a mismatch needs explaining.
    pub gse_c75: f64,
}

/// One `adapt_params` case. Field names are the verbatim JSON keys; `num_reads`
/// and `num_sweeps` are the expected outputs, the rest are inputs.
#[derive(Debug, Clone, Deserialize)]
pub struct AdaptParamsCase {
    /// Acceptance ceiling in milli-energy.
    pub target_milli: i64,
    /// Variables in the graph.
    pub num_nodes: usize,
    /// Edges in the graph.
    pub num_edges: usize,
    /// The h-field values the topology advertises, in milli units.
    pub allowed_h_milli: Vec<i32>,
    /// Solutions the coordinator asks for.
    pub min_solutions: u32,
    /// Which `AdaptBounds` preset the case is measured against. Every current
    /// case says `"cpu_sa"`.
    pub bounds: String,
    /// Reads the Python source produces. Expected output.
    pub num_reads: usize,
    /// Sweeps the Python source produces. Expected output.
    pub num_sweeps: usize,
}

#[derive(Deserialize)]
struct AdaptFile {
    energy_to_difficulty: Vec<EnergyToDifficultyCase>,
    adapt_params_cpu_sa: Vec<AdaptParamsCase>,
}

/// Every `energy_to_difficulty` case.
///
/// # Panics
///
/// Panics when the embedded JSON does not match the case schema. That is a
/// build-time fixture error, not a runtime condition.
#[must_use]
pub fn adapt_cases() -> Vec<EnergyToDifficultyCase> {
    parse_adapt().energy_to_difficulty
}

/// Every `adapt_params` case for the CPU simulated-annealing bounds.
///
/// # Panics
///
/// Panics when the embedded JSON does not match the case schema.
#[must_use]
pub fn adapt_params_cases() -> Vec<AdaptParamsCase> {
    parse_adapt().adapt_params_cpu_sa
}

fn parse_adapt() -> AdaptFile {
    #[expect(
        clippy::expect_used,
        reason = "the fixture is embedded at build time; a schema mismatch is a build error"
    )]
    serde_json::from_str(GOLDEN_ADAPT).expect("golden_adapt.json matches the case schema")
}

/// JSON Schema for the problem-family fixtures under `vectors/families/`
/// (schema revision 1). It uses a small keyword subset that the integration
/// tests check themselves.
pub const FAMILY_SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/families/schema.json"
));

/// Specified family golden vectors: identity, parameters, descriptors, and the
/// rest of the SC-2 set (schema revision 1, provenance `specified`).
pub const FAMILY_CORE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/families/core.json"
));

/// References to the legacy production draw vectors, pinned by case hash
/// (schema revision 1, provenance `legacy-production`).
pub const FAMILY_LEGACY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/families/legacy.json"
));

/// Generated instances of the layered test construction for fixed lease
/// nonces (schema revision 1, provenance `test-construction`).
pub const FAMILY_LAYERED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/families/layered.json"
));

/// The family registry: status, contract, and frozen vectors of each family revision (FAM-08, FAM-09).
pub const FAMILY_REGISTRY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/families/registry.json"
));

/// The topology index: every file under `vectors/topologies/` with its name,
/// kind, counts, digests, provenance, and source (`docs/topology-format.md`).
pub const TOPOLOGY_INDEX: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/vectors/topologies/index.json"
));

macro_rules! topology_files {
    ($($name:literal),* $(,)?) => {
        &[$((
            $name,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/vectors/topologies/",
                $name
            )),
        )),*]
    };
}

/// Every topology file under `vectors/topologies/` as `(file name, bytes)`,
/// sorted by name. A test checks that this list matches the directory.
pub const TOPOLOGY_FILES: &[(&str, &[u8])] = topology_files![
    "error-degree-above-limit.bin.gz",
    "error-duplicate-edge.bin.gz",
    "error-membership-above-two.bin.gz",
    "error-mixed-shape.bin.gz",
    "error-not-bipartite.bin.gz",
    "error-not-zephyr-coupler.bin.gz",
    "error-not-zephyr-node.bin.gz",
    "error-spine-conflict.bin.gz",
    "first-check-duplicate-before-coupler.bin.gz",
    "first-check-node-before-coupler.bin.gz",
    "reverse-ten.bin.gz",
    "sparse-edge-flipped.bin.gz",
    "sparse-edges-reordered.bin.gz",
    "sparse-nodes-reordered.bin.gz",
    "t-sparse.bin.gz",
    "tie-ring-4.bin.gz",
    "z12-zephyr.bin.gz",
    "z22-cut-edge-zephyr.bin.gz",
    "z22-permuted-order-zephyr.bin.gz",
    "z22-zephyr.bin.gz",
    "zephyr-z12t4-live.bin.gz",
];

/// Why [`topology`] failed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TopologyError {
    /// The reference does not name a file under `vectors/topologies/`.
    NotFound(String),
    /// The file is not a valid topology blob.
    Blob(blob::BlobError),
}

impl core::fmt::Display for TopologyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotFound(reference) => {
                write!(f, "no topology file for the reference {reference}")
            }
            Self::Blob(error) => write!(f, "invalid topology file: {error}"),
        }
    }
}

impl std::error::Error for TopologyError {}

/// The bytes of the topology file that `reference` names.
///
/// `reference` is the string a JSON file under `vectors/families/` holds, for
/// example `../topologies/t-sparse.bin.gz`. It is resolved from
/// `vectors/families/`, the only directory that references topologies today,
/// and only a file in `vectors/topologies/` is found.
///
/// # Errors
///
/// [`TopologyError::NotFound`] when the reference names no embedded file.
pub fn topology_bytes(reference: &str) -> Result<&'static [u8], TopologyError> {
    let not_found = || TopologyError::NotFound(reference.to_owned());
    let mut at = vec!["families"];
    for part in reference.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if at.pop().is_none() {
                    return Err(not_found());
                }
            }
            name => at.push(name),
        }
    }
    let ["topologies", file] = at.as_slice() else {
        return Err(not_found());
    };
    TOPOLOGY_FILES
        .iter()
        .find(|(name, _)| name == file)
        .map(|&(_, bytes)| bytes)
        .ok_or_else(not_found)
}

/// Decodes the topology file that `reference` names into native ids, in
/// stored order. See [`topology_bytes`] for the form of `reference`.
///
/// # Errors
///
/// [`TopologyError::NotFound`] for an unknown reference, and
/// [`TopologyError::Blob`] when the file does not decode.
pub fn topology(reference: &str) -> Result<blob::Topology, TopologyError> {
    blob::decode(topology_bytes(reference)?)
        .and_then(|body| body.topology())
        .map_err(TopologyError::Blob)
}

/// The shared `T_SPARSE` topology: sparse native ids, one of them
/// `u32::MAX`. It decodes `vectors/topologies/t-sparse.bin.gz`.
///
/// # Errors
///
/// [`TopologyError`] when the embedded file is missing or does not decode.
/// A test pins that it does, so callers treat an error as a broken build.
pub fn t_sparse() -> Result<blob::Topology, TopologyError> {
    topology("../topologies/t-sparse.bin.gz")
}
