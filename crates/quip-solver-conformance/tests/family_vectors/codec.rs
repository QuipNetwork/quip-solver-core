//! Hand-written recomputation of identity grammars and parameter bytes
//! (FAM-01, 02, 05, 06, 10 to 16, 22, 60). It shares no code with the oracle.

use serde_json::{Map, Value};

const FORMAT_BYTE: u8 = 0x01;
const MAX_PARAM_BYTES: usize = 4096;
/// Code 3, `FamilyIdInvalid`.
const CODE_FAMILY_ID: u8 = 3;
/// Code 4, `RevisionZero`.
const CODE_REVISION_ZERO: u8 = 4;
/// Code 7, `ParamsMalformed`.
const CODE_MALFORMED: u8 = 7;
/// Code 9, `ParamsOutOfBounds`.
const CODE_OUT_OF_BOUNDS: u8 = 9;

fn hex_nibble(c: u8) -> Result<u8, String> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        _ => Err(format!("not a lowercase hex digit: {c:#04x}")),
    }
}

/// Decodes lowercase hex of even length.
pub(crate) fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err("odd-length hex".to_owned());
    }
    bytes
        .chunks_exact(2)
        .map(|pair| match pair {
            [hi, lo] => Ok(hex_nibble(*hi)? << 4 | hex_nibble(*lo)?),
            _ => Err("unreachable chunk".to_owned()),
        })
        .collect()
}

/// Encodes bytes as lowercase hex.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        for nibble in [byte >> 4, byte & 0x0f] {
            if let Some(digit) = DIGITS.get(usize::from(nibble)) {
                out.push(char::from(*digit));
            }
        }
    }
    out
}

/// Decodes a 64-character hex string into 32 bytes.
pub(crate) fn hex32(text: &str) -> Result<[u8; 32], String> {
    let bytes = hex_decode(text)?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| format!("{} bytes, expected 32", bytes.len()))
}

/// `[a-z][a-z0-9-]*`
fn is_name_segment(seg: &[u8]) -> bool {
    match seg.split_first() {
        Some((first, rest)) => {
            first.is_ascii_lowercase()
                && rest
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        }
        None => false,
    }
}

/// `[a-z0-9][a-z0-9.-]*`
fn is_version_segment(seg: &[u8]) -> bool {
    match seg.split_first() {
        Some((first, rest)) => {
            (first.is_ascii_lowercase() || first.is_ascii_digit())
                && rest.iter().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'.'
                })
        }
        None => false,
    }
}

/// Returns code 3 when `id` is not a valid `family_id`. The `example`
/// namespace is refused only in the lease context.
pub(crate) fn family_id_code(id: &[u8], lease_context: bool) -> Option<u8> {
    if !(3..=64).contains(&id.len()) {
        return Some(CODE_FAMILY_ID);
    }
    let mut parts = id.split(|b| *b == b'/');
    let (Some(namespace), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return Some(CODE_FAMILY_ID);
    };
    if !namespace.split(|b| *b == b'.').all(is_name_segment) || !is_name_segment(name) {
        return Some(CODE_FAMILY_ID);
    }
    if lease_context && namespace == b"example" {
        return Some(CODE_FAMILY_ID);
    }
    None
}

/// Returns code 4 for revision 0.
pub(crate) fn revision_code(revision: u32) -> Option<u8> {
    (revision == 0).then_some(CODE_REVISION_ZERO)
}

/// Outcome of the `solver_id` grammar.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SolverIdClass {
    /// Matches FAM-60.
    Valid,
    /// Non-empty and does not match FAM-60.
    Invalid,
    /// Empty: not a pin target.
    Empty,
}

/// Classifies a `solver_id`.
pub(crate) fn solver_id_class(id: &[u8]) -> SolverIdClass {
    if id.is_empty() {
        return SolverIdClass::Empty;
    }
    if !(3..=64).contains(&id.len()) {
        return SolverIdClass::Invalid;
    }
    let mut segments = id.split(|b| *b == b'/');
    let first_ok = segments.next().is_some_and(is_name_segment);
    if first_ok && segments.all(is_version_segment) {
        SolverIdClass::Valid
    } else {
        SolverIdClass::Invalid
    }
}

/// A substream label: 1 to 32 bytes, each at most 0x7F.
pub(crate) fn label_valid(label: &[u8]) -> bool {
    (1..=32).contains(&label.len()) && label.iter().all(|b| *b <= 0x7F)
}

/// FAM-13 value types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ValueType {
    U8,
    U16,
    U32,
    U64,
    I32,
    I64,
    Bool,
    Bytes,
    Ascii,
}

impl ValueType {
    fn parse(name: &str) -> Result<Self, String> {
        Ok(match name {
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "bool" => Self::Bool,
            "bytes" => Self::Bytes,
            "ascii" => Self::Ascii,
            other => return Err(format!("unknown value type {other}")),
        })
    }

    /// Fixed width in bytes, or `None` for variable-length types.
    fn width(self) -> Option<usize> {
        match self {
            Self::U8 | Self::Bool => Some(1),
            Self::U16 => Some(2),
            Self::U32 | Self::I32 => Some(4),
            Self::U64 | Self::I64 => Some(8),
            Self::Bytes | Self::Ascii => None,
        }
    }

    fn signed(self) -> bool {
        matches!(self, Self::I32 | Self::I64)
    }
}

/// One schema field.
#[derive(Debug)]
pub(crate) struct ParamField {
    pub(crate) tag: u16,
    pub(crate) name: String,
    pub(crate) kind: ValueType,
    pub(crate) min: i128,
    pub(crate) max: i128,
}

/// A parameter schema for one family revision.
#[derive(Debug)]
pub(crate) struct ParamSchema {
    pub(crate) fields: Vec<ParamField>,
}

impl ParamSchema {
    /// Parses one element of the fixture `schemas` section.
    pub(crate) fn from_json(value: &Value) -> Result<Self, String> {
        let fields = value
            .get("fields")
            .and_then(Value::as_array)
            .ok_or("schema has no fields array")?;
        let mut out = Vec::new();
        for field in fields {
            let tag = field
                .get("tag")
                .and_then(Value::as_u64)
                .and_then(|t| u16::try_from(t).ok())
                .ok_or("field tag is not a u16")?;
            let bound = |key: &str| {
                super::str_field(field, key)?
                    .parse::<i128>()
                    .map_err(|e| format!("bound {key}: {e}"))
            };
            out.push(ParamField {
                tag,
                name: super::str_field(field, "name")?.to_owned(),
                kind: ValueType::parse(super::str_field(field, "type")?)?,
                min: bound("min")?,
                max: bound("max")?,
            });
        }
        out.sort_by_key(|f| f.tag);
        Ok(Self { fields: out })
    }
}

fn int_bytes(kind: ValueType, n: i128) -> Result<Vec<u8>, String> {
    let width = kind.width().ok_or("not an integer type")?;
    let (lo, hi) = if kind.signed() {
        (
            -(1_i128 << (width * 8 - 1)),
            (1_i128 << (width * 8 - 1)) - 1,
        )
    } else {
        (0, (1_i128 << (width * 8)) - 1)
    };
    if n < lo || n > hi {
        return Err(format!("{n} does not fit {kind:?}"));
    }
    Ok(n.to_le_bytes().get(..width).ok_or("bad width")?.to_vec())
}

/// Encodes `values` (fixture encoding) as canonical parameter bytes.
pub(crate) fn encode_params(schema: &ParamSchema, values: &Value) -> Result<Vec<u8>, String> {
    let mut out = vec![FORMAT_BYTE];
    for field in &schema.fields {
        let v = values
            .get(&field.name)
            .ok_or_else(|| format!("no value for {}", field.name))?;
        let payload = match field.kind {
            ValueType::Bool => vec![u8::from(v.as_bool().ok_or("bool value is not a boolean")?)],
            ValueType::Bytes | ValueType::Ascii => {
                hex_decode(v.as_str().ok_or("bytes value is not a string")?)?
            }
            kind => {
                let n = v
                    .as_str()
                    .ok_or("integer value is not a string")?
                    .parse::<i128>()
                    .map_err(|e| e.to_string())?;
                int_bytes(kind, n)?
            }
        };
        out.extend_from_slice(&field.tag.to_le_bytes());
        out.extend_from_slice(
            &u32::try_from(payload.len())
                .map_err(|_| "payload too long")?
                .to_le_bytes(),
        );
        out.extend_from_slice(&payload);
    }
    Ok(out)
}

/// A parsed field: its schema entry and raw payload.
type Raw<'a> = (&'a ParamField, &'a [u8]);

/// Layout pass (code 7). Returns the raw payloads in schema tag order.
fn parse_layout<'a>(schema: &'a ParamSchema, bytes: &'a [u8]) -> Result<Vec<Raw<'a>>, u8> {
    if bytes.len() > MAX_PARAM_BYTES {
        return Err(CODE_MALFORMED);
    }
    let (format, mut rest) = bytes.split_first().ok_or(CODE_MALFORMED)?;
    if *format != FORMAT_BYTE {
        return Err(CODE_MALFORMED);
    }
    let mut found: Vec<Raw<'a>> = Vec::new();
    let mut previous: Option<u16> = None;
    while !rest.is_empty() {
        let header: [u8; 6] = rest
            .get(..6)
            .and_then(|h| <[u8; 6]>::try_from(h).ok())
            .ok_or(CODE_MALFORMED)?;
        let [t0, t1, l0, l1, l2, l3] = header;
        let tag = u16::from_le_bytes([t0, t1]);
        let length =
            usize::try_from(u32::from_le_bytes([l0, l1, l2, l3])).map_err(|_| CODE_MALFORMED)?;
        if previous.is_some_and(|p| tag <= p) {
            return Err(CODE_MALFORMED);
        }
        previous = Some(tag);
        let field = schema
            .fields
            .iter()
            .find(|f| f.tag == tag)
            .ok_or(CODE_MALFORMED)?;
        let end = 6_usize.checked_add(length).ok_or(CODE_MALFORMED)?;
        let payload = rest.get(6..end).ok_or(CODE_MALFORMED)?;
        if field.kind.width().is_some_and(|w| w != length) {
            return Err(CODE_MALFORMED);
        }
        let well_formed = match field.kind {
            ValueType::Bool => payload.iter().all(|b| *b <= 1),
            ValueType::Ascii => payload.iter().all(|b| (0x20..=0x7E).contains(b)),
            _ => true,
        };
        if !well_formed {
            return Err(CODE_MALFORMED);
        }
        found.push((field, payload));
        rest = rest.get(end..).ok_or(CODE_MALFORMED)?;
    }
    if found.len() != schema.fields.len() {
        return Err(CODE_MALFORMED);
    }
    Ok(found)
}

fn decode_int(kind: ValueType, payload: &[u8]) -> Result<i128, u8> {
    let mut buf = [0_u8; 16];
    buf.get_mut(..payload.len())
        .ok_or(CODE_MALFORMED)?
        .copy_from_slice(payload);
    let unsigned = i128::from_le_bytes(buf);
    if kind.signed() {
        let bits = payload.len() * 8;
        let sign = 1_i128 << (bits - 1);
        Ok(if unsigned & sign == 0 {
            unsigned
        } else {
            unsigned - (sign << 1)
        })
    } else {
        Ok(unsigned)
    }
}

/// Decodes parameter bytes: the whole layout first (code 7), then bounds
/// (code 9). The result uses the fixture `values` encoding.
pub(crate) fn decode_params(schema: &ParamSchema, bytes: &[u8]) -> Result<Value, u8> {
    let raw = parse_layout(schema, bytes)?;
    let mut values = Map::new();
    let mut in_bounds = true;
    for (field, payload) in raw {
        let (value, measure) = match field.kind {
            ValueType::Bool => (
                Value::Bool(payload.first() == Some(&1)),
                i128::from(payload.first().copied().unwrap_or(0)),
            ),
            ValueType::Bytes | ValueType::Ascii => (
                Value::String(hex_encode(payload)),
                i128::try_from(payload.len()).map_err(|_| CODE_MALFORMED)?,
            ),
            kind => {
                let n = decode_int(kind, payload)?;
                (Value::String(n.to_string()), n)
            }
        };
        in_bounds &= (field.min..=field.max).contains(&measure);
        drop(values.insert(field.name.clone(), value));
    }
    if in_bounds {
        Ok(Value::Object(values))
    } else {
        Err(CODE_OUT_OF_BOUNDS)
    }
}
