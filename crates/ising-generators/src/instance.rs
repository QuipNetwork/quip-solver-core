//! Canonical instance bytes, limits, and metadata validation.

use alloc::collections::TryReserveError;
use alloc::vec::Vec;

use crate::digest::{BufferedHasher, ByteSink};
use crate::Code;

/// The canonical instance format byte (FAM-30).
pub const INSTANCE_FORMAT: u8 = 0x01;

/// The coefficient scale: one unit is 1000 milli.
pub const MILLI_SCALE: i32 = 1000;

/// The highest registered metadata section kind (FAM-34).
const MAX_KIND: u16 = 5;

/// Bytes of the fixed head: format byte and three 32-byte fields.
const HEAD_LEN: usize = 1 + 32 * 3;

/// Instance bounds (FAM-37). A family revision may declare tighter bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceLimits {
    nodes: u32,
    edges: u32,
    coefficient: u32,
    offset: u64,
    metadata: u32,
}

impl InstanceLimits {
    /// The protocol bounds of FAM-37.
    pub const FAM_37: Self = Self {
        nodes: 16_777_216,
        edges: 67_108_864,
        coefficient: 1_073_741_824,
        offset: 72_057_594_037_927_936,
        metadata: 67_108_864,
    };

    /// Build limits that are at most the FAM-37 bounds.
    ///
    /// Returns `None` when any value is above the matching FAM-37 bound.
    #[must_use]
    pub const fn new(
        max_nodes: u32,
        max_edges: u32,
        max_coefficient: u32,
        max_offset: u64,
        max_metadata: u32,
    ) -> Option<Self> {
        let top = Self::FAM_37;
        if max_nodes > top.nodes
            || max_edges > top.edges
            || max_coefficient > top.coefficient
            || max_offset > top.offset
            || max_metadata > top.metadata
        {
            return None;
        }
        Some(Self {
            nodes: max_nodes,
            edges: max_edges,
            coefficient: max_coefficient,
            offset: max_offset,
            metadata: max_metadata,
        })
    }

    /// The largest node count.
    #[must_use]
    pub const fn max_nodes(&self) -> u32 {
        self.nodes
    }

    /// The largest edge count.
    #[must_use]
    pub const fn max_edges(&self) -> u32 {
        self.edges
    }

    /// The largest magnitude of an `h` or `J` value, in milli.
    #[must_use]
    pub const fn max_coefficient(&self) -> u32 {
        self.coefficient
    }

    /// The largest magnitude of the offset, in milli.
    #[must_use]
    pub const fn max_offset(&self) -> u64 {
        self.offset
    }

    /// The largest metadata region, in bytes.
    #[must_use]
    pub const fn max_metadata(&self) -> u32 {
        self.metadata
    }
}

/// Check the metadata region layout (FAM-34).
///
/// The region size bound is not checked here. `Instance::new` checks it.
///
/// # Errors
///
/// Returns [`Code::MetadataInvalid`] (16) for an unregistered kind, a kind the
/// contract does not list, a kind not above the preceding kind, a section
/// length past the end of the region, a region shorter than the 2-byte
/// section count, or bytes after the last declared section.
pub fn validate_metadata(region: &[u8], contract_kinds: &[u16]) -> Result<(), Code> {
    let bad = Code::MetadataInvalid;
    let count = read_u16(region, 0).ok_or(bad)?;
    let mut pos: usize = 2;
    let mut last_kind: u16 = 0;
    for _ in 0..count {
        let kind = read_u16(region, pos).ok_or(bad)?;
        let length = read_u32(region, pos.checked_add(2).ok_or(bad)?).ok_or(bad)?;
        pos = pos.checked_add(6).ok_or(bad)?;
        if kind == 0 || kind > MAX_KIND || !contract_kinds.contains(&kind) || kind <= last_kind {
            return Err(bad);
        }
        last_kind = kind;
        let length = usize::try_from(length).map_err(|_| bad)?;
        let end = pos.checked_add(length).ok_or(bad)?;
        if end > region.len() {
            return Err(bad);
        }
        pos = end;
    }
    if pos == region.len() {
        Ok(())
    } else {
        Err(bad)
    }
}

fn read_u16(region: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    let bytes: [u8; 2] = region.get(at..end)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

fn read_u32(region: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    let bytes: [u8; 4] = region.get(at..end)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

/// The fields of an instance, before validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceParts {
    /// The descriptor digest.
    pub descriptor_digest: [u8; 32],
    /// The topology order digest.
    pub topology_order_digest: [u8; 32],
    /// The nonce. `None` is an archived instance and encodes as 32 zero bytes.
    pub nonce: Option<[u8; 32]>,
    /// Node ids in output order.
    pub node_ids: Vec<u32>,
    /// Edges as dense positions into `node_ids`.
    pub edges: Vec<(u32, u32)>,
    /// The linear coefficients in milli, in node output order.
    pub h: Vec<i32>,
    /// The quadratic coefficients in milli, in edge order.
    pub j: Vec<i32>,
    /// The offset in milli.
    pub offset_milli: i64,
    /// The metadata region, from the `u16` section count.
    pub metadata: Vec<u8>,
}

/// A validated instance with its canonical encoding length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    parts: InstanceParts,
    canonical_len: usize,
}

fn exceeds(len: usize, max: u32) -> bool {
    usize::try_from(max).is_ok_and(|m| len > m)
}

/// The canonical length in checked arithmetic, `None` on overflow.
fn checked_len(parts: &InstanceParts) -> Option<usize> {
    let n = parts.node_ids.len();
    let e = parts.edges.len();
    HEAD_LEN
        .checked_add(4)?
        .checked_add(n.checked_mul(4)?)?
        .checked_add(4)?
        .checked_add(e.checked_mul(8)?)?
        .checked_add(n.checked_mul(4)?)?
        .checked_add(e.checked_mul(4)?)?
        .checked_add(8)?
        .checked_add(parts.metadata.len())
}

fn count_bytes(len: usize) -> [u8; 4] {
    u32::try_from(len).unwrap_or(u32::MAX).to_le_bytes()
}

/// The one canonical encoder (FAM-30). Counts are below 2^32 because the
/// instance bounds were checked.
fn encode<S: ByteSink>(parts: &InstanceParts, sink: &mut S) {
    sink.put(&[INSTANCE_FORMAT]);
    sink.put(&parts.descriptor_digest);
    sink.put(&parts.topology_order_digest);
    sink.put(&parts.nonce.unwrap_or([0; 32]));
    sink.put(&count_bytes(parts.node_ids.len()));
    for id in &parts.node_ids {
        sink.put(&id.to_le_bytes());
    }
    sink.put(&count_bytes(parts.edges.len()));
    for (u, v) in &parts.edges {
        sink.put(&u.to_le_bytes());
        sink.put(&v.to_le_bytes());
    }
    for value in &parts.h {
        sink.put(&value.to_le_bytes());
    }
    for value in &parts.j {
        sink.put(&value.to_le_bytes());
    }
    sink.put(&parts.offset_milli.to_le_bytes());
    sink.put(&parts.metadata);
}

/// The instance digest (FAM-18) of parts, streamed without allocation.
fn digest_of(parts: &InstanceParts) -> [u8; 32] {
    let mut hasher = BufferedHasher::new("quip family instance v1");
    encode(parts, &mut hasher);
    hasher.finish()
}

impl Instance {
    /// Validate the parts. The first failure wins, in the FAM-65 order
    /// (steps 2 to 4):
    ///
    /// 1. Code 13: the bounds.
    /// 2. Code 15: the `h` and `j` lengths, the edge positions, `u < v`, and
    ///    the ascending edge order.
    /// 3. Code 15: when `expected_digest` is `Some`, a comparison with the
    ///    instance digest of `parts` as given.
    /// 4. Code 16: the metadata layout.
    /// 5. Code 13: the canonical length overflows.
    ///
    /// `None` skips the digest comparison. Use it for a generated or archived
    /// instance that has no sent digest.
    ///
    /// # Errors
    ///
    /// - [`Code::BoundsExceeded`] (13): a count, coefficient, offset, or
    ///   metadata size is above `limits`, or the canonical length overflows.
    /// - [`Code::InstanceDigestMismatch`] (15): `h` or `j` has the wrong
    ///   length, an edge position is at or past the node count, an edge has
    ///   `u >= v`, the edges are not strictly ascending, or the digest of
    ///   `parts` differs from `expected_digest`.
    /// - [`Code::MetadataInvalid`] (16): the metadata layout is faulty.
    pub fn new(
        parts: InstanceParts,
        limits: &InstanceLimits,
        contract_kinds: &[u16],
        expected_digest: Option<&[u8; 32]>,
    ) -> Result<Self, Code> {
        let over = Code::BoundsExceeded;
        if exceeds(parts.node_ids.len(), limits.nodes)
            || exceeds(parts.edges.len(), limits.edges)
            || exceeds(parts.metadata.len(), limits.metadata)
            || parts
                .h
                .iter()
                .chain(&parts.j)
                .any(|v| v.unsigned_abs() > limits.coefficient)
            || parts.offset_milli.unsigned_abs() > limits.offset
        {
            return Err(over);
        }
        let mismatch = Code::InstanceDigestMismatch;
        let n = parts.node_ids.len();
        if parts.h.len() != n || parts.j.len() != parts.edges.len() {
            return Err(mismatch);
        }
        let outside = |p: u32| usize::try_from(p).map_or(true, |p| p >= n);
        if parts.edges.iter().any(|&(u, v)| outside(u) || outside(v))
            || parts.edges.iter().any(|&(u, v)| u >= v)
            || parts
                .edges
                .windows(2)
                .any(|w| matches!(w, [a, b] if a >= b))
        {
            return Err(mismatch);
        }
        if expected_digest.is_some_and(|sent| *sent != digest_of(&parts)) {
            return Err(mismatch);
        }
        validate_metadata(&parts.metadata, contract_kinds)?;
        let canonical_len = checked_len(&parts).ok_or(over)?;
        Ok(Self {
            parts,
            canonical_len,
        })
    }

    /// The descriptor digest.
    #[must_use]
    pub const fn descriptor_digest(&self) -> &[u8; 32] {
        &self.parts.descriptor_digest
    }

    /// The topology order digest.
    #[must_use]
    pub const fn topology_order_digest(&self) -> &[u8; 32] {
        &self.parts.topology_order_digest
    }

    /// The nonce, `None` for an archived instance.
    #[must_use]
    pub const fn nonce(&self) -> Option<&[u8; 32]> {
        self.parts.nonce.as_ref()
    }

    /// Node ids in output order.
    #[must_use]
    pub fn node_ids(&self) -> &[u32] {
        &self.parts.node_ids
    }

    /// Edges as dense positions.
    #[must_use]
    pub fn edges(&self) -> &[(u32, u32)] {
        &self.parts.edges
    }

    /// The linear coefficients in milli.
    #[must_use]
    pub fn h(&self) -> &[i32] {
        &self.parts.h
    }

    /// The quadratic coefficients in milli.
    #[must_use]
    pub fn j(&self) -> &[i32] {
        &self.parts.j
    }

    /// The offset in milli.
    #[must_use]
    pub const fn offset_milli(&self) -> i64 {
        self.parts.offset_milli
    }

    /// The metadata region.
    #[must_use]
    pub fn metadata(&self) -> &[u8] {
        &self.parts.metadata
    }

    /// The length of the canonical bytes.
    #[must_use]
    pub const fn canonical_len(&self) -> usize {
        self.canonical_len
    }

    /// The canonical instance bytes (FAM-30). One reservation, no growth.
    ///
    /// # Errors
    ///
    /// Returns the allocator error when the reservation fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TryReserveError> {
        let mut out = Vec::new();
        out.try_reserve_exact(self.canonical_len)?;
        encode(&self.parts, &mut out);
        debug_assert_eq!(out.len(), self.canonical_len);
        Ok(out)
    }

    /// The instance digest (FAM-18): BLAKE3 in key-derivation mode over the
    /// canonical bytes, streamed without allocation.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        digest_of(&self.parts)
    }
}
