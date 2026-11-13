"""Independent compact columns, palettes, and the common instance prefix."""

import struct
from dataclasses import dataclass

from .digests import instance_digest
from .instance import canonical_bytes, edge_fault, metadata_code


class CodecError(ValueError):
    """A stable snake-case codec failure."""

    def __init__(self, name: str):
        self.name = name
        super().__init__(name)


LIMITS = {
    "nodes": 2**24,
    "edges": 2**26,
    "coefficient": 2**30,
    "offset": 2**56,
    "metadata": 2**26,
}


def _limits(limits: dict | None) -> dict:
    return LIMITS | (limits or {})


def check_bit_array(data: bytes | memoryview, bits: int) -> None:
    if bits < 0 or bits > 2**64 - 8:
        raise CodecError("bounds_exceeded")
    if len(data) != (bits + 7) // 8:
        raise CodecError("length")
    if bits % 8 and data[-1] >> (bits % 8):
        raise CodecError("padding")


@dataclass(frozen=True)
class Column:
    """Borrowed values, with no allocation proportional to count."""

    data: memoryview
    count: int
    width: int

    def get(self, index: int) -> int:
        if not 0 <= index < self.count:
            raise CodecError("length")
        value = 0
        for bit in range(self.width):
            at = index * self.width + bit
            value |= ((self.data[at // 8] >> (at % 8)) & 1) << bit
        return value

    def minimal(self) -> None:
        maximum = 0
        for index in range(self.count):
            maximum |= self.get(index)
        if maximum.bit_length() != self.width:
            raise CodecError("non_canonical")


def read_column(data: bytes | memoryview, count: int) -> tuple[Column, int]:
    """Preflight a borrowed column. Call minimal after structural checks."""
    if count < 0 or count > 2**64 - 1:
        raise CodecError("bounds_exceeded")
    if not data:
        raise CodecError("length")
    width = data[0]
    if width > 32:
        raise CodecError("invalid_symbol")
    bits = count * width
    if bits > 2**64 - 8:
        raise CodecError("bounds_exceeded")
    end = 1 + (bits + 7) // 8
    if end > len(data):
        raise CodecError("length")
    payload = memoryview(data)[1:end]
    check_bit_array(payload, bits)
    return Column(payload, count, width), end


def encode_column(values: list[int]) -> bytes:
    if any(v < 0 or v > 2**32 - 1 for v in values):
        raise CodecError("bounds_exceeded")
    width = max(values, default=0).bit_length()
    out = bytearray(1 + (len(values) * width + 7) // 8)
    out[0] = width
    for index, value in enumerate(values):
        for bit in range(width):
            at = index * width + bit
            out[1 + at // 8] |= ((value >> bit) & 1) << (at % 8)
    return bytes(out)


def decode_column(data: bytes, count: int) -> tuple[list[int], int]:
    column, end = read_column(data, count)
    column.minimal()
    return [column.get(i) for i in range(count)], end


def encode_palette(values: list[int]) -> bytes:
    if any(v < -(2**31) or v >= 2**31 for v in values):
        raise CodecError("bounds_exceeded")
    # Four stable radix passes give signed ascending order.
    ordered = list(values)
    for shift in (0, 8, 16, 24):
        buckets: list[list[int]] = [[] for _ in range(256)]
        for value in ordered:
            buckets[((value ^ (1 << 31)) >> shift) & 255].append(value)
        ordered = [value for bucket in buckets for value in bucket]
    palette = []
    for value in ordered:
        if not palette or palette[-1] != value:
            palette.append(value)
    ranks = {value: rank for rank, value in enumerate(palette)}
    return (
        struct.pack("<I", len(palette))
        + b"".join(struct.pack("<i", v) for v in palette)
        + encode_column([ranks[v] for v in values])
    )


@dataclass(frozen=True)
class Palette:
    data: memoryview
    count: int
    ranks: Column

    def value(self, index: int) -> int:
        if not 0 <= index < self.count:
            raise CodecError("invalid_symbol")
        return struct.unpack_from("<i", self.data, index * 4)[0]

    def minimal(self) -> None:
        self.ranks.minimal()
        used = bytearray(self.count)
        for index in range(1, self.count):
            if self.value(index - 1) >= self.value(index):
                raise CodecError("non_canonical")
        for index in range(self.ranks.count):
            used[self.ranks.get(index)] = 1
        if 0 in used:
            raise CodecError("non_canonical")

    def decode(self) -> list[int]:
        return [self.value(self.ranks.get(i)) for i in range(self.ranks.count)]


def _palette(
    data: bytes | memoryview, count: int, coefficient: int
) -> tuple[Palette, int]:
    if len(data) < 4:
        raise CodecError("length")
    size = struct.unpack_from("<I", data)[0]
    if size > count:
        raise CodecError("bounds_exceeded")
    if count and not size:
        raise CodecError("invalid_symbol")
    end = 4 + 4 * size
    if end > len(data):
        raise CodecError("length")
    values = memoryview(data)[4:end]
    for index in range(size):
        if abs(struct.unpack_from("<i", values, 4 * index)[0]) > coefficient:
            raise CodecError("bounds_exceeded")
    ranks, consumed = read_column(memoryview(data)[end:], count)
    for index in range(count):
        if ranks.get(index) >= size:
            raise CodecError("invalid_symbol")
    return Palette(values, size, ranks), end + consumed


def decode_palette(
    data: bytes, count: int, coefficient: int = 2**30
) -> tuple[list[int], int]:
    palette, end = _palette(data, count, coefficient)
    palette.minimal()
    return palette.decode(), end


def encode_prefix(instance: dict) -> bytes:
    """Use explicit head fields or recover them from an oracle canonical instance."""
    if "canonical" in instance:
        canonical = instance["canonical"]
        descriptor, topology, nonce = (
            canonical[1:33],
            canonical[33:65],
            canonical[65:97],
        )
    else:
        descriptor = instance["descriptor_digest"]
        topology = instance["topology_order_digest"]
        nonce = instance.get("nonce") or bytes(32)
    if any(len(value) != 32 for value in (descriptor, topology, nonce)):
        raise CodecError("unencodable_instance")
    ids, edges = instance["node_ids"], instance["edges"]
    if len(instance["h"]) != len(ids) or len(instance["j"]) != len(edges):
        raise CodecError("unencodable_instance")
    return (
        descriptor
        + topology
        + nonce
        + struct.pack("<IIq", len(ids), len(edges), instance["offset_milli"])
        + encode_column(ids)
        + encode_column([p for edge in edges for p in edge])
        + encode_palette(instance["h"])
        + encode_palette(instance["j"])
    )


@dataclass(frozen=True)
class InstancePrefix:
    head: memoryview
    node_count: int
    edge_count: int
    ids: Column
    endpoints: Column
    h: Palette
    j: Palette
    metadata_start: int

    @property
    def descriptor_digest(self) -> bytes:
        return bytes(self.head[:32])


def preflight_prefix(data: bytes, limits: dict | None = None) -> InstancePrefix:
    limits = _limits(limits)
    if (
        len(data)
        > 124 + 12 * limits["nodes"] + 16 * limits["edges"] + limits["metadata"]
    ):
        raise CodecError("bounds_exceeded")
    if len(data) < 100:
        raise CodecError("length")
    n = struct.unpack_from("<I", data, 96)[0]
    if n > limits["nodes"]:
        raise CodecError("bounds_exceeded")
    if len(data) < 104:
        raise CodecError("length")
    e = struct.unpack_from("<I", data, 100)[0]
    if e > limits["edges"]:
        raise CodecError("bounds_exceeded")
    if len(data) < 112:
        raise CodecError("length")
    offset = struct.unpack_from("<q", data, 104)[0]
    if abs(offset) > limits["offset"]:
        raise CodecError("bounds_exceeded")
    view = memoryview(data)
    at = 112
    ids, used = read_column(view[at:], n)
    at += used
    endpoints, used = read_column(view[at:], 2 * e)
    at += used
    h, used = _palette(view[at:], n, limits["coefficient"])
    at += used
    j, used = _palette(view[at:], e, limits["coefficient"])
    at += used
    return InstancePrefix(view[:112], n, e, ids, endpoints, h, j, at)


def decode_prefix(
    prefix: InstancePrefix,
    metadata: bytes,
    limits: dict | None = None,
    kinds: tuple[int, ...] = (),
) -> dict:
    """Reconstruct canonical arrays after the family has preflighted all metadata."""
    limits = _limits(limits)
    if (
        len(metadata) > limits["metadata"]
        or prefix.node_count > limits["nodes"]
        or prefix.edge_count > limits["edges"]
    ):
        raise CodecError("bounds_exceeded")
    if metadata_code(metadata, list(kinds)):
        raise CodecError("invalid_instance")
    previous = None
    for index in range(prefix.edge_count):
        edge = (prefix.endpoints.get(2 * index), prefix.endpoints.get(2 * index + 1))
        if (
            edge[0] >= edge[1]
            or edge[1] >= prefix.node_count
            or (previous is not None and previous >= edge)
        ):
            raise CodecError("invalid_instance")
        previous = edge
    for column in (prefix.ids, prefix.endpoints, prefix.h, prefix.j):
        column.minimal()
    instance = {
        "descriptor_digest": prefix.descriptor_digest,
        "topology_order_digest": bytes(prefix.head[32:64]),
        "nonce": bytes(prefix.head[64:96]),
        "node_ids": [prefix.ids.get(i) for i in range(prefix.node_count)],
        "edges": [
            (prefix.endpoints.get(2 * i), prefix.endpoints.get(2 * i + 1))
            for i in range(prefix.edge_count)
        ],
        "h": prefix.h.decode(),
        "j": prefix.j.decode(),
        "offset_milli": struct.unpack_from("<q", prefix.head, 104)[0],
        "metadata": metadata,
    }
    if edge_fault(instance["edges"], prefix.node_count):
        raise CodecError("invalid_instance")
    if abs(instance["offset_milli"]) > limits["offset"] or any(
        abs(v) > limits["coefficient"] for v in (*instance["h"], *instance["j"])
    ):
        raise CodecError("bounds_exceeded")
    instance["canonical"] = canonical_bytes(**instance)
    instance["digest"] = instance_digest(instance["canonical"])
    return instance
