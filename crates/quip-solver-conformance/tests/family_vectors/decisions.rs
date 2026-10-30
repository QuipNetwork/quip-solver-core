//! Stable codes, decision orders, support entries, field lengths, and size
//! bounds (FAM-38, FAM-42, FAM-43, FAM-46, FAM-47, FAM-61, FAM-62, FAM-65),
//! written from the spec without the Python oracle.

use super::codec;
use serde_json::Value;

/// Largest node count a family instance may have (FAM-37).
const MAX_NODES: u64 = 1 << 24;
/// Largest edge count a family instance may have (FAM-37).
const MAX_EDGES: u64 = 1 << 26;
/// Stable code for exceeded bounds (FAM-42).
const BOUNDS_EXCEEDED: u8 = 13;

/// One stable code row (FAM-42). Generation-phase wire is always null.
pub(crate) struct CodeRow {
    pub(crate) code: u8,
    pub(crate) name: &'static str,
    pub(crate) phases: &'static [&'static str],
    pub(crate) wire_admission: Option<&'static str>,
    pub(crate) wire_explicit_job: Option<&'static str>,
}

const fn row(
    code: u8,
    name: &'static str,
    phases: &'static [&'static str],
    wire_admission: Option<&'static str>,
    wire_explicit_job: Option<&'static str>,
) -> CodeRow {
    CodeRow {
        code,
        name,
        phases,
        wire_admission,
        wire_explicit_job,
    }
}

pub(crate) const CODES: [CodeRow; 16] = [
    row(
        1,
        "DescriptorMissing",
        &["admission"],
        Some("MALFORMED"),
        None,
    ),
    row(
        2,
        "DescriptorUnexpected",
        &["admission"],
        Some("MALFORMED"),
        None,
    ),
    row(
        3,
        "FamilyIdInvalid",
        &["admission"],
        Some("MALFORMED"),
        None,
    ),
    row(4, "RevisionZero", &["admission"], Some("MALFORMED"), None),
    row(
        5,
        "FamilyUnsupported",
        &["admission"],
        Some("FAMILY_UNSUPPORTED"),
        None,
    ),
    row(
        6,
        "RevisionUnsupported",
        &["admission"],
        Some("FAMILY_UNSUPPORTED"),
        None,
    ),
    row(
        7,
        "ParamsMalformed",
        &["admission", "explicit-job"],
        Some("MALFORMED"),
        Some("MALFORMED"),
    ),
    row(
        8,
        "ParamsDigestMismatch",
        &["admission", "explicit-job"],
        Some("MALFORMED"),
        Some("MALFORMED"),
    ),
    row(
        9,
        "ParamsOutOfBounds",
        &["admission", "explicit-job"],
        Some("MALFORMED"),
        Some("MALFORMED"),
    ),
    row(
        10,
        "ParamsRestricted",
        &["admission", "explicit-job"],
        Some("FAMILY_UNSUPPORTED"),
        Some("FAMILY_UNSUPPORTED"),
    ),
    row(
        11,
        "TopologyOrderMismatch",
        &["admission"],
        Some("TOPOLOGY_MISMATCH"),
        None,
    ),
    row(
        12,
        "TopologyIncompatible",
        &["admission"],
        Some("FAMILY_UNSUPPORTED"),
        None,
    ),
    row(
        13,
        "BoundsExceeded",
        &["admission", "generation", "explicit-job"],
        Some("TOO_LARGE"),
        Some("TOO_LARGE"),
    ),
    row(14, "NoInstance", &["generation"], None, None),
    row(
        15,
        "InstanceDigestMismatch",
        &["explicit-job"],
        None,
        Some("MALFORMED"),
    ),
    row(
        16,
        "MetadataInvalid",
        &["explicit-job"],
        None,
        Some("MALFORMED"),
    ),
];

/// Lease admission steps in order (FAM-47). The first failing step wins.
pub(crate) const LEASE_ADMISSION_ORDER: [&str; 22] = [
    "session:expired",
    "session:topology-missing",
    "session:target-missing",
    "field:generator-absent",
    "field:unknown-generator",
    "field:hash-length",
    "field:salt-count-zero",
    "field:salt-range-overflow",
    "code:1",
    "code:2",
    "code:3",
    "code:4",
    "code:5",
    "code:6",
    "code:7",
    "code:8",
    "code:9",
    "code:10",
    "session:topology-hash-mismatch",
    "code:11",
    "code:12",
    "code:13",
];

/// Explicit-job steps in order (FAM-65). The first failing step wins.
pub(crate) const EXPLICIT_JOB_ORDER: [&str; 8] = [
    "tier:not-listed",
    "code:7",
    "code:8",
    "code:9",
    "code:10",
    "code:13",
    "code:15",
    "code:16",
];

/// Index in `order` of the first step that appears in `faults`.
pub(crate) fn winner(order: &[&str], faults: &[String]) -> Result<usize, String> {
    if let Some(unknown) = faults.iter().find(|f| !order.contains(&f.as_str())) {
        return Err(format!("fault {unknown} is not a step of this order"));
    }
    order
        .iter()
        .position(|step| faults.iter().any(|f| f == step))
        .ok_or_else(|| "no faults".to_owned())
}

fn wire_of(code: u8, explicit_job: bool) -> Option<&'static str> {
    CODES.iter().find(|r| r.code == code).and_then(|r| {
        if explicit_job {
            r.wire_explicit_job
        } else {
            r.wire_admission
        }
    })
}

/// Stable code and wire reason that a winning step produces.
pub(crate) fn step_outcome(step: &str, explicit_job: bool) -> (Option<u8>, Option<&'static str>) {
    match step {
        "session:expired" => (None, Some("EXPIRED")),
        "session:topology-missing" => (None, Some("TOPOLOGY_MISSING")),
        "session:target-missing" => (None, Some("TARGET_MISSING")),
        "session:topology-hash-mismatch" => (None, Some("TOPOLOGY_MISMATCH")),
        _ if step.starts_with("field:") => (None, Some("MALFORMED")),
        _ => match step
            .strip_prefix("code:")
            .and_then(|n| n.parse::<u8>().ok())
        {
            Some(code) => (Some(code), wire_of(code, explicit_job)),
            None => (None, None),
        },
    }
}

/// Which support entries claim support (FAM-61, FAM-62).
pub(crate) fn entry_claims(entries: &[Value]) -> Result<Vec<bool>, String> {
    let ids = entries
        .iter()
        .map(|e| entry_id(e))
        .collect::<Result<Vec<_>, _>>()?;
    let mut claims = Vec::with_capacity(entries.len());
    for (entry, id) in entries.iter().zip(&ids) {
        let unique = ids.iter().filter(|other| *other == id).count() == 1;
        let revisions = entry
            .get("revisions")
            .and_then(Value::as_array)
            .ok_or("entry has no revisions array")?;
        let revisions_ok =
            !revisions.is_empty() && revisions.iter().all(|r| r.as_u64().is_some_and(|n| n != 0));
        let digests_ok = entry_digests(entry)?
            .iter()
            .all(|d| matches!(codec::hex_decode(d), Ok(bytes) if bytes.len() == 32));
        let grammar_ok = codec::family_id_code(id.as_bytes(), true).is_none();
        claims.push(unique && revisions_ok && digests_ok && grammar_ok);
    }
    Ok(claims)
}

fn entry_id(entry: &Value) -> Result<&str, String> {
    entry
        .get("family_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "entry has no family_id".to_owned())
}

fn entry_digests(entry: &Value) -> Result<Vec<&str>, String> {
    entry
        .get("params_digests")
        .and_then(Value::as_array)
        .ok_or("entry has no params_digests array")?
        .iter()
        .map(|d| {
            d.as_str()
                .ok_or_else(|| "digest is not a string".to_owned())
        })
        .collect()
}

/// Lease admission code from generated families (FAM-43, FAM-62): 5, 6, 10,
/// or none. Only claiming entries count.
pub(crate) fn lease_code(
    entries: &[Value],
    family_id: &str,
    revision: u32,
    params_digest: &[u8],
) -> Result<Option<u8>, String> {
    let claims = entry_claims(entries)?;
    let mut family_seen = false;
    let mut listed = Vec::new();
    for (entry, claims_support) in entries.iter().zip(claims) {
        if !claims_support || entry_id(entry)? != family_id {
            continue;
        }
        family_seen = true;
        let has_revision = entry
            .get("revisions")
            .and_then(Value::as_array)
            .is_some_and(|rs| rs.iter().any(|r| r.as_u64() == Some(u64::from(revision))));
        if has_revision {
            listed.push(entry);
        }
    }
    if !family_seen {
        return Ok(Some(5));
    }
    if listed.is_empty() {
        return Ok(Some(6));
    }
    for entry in listed {
        let digests = entry_digests(entry)?;
        if digests.is_empty() {
            return Ok(None);
        }
        for d in digests {
            if codec::hex_decode(d)? == params_digest {
                return Ok(None);
            }
        }
    }
    Ok(Some(10))
}

/// Code for a wrong-length extension field (FAM-46).
pub(crate) fn field_length_code(
    field: &str,
    length: usize,
    plain_node_count: Option<usize>,
) -> Result<Option<u8>, String> {
    let wrong = |bad: bool, code: u8| Ok(bad.then_some(code));
    match field {
        "FamilyDescriptor.params_digest" => wrong(length != 32, 8),
        "IsingProblemGenerator.topology_order_digest" => wrong(length != 32, 1),
        "FamilyBinding.nonce" => wrong(length != 0 && length != 32, 15),
        "FamilyBinding.instance_digest" | "FamilyBinding.topology_order_digest" => {
            wrong(length != 32, 15)
        }
        "FamilyBinding.node_ids" => {
            let plain = plain_node_count.ok_or("node_ids needs plain_node_count")?;
            wrong(length != plain, 15)
        }
        other => Err(format!("unknown field {other}")),
    }
}

/// Size bounds of an example family (spec Examples): `(nodes, edges, degree)`.
/// `None` means a u64 overflow.
pub(crate) fn example_size_bounds(
    n_layers: u64,
    n_nodes: u64,
    n_edges: u64,
    max_degree: u64,
) -> Option<(u64, u64, u64)> {
    let extra_layers = n_layers.saturating_sub(1);
    let nodes = n_layers.checked_mul(n_nodes)?;
    let edges = n_layers
        .checked_mul(n_edges)?
        .checked_add(extra_layers.checked_mul(n_nodes)?)?;
    let degree = max_degree.checked_add(extra_layers.min(2))?;
    Some((nodes, edges, degree))
}

/// Code 13 when the bounds overflow, break the instance bounds, or exceed a
/// claimed limit. A limit of `None` or 0 is no claim.
pub(crate) fn size_bound_code(
    bounds: Option<(u64, u64, u64)>,
    limit_nodes: Option<u64>,
    limit_edges: Option<u64>,
    limit_degree: Option<u64>,
) -> Option<u8> {
    let Some((nodes, edges, degree)) = bounds else {
        return Some(BOUNDS_EXCEEDED);
    };
    let over = |value: u64, limit: Option<u64>| limit.is_some_and(|l| l != 0 && value > l);
    let exceeded = nodes > MAX_NODES
        || edges > MAX_EDGES
        || over(nodes, limit_nodes)
        || over(edges, limit_edges)
        || over(degree, limit_degree);
    exceeded.then_some(BOUNDS_EXCEEDED)
}
