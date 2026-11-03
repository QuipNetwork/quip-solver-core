//! The topology blob format: a gzip member that holds a varint payload.
//!
//! The format is specified in `docs/topology-format.md`. A payload stores
//! either an explicit topology or a Zephyr lattice minus removals. Decoding
//! is bounded: the payload is at most [`MAX_PAYLOAD`] bytes, every count is
//! checked against the remaining bytes before any allocation, and all
//! arithmetic is checked.

use alloc::boxed::Box;
use alloc::vec::Vec;

use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZFlush, MZStatus};

use crate::ice::{zephyr_coordinates, zephyr_coupler, zephyr_lattice};

/// The largest payload, in bytes, that a reader accepts or a writer emits.
pub const MAX_PAYLOAD: usize = 64 << 20;

const MAGIC: [u8; 4] = *b"QTOP";
const VERSION: u8 = 1;
const KIND_EXPLICIT: u8 = 0;
const KIND_ZEPHYR: u8 = 1;
/// Gzip header written by [`compress`]: no flags, MTIME 0, XFL 2, OS 255.
const GZIP_HEADER: [u8; 10] = [0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x02, 0xff];
const GZIP_TRAILER: usize = 8;
const CHUNK: usize = 32 * 1024;
/// Largest up-front reservation for a decoded sequence. Vectors grow past it.
const INITIAL_CAPACITY: usize = 1 << 16;

/// A decoded topology in native ids, in stored order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Topology {
    /// Node ids in stored order.
    pub nodes: Vec<u32>,
    /// Edges as stored: order, orientation, duplicates, and self-loops kept.
    pub edges: Vec<(u32, u32)>,
}

/// How a payload stores its topology.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// Every node and edge, in registration order.
    Explicit(Topology),
    /// A Zephyr z(m, t) lattice minus removals.
    ZephyrRemoved {
        /// Lattice size, 1 to 32.
        m: u16,
        /// Tile size, 1 to 8.
        t: u8,
        /// Removed lattice nodes, strictly ascending.
        removed_nodes: Vec<u32>,
        /// Removed lattice couplers `(a, b)` with `a < b`, strictly ascending,
        /// with both ends present.
        removed_edges: Vec<(u32, u32)>,
    },
}

/// Why a blob failed to encode or decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlobError {
    /// The gzip framing is malformed, truncated, or followed by more bytes.
    Gzip,
    /// The CRC-32 or the ISIZE field does not match the payload.
    Checksum,
    /// The payload is above [`MAX_PAYLOAD`].
    TooLarge,
    /// The payload does not start with the `QTOP` magic.
    Magic,
    /// The version byte is missing or not 1.
    Version,
    /// The kind byte is missing or not 0 or 1.
    Kind,
    /// A varint is truncated, longer than 10 bytes, above `u64::MAX`, or not
    /// minimal.
    Varint,
    /// A value or count is out of range, or a count exceeds the remaining
    /// bytes.
    Range,
    /// A sequence that must ascend does not.
    Order,
    /// A removed node or edge is not part of the lattice.
    NotLattice,
    /// The payload has bytes after the body.
    Trailing,
}

// --- CRC-32 and gzip -------------------------------------------------------

#[expect(
    clippy::cast_possible_truncation,
    clippy::indexing_slicing,
    reason = "const fn: i < 256 fits u32 and indexes a 256-entry table"
)]
const fn crc_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut i = 0_usize;
    while i < 256 {
        let mut c = i as u32; // i < 256
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in data {
        let index = usize::try_from((crc ^ u32::from(byte)) & 0xff).unwrap_or(0);
        let entry = CRC_TABLE.get(index).copied().unwrap_or(0);
        crc = entry ^ (crc >> 8);
    }
    !crc
}

/// The length modulo 2^32, as the gzip ISIZE field stores it.
fn isize_field(len: usize) -> u32 {
    u32::try_from(len & 0xFFFF_FFFF).unwrap_or(u32::MAX)
}

/// Wraps a payload in one gzip member with the header the spec fixes.
#[must_use]
pub fn compress(payload: &[u8]) -> Vec<u8> {
    let deflated = compress_to_vec(payload, 9);
    let mut out = Vec::with_capacity(GZIP_HEADER.len() + deflated.len() + GZIP_TRAILER);
    out.extend_from_slice(&GZIP_HEADER);
    out.extend_from_slice(&deflated);
    out.extend_from_slice(&crc32(payload).to_le_bytes());
    out.extend_from_slice(&isize_field(payload.len()).to_le_bytes());
    out
}

/// Inflates a raw DEFLATE stream. Returns the payload and the bytes consumed.
fn inflate_raw(input: &[u8]) -> Result<(Vec<u8>, usize), BlobError> {
    let mut state: Box<InflateState> = InflateState::new_boxed(DataFormat::Raw);
    let mut out: Vec<u8> = Vec::new();
    let mut chunk = alloc::vec![0_u8; CHUNK];
    let mut consumed = 0_usize;
    loop {
        let rest = input.get(consumed..).ok_or(BlobError::Gzip)?;
        let step = inflate(&mut state, rest, &mut chunk, MZFlush::None);
        consumed = consumed
            .checked_add(step.bytes_consumed)
            .ok_or(BlobError::Gzip)?;
        let written = chunk.get(..step.bytes_written).ok_or(BlobError::Gzip)?;
        let total = out
            .len()
            .checked_add(written.len())
            .ok_or(BlobError::TooLarge)?;
        if total > MAX_PAYLOAD {
            return Err(BlobError::TooLarge);
        }
        out.extend_from_slice(written);
        match step.status {
            Ok(MZStatus::StreamEnd) => return Ok((out, consumed)),
            Ok(_) if step.bytes_consumed > 0 || step.bytes_written > 0 => {}
            _ => return Err(BlobError::Gzip),
        }
    }
}

/// Strips the gzip member and checks it. Returns the payload.
///
/// # Errors
///
/// [`BlobError::Gzip`] for bad framing, truncation, or bytes after the
/// member. [`BlobError::Checksum`] for a CRC-32 or ISIZE mismatch.
/// [`BlobError::TooLarge`] when the payload would exceed [`MAX_PAYLOAD`].
pub fn decompress(file: &[u8]) -> Result<Vec<u8>, BlobError> {
    let header = file.get(..GZIP_HEADER.len()).ok_or(BlobError::Gzip)?;
    // Bytes 0 to 3 are ID1, ID2, CM, FLG. MTIME, XFL, and OS are ignored.
    if header.get(..4) != GZIP_HEADER.get(..4) {
        return Err(BlobError::Gzip);
    }
    let body = file.get(GZIP_HEADER.len()..).ok_or(BlobError::Gzip)?;
    let (payload, consumed) = inflate_raw(body)?;
    let trailer = body.get(consumed..).ok_or(BlobError::Gzip)?;
    if trailer.len() != GZIP_TRAILER {
        return Err(BlobError::Gzip);
    }
    let crc = trailer.get(..4).ok_or(BlobError::Gzip)?;
    let size = trailer.get(4..).ok_or(BlobError::Gzip)?;
    if crc != crc32(&payload).to_le_bytes() || size != isize_field(payload.len()).to_le_bytes() {
        return Err(BlobError::Checksum);
    }
    Ok(payload)
}

// --- varints ---------------------------------------------------------------

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = u8::try_from(value & 0x7f).unwrap_or(0);
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

fn count(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

fn zigzag(delta: i64) -> u64 {
    if delta >= 0 {
        u64::try_from(delta).unwrap_or(0) << 1
    } else {
        (u64::try_from(!delta).unwrap_or(0) << 1) | 1
    }
}

fn unzigzag(value: u64) -> Result<i64, BlobError> {
    let half = i64::try_from(value >> 1).map_err(|_| BlobError::Range)?;
    Ok(if value & 1 == 1 { !half } else { half })
}

fn put_zigzag(out: &mut Vec<u8>, delta: i64) {
    put_varint(out, zigzag(delta));
}

/// A bounds-checked cursor over the payload.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn byte(&mut self) -> Option<u8> {
        let value = *self.bytes.get(self.pos)?;
        self.pos = self.pos.checked_add(1)?;
        Some(value)
    }

    fn varint(&mut self) -> Result<u64, BlobError> {
        let mut value = 0_u64;
        for index in 0..10_u32 {
            let byte = self.byte().ok_or(BlobError::Varint)?;
            let group = u64::from(byte & 0x7f);
            let shifted = group
                .checked_shl(index * 7)
                .filter(|shifted| shifted >> (index * 7) == group)
                .ok_or(BlobError::Varint)?;
            value |= shifted;
            if byte & 0x80 == 0 {
                if byte == 0 && index > 0 {
                    return Err(BlobError::Varint);
                }
                return Ok(value);
            }
        }
        Err(BlobError::Varint)
    }

    /// A count whose items each take at least `min_bytes` of what remains.
    fn count(&mut self, min_bytes: usize) -> Result<usize, BlobError> {
        let count = usize::try_from(self.varint()?).map_err(|_| BlobError::Range)?;
        let needed = count.checked_mul(min_bytes).ok_or(BlobError::Range)?;
        if needed > self.remaining() {
            return Err(BlobError::Range);
        }
        Ok(count)
    }

    fn id_delta(&mut self, base: i64) -> Result<i64, BlobError> {
        let delta = unzigzag(self.varint()?)?;
        base.checked_add(delta).ok_or(BlobError::Range)
    }
}

fn to_id(value: i64) -> Result<u32, BlobError> {
    u32::try_from(value).map_err(|_| BlobError::Range)
}

// --- payload ---------------------------------------------------------------

/// Checks the lattice rules of a `ZephyrRemoved` body.
fn validate_zephyr(
    m: u16,
    t: u8,
    removed_nodes: &[u32],
    removed_edges: &[(u32, u32)],
) -> Result<(), BlobError> {
    if !(1..=32).contains(&m) || !(1..=8).contains(&t) {
        return Err(BlobError::Range);
    }
    let mut previous: Option<u32> = None;
    for &node in removed_nodes {
        if previous.is_some_and(|before| node <= before) {
            return Err(BlobError::Order);
        }
        previous = Some(node);
        if zephyr_coordinates(m, t, node).is_none() {
            return Err(BlobError::NotLattice);
        }
    }
    let mut previous: Option<(u32, u32)> = None;
    for &(a, b) in removed_edges {
        if a >= b || previous.is_some_and(|before| (a, b) <= before) {
            return Err(BlobError::Order);
        }
        previous = Some((a, b));
        let present =
            removed_nodes.binary_search(&a).is_err() && removed_nodes.binary_search(&b).is_err();
        if !present || !zephyr_coupler(m, t, a, b) {
            return Err(BlobError::NotLattice);
        }
    }
    Ok(())
}

/// Serialises a body to the payload bytes.
///
/// # Errors
///
/// [`BlobError::Range`], [`BlobError::Order`], or [`BlobError::NotLattice`]
/// when a `ZephyrRemoved` body breaks the lattice rules.
/// [`BlobError::TooLarge`] when the payload would exceed [`MAX_PAYLOAD`].
pub fn encode_payload(body: &Body) -> Result<Vec<u8>, BlobError> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    match body {
        Body::Explicit(topology) => {
            out.push(KIND_EXPLICIT);
            put_varint(&mut out, count(topology.nodes.len()));
            let mut previous = 0_i64;
            for &id in &topology.nodes {
                put_zigzag(&mut out, i64::from(id) - previous);
                previous = i64::from(id);
            }
            put_varint(&mut out, count(topology.edges.len()));
            let mut previous_u = 0_i64;
            for &(u, v) in &topology.edges {
                put_zigzag(&mut out, i64::from(u) - previous_u);
                put_zigzag(&mut out, i64::from(v) - i64::from(u));
                previous_u = i64::from(u);
            }
        }
        Body::ZephyrRemoved {
            m,
            t,
            removed_nodes,
            removed_edges,
        } => {
            validate_zephyr(*m, *t, removed_nodes, removed_edges)?;
            out.push(KIND_ZEPHYR);
            put_varint(&mut out, u64::from(*m));
            put_varint(&mut out, u64::from(*t));
            put_varint(&mut out, count(removed_nodes.len()));
            let mut previous = 0_u32;
            for &id in removed_nodes {
                put_varint(&mut out, u64::from(id - previous));
                previous = id;
            }
            put_varint(&mut out, count(removed_edges.len()));
            let mut previous_a = 0_u32;
            for &(a, b) in removed_edges {
                put_varint(&mut out, u64::from(a - previous_a));
                put_varint(&mut out, u64::from(b - a));
                previous_a = a;
            }
        }
    }
    if out.len() > MAX_PAYLOAD {
        return Err(BlobError::TooLarge);
    }
    Ok(out)
}

fn decode_explicit(reader: &mut Reader<'_>) -> Result<Body, BlobError> {
    let node_count = reader.count(1)?;
    let mut nodes = Vec::with_capacity(node_count.min(INITIAL_CAPACITY));
    let mut previous = 0_i64;
    for _ in 0..node_count {
        previous = reader.id_delta(previous)?;
        nodes.push(to_id(previous)?);
    }
    let edge_count = reader.count(2)?;
    let mut edges = Vec::with_capacity(edge_count.min(INITIAL_CAPACITY));
    let mut previous_u = 0_i64;
    for _ in 0..edge_count {
        let u = reader.id_delta(previous_u)?;
        let v = reader.id_delta(u)?;
        edges.push((to_id(u)?, to_id(v)?));
        previous_u = u;
    }
    Ok(Body::Explicit(Topology { nodes, edges }))
}

fn decode_zephyr(reader: &mut Reader<'_>) -> Result<Body, BlobError> {
    let m = u16::try_from(reader.varint()?).map_err(|_| BlobError::Range)?;
    let t = u8::try_from(reader.varint()?).map_err(|_| BlobError::Range)?;
    if !(1..=32).contains(&m) || !(1..=8).contains(&t) {
        return Err(BlobError::Range);
    }
    let node_count = reader.count(1)?;
    let mut removed_nodes = Vec::with_capacity(node_count.min(INITIAL_CAPACITY));
    let mut previous = 0_u64;
    for index in 0..node_count {
        let delta = reader.varint()?;
        if index > 0 && delta == 0 {
            return Err(BlobError::Order);
        }
        previous = previous.checked_add(delta).ok_or(BlobError::Range)?;
        removed_nodes.push(u32::try_from(previous).map_err(|_| BlobError::Range)?);
    }
    let edge_count = reader.count(2)?;
    let mut removed_edges = Vec::with_capacity(edge_count.min(INITIAL_CAPACITY));
    let mut previous_a = 0_u64;
    for _ in 0..edge_count {
        let a = previous_a
            .checked_add(reader.varint()?)
            .ok_or(BlobError::Range)?;
        let gap = reader.varint()?;
        if gap == 0 {
            return Err(BlobError::Order);
        }
        let b = a.checked_add(gap).ok_or(BlobError::Range)?;
        removed_edges.push((
            u32::try_from(a).map_err(|_| BlobError::Range)?,
            u32::try_from(b).map_err(|_| BlobError::Range)?,
        ));
        previous_a = a;
    }
    validate_zephyr(m, t, &removed_nodes, &removed_edges)?;
    Ok(Body::ZephyrRemoved {
        m,
        t,
        removed_nodes,
        removed_edges,
    })
}

/// Parses payload bytes.
///
/// # Errors
///
/// A [`BlobError`] for the first fault found: [`BlobError::TooLarge`],
/// [`BlobError::Magic`], [`BlobError::Version`], [`BlobError::Kind`],
/// [`BlobError::Varint`], [`BlobError::Range`], [`BlobError::Order`],
/// [`BlobError::NotLattice`], or [`BlobError::Trailing`].
pub fn decode_payload(bytes: &[u8]) -> Result<Body, BlobError> {
    if bytes.len() > MAX_PAYLOAD {
        return Err(BlobError::TooLarge);
    }
    let mut reader = Reader { bytes, pos: 0 };
    for expected in MAGIC {
        if reader.byte() != Some(expected) {
            return Err(BlobError::Magic);
        }
    }
    if reader.byte() != Some(VERSION) {
        return Err(BlobError::Version);
    }
    let body = match reader.byte() {
        Some(KIND_EXPLICIT) => decode_explicit(&mut reader)?,
        Some(KIND_ZEPHYR) => decode_zephyr(&mut reader)?,
        _ => return Err(BlobError::Kind),
    };
    if reader.remaining() != 0 {
        return Err(BlobError::Trailing);
    }
    Ok(body)
}

/// Encodes a body to file bytes: [`encode_payload`] then [`compress`].
///
/// # Errors
///
/// The errors of [`encode_payload`].
pub fn encode(body: &Body) -> Result<Vec<u8>, BlobError> {
    Ok(compress(&encode_payload(body)?))
}

/// Decodes file bytes: [`decompress`] then [`decode_payload`].
///
/// # Errors
///
/// The errors of [`decompress`] and [`decode_payload`].
pub fn decode(file: &[u8]) -> Result<Body, BlobError> {
    decode_payload(&decompress(file)?)
}

impl Body {
    /// The topology in native ids. A `ZephyrRemoved` body expands to the
    /// lattice nodes ascending minus the removed nodes, and the lattice edges
    /// `(a, b)` with `a < b` ascending, minus every edge at a removed node and
    /// minus the removed edges.
    ///
    /// # Errors
    ///
    /// The lattice errors of [`encode_payload`] for an invalid body.
    pub fn topology(&self) -> Result<Topology, BlobError> {
        match self {
            Self::Explicit(topology) => Ok(topology.clone()),
            Self::ZephyrRemoved {
                m,
                t,
                removed_nodes,
                removed_edges,
            } => {
                validate_zephyr(*m, *t, removed_nodes, removed_edges)?;
                let (nodes, edges) = zephyr_lattice(*m, *t).ok_or(BlobError::Range)?;
                let nodes = nodes
                    .into_iter()
                    .filter(|id| removed_nodes.binary_search(id).is_err())
                    .collect();
                let edges = edges
                    .into_iter()
                    .filter(|&(a, b)| {
                        removed_nodes.binary_search(&a).is_err()
                            && removed_nodes.binary_search(&b).is_err()
                            && removed_edges.binary_search(&(a, b)).is_err()
                    })
                    .collect();
                Ok(Topology { nodes, edges })
            }
        }
    }
}
