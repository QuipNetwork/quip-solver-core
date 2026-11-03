"""Topology blob codec, written from docs/topology-format.md alone.

A `.bin.gz` file is one gzip member holding a varint payload. `encode` is the
only writer of committed blobs, so it is byte-deterministic. `decode` enforces
every reader rule of the format document.
"""

import struct
import zlib
from typing import Any

from . import ice

MAX_PAYLOAD = 64 * 1024 * 1024
MAX_ID = 0xFFFFFFFF
U64_MAX = (1 << 64) - 1
MAGIC = b"QTOP"
VERSION = 1
KIND_EXPLICIT = 0
KIND_ZEPHYR = 1
HEADER = bytes([0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 2, 0xFF])

Body = dict[str, Any]


class BlobError(ValueError):
    """A file or payload that breaks the format. `code` names the rule."""

    def __init__(self, code: str, detail: str = "") -> None:
        super().__init__(f"{code}: {detail}" if detail else code)
        self.code = code


# ---- varint and zigzag ----


def zigzag(value: int) -> int:
    if not -(1 << 63) <= value < (1 << 63):
        raise BlobError("overflow", f"{value} does not fit i64")
    return ((value << 1) ^ (value >> 63)) & U64_MAX


def unzigzag(value: int) -> int:
    return (value >> 1) ^ -(value & 1)


def put_varint(out: bytearray, value: int) -> None:
    if not 0 <= value <= U64_MAX:
        raise BlobError("overflow", f"{value} does not fit u64")
    while value >= 0x80:
        out.append((value & 0x7F) | 0x80)
        value >>= 7
    out.append(value)


class _Reader:
    def __init__(self, data: bytes) -> None:
        self.data = data
        self.pos = 0

    def remaining(self) -> int:
        return len(self.data) - self.pos

    def byte(self) -> int:
        if self.pos >= len(self.data):
            raise BlobError("truncated", "payload ends early")
        value = self.data[self.pos]
        self.pos += 1
        return value

    def varint(self) -> int:
        value = 0
        for index in range(10):
            byte = self.byte()
            value |= (byte & 0x7F) << (7 * index)
            if value > U64_MAX:
                raise BlobError("varint-overflow", "value above u64::MAX")
            if byte < 0x80:
                if byte == 0 and index > 0:
                    raise BlobError("varint-non-minimal", "final group is zero")
                return value
        raise BlobError("varint-overflow", "more than 10 bytes")

    def count(self, min_bytes_each: int) -> int:
        """A count, checked against the bytes left before any allocation."""
        value = self.varint()
        if value * min_bytes_each > self.remaining():
            raise BlobError(
                "count-too-large", f"{value} items, {self.remaining()} bytes left"
            )
        return value

    def signed(self) -> int:
        return unzigzag(self.varint())


# ---- payload ----


def _check_id(value: int, what: str) -> int:
    if not 0 <= value <= MAX_ID:
        raise BlobError("id-range", f"{what} {value} outside 0..=4294967295")
    return value


def _check_lattice(m: int, t: int) -> None:
    if not 1 <= m <= 32:
        raise BlobError("m-range", f"m = {m}")
    if not 1 <= t <= 8:
        raise BlobError("t-range", f"t = {t}")


def encode_payload(body: Body) -> bytes:
    if not isinstance(body, dict):
        raise BlobError("shape", "the topology must be a JSON object")
    out = bytearray(MAGIC)
    out.append(VERSION)
    kind = body.get("kind")
    if kind == "explicit":
        out.append(KIND_EXPLICIT)
        nodes, edges = body["nodes"], body["edges"]
        put_varint(out, len(nodes))
        previous = 0
        for node in nodes:
            put_varint(out, zigzag(_check_id(node, "node") - previous))
            previous = node
        put_varint(out, len(edges))
        prev_u = 0
        for u, v in edges:
            _check_id(u, "edge end")
            _check_id(v, "edge end")
            put_varint(out, zigzag(u - prev_u))
            put_varint(out, zigzag(v - u))
            prev_u = u
    elif kind == "zephyr-removed":
        out.append(KIND_ZEPHYR)
        m, t = body["m"], body["t"]
        _check_lattice(m, t)
        put_varint(out, m)
        put_varint(out, t)
        removed_nodes = body["removed_nodes"]
        removed_edges = [tuple(edge) for edge in body["removed_edges"]]
        _check_removals(m, t, removed_nodes, removed_edges)
        put_varint(out, len(removed_nodes))
        previous = 0
        for node in removed_nodes:
            put_varint(out, node - previous)
            previous = node
        put_varint(out, len(removed_edges))
        prev_a = 0
        for a, b in removed_edges:
            put_varint(out, a - prev_a)
            put_varint(out, b - a)
            prev_a = a
    else:
        raise BlobError("kind", f"unknown kind {kind!r}")
    return bytes(out)


def _check_removals(
    m: int, t: int, removed_nodes: list[int], removed_edges: list[tuple[int, int]]
) -> None:
    count = ice.node_count(m, t)
    previous = -1
    for node in removed_nodes:
        if not 0 <= node < count:
            raise BlobError("not-a-node", f"removed node {node}")
        if node <= previous:
            raise BlobError("order", "removed nodes must be strictly ascending")
        previous = node
    removed = set(removed_nodes)
    couplers = set(ice.lattice_edges(m, t)) if removed_edges else set()
    last: tuple[int, int] | None = None
    for edge in removed_edges:
        a, b = edge
        if not a < b:
            raise BlobError("order", f"removed edge {edge} needs a < b")
        if last is not None and edge <= last:
            raise BlobError("order", "removed edges must be strictly ascending")
        last = edge
        if edge not in couplers:
            raise BlobError("not-a-coupler", f"removed edge {edge}")
        if a in removed or b in removed:
            raise BlobError("edge-at-removed-node", f"removed edge {edge}")


def decode_payload(payload: bytes) -> Body:
    reader = _Reader(payload)
    if payload[:4] != MAGIC:
        raise BlobError("magic", "payload does not start with QTOP")
    reader.pos = 4
    version = reader.byte()
    if version != VERSION:
        raise BlobError("version", f"version {version}")
    kind = reader.byte()
    if kind == KIND_EXPLICIT:
        body = _decode_explicit(reader)
    elif kind == KIND_ZEPHYR:
        body = _decode_zephyr(reader)
    else:
        raise BlobError("kind", f"kind {kind}")
    if reader.remaining():
        raise BlobError("trailing-bytes", f"{reader.remaining()} bytes after the body")
    return body


def _decode_explicit(reader: _Reader) -> Body:
    nodes: list[int] = []
    previous = 0
    for _ in range(reader.count(1)):
        previous = _check_id(previous + reader.signed(), "node")
        nodes.append(previous)
    edges: list[list[int]] = []
    prev_u = 0
    for _ in range(reader.count(2)):
        u = _check_id(prev_u + reader.signed(), "edge end")
        v = _check_id(u + reader.signed(), "edge end")
        edges.append([u, v])
        prev_u = u
    return {"kind": "explicit", "nodes": nodes, "edges": edges}


def _decode_zephyr(reader: _Reader) -> Body:
    m = reader.varint()
    t = reader.varint()
    _check_lattice(m, t)
    count = ice.node_count(m, t)
    removed_nodes: list[int] = []
    previous = 0
    for index in range(reader.count(1)):
        delta = reader.varint()
        if index > 0 and delta == 0:
            raise BlobError("order", "removed nodes must be strictly ascending")
        previous += delta
        if previous >= count:
            raise BlobError("not-a-node", f"removed node {previous}")
        removed_nodes.append(previous)
    removed_edges: list[list[int]] = []
    prev_a = 0
    for _ in range(reader.count(2)):
        a = prev_a + reader.varint()
        gap = reader.varint()
        if gap == 0:
            raise BlobError("order", "removed edge needs b > a")
        b = a + gap
        if b >= count:
            raise BlobError("not-a-node", f"removed edge end {b}")
        if removed_edges and (a, b) <= tuple(removed_edges[-1]):
            raise BlobError("order", "removed edges must be strictly ascending")
        removed_edges.append([a, b])
        prev_a = a
    # The same rules as the encoder, so a decoded body always re-encodes.
    _check_removals(m, t, removed_nodes, [(a, b) for a, b in removed_edges])
    return {
        "kind": "zephyr-removed",
        "m": m,
        "t": t,
        "removed_nodes": removed_nodes,
        "removed_edges": removed_edges,
    }


def expand(body: Body) -> Body:
    """The explicit topology a body stands for."""
    if body["kind"] == "explicit":
        return body
    nodes, edges = ice.lattice(body["m"], body["t"])
    removed = set(body["removed_nodes"])
    dropped = {tuple(edge) for edge in body["removed_edges"]}
    return {
        "kind": "explicit",
        "nodes": [n for n in nodes if n not in removed],
        "edges": [
            [a, b]
            for a, b in edges
            if a not in removed and b not in removed and (a, b) not in dropped
        ],
    }


# ---- gzip framing ----


def encode(body: Body) -> bytes:
    payload = encode_payload(body)
    if len(payload) > MAX_PAYLOAD:
        raise BlobError("too-large", "payload above 64 MiB")
    compressor = zlib.compressobj(9, zlib.DEFLATED, -15)
    stream = compressor.compress(payload) + compressor.flush()
    trailer = struct.pack("<II", zlib.crc32(payload), len(payload) & 0xFFFFFFFF)
    return HEADER + stream + trailer


def decompress(data: bytes) -> bytes:
    if len(data) < 18:
        raise BlobError("gzip", "shorter than a gzip member")
    if data[0] != 0x1F or data[1] != 0x8B or data[2] != 8 or data[3] != 0:
        raise BlobError("gzip", "header is not ID1 ID2 CM=8 FLG=0")
    inflater = zlib.decompressobj(-15)
    # max_length bounds the output, so a DEFLATE bomb stops at 64 MiB + 1 byte.
    try:
        out = inflater.decompress(data[10:], MAX_PAYLOAD + 1)
    except zlib.error as error:
        raise BlobError("gzip", f"bad DEFLATE stream: {error}") from error
    if len(out) > MAX_PAYLOAD:
        raise BlobError("too-large", "payload above 64 MiB")
    if not inflater.eof:
        raise BlobError("gzip", "DEFLATE stream is truncated")
    tail = inflater.unused_data
    if len(tail) < 8:
        raise BlobError("gzip", "trailer is truncated")
    if len(tail) > 8:
        raise BlobError("gzip", "bytes after the member")
    crc, size = struct.unpack("<II", tail)
    if crc != zlib.crc32(out):
        raise BlobError("crc", "CRC-32 mismatch")
    if size != len(out) & 0xFFFFFFFF:
        raise BlobError("isize", "ISIZE mismatch")
    return bytes(out)


def decode(data: bytes) -> Body:
    return decode_payload(decompress(data))
