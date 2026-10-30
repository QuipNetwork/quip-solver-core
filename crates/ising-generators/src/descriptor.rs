//! Family descriptors, parameter schemas, and admission.
//!
//! This part holds the family id grammar (FAM-01, 02, 05) and the canonical
//! parameter codec (FAM-10 to FAM-16). It does not allocate.

use crate::digest::BufferedHasher;
use crate::Code;

/// The first byte of every canonical parameter encoding.
pub const PARAMS_FORMAT: u8 = 0x01;

/// The largest canonical parameter encoding in bytes. A longer one is code 7.
pub const MAX_PARAMS_LEN: usize = 4096;

/// Shortest family id in bytes.
const MIN_FAMILY_ID_LEN: usize = 3;
/// Longest family id in bytes.
const MAX_FAMILY_ID_LEN: usize = 64;
/// Bytes in a field header: `tag: u16 LE`, `length: u32 LE`.
const HEADER_LEN: usize = 6;

/// Where a family id is checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdContext {
    /// A lease. The `example` namespace is not allowed.
    Lease,
    /// A direct call. The `example` namespace is allowed.
    Direct,
}

/// A lowercase label: `[a-z][a-z0-9-]*`.
fn is_label(label: &[u8]) -> bool {
    match label.split_first() {
        Some((first, rest)) => {
            first.is_ascii_lowercase()
                && rest
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        }
        None => false,
    }
}

/// Check a family id against FAM-01, FAM-02, and FAM-05.
///
/// The id is `<namespace>/<name>` in ASCII, 3 to 64 bytes. The namespace is
/// one or more labels separated by dots, and the name is one label. A label is
/// `[a-z][a-z0-9-]*`. The `example` namespace is code 3 in a lease.
///
/// # Errors
///
/// Returns [`Code::FamilyIdInvalid`] (3) when the id is outside the grammar or
/// the length bounds, or when the context is [`IdContext::Lease`] and the
/// namespace is `example`.
pub fn check_family_id(id: &[u8], context: IdContext) -> Result<(), Code> {
    if !(MIN_FAMILY_ID_LEN..=MAX_FAMILY_ID_LEN).contains(&id.len()) {
        return Err(Code::FamilyIdInvalid);
    }
    let slash = id
        .iter()
        .position(|b| *b == b'/')
        .ok_or(Code::FamilyIdInvalid)?;
    let (namespace, rest) = id.split_at(slash);
    // `rest` starts with the slash; a second slash fails `is_label` below.
    let name = rest.get(1..).ok_or(Code::FamilyIdInvalid)?;
    if !is_label(name) || !namespace.split(|b| *b == b'.').all(is_label) {
        return Err(Code::FamilyIdInvalid);
    }
    if context == IdContext::Lease && namespace == b"example" {
        return Err(Code::FamilyIdInvalid);
    }
    Ok(())
}

/// The type of one parameter value (FAM-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    /// Unsigned, 1 byte.
    U8,
    /// Unsigned, 2 bytes, little endian.
    U16,
    /// Unsigned, 4 bytes, little endian.
    U32,
    /// Unsigned, 8 bytes, little endian.
    U64,
    /// Two's complement, 4 bytes, little endian.
    I32,
    /// Two's complement, 8 bytes, little endian.
    I64,
    /// One byte, `0x00` or `0x01`.
    Bool,
    /// Raw bytes of any length.
    Bytes,
    /// Bytes of any length, each from `0x20` to `0x7E`.
    Ascii,
}

impl ValueType {
    /// The exact width of a fixed-width type, or `None` for a variable one.
    const fn width(self) -> Option<usize> {
        match self {
            Self::U8 | Self::Bool => Some(1),
            Self::U16 => Some(2),
            Self::U32 | Self::I32 => Some(4),
            Self::U64 | Self::I64 => Some(8),
            Self::Bytes | Self::Ascii => None,
        }
    }
}

/// One schema field. The bounds are inclusive and apply to the stored integer,
/// to 0 or 1 for a `bool`, and to the length for `bytes` and `ascii` (FAM-15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldSpec {
    /// The field tag. A schema lists its tags in strictly ascending order.
    pub tag: u16,
    /// The value type.
    pub kind: ValueType,
    /// The smallest allowed value or length.
    pub min: i128,
    /// The largest allowed value or length.
    pub max: i128,
}

/// A decoded parameter value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value<'a> {
    /// A `u8`, `u16`, `u32`, or `u64`, widened to `u64`.
    Unsigned(u64),
    /// An `i32` or `i64`, sign-extended to `i64`.
    Signed(i64),
    /// A `bool`.
    Bool(bool),
    /// A `bytes` value.
    Bytes(&'a [u8]),
    /// An `ascii` value.
    Ascii(&'a str),
}

/// Decode a payload that already passed the layout checks.
fn decode(kind: ValueType, payload: &[u8]) -> Option<Value<'_>> {
    fn le<const N: usize>(payload: &[u8]) -> Option<[u8; N]> {
        payload.try_into().ok()
    }
    Some(match kind {
        ValueType::U8 => Value::Unsigned(u64::from(u8::from_le_bytes(le(payload)?))),
        ValueType::U16 => Value::Unsigned(u64::from(u16::from_le_bytes(le(payload)?))),
        ValueType::U32 => Value::Unsigned(u64::from(u32::from_le_bytes(le(payload)?))),
        ValueType::U64 => Value::Unsigned(u64::from_le_bytes(le(payload)?)),
        ValueType::I32 => Value::Signed(i64::from(i32::from_le_bytes(le(payload)?))),
        ValueType::I64 => Value::Signed(i64::from_le_bytes(le(payload)?)),
        ValueType::Bool => Value::Bool(*payload.first()? == 1),
        ValueType::Bytes => Value::Bytes(payload),
        // The layout check limits every byte to 0x20..=0x7E, so this cannot fail.
        ValueType::Ascii => Value::Ascii(core::str::from_utf8(payload).ok()?),
    })
}

/// The number that FAM-15 bounds measure.
fn measure(value: &Value<'_>) -> i128 {
    match value {
        Value::Unsigned(v) => i128::from(*v),
        Value::Signed(v) => i128::from(*v),
        Value::Bool(v) => i128::from(*v),
        Value::Bytes(b) => i128::try_from(b.len()).unwrap_or(i128::MAX),
        Value::Ascii(s) => i128::try_from(s.len()).unwrap_or(i128::MAX),
    }
}

/// Walks the schema and the encoded fields in lockstep and checks the layout
/// of each field. It yields one `(spec, payload)` per schema field.
struct Fields<'a> {
    schema: core::slice::Iter<'a, FieldSpec>,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Fields<'a> {
    /// Start after the format byte, or fail with code 7.
    fn new(schema: &'a [FieldSpec], bytes: &'a [u8]) -> Result<Self, Code> {
        if bytes.len() > MAX_PARAMS_LEN || bytes.first() != Some(&PARAMS_FORMAT) {
            return Err(Code::ParamsMalformed);
        }
        Ok(Self {
            schema: schema.iter(),
            bytes,
            pos: 1,
        })
    }

    fn read(&mut self, spec: &'a FieldSpec) -> Result<(&'a FieldSpec, &'a [u8]), Code> {
        let malformed = Code::ParamsMalformed;
        let header_end = self.pos.checked_add(HEADER_LEN).ok_or(malformed)?;
        let header = self.bytes.get(self.pos..header_end).ok_or(malformed)?;
        let tag = header
            .get(..2)
            .and_then(|b| <[u8; 2]>::try_from(b).ok())
            .map(u16::from_le_bytes)
            .ok_or(malformed)?;
        let length = header
            .get(2..)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(u32::from_le_bytes)
            .ok_or(malformed)?;
        if tag != spec.tag {
            return Err(malformed);
        }
        let length = usize::try_from(length).map_err(|_| malformed)?;
        let end = header_end.checked_add(length).ok_or(malformed)?;
        let payload = self.bytes.get(header_end..end).ok_or(malformed)?;
        if spec.kind.width().is_some_and(|w| w != length) {
            return Err(malformed);
        }
        let valid = match spec.kind {
            ValueType::Bool => matches!(payload, [0 | 1]),
            ValueType::Ascii => payload.iter().all(|b| (0x20..=0x7E).contains(b)),
            _ => true,
        };
        if !valid {
            return Err(malformed);
        }
        self.pos = end;
        Ok((spec, payload))
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = Result<(&'a FieldSpec, &'a [u8]), Code>;

    fn next(&mut self) -> Option<Self::Item> {
        let spec = self.schema.next()?;
        Some(self.read(spec))
    }
}

/// Canonical parameter bytes that passed the layout checks against a schema.
#[derive(Clone, Copy, Debug)]
pub struct Params<'a> {
    schema: &'a [FieldSpec],
    bytes: &'a [u8],
}

impl<'a> Params<'a> {
    /// Check the layout of canonical parameter bytes against a schema.
    ///
    /// The schema tags must be strictly ascending. The checks run in this
    /// order: the length (at most [`MAX_PARAMS_LEN`]), the format byte, then
    /// each schema field in order (header, tag, payload fit, fixed width, the
    /// `bool` byte, the `ascii` bytes), then the end of the input. The
    /// bounds are not checked here, see [`Params::check_bounds`].
    ///
    /// # Errors
    ///
    /// Returns [`Code::ParamsMalformed`] (7) on any layout fault: a length
    /// over 4096, a wrong format byte, an empty input, a missing, unknown,
    /// duplicate, or out of order tag, a truncated header or payload, a wrong
    /// fixed width, a bad `bool` or `ascii` byte, or trailing bytes.
    pub fn parse(schema: &'a [FieldSpec], bytes: &'a [u8]) -> Result<Self, Code> {
        let mut fields = Fields::new(schema, bytes)?;
        for field in &mut fields {
            let _checked = field?;
        }
        if fields.pos != bytes.len() {
            return Err(Code::ParamsMalformed);
        }
        Ok(Self { schema, bytes })
    }

    fn fields(&self) -> Result<Fields<'a>, Code> {
        Fields::new(self.schema, self.bytes)
    }

    /// Check every value against its inclusive bounds.
    ///
    /// # Errors
    ///
    /// Returns [`Code::ParamsOutOfBounds`] (9) on the first value outside its
    /// bounds.
    pub fn check_bounds(&self) -> Result<(), Code> {
        for field in self.fields()? {
            let (spec, payload) = field?;
            // A decode failure cannot happen after `parse`; treat it as a fault.
            let value = decode(spec.kind, payload).ok_or(Code::ParamsMalformed)?;
            if !(spec.min..=spec.max).contains(&measure(&value)) {
                return Err(Code::ParamsOutOfBounds);
            }
        }
        Ok(())
    }

    /// The decoded value of the field with `tag`, or `None` if the schema has
    /// no such tag.
    #[must_use]
    pub fn get(&self, tag: u16) -> Option<Value<'a>> {
        let fields = self.fields().ok()?;
        for field in fields {
            let (spec, payload) = field.ok()?;
            if spec.tag == tag {
                return decode(spec.kind, payload);
            }
        }
        None
    }

    /// The canonical parameter bytes.
    #[must_use]
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

/// The parameter digest (FAM-18): BLAKE3 in key-derivation mode with the
/// context `quip family params v1` over the canonical parameter bytes.
#[must_use]
pub fn params_digest(params: &[u8]) -> [u8; 32] {
    let mut hasher = BufferedHasher::new("quip family params v1");
    hasher.put(params);
    hasher.finish()
}

/// A family descriptor: a family id, a revision, and canonical parameter
/// bytes. It borrows its inputs, hashes nothing on creation, and does not
/// check the parameter bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Descriptor<'a> {
    family_id: &'a str,
    revision: u32,
    params: &'a [u8],
}

impl<'a> Descriptor<'a> {
    /// Check the family id, then the revision, and keep the inputs.
    ///
    /// # Errors
    ///
    /// Returns [`Code::FamilyIdInvalid`] (3) when the id fails
    /// [`check_family_id`] in `context`, then [`Code::RevisionZero`] (4) when
    /// the revision is zero.
    pub fn new(
        family_id: &'a [u8],
        revision: u32,
        params: &'a [u8],
        context: IdContext,
    ) -> Result<Self, Code> {
        check_family_id(family_id, context)?;
        if revision == 0 {
            return Err(Code::RevisionZero);
        }
        // `check_family_id` allows only ASCII bytes, so this cannot fail.
        let family_id = core::str::from_utf8(family_id).map_err(|_| Code::FamilyIdInvalid)?;
        Ok(Self {
            family_id,
            revision,
            params,
        })
    }

    /// The family id.
    #[must_use]
    pub const fn family_id(&self) -> &'a str {
        self.family_id
    }

    /// The revision, never zero.
    #[must_use]
    pub const fn revision(&self) -> u32 {
        self.revision
    }

    /// The canonical parameter bytes.
    #[must_use]
    pub const fn params(&self) -> &'a [u8] {
        self.params
    }

    /// The digest of the parameter bytes, see [`params_digest`].
    #[must_use]
    pub fn params_digest(&self) -> [u8; 32] {
        params_digest(self.params)
    }

    /// The descriptor digest (FAM-18): BLAKE3 in key-derivation mode with the
    /// context `quip family descriptor v1` over `u8 len(family_id)`, the id,
    /// `u32 LE revision`, and the parameter digest.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let id = self.family_id.as_bytes();
        // The id is at most 64 bytes, so the length always fits in a byte.
        let id_len = u8::try_from(id.len()).unwrap_or(u8::MAX);
        let mut hasher = BufferedHasher::new("quip family descriptor v1");
        hasher.put(&[id_len]);
        hasher.put(id);
        hasher.put(&self.revision.to_le_bytes());
        hasher.put(&self.params_digest());
        hasher.finish()
    }
}

/// The topology order digest (FAM-19): BLAKE3 in key-derivation mode with the
/// context `quip family topology order v1` over `u32 LE n_nodes`, each node id
/// as `u32 LE`, `u32 LE n_edges`, and each edge as two `u32 LE` native ids.
/// Nodes and edges are hashed in the order given, and an edge is not
/// normalized.
///
/// Returns `None` when a count exceeds `u32::MAX`.
#[must_use]
pub fn topology_order_digest(nodes: &[u32], edges: &[(u32, u32)]) -> Option<[u8; 32]> {
    let n_nodes = u32::try_from(nodes.len()).ok()?;
    let n_edges = u32::try_from(edges.len()).ok()?;
    let mut hasher = BufferedHasher::new("quip family topology order v1");
    hasher.put(&n_nodes.to_le_bytes());
    for node in nodes {
        hasher.put(&node.to_le_bytes());
    }
    hasher.put(&n_edges.to_le_bytes());
    for (u, v) in edges {
        hasher.put(&u.to_le_bytes());
        hasher.put(&v.to_le_bytes());
    }
    Some(hasher.finish())
}
