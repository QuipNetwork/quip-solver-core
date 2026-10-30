"""Parameter schemas, the canonical parameter codec, and identity grammars."""

import re
import struct
from dataclasses import dataclass

MAX_PARAMS_LEN = 4096
FORMAT_BYTE = 0x01
CODE_FAMILY_ID_INVALID = 3
CODE_REVISION_ZERO = 4
CODE_PARAMS_MALFORMED = 7
CODE_PARAMS_OUT_OF_BOUNDS = 9

# type name -> (byte width, signed); variable and bool types are handled apart.
_INT_TYPES = {
    "u8": (1, False),
    "u16": (2, False),
    "u32": (4, False),
    "u64": (8, False),
    "i32": (4, True),
    "i64": (8, True),
}

_FAMILY_ID = re.compile(
    r"[a-z][a-z0-9-]*(\.[a-z][a-z0-9-]*)*/[a-z][a-z0-9-]*", re.ASCII
)
_SOLVER_ID = re.compile(r"[a-z][a-z0-9-]*(/[a-z0-9][a-z0-9.-]*)*", re.ASCII)


@dataclass(frozen=True)
class Field:
    tag: int
    name: str
    type: str
    scale: int | None
    min: int
    max: int


@dataclass(frozen=True)
class Schema:
    family_id: str
    revision: int
    fields: tuple[Field, ...]


UNIFORM_FIELD = Schema(
    "example/uniform-field",
    1,
    (
        Field(1, "n_layers", "u16", 1, 1, 16),
        Field(2, "field_milli", "i32", 1000, -4000, 4000),
    ),
)

ALL_TYPES = Schema(
    "example/all-types",
    1,
    (
        Field(1, "a_u8", "u8", 1, 0, 255),
        Field(2, "a_u16", "u16", 1, 0, 65535),
        Field(3, "a_u32", "u32", 1, 0, 4294967295),
        Field(4, "a_u64", "u64", 1, 1, 18446744073709551615),
        Field(5, "a_i32", "i32", 1000, -2147483648, 2147483647),
        Field(6, "a_i64", "i64", 1, -72057594037927936, 72057594037927936),
        Field(7, "a_bool", "bool", None, 0, 1),
        Field(8, "a_bytes", "bytes", None, 0, 4096),
        Field(9, "a_ascii", "ascii", None, 0, 64),
    ),
)


def _encode_value(field: Field, value: object) -> bytes:
    if field.type in _INT_TYPES:
        width, signed = _INT_TYPES[field.type]
        return int(value).to_bytes(width, "little", signed=signed)
    if field.type == "bool":
        return bytes([int(value)])
    return bytes(value)


def encode_params(schema: Schema, values: dict) -> bytes:
    """Pack values in schema order. No validation beyond packing."""
    out = bytes([FORMAT_BYTE])
    for field in schema.fields:
        raw = _encode_value(field, values[field.name])
        out += struct.pack("<HI", field.tag, len(raw)) + raw
    return out


def _decode_value(field: Field, raw: bytes) -> int | bool | bytes:
    if field.type in _INT_TYPES:
        _, signed = _INT_TYPES[field.type]
        return int.from_bytes(raw, "little", signed=signed)
    if field.type == "bool":
        return raw[0] == 1
    return raw


def _layout(schema: Schema, data: bytes) -> dict[str, bytes] | None:
    """Return raw values by field name, or None on any layout fault."""
    if len(data) > MAX_PARAMS_LEN or not data or data[0] != FORMAT_BYTE:
        return None
    by_tag = {field.tag: field for field in schema.fields}
    raws: dict[str, bytes] = {}
    pos = 1
    previous = 0
    while pos < len(data):
        if pos + 6 > len(data):
            return None
        tag, length = struct.unpack_from("<HI", data, pos)
        pos += 6
        if tag <= previous or tag not in by_tag or pos + length > len(data):
            return None
        previous = tag
        field = by_tag[tag]
        raw = data[pos : pos + length]
        pos += length
        if field.type in _INT_TYPES and length != _INT_TYPES[field.type][0]:
            return None
        if field.type == "bool" and raw not in (b"\x00", b"\x01"):
            return None
        if field.type == "ascii" and any(not 0x20 <= byte <= 0x7E for byte in raw):
            return None
        raws[field.name] = raw
    if len(raws) != len(by_tag):
        return None
    return raws


def decode_params(schema: Schema, data: bytes) -> tuple[int | None, dict | None]:
    """Return (code, None) on a fault, or (None, values). Layout before bounds."""
    raws = _layout(schema, data)
    if raws is None:
        return CODE_PARAMS_MALFORMED, None
    values = {}
    for field in schema.fields:
        value = _decode_value(field, raws[field.name])
        measured = len(value) if isinstance(value, bytes) else int(value)
        if not field.min <= measured <= field.max:
            return CODE_PARAMS_OUT_OF_BOUNDS, None
        values[field.name] = value
    return None, values


def _ascii_text(raw: bytes) -> str | None:
    try:
        return raw.decode("ascii")
    except UnicodeDecodeError:
        return None


def family_id_code(family_id: bytes, context: str) -> int | None:
    """Return 3 for an invalid family_id, else None. `context` is lease or direct."""
    text = _ascii_text(family_id)
    if text is None or not 3 <= len(family_id) <= 64:
        return CODE_FAMILY_ID_INVALID
    if _FAMILY_ID.fullmatch(text) is None:
        return CODE_FAMILY_ID_INVALID
    if context == "lease" and text.split("/")[0] == "example":
        return CODE_FAMILY_ID_INVALID
    return None


def revision_code(revision: int) -> int | None:
    return CODE_REVISION_ZERO if revision == 0 else None


def solver_id_class(solver_id: bytes) -> str:
    if not solver_id:
        return "empty"
    text = _ascii_text(solver_id)
    if text is None or not 3 <= len(solver_id) <= 64:
        return "invalid"
    return "valid" if _SOLVER_ID.fullmatch(text) else "invalid"


def label_valid(label: bytes) -> bool:
    return 1 <= len(label) <= 32 and all(byte <= 0x7F for byte in label)
