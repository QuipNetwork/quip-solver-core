//! Generated family instances for conformance: the layered test
//! construction, the export adapter that regenerates canonical instances for
//! fixed nonces, and the comparison with the checked-in vectors that names
//! the first differing FAM-30 component.

use core::fmt;

use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits, Contract};
use ising_generators::family::{admit_topology, generate_instance, BaseTopology, SaltFault};
use ising_generators::instance::Instance;
use ising_generators::Code;
use serde_json::Value;

mod drive;
pub mod layered;

pub use drive::{FamilyCase, FamilyReport};

use layered::{P_F5000, P_L2_F0, P_L3_F0, P_SPEC, RESTRICTED_V1, UNIFORM_V1};

/// A family revision the `--family` gate can drive.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ReferenceRevision {
    /// The contract the gate regenerates with.
    pub contract: &'static Contract,
    /// Valid parameter sets, in the order the gate tries them.
    pub params: &'static [&'static [u8]],
    /// A parameter set that decodes but is out of bounds (code 9).
    pub out_of_bounds_params: &'static [u8],
}

/// Every revision the gate can drive: `UNIFORM_V1` and `RESTRICTED_V1`, each
/// with the parameter sets `P_SPEC`, `P_L2_F0`, and `P_L3_F0`.
pub static REFERENCE_REVISIONS: &[ReferenceRevision] = &[
    ReferenceRevision {
        contract: &UNIFORM_V1,
        params: &[&P_SPEC, &P_L2_F0, &P_L3_F0],
        out_of_bounds_params: &P_F5000,
    },
    ReferenceRevision {
        contract: &RESTRICTED_V1,
        params: &[&P_SPEC, &P_L2_F0, &P_L3_F0],
        out_of_bounds_params: &P_F5000,
    },
];

/// The reference revision for `family_id` at `revision`, if the gate can drive it.
#[must_use]
pub fn reference_revision(family_id: &str, revision: u32) -> Option<&'static ReferenceRevision> {
    REFERENCE_REVISIONS.iter().find(|reference| {
        reference.contract.family_id == family_id && reference.contract.revision == revision
    })
}

/// Generate the canonical instance of each nonce with `contract`, without
/// sampling. Admission uses no advertised limits, because the limits of one
/// solver never change generation (FAM-27).
///
/// # Errors
///
/// The admission code (3 to 13) when `params` or `topology` do not admit.
pub fn export_instances(
    contract: &Contract,
    params: &[u8],
    topology: &BaseTopology,
    nonces: &[[u8; 32]],
) -> Result<Vec<Result<Instance, SaltFault>>, Code> {
    let contracts = [*contract];
    let admitted = admit(
        &contracts,
        contract.family_id.as_bytes(),
        contract.revision,
        params,
        &params_digest(params),
    )?;
    let bounds = admit_topology(
        &admitted,
        topology,
        topology.order_digest(),
        &AdvertisedLimits::default(),
    )?;
    Ok(nonces
        .iter()
        .map(|nonce| generate_instance(&admitted, topology, &bounds, *nonce))
        .collect())
}

/// One generated-instance vector case.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VectorCase {
    /// The case name.
    pub name: String,
    /// The family id.
    pub family_id: String,
    /// The family revision.
    pub revision: u32,
    /// The parameter bytes.
    pub params: Vec<u8>,
    /// The base topology.
    pub topology: BaseTopology,
    /// The lease slots, sorted by index.
    pub salts: Vec<ExpectedSalt>,
}

/// What a vector says one lease slot gives.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExpectedSalt {
    /// The slot index within the lease.
    pub index: u64,
    /// The nonce of the slot.
    pub nonce: [u8; 32],
    /// The expected outcome.
    pub expected: Expected,
}

/// The expected outcome of one slot.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Expected {
    /// An instance with its canonical bytes, digest, and all-+1 energy.
    Instance {
        /// The canonical bytes (FAM-30).
        canonical: Vec<u8>,
        /// The instance digest (FAM-18).
        digest: [u8; 32],
        /// The all-+1 energy in milli.
        energy_all_up_milli: i64,
    },
    /// A skip with this code (13 or 14).
    Skip(u8),
}

fn member<'a>(value: &'a Value, key: &str, path: &str) -> Result<&'a Value, String> {
    value
        .get(key)
        .ok_or_else(|| format!("{path}.{key}: missing field"))
}

fn text<'a>(value: &'a Value, key: &str, path: &str) -> Result<&'a str, String> {
    member(value, key, path)?
        .as_str()
        .ok_or_else(|| format!("{path}.{key}: not a string"))
}

fn array<'a>(value: &'a Value, key: &str, path: &str) -> Result<&'a [Value], String> {
    member(value, key, path)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{path}.{key}: not an array"))
}

fn small_uint(value: &Value, key: &str, path: &str) -> Result<u64, String> {
    member(value, key, path)?
        .as_u64()
        .ok_or_else(|| format!("{path}.{key}: not an unsigned number"))
}

fn decimal<T: core::str::FromStr>(value: &Value, key: &str, path: &str) -> Result<T, String> {
    text(value, key, path)?
        .parse()
        .map_err(|_| format!("{path}.{key}: not a decimal string"))
}

fn hex_bytes(value: &Value, key: &str, path: &str) -> Result<Vec<u8>, String> {
    let hex = text(value, key, path)?;
    let bad = || format!("{path}.{key}: not lowercase hex of whole bytes");
    if hex.len() % 2 != 0 {
        return Err(bad());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .filter(|pair| pair.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(bad)
        })
        .collect()
}

fn hex_32(value: &Value, key: &str, path: &str) -> Result<[u8; 32], String> {
    hex_bytes(value, key, path)?
        .try_into()
        .map_err(|_| format!("{path}.{key}: not 32 bytes"))
}

fn node_pair(value: &Value, path: &str) -> Result<(u32, u32), String> {
    let pair = value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{path}: not an array"))?;
    let [u, v] = pair else {
        return Err(format!("{path}: not a pair"));
    };
    let id = |value: &Value| {
        value
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .ok_or_else(|| format!("{path}: not a node id"))
    };
    Ok((id(u)?, id(v)?))
}

fn parse_topology(value: &Value, path: &str) -> Result<BaseTopology, String> {
    let mut nodes = Vec::new();
    for (i, node) in array(value, "nodes", path)?.iter().enumerate() {
        let id = node
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .ok_or_else(|| format!("{path}.nodes[{i}]: not a node id"))?;
        nodes.push(id);
    }
    let mut edges = Vec::new();
    for (i, edge) in array(value, "edges", path)?.iter().enumerate() {
        edges.push(node_pair(edge, &format!("{path}.edges[{i}]"))?);
    }
    BaseTopology::new(nodes, &edges).map_err(|fault| format!("{path}: {fault:?}"))
}

fn parse_case(case: &Value, path: &str) -> Result<VectorCase, String> {
    let revision = u32::try_from(small_uint(case, "revision", path)?)
        .map_err(|_| format!("{path}.revision: out of range"))?;
    let topology = parse_topology(member(case, "topology", path)?, &format!("{path}.topology"))?;
    let mut salts = Vec::new();
    for (i, skip) in array(case, "skipped", path)?.iter().enumerate() {
        let at = format!("{path}.skipped[{i}]");
        let code = u8::try_from(small_uint(skip, "skip_code", &at)?)
            .map_err(|_| format!("{at}.skip_code: out of range"))?;
        salts.push(ExpectedSalt {
            index: decimal(skip, "index", &at)?,
            nonce: hex_32(skip, "nonce", &at)?,
            expected: Expected::Skip(code),
        });
    }
    for (i, instance) in array(case, "instances", path)?.iter().enumerate() {
        let at = format!("{path}.instances[{i}]");
        salts.push(ExpectedSalt {
            index: decimal(instance, "index", &at)?,
            nonce: hex_32(instance, "nonce", &at)?,
            expected: Expected::Instance {
                canonical: hex_bytes(instance, "canonical_hex", &at)?,
                digest: hex_32(instance, "instance_digest", &at)?,
                energy_all_up_milli: decimal(instance, "energy_all_up_milli", &at)?,
            },
        });
    }
    salts.sort_by_key(|salt| salt.index);
    Ok(VectorCase {
        name: text(case, "name", path)?.to_owned(),
        family_id: text(case, "family_id", path)?.to_owned(),
        revision,
        params: hex_bytes(case, "params_hex", path)?,
        topology,
        salts,
    })
}

/// Parse the `layered` section of a vector file such as `FAMILY_LAYERED`.
///
/// # Errors
///
/// A missing or malformed field, named by its JSON path.
pub fn parse_vector_cases(json: &str) -> Result<Vec<VectorCase>, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("$: {e}"))?;
    let cases = root
        .get("layered")
        .and_then(Value::as_array)
        .ok_or_else(|| "$.layered: missing array".to_owned())?;
    let mut parsed = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        parsed.push(parse_case(case, &format!("layered[{i}]"))?);
    }
    Ok(parsed)
}

/// A FAM-30 component, in FAM-30 order after the first two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Component {
    /// Admission of the parameters or the topology.
    Admission,
    /// The outcome of the slot: an instance or a skip.
    Outcome,
    /// The format byte.
    Format,
    /// The descriptor digest.
    DescriptorDigest,
    /// The topology order digest.
    TopologyOrderDigest,
    /// The nonce.
    Nonce,
    /// The node count.
    NodeCount,
    /// A node id.
    NodeIds,
    /// The edge count.
    EdgeCount,
    /// An edge.
    Edges,
    /// A field `h`.
    H,
    /// A coupling `J`.
    J,
    /// The energy offset.
    Offset,
    /// The metadata section count.
    MetadataSectionCount,
    /// A metadata section.
    MetadataSection,
    /// A side ends early, or has bytes left over.
    Length,
    /// The instance digest.
    InstanceDigest,
}

/// The first difference between an expected and a generated salt.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FirstDiff {
    /// The component that differs.
    pub component: Component,
    /// The element or section position, for the array components.
    pub index: Option<usize>,
    /// The expected value, printed.
    pub expected: String,
    /// The generated value, printed.
    pub got: String,
}

impl fmt::Display for FirstDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.component)?;
        if let Some(index) = self.index {
            write!(f, "[{index}]")?;
        }
        write!(f, ": expected {}, got {}", self.expected, self.got)
    }
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn le_u16(bytes: &[u8]) -> Option<u16> {
    bytes.first_chunk::<2>().map(|b| u16::from_le_bytes(*b))
}

fn le_u32(bytes: &[u8]) -> Option<u32> {
    bytes.first_chunk::<4>().map(|b| u32::from_le_bytes(*b))
}

fn show_u8(bytes: &[u8]) -> String {
    bytes.first().map_or_else(String::new, u8::to_string)
}

fn show_u32(bytes: &[u8]) -> String {
    le_u32(bytes).map_or_else(|| hex(bytes), |v| v.to_string())
}

fn show_i32(bytes: &[u8]) -> String {
    bytes
        .first_chunk::<4>()
        .map_or_else(|| hex(bytes), |b| i32::from_le_bytes(*b).to_string())
}

fn show_i64(bytes: &[u8]) -> String {
    bytes
        .first_chunk::<8>()
        .map_or_else(|| hex(bytes), |b| i64::from_le_bytes(*b).to_string())
}

fn show_edge(bytes: &[u8]) -> String {
    match (bytes.get(..4), bytes.get(4..8)) {
        (Some(u), Some(v)) => format!("({}, {})", show_u32(u), show_u32(v)),
        _ => hex(bytes),
    }
}

/// The two canonical byte strings, walked with the same position.
struct Walk<'a> {
    expected: &'a [u8],
    got: &'a [u8],
}

impl Walk<'_> {
    fn length_diff(&self) -> FirstDiff {
        FirstDiff {
            component: Component::Length,
            index: None,
            expected: format!("{} bytes", self.expected.len()),
            got: format!("{} bytes", self.got.len()),
        }
    }

    /// `a + b * c`, with an overflow reported as a `Length` difference. The
    /// bytes are not trusted, so a count can name an offset beyond `usize`.
    fn offset(&self, a: usize, b: usize, c: usize) -> Result<usize, FirstDiff> {
        b.checked_mul(c)
            .and_then(|product| a.checked_add(product))
            .ok_or_else(|| self.length_diff())
    }

    /// Compare `width` bytes at `start` on both sides. A side too short is a
    /// `Length` difference.
    fn step(
        &self,
        component: Component,
        index: Option<usize>,
        start: usize,
        width: usize,
        show: fn(&[u8]) -> String,
    ) -> Result<(), FirstDiff> {
        let range = start..self.offset(start, width, 1)?;
        let (Some(expected), Some(got)) = (self.expected.get(range.clone()), self.got.get(range))
        else {
            return Err(self.length_diff());
        };
        if expected == got {
            return Ok(());
        }
        Err(FirstDiff {
            component,
            index,
            expected: show(expected),
            got: show(got),
        })
    }

    /// Compare a count at `at`, then return its value from the expected side.
    fn count(&self, component: Component, at: usize) -> Result<usize, FirstDiff> {
        self.step(component, None, at, 4, show_u32)?;
        self.expected
            .get(at..)
            .and_then(le_u32)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| self.length_diff())
    }

    /// Compare `count` elements of `width` bytes starting at `start`.
    fn elements(
        &self,
        component: Component,
        start: usize,
        count: usize,
        width: usize,
        show: fn(&[u8]) -> String,
    ) -> Result<(), FirstDiff> {
        for i in 0..count {
            self.step(
                component,
                Some(i),
                self.offset(start, i, width)?,
                width,
                show,
            )?;
        }
        Ok(())
    }

    fn metadata(&self, start: usize) -> Result<(), FirstDiff> {
        self.step(Component::MetadataSectionCount, None, start, 2, |b| {
            le_u16(b).map_or_else(|| hex(b), |v| v.to_string())
        })?;
        let sections = self
            .expected
            .get(start..)
            .and_then(le_u16)
            .ok_or_else(|| self.length_diff())?;
        let mut at = self.offset(start, 2, 1)?;
        for section in 0..usize::from(sections) {
            // The header (kind, then length) is compared before the payload.
            self.step(Component::MetadataSection, Some(section), at, 6, hex)?;
            let length = self
                .expected
                .get(self.offset(at, 2, 1)?..)
                .and_then(le_u32)
                .and_then(|length| usize::try_from(length).ok())
                .ok_or_else(|| self.length_diff())?;
            let payload_at = self.offset(at, 6, 1)?;
            self.step(
                Component::MetadataSection,
                Some(section),
                payload_at,
                length,
                hex,
            )?;
            at = self.offset(payload_at, length, 1)?;
        }
        if self.expected.len() != at || self.got.len() != at {
            return Err(self.length_diff());
        }
        Ok(())
    }

    fn run(&self) -> Result<(), FirstDiff> {
        self.step(Component::Format, None, 0, 1, show_u8)?;
        self.step(Component::DescriptorDigest, None, 1, 32, hex)?;
        self.step(Component::TopologyOrderDigest, None, 33, 32, hex)?;
        self.step(Component::Nonce, None, 65, 32, hex)?;
        let nodes = self.count(Component::NodeCount, 97)?;
        self.elements(Component::NodeIds, 101, nodes, 4, show_u32)?;
        let edge_count_at = self.offset(101, nodes, 4)?;
        let edges = self.count(Component::EdgeCount, edge_count_at)?;
        let edges_at = self.offset(edge_count_at, 1, 4)?;
        self.elements(Component::Edges, edges_at, edges, 8, show_edge)?;
        let h_at = self.offset(edges_at, edges, 8)?;
        self.elements(Component::H, h_at, nodes, 4, show_i32)?;
        let j_at = self.offset(h_at, nodes, 4)?;
        self.elements(Component::J, j_at, edges, 4, show_i32)?;
        let offset_at = self.offset(j_at, edges, 4)?;
        self.step(Component::Offset, None, offset_at, 8, show_i64)?;
        self.metadata(self.offset(offset_at, 8, 1)?)
    }
}

/// Walk both canonical byte strings in FAM-30 order, one cursor each.
/// `None` when they are equal.
#[must_use]
pub fn first_diff(expected: &[u8], got: &[u8]) -> Option<FirstDiff> {
    Walk { expected, got }.run().err()
}

/// Compare one generated salt with its vector: outcome, then `first_diff`,
/// then the digest.
///
/// # Errors
///
/// The first difference.
pub fn compare(
    expected: &ExpectedSalt,
    got: &Result<Instance, SaltFault>,
) -> Result<(), FirstDiff> {
    let outcome = |expected: String, got: String| FirstDiff {
        component: Component::Outcome,
        index: None,
        expected,
        got,
    };
    match (&expected.expected, got) {
        (Expected::Skip(code), Err(SaltFault::Skip(skip))) if skip.value() == *code => Ok(()),
        (Expected::Skip(code), Err(fault)) => {
            Err(outcome(format!("skip code {code}"), format!("{fault:?}")))
        }
        (Expected::Skip(code), Ok(_)) => {
            Err(outcome(format!("skip code {code}"), "instance".to_owned()))
        }
        (Expected::Instance { .. }, Err(fault)) => {
            Err(outcome("instance".to_owned(), format!("{fault:?}")))
        }
        (
            Expected::Instance {
                canonical, digest, ..
            },
            Ok(instance),
        ) => {
            let bytes = instance.canonical_bytes().map_err(|e| FirstDiff {
                component: Component::Length,
                index: None,
                expected: format!("{} bytes", canonical.len()),
                got: e.to_string(),
            })?;
            if let Some(diff) = first_diff(canonical, &bytes) {
                return Err(diff);
            }
            let generated = instance.digest();
            if generated == *digest {
                return Ok(());
            }
            Err(FirstDiff {
                component: Component::InstanceDigest,
                index: None,
                expected: hex(digest),
                got: hex(&generated),
            })
        }
    }
}

/// Where a case first differs. `salt_index` is `None` for an admission
/// failure.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CaseMismatch {
    /// The slot index, or `None` for an admission failure.
    pub salt_index: Option<u64>,
    /// The first differing component.
    pub diff: FirstDiff,
}

/// Export every salt of `case` with `contract` and compare each in index
/// order.
///
/// # Errors
///
/// The first salt that differs, or the admission code as
/// `Component::Admission`.
pub fn check_case(contract: &Contract, case: &VectorCase) -> Result<(), CaseMismatch> {
    let nonces: Vec<[u8; 32]> = case.salts.iter().map(|salt| salt.nonce).collect();
    let generated =
        export_instances(contract, &case.params, &case.topology, &nonces).map_err(|code| {
            CaseMismatch {
                salt_index: None,
                diff: FirstDiff {
                    component: Component::Admission,
                    index: None,
                    expected: "admission".to_owned(),
                    got: format!("code {}", code.value()),
                },
            }
        })?;
    for (salt, got) in case.salts.iter().zip(&generated) {
        compare(salt, got).map_err(|diff| CaseMismatch {
            salt_index: Some(salt.index),
            diff,
        })?;
    }
    Ok(())
}
