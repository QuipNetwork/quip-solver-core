"""Independent polariton forest and lossless instance codecs."""

import struct

from . import polariton
from .codec_common import (
    CodecError,
    check_bit_array,
    decode_prefix,
    encode_column,
    encode_prefix,
    preflight_prefix,
    read_column,
)
from .digests import descriptor_digest, params_digest
from .params import POLARITON, decode_params

LIMITS = {
    "nodes": 131072,
    "edges": 2097152,
    "coefficient": 1000,
    "offset": 1099511627776,
    "metadata": 1048576,
}


def _side(params):
    _, values = decode_params(POLARITON, params)
    if values is None:
        raise CodecError("invalid_instance")
    return values["side"], 2 * values["zephyr_m"] - 1


def _metadata(instance):
    data = instance["metadata"]
    if len(data) < 10 or data[:4] != b"\x02\x00\x04\x00":
        raise CodecError("unencodable_instance")
    length = int.from_bytes(data[4:8], "little")
    at = 8 + length
    if (
        len(data) < at + 10
        or data[at : at + 2] != b"\x05\x00"
        or int.from_bytes(data[at + 2 : at + 6], "little") != len(data) - at - 6
    ):
        raise CodecError("unencodable_instance")
    side = int.from_bytes(data[8:10], "little")
    labels = [None if b == 127 else b if b < 128 else b - 256 for b in data[10:at]]
    c, r = struct.unpack_from("<HH", data, at + 6)
    rest = data[at + 10 :]
    if len(rest) % 4:
        raise CodecError("unencodable_instance")
    positions = [v[0] for v in struct.iter_unpack("<I", rest)]
    return side, labels, c, r, positions


def _forest(instance):
    side, labels, _, _, positions = _metadata(instance)
    lattice = polariton.Lattice(side)
    if len(labels) != len(lattice.incident) or sorted(positions) != list(
        range(len(lattice.links))
    ):
        raise CodecError("invalid_instance")
    parents = [None] * len(labels)
    seen = [q is None for q in labels]
    queue = [v for v, free in enumerate(seen) if free]
    for v in queue:
        for edge, _ in lattice.incident[v]:
            tail, head, _ = lattice.links[edge]
            other = head if tail == v else tail
            if not seen[other]:
                seen[other] = True
                parents[other] = edge
                queue.append(other)
    omitted = {e for e in parents if e is not None}
    explicit = [e for e in range(len(positions)) if e not in omitted]
    return lattice, labels, positions, parents, queue, explicit


def encode(instance: dict, spins: list[int]) -> bytes:
    lattice, labels, positions, _, _, explicit = _forest(instance)
    if len(spins) != len(positions):
        raise CodecError("length")
    if any(s not in (-1, 1) for s in spins):
        raise CodecError("spin_value")
    links = [spins[p] for p in positions]
    if any(
        q is not None and q != got for q, got in zip(labels, lattice.charges(links))
    ):
        raise CodecError("infeasible")
    out = bytearray((len(explicit) + 7) // 8)
    for i, edge in enumerate(explicit):
        out[i // 8] |= (links[edge] == 1) << (i % 8)
    return bytes(out)


def decode(instance: dict, data: bytes) -> list[int]:
    lattice, labels, positions, parents, queue, explicit = _forest(instance)
    check_bit_array(data, len(explicit))
    links = [0] * len(positions)
    for i, edge in enumerate(explicit):
        links[edge] = 1 if data[i // 8] >> (i % 8) & 1 else -1
    for v in reversed(queue):
        edge = parents[v]
        if edge is None:
            continue
        residual = labels[v] - sum(sign * links[e] for e, sign in lattice.incident[v])
        sign = next(sign for e, sign in lattice.incident[v] if e == edge)
        value = residual * sign
        if value not in (-1, 1):
            raise CodecError("infeasible")
        links[edge] = value
    spins = [0] * len(positions)
    for e, p in enumerate(positions):
        spins[p] = links[e]
    return spins


def encode_instance(params: bytes, instance: dict) -> bytes:
    side, grid = _side(params)
    got, labels, c, r, positions = _metadata(instance)
    lattice = polariton.Lattice(side)
    interior = set(lattice.interior())
    if (
        got != side
        or len(labels) != len(lattice.incident)
        or len(instance["node_ids"]) != len(lattice.links)
        or len(positions) != len(lattice.links)
        or set(positions) != set(range(len(lattice.links)))
        or not (1 <= c <= grid - side and 1 <= r <= grid - side)
    ):
        raise CodecError("unencodable_instance")
    ranks = []
    for v, q in enumerate(labels):
        if v in interior:
            if q not in (-2, 0, 2):
                raise CodecError("unencodable_instance")
            ranks.append((q + 2) // 2)
        elif q is not None:
            raise CodecError("unencodable_instance")
    packed = bytearray((2 * len(ranks) + 7) // 8)
    for i, rank in enumerate(ranks):
        packed[i // 4] |= rank << (2 * (i % 4))
    out = (
        encode_prefix(instance)
        + struct.pack("<HH", c, r)
        + packed
        + encode_column(positions)
    )
    preflight_prefix(out, LIMITS)
    return out


def decode_instance(params: bytes, data: bytes) -> dict:
    prefix = preflight_prefix(data, LIMITS)
    side, grid = _side(params)
    vertices, nodes, inner = (side + 1) ** 2, 2 * side * (side + 1), (side - 1) ** 2
    if 20 + vertices + 4 * nodes > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    meta = data[prefix.metadata_start :]
    end = 4 + (2 * inner + 7) // 8
    if len(meta) < end:
        raise CodecError("length")
    c, r = struct.unpack_from("<HH", meta)
    check_bit_array(meta[4:end], 2 * inner)
    for i in range(inner):
        if meta[4 + i // 4] >> (2 * (i % 4)) & 3 == 3:
            raise CodecError("invalid_symbol")
    column, used = read_column(meta[end:], nodes)
    if end + used != len(meta):
        raise CodecError("length")
    digest = descriptor_digest(b"quip/polariton-static", 1, params_digest(params))
    if prefix.descriptor_digest != digest:
        raise CodecError("descriptor_mismatch")
    if prefix.node_count != nodes or not (
        1 <= c <= grid - side and 1 <= r <= grid - side
    ):
        raise CodecError("invalid_instance")
    positions = [column.get(i) for i in range(nodes)]
    if set(positions) != set(range(nodes)):
        raise CodecError("invalid_instance")
    column.minimal()
    labels = [None] * vertices
    for i, v in enumerate(polariton.Lattice(side).interior()):
        labels[v] = 2 * (meta[4 + i // 4] >> (2 * (i % 4)) & 3) - 2
    return decode_prefix(
        prefix, polariton.metadata(side, labels, c, r, positions), LIMITS, (4, 5)
    )


def cases() -> list[dict]:
    from .build import _polariton_params
    from .ice import lattice

    rows = []
    for side in (2, 4, 8):
        params = _polariton_params(6, 1, side, 1 if side == 4 else 0)
        nodes, edges = lattice(6, 1)
        for reverse in (False, True):
            code, instance = polariton.generate(
                b"quip/polariton-static",
                1,
                params,
                list(reversed(nodes)) if reverse else nodes,
                edges,
                bytes(32),
            )
            if code or instance is None:
                raise ValueError("golden construction failed")
            good, bad = polariton.witnesses(instance)
            compact = encode_instance(params, instance)
            witness = encode(instance, good)
            rows.append(
                {
                    "name": f"l{side}-reverse-{reverse}",
                    "params": params.hex(),
                    "canonical": instance["canonical"].hex(),
                    "compact_instance": compact.hex(),
                    "spins": good,
                    "witness": witness.hex(),
                    "negative_witness": [
                        {"bytes": (witness + b"\x00").hex(), "error": "length"},
                        {"spins": bad, "error": "infeasible"},
                    ],
                    "negative_instance": [
                        {"bytes": (compact + b"\x00").hex(), "error": "length"},
                        {
                            "bytes": (bytes([compact[0] ^ 1]) + compact[1:]).hex(),
                            "error": "descriptor_mismatch",
                        },
                    ],
                }
            )
    return rows
