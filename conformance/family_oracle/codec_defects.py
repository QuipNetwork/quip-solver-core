"""Independent defects metadata packing and pinned-forest witness encoding."""

import struct

from . import defects
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
from .params import DEFECTS, decode_params
from .polariton import Lattice

LIMITS = {
    "nodes": 24,
    "edges": 96,
    "coefficient": 189328,
    "offset": 1318296,
    "metadata": 254,
}


def _params(params):
    code, values = decode_params(DEFECTS, params)
    if code or values is None:
        raise CodecError("invalid_instance")
    return values


def _take(data, at, count):
    end = at + count
    if end > len(data):
        raise CodecError("length")
    return data[at:end], end


def _sections(data):
    if data[:2] != b"\x03\x00":
        raise CodecError("invalid_instance")
    at = 2
    out = []
    for kind in (4, 5, 8):
        header, at = _take(data, at, 6)
        k, count = struct.unpack("<HI", header)
        if k != kind:
            raise CodecError("invalid_instance")
        payload, at = _take(data, at, count)
        out.append(payload)
    if at != len(data):
        raise CodecError("invalid_instance")
    return out


def _border(side, v):
    x, y = v % (side + 1), v // (side + 1)
    return x in (0, side) or y in (0, side)


def _window(values, origin):
    c0, r0 = struct.unpack("<HH", origin)
    grid = 2 * values["zephyr_m"] - 1
    return min(c0, r0) >= 1 and max(c0, r0) + values["side"] <= grid


def _raw_list(data, at, bound):
    count, at = _take(data, at, 2)
    count = int.from_bytes(count, "little")
    if count > bound:
        raise CodecError("bounds_exceeded")
    raw, at = _take(data, at, 4 * count)
    values = list(struct.unpack(f"<{count}I", raw))
    if any(v >= bound for v in values):
        raise CodecError("invalid_instance")
    return values, at


def _metadata(values, instance):
    side = values["side"]
    n, vertices = 2 * side * (side + 1), (side + 1) ** 2
    charges, incidence, rest = _sections(instance["metadata"])
    if len(instance["node_ids"]) != n or charges[:2] != struct.pack("<H", side):
        raise CodecError("invalid_instance")
    labels = charges[2:]
    if len(labels) != vertices or len(incidence) != 4 + 4 * n:
        raise CodecError("invalid_instance")
    for v, q in enumerate(labels):
        if q not in ((127,) if _border(side, v) else (254, 0, 2)):
            raise CodecError("invalid_instance")
    positions = list(struct.unpack(f"<{n}I", incidence[4:]))
    if sorted(positions) != list(range(n)) or not _window(values, incidence[:4]):
        raise CodecError("invalid_instance")
    fixed, at = _take(rest, 0, 48)
    obstacles, at = _raw_list(rest, at, vertices)
    pins, at = _raw_list(rest, at, n)
    if at != len(rest):
        raise CodecError("invalid_instance")
    return labels, incidence[:4], positions, fixed, obstacles, pins


def encode_instance(params: bytes, instance: dict) -> bytes:
    values = _params(params)
    try:
        labels, origin, positions, fixed, obstacles, pins = _metadata(values, instance)
    except CodecError as error:
        raise CodecError("unencodable_instance") from error
    prefix = encode_prefix(instance)
    checked = preflight_prefix(prefix, LIMITS)
    expected = descriptor_digest(b"quip/defects-static", 1, params_digest(params))
    if checked.descriptor_digest != expected:
        raise CodecError("descriptor_mismatch")
    if len(instance["metadata"]) > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    ranks = [
        {254: 0, 0: 1, 2: 2}[q]
        for v, q in enumerate(labels)
        if not _border(values["side"], v)
    ]
    packed = sum(rank << (2 * i) for i, rank in enumerate(ranks))
    return (
        prefix
        + origin
        + packed.to_bytes((2 * len(ranks) + 7) // 8, "little")
        + encode_column(positions)
        + fixed
        + struct.pack("<H", len(obstacles))
        + encode_column(obstacles)
        + struct.pack("<H", len(pins))
        + encode_column(pins)
    )


def decode_instance(params: bytes, data: bytes) -> dict:
    values = _params(params)
    side = values["side"]
    n, vertices, interior = 2 * side * (side + 1), (side + 1) ** 2, (side - 1) ** 2
    prefix = preflight_prefix(data, LIMITS)
    at = prefix.metadata_start
    origin, at = _take(data, at, 4)
    packed, at = _take(data, at, (2 * interior + 7) // 8)
    check_bit_array(packed, 2 * interior)
    ranks = [(int.from_bytes(packed, "little") >> (2 * i)) & 3 for i in range(interior)]
    if 3 in ranks:
        raise CodecError("invalid_symbol")
    positions, end = read_column(memoryview(data)[at:], n)
    at += end
    fixed, at = _take(data, at, 48)
    columns = []
    for bound in (vertices, n):
        raw, at = _take(data, at, 2)
        count = int.from_bytes(raw, "little")
        if count > bound:
            raise CodecError("bounds_exceeded")
        column, end = read_column(memoryview(data)[at:], count)
        at += end
        columns.append(column)
    obstacles, pins = columns
    expanded = (
        20 + 2 + vertices + 4 + 4 * n + 48 + 4 + 4 * (obstacles.count + pins.count)
    )
    if expanded > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    if at != len(data):
        raise CodecError("length")
    expected = descriptor_digest(b"quip/defects-static", 1, params_digest(params))
    if prefix.descriptor_digest != expected:
        raise CodecError("descriptor_mismatch")
    if prefix.node_count != n or not _window(values, origin):
        raise CodecError("invalid_instance")
    seen = 0
    for i in range(n):
        v = positions.get(i)
        if v >= n or seen & (1 << v):
            raise CodecError("invalid_instance")
        seen |= 1 << v
    for column, bound in ((obstacles, vertices), (pins, n)):
        if any(column.get(i) >= bound for i in range(column.count)):
            raise CodecError("invalid_instance")
    labels = bytearray()
    cursor = iter(ranks)
    for v in range(vertices):
        labels.append(127 if _border(side, v) else (254, 0, 2)[next(cursor)])
    charges = struct.pack("<H", side) + labels
    incidence = origin + b"".join(struct.pack("<I", positions.get(i)) for i in range(n))
    rest = fixed
    for column in (obstacles, pins):
        rest += struct.pack("<H", column.count)
        rest += b"".join(struct.pack("<I", column.get(i)) for i in range(column.count))
    metadata = b"\x03\x00" + b"".join(
        struct.pack("<HI", kind, len(payload)) + payload
        for kind, payload in ((4, charges), (5, incidence), (8, rest))
    )
    instance = decode_prefix(prefix, metadata, LIMITS, (4, 5, 8))
    for column in (positions, obstacles, pins):
        column.minimal()
    return instance


def _forest(instance):
    parsed = defects.parse_metadata(instance)
    if parsed is None:
        raise CodecError("invalid_instance")
    lattice = Lattice(parsed["side"])
    labels, positions, pins = parsed["labels"], parsed["positions"], set(parsed["pins"])
    visited = {v for v, q in enumerate(labels) if q is None}
    order = sorted(visited)
    parents = {}
    cursor = 0
    for root in [None, *range(len(labels))]:
        if root is not None:
            if root in visited:
                continue
            visited.add(root)
            order.append(root)
        while cursor < len(order):
            v = order[cursor]
            cursor += 1
            for e, sign in lattice.incident[v]:
                if e in pins:
                    continue
                tail, head, _ = lattice.links[e]
                other = head if sign == -1 else tail
                if other in visited:
                    continue
                visited.add(other)
                parents[other] = (e, -sign)
                order.append(other)
    dependent = {e for e, _ in parents.values()}
    explicit = [e for e in range(len(positions)) if e not in pins | dependent]
    return lattice, labels, positions, pins, parents, order, explicit


def encode(instance: dict, spins: list[int]) -> bytes:
    lattice, labels, positions, pins, _, _, explicit = _forest(instance)
    if len(spins) != len(positions):
        raise CodecError("length")
    if any(s not in (-1, 1) for s in spins):
        raise CodecError("spin_value")
    links = [spins[p] for p in positions]
    if any(links[e] != 1 for e in pins) or any(
        q is not None and got != q
        for q, got in zip(labels, lattice.charges(links), strict=True)
    ):
        raise CodecError("infeasible")
    bits = sum((links[e] == 1) << i for i, e in enumerate(explicit))
    return bits.to_bytes((len(explicit) + 7) // 8, "little")


def decode(instance: dict, data: bytes) -> list[int]:
    lattice, labels, positions, pins, parents, order, explicit = _forest(instance)
    check_bit_array(data, len(explicit))
    links = [0] * len(positions)
    for e in pins:
        links[e] = 1
    bits = int.from_bytes(data, "little")
    for i, e in enumerate(explicit):
        links[e] = 1 if bits & (1 << i) else -1
    for v in reversed(order):
        if v not in parents:
            continue
        e, sign = parents[v]
        residual = labels[v] - sum(s * links[a] for a, s in lattice.incident[v])
        spin = residual * sign
        if spin not in (-1, 1):
            raise CodecError("infeasible")
        links[e] = spin
    spins = [0] * len(positions)
    for e, p in enumerate(positions):
        spins[p] = links[e]
    # This also checks closed and isolated roots.
    if encode(instance, spins) != data:
        raise CodecError("non_canonical")
    return spins


def malformed_instances(params, instance, compact):
    """Independent mutations of each family field and shared prefix count."""
    prefix = preflight_prefix(compact, LIMITS)
    start = prefix.metadata_start
    rows = []

    def add(data, error):
        rows.append({"data": bytes(data).hex(), "error": error})

    add(compact + b"\0", "length")
    add(compact[:-1], "length")
    for at in (96, 100):
        data = bytearray(compact)
        struct.pack_into("<I", data, at, 2**32 - 1)
        add(data, "bounds_exceeded")
    data = bytearray(compact)
    data[112] = 33
    add(data, "invalid_symbol")
    data = bytearray(compact)
    data[0] ^= 1
    add(data, "descriptor_mismatch")
    side = _params(params)["side"]
    bits = 2 * (side - 1) ** 2
    if bits % 8:
        data = bytearray(compact)
        data[start + 4 + bits // 8] |= 1 << (bits % 8)
        add(data, "padding")
    data = bytearray(compact)
    data[start + 4] |= 3
    add(data, "invalid_symbol")
    at = start + 4 + (bits + 7) // 8
    positions, end = read_column(memoryview(compact)[at:], prefix.node_count)
    width = positions.width + 1
    packed = sum(positions.get(i) << (width * i) for i in range(positions.count))
    nonminimal = bytes([width]) + packed.to_bytes(
        (positions.count * width + 7) // 8, "little"
    )
    add(compact[:at] + nonminimal + compact[at + end :], "non_canonical")
    # Duplicate position: structural membership precedes metadata minimality.
    packed = 0
    for i in range(positions.count):
        packed |= positions.get(0 if i == 1 else i) << (positions.width * i)
    duplicate = bytes([positions.width]) + packed.to_bytes(end - 1, "little")
    add(compact[:at] + duplicate + compact[at + end :], "invalid_instance")
    at += end + 48
    data = bytearray(compact)
    struct.pack_into("<H", data, at, 65535)
    add(data, "bounds_exceeded")
    count = int.from_bytes(compact[at : at + 2], "little")
    _, end = read_column(memoryview(compact)[at + 2 :], count)
    at += 2 + end
    data = bytearray(compact)
    struct.pack_into("<H", data, at, 65535)
    add(data, "bounds_exceeded")
    # Too many palette entries must fail before reading their bytes.
    at = 112
    _, end = read_column(memoryview(compact)[at:], prefix.node_count)
    at += end
    _, end = read_column(memoryview(compact)[at:], 2 * prefix.edge_count)
    at += end
    data = bytearray(compact)
    struct.pack_into("<I", data, at, 2**32 - 1)
    add(data, "bounds_exceeded")
    # Append one unused, sorted h coefficient while retaining every original rank.
    size = int.from_bytes(compact[at : at + 4], "little")
    palette_end = at + 4 + 4 * size
    ranks, rank_end = read_column(memoryview(compact)[palette_end:], prefix.node_count)
    maximum = int.from_bytes(
        compact[palette_end - 4 : palette_end], "little", signed=True
    )
    if size < prefix.node_count and maximum < LIMITS["coefficient"]:
        replacement = (
            struct.pack("<I", size + 1)
            + compact[at + 4 : palette_end]
            + struct.pack("<i", maximum + 1)
            + encode_column([ranks.get(i) for i in range(ranks.count)])
        )
        add(
            compact[:at] + replacement + compact[palette_end + rank_end :],
            "non_canonical",
        )
    return rows


def cases() -> list[dict]:
    from .build import _defects_instances, _defects_rows

    rows = []
    for name, params, count, (nodes, edges), _ in _defects_rows():
        for index, _, _, instance in _defects_instances(params, nodes, edges, count):
            if instance is None:
                continue
            compact = encode_instance(params, instance)
            spins = defects.node_spins(instance, instance["orientation"])
            witness = encode(instance, spins)
            bits = len(_forest(instance)[-1])
            bad_witnesses = [
                {"data": (witness + b"\0").hex(), "error": "length"},
                {"data": witness[:-1].hex(), "error": "length"},
            ]
            for bit in range(bits, 8 * len(witness)):
                bad = bytearray(witness)
                bad[bit // 8] |= 1 << (bit % 8)
                bad_witnesses.append({"data": bad.hex(), "error": "padding"})
            rows.append(
                {
                    "name": f"{name}-{index}",
                    "params": params.hex(),
                    "canonical": instance["canonical"].hex(),
                    "compact": compact.hex(),
                    "spins": spins,
                    "witness": witness.hex(),
                    "witness_bits": bits,
                    "invalid_instances": malformed_instances(params, instance, compact),
                    "invalid_witnesses": bad_witnesses,
                }
            )
    return rows
