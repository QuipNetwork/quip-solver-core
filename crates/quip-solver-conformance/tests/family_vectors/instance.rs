//! Canonical instance bytes, edge and metadata rules, bounds, energy, and spins
//! (FAM-30 to FAM-37, FAM-53), written from the spec without the Python oracle.

/// Largest metadata region in bytes (FAM-37).
const MAX_METADATA: usize = 1 << 26;
/// Largest `|h|` or `|J|` in milli units (FAM-37).
const MAX_COEFFICIENT: u64 = 1 << 30;
/// Largest `|offset|` in milli units (FAM-37).
const MAX_OFFSET: u64 = 1 << 56;
/// Highest metadata section kind (FAM-34).
const MAX_KIND: u16 = 5;

/// Everything that goes into the canonical instance bytes.
pub(crate) struct InstanceParts {
    pub(crate) descriptor_digest: [u8; 32],
    pub(crate) topology_order_digest: [u8; 32],
    pub(crate) nonce: Vec<u8>,
    pub(crate) node_ids: Vec<u32>,
    pub(crate) edges: Vec<(u32, u32)>,
    pub(crate) h: Vec<i32>,
    pub(crate) j: Vec<i32>,
    pub(crate) offset_milli: i64,
    pub(crate) metadata: Vec<u8>,
}

fn push_len(out: &mut Vec<u8>, len: usize) -> Result<(), String> {
    let n = u32::try_from(len).map_err(|_| format!("count {len} does not fit u32"))?;
    out.extend_from_slice(&n.to_le_bytes());
    Ok(())
}

/// FAM-30 canonical bytes. The nonce is 0 bytes (archived, written as 32 zero
/// bytes) or 32 bytes.
pub(crate) fn canonical_bytes(parts: &InstanceParts) -> Result<Vec<u8>, String> {
    if parts.h.len() != parts.node_ids.len() {
        return Err("h length differs from n_nodes".to_owned());
    }
    if parts.j.len() != parts.edges.len() {
        return Err("j length differs from n_edges".to_owned());
    }
    let mut out = vec![0x01];
    out.extend_from_slice(&parts.descriptor_digest);
    out.extend_from_slice(&parts.topology_order_digest);
    match parts.nonce.len() {
        0 => out.extend_from_slice(&[0; 32]),
        32 => out.extend_from_slice(&parts.nonce),
        n => return Err(format!("nonce is {n} bytes, expected 0 or 32")),
    }
    push_len(&mut out, parts.node_ids.len())?;
    for id in &parts.node_ids {
        out.extend_from_slice(&id.to_le_bytes());
    }
    push_len(&mut out, parts.edges.len())?;
    for (u, v) in &parts.edges {
        out.extend_from_slice(&u.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    }
    for value in parts.h.iter().chain(&parts.j) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&parts.offset_milli.to_le_bytes());
    out.extend_from_slice(&parts.metadata);
    Ok(out)
}

/// First edge rule broken: `FAM-30` for a position at or past `n_nodes`
/// (checked over every edge first), then `FAM-32` for `u >= v`, a duplicate, or
/// an order that is not strictly ascending.
pub(crate) fn edge_fault(edges: &[(u32, u32)], n_nodes: usize) -> Option<&'static str> {
    let in_range = |p: u32| usize::try_from(p).is_ok_and(|p| p < n_nodes);
    if edges.iter().any(|&(u, v)| !in_range(u) || !in_range(v)) {
        return Some("FAM-30");
    }
    if edges.iter().any(|&(u, v)| u >= v) {
        return Some("FAM-32");
    }
    if edges.windows(2).any(|pair| match pair {
        [a, b] => a >= b,
        _ => false,
    }) {
        return Some("FAM-32");
    }
    None
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let raw: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(raw))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let raw: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}

/// FAM-34 and FAM-37 metadata checks: 13 for an oversized region, 16 for any
/// layout or kind fault, `None` when valid.
pub(crate) fn metadata_code(region: &[u8], contract_kinds: &[u16]) -> Option<u8> {
    if region.len() > MAX_METADATA {
        return Some(13);
    }
    let Some(count) = read_u16(region, 0) else {
        return Some(16);
    };
    let mut pos = 2_usize;
    let mut previous = 0_u16;
    for _ in 0..count {
        let (Some(kind), Some(length)) = (read_u16(region, pos), read_u32(region, pos + 2)) else {
            return Some(16);
        };
        if kind == 0 || kind > MAX_KIND || kind <= previous || !contract_kinds.contains(&kind) {
            return Some(16);
        }
        previous = kind;
        let Some(end) = usize::try_from(length)
            .ok()
            .and_then(|l| l.checked_add(pos + 6))
        else {
            return Some(16);
        };
        if end > region.len() {
            return Some(16);
        }
        pos = end;
    }
    if pos == region.len() {
        None
    } else {
        Some(16)
    }
}

/// Code 13 when `|v|` exceeds the coefficient bound.
pub(crate) fn coefficient_code(value: i64) -> Option<u8> {
    (value.unsigned_abs() > MAX_COEFFICIENT).then_some(13)
}

/// Code 13 when `|v|` exceeds the offset bound.
pub(crate) fn offset_code(value: i64) -> Option<u8> {
    (value.unsigned_abs() > MAX_OFFSET).then_some(13)
}

fn spin_value(spin: i8) -> i64 {
    i64::from(spin)
}

/// FAM-33 plain energy: `sum(h_i s_i) + sum(J_uv s_u s_v)` in exact `i64`.
pub(crate) fn energy_plain(parts: &InstanceParts, spins: &[i8]) -> Result<i64, String> {
    if spins.len() != parts.h.len() {
        return Err("spin count differs from n_nodes".to_owned());
    }
    let overflow = || "energy overflows i64".to_owned();
    let mut total = 0_i64;
    for (h, s) in parts.h.iter().zip(spins) {
        let term = i64::from(*h)
            .checked_mul(spin_value(*s))
            .ok_or_else(overflow)?;
        total = total.checked_add(term).ok_or_else(overflow)?;
    }
    for ((u, v), j) in parts.edges.iter().zip(&parts.j) {
        let su = spins
            .get(usize::try_from(*u).map_err(|e| e.to_string())?)
            .ok_or("edge position out of range")?;
        let sv = spins
            .get(usize::try_from(*v).map_err(|e| e.to_string())?)
            .ok_or("edge position out of range")?;
        let term = i64::from(*j)
            .checked_mul(spin_value(*su))
            .and_then(|t| t.checked_mul(spin_value(*sv)))
            .ok_or_else(overflow)?;
        total = total.checked_add(term).ok_or_else(overflow)?;
    }
    Ok(total)
}

/// Energy including the offset.
pub(crate) fn energy_with_offset(parts: &InstanceParts, spins: &[i8]) -> Result<i64, String> {
    energy_plain(parts, spins)?
        .checked_add(parts.offset_milli)
        .ok_or_else(|| "energy overflows i64".to_owned())
}

/// FAM-53 packing: node `i` is bit `i % 8` of byte `i / 8`; bit 1 is +1.
pub(crate) fn pack_spins(spins: &[i8]) -> Vec<u8> {
    let mut out = vec![0_u8; spins.len().div_ceil(8)];
    for (i, spin) in spins.iter().enumerate() {
        if *spin > 0 {
            if let Some(byte) = out.get_mut(i / 8) {
                *byte |= 1 << (i % 8);
            }
        }
    }
    out
}

/// FAM-53 unpacking. Errors are the fixture reasons `wrong-length` and
/// `padding-bit-set`.
pub(crate) fn unpack_spins(bytes: &[u8], n_nodes: usize) -> Result<Vec<i8>, &'static str> {
    if bytes.len() != n_nodes.div_ceil(8) {
        return Err("wrong-length");
    }
    let bit = |i: usize| bytes.get(i / 8).is_some_and(|b| b >> (i % 8) & 1 == 1);
    if (n_nodes..bytes.len() * 8).any(bit) {
        return Err("padding-bit-set");
    }
    Ok((0..n_nodes).map(|i| if bit(i) { 1 } else { -1 }).collect())
}
