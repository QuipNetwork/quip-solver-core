"""Independent compact routing instance and chain witness codecs."""

import struct

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
from .routing import metadata as make_metadata

LIMITS = {
    "nodes": 131072,
    "edges": 2097152,
    "coefficient": 1000,
    "offset": 1099511627776,
    "metadata": 1048576,
}
PARAMS = bytes.fromhex("010100020000000200020001000000040300020000002000")


def _descriptor(params):
    return descriptor_digest(b"quip/two-commodity-routing", 1, params_digest(params))


def _take(data, at, size):
    if at + size > len(data):
        raise CodecError("length")
    return data[at : at + size]


def _word(data, at):
    return int.from_bytes(_take(data, at, 4), "little")


def _expanded(cells, arcs):
    size = 18 + 6 * cells + 24 * arcs
    if size > LIMITS["metadata"] or arcs * 3 > LIMITS["nodes"]:
        raise CodecError("bounds_exceeded")
    return size


def _structure(n, cells, arcs):
    if 3 * len(arcs) != n:
        raise CodecError("invalid_instance")
    used = set()
    for arc in arcs:
        if any(v >= len(cells) for v in arc[:2]):
            raise CodecError("invalid_instance")
        for v in arc[2:5]:
            if v >= n or v in used:
                raise CodecError("invalid_instance")
            used.add(v)


def _metadata(instance):
    data = instance["metadata"]
    if _take(data, 0, 4) != b"\x01\x00\x07\x00" or _word(data, 4) != len(data) - 8:
        raise CodecError("invalid_instance")
    count = _word(data, 8)
    _expanded(count, 0)
    end = 12 + 6 * count
    raw = _take(data, 12, 6 * count)
    arcs = _word(data, end)
    size = _expanded(count, arcs)
    if len(data) != size:
        raise CodecError("length")
    cells = list(struct.iter_unpack("<HHbb", raw))
    if any(s not in (-1, 0, 1) for cell in cells for s in cell[2:]):
        raise CodecError("invalid_instance")
    records = list(struct.iter_unpack("<5I2H", data[end + 4 : -2]))
    _structure(len(instance["node_ids"]), cells, records)
    return cells, records, data[-2:]


def _pack(bits):
    out = bytearray((len(bits) + 7) // 8)
    for i, bit in enumerate(bits):
        out[i // 8] |= bit << (i % 8)
    return bytes(out)


def encode(instance: dict, spins: list[int]) -> bytes:
    _, arcs, _ = _metadata(instance)
    if len(spins) != len(instance["node_ids"]):
        raise CodecError("length")
    if any(s not in (-1, 1) for s in spins):
        raise CodecError("spin_value")
    if any(spins[a[3]] != spins[a[4]] for a in arcs):
        return _pack([1] + [int(s == 1) for s in spins])
    covered = {v for a in arcs for v in a[3:5]}
    return _pack(
        [0]
        + [int(spins[a[3]] == 1) for a in arcs]
        + [int(s == 1) for i, s in enumerate(spins) if i not in covered]
    )


def decode(instance: dict, data: bytes) -> list[int]:
    _, arcs, _ = _metadata(instance)
    if not data:
        raise CodecError("length")
    fallback = data[0] & 1
    n = len(instance["node_ids"])
    check_bit_array(data, 1 + (n if fallback else 2 * len(arcs)))

    def spin(i):
        return 1 if data[i // 8] & (1 << (i % 8)) else -1

    if fallback:
        spins = [spin(i + 1) for i in range(n)]
        if all(spins[a[3]] == spins[a[4]] for a in arcs):
            raise CodecError("non_canonical")
        return spins
    spins = [0] * n
    for i, a in enumerate(arcs):
        spins[a[3]] = spins[a[4]] = spin(i + 1)
    at = len(arcs) + 1
    for i in range(n):
        if spins[i] == 0:
            spins[i] = spin(at)
            at += 1
    return spins


def encode_instance(params: bytes, instance: dict) -> bytes:
    prefix = encode_prefix(instance)
    if prefix[:32] != _descriptor(params):
        raise CodecError("descriptor_mismatch")
    preflight_prefix(prefix, LIMITS)
    try:
        cells, arcs, strength = _metadata(instance)
    except CodecError as error:
        if error.name == "bounds_exceeded":
            raise
        raise CodecError("unencodable_instance") from error
    coords = encode_column([v for cell in cells for v in cell[:2]])
    supplies = _pack(
        [bit for cell in cells for s in cell[2:] for bit in ((s + 1) & 1, (s + 1) >> 1)]
    )
    return (
        prefix
        + struct.pack("<I", len(cells))
        + coords
        + supplies
        + struct.pack("<I", len(arcs))
        + encode_column([v for a in arcs for v in a[:5]])
        + encode_column([v for a in arcs for v in a[5:]])
        + strength
    )


def decode_instance(params: bytes, data: bytes) -> dict:
    prefix = preflight_prefix(data, LIMITS)
    at = prefix.metadata_start
    cells = _word(data, at)
    _expanded(cells, 0)
    at += 4
    coordinates, length = read_column(memoryview(data)[at:], cells * 2)
    at += length
    supply = _take(data, at, (cells * 4 + 7) // 8)
    check_bit_array(supply, cells * 4)
    for i in range(cells * 2):
        if (supply[i // 4] >> (2 * (i % 4))) & 3 == 3:
            raise CodecError("invalid_symbol")
    at += len(supply)
    arcs = _word(data, at)
    _expanded(cells, arcs)
    at += 4
    positions, length = read_column(memoryview(data)[at:], arcs * 5)
    at += length
    costs, length = read_column(memoryview(data)[at:], arcs * 2)
    at += length
    strength = int.from_bytes(_take(data, at, 2), "little")
    if at + 2 != len(data):
        raise CodecError("length")
    if prefix.descriptor_digest != _descriptor(params):
        raise CodecError("descriptor_mismatch")
    if 3 * arcs != prefix.node_count:
        raise CodecError("invalid_instance")
    if any(coordinates.get(i) > 65535 for i in range(2 * cells)) or any(
        costs.get(i) > 65535 for i in range(2 * arcs)
    ):
        raise CodecError("invalid_instance")
    cell_records = [
        (coordinates.get(2 * i), coordinates.get(2 * i + 1)) for i in range(cells)
    ]
    supplies = [
        [
            ((supply[(2 * i + k) // 4] >> (2 * ((2 * i + k) % 4))) & 3) - 1
            for k in range(2)
        ]
        for i in range(cells)
    ]
    records = [
        tuple(positions.get(5 * i + k) for k in range(5))
        + tuple(costs.get(2 * i + k) for k in range(2))
        for i in range(arcs)
    ]
    _structure(prefix.node_count, cell_records, records)
    arc_records = [
        {"tail": a[0], "head": a[1], "x1": a[2], "chain": a[3:5], "cost": a[5:]}
        for a in records
    ]
    metadata = make_metadata(cell_records, supplies, arc_records, strength)
    instance = decode_prefix(prefix, metadata, LIMITS, (7,))
    for column in (coordinates, positions, costs):
        column.minimal()
    return instance


def cases() -> list[dict]:
    from .ice import lattice
    from .instance import canonical_bytes
    from .routing import generate

    nodes, edges = lattice(2, 4)
    code, instance = generate(
        b"quip/two-commodity-routing", 1, PARAMS, nodes, edges, bytes(32)
    )
    if code or instance is None:
        raise ValueError("routing golden construction failed")
    instances = [("generated", instance)]
    wider: dict = {
        "descriptor_digest": _descriptor(PARAMS),
        "topology_order_digest": bytes(range(32)),
        "nonce": bytes(32),
        "node_ids": [9, 3, 7],
        "edges": [(0, 2)],
        "h": [-999, 0, 999],
        "j": [-1],
        "offset_milli": -123,
        "metadata": make_metadata(
            [(65535, 0), (0, 2)],
            [[-1, 1], [0, 0]],
            [{"tail": 0, "head": 1, "x1": 2, "chain": [0, 1], "cost": [0, 65535]}],
            65535,
        ),
    }
    wider["canonical"] = canonical_bytes(**wider)
    instances.append(("wider", wider))
    result = []
    for name, instance in instances:
        _, arcs, _ = _metadata(instance)
        n = len(instance["node_ids"])
        witnesses: list[dict] = []
        for label, spins in [
            ("negative", [-1] * n),
            ("positive", [1] * n),
            ("broken", [-1] * n),
        ]:
            if label == "broken":
                spins[arcs[0][3]] = 1
            payload = encode(instance, spins)
            witnesses.append(
                {
                    "name": label,
                    "spins": spins,
                    "compact": payload.hex(),
                    "bits": 1 + (n if payload[0] & 1 else 2 * len(arcs)),
                }
            )
        compact = encode_instance(PARAMS, instance)
        bad_witness = [
            ("empty", b""),
            ("appended", bytes.fromhex(witnesses[0]["compact"]) + b"\0"),
            ("redundant", _pack([1] + [1] * n)),
        ]
        bad_instance = [
            ("truncated", compact[:-1]),
            ("appended", compact + b"\0"),
            ("descriptor", bytes([compact[0] ^ 1]) + compact[1:]),
        ]

        prefix = preflight_prefix(compact, LIMITS)
        start = prefix.metadata_start
        cells = _word(compact, start)
        _, length = read_column(compact[start + 4 :], cells * 2)
        supply_at = start + 4 + length
        arc_at = supply_at + (cells * 4 + 7) // 8
        position_at = arc_at + 4
        positions, position_len = read_column(compact[position_at:], len(arcs) * 5)

        def replace(at, value, compact=compact):
            return compact[:at] + value + compact[at + len(value) :]

        bad_instance.extend(
            [
                ("excessive_nodes", replace(96, b"\xff" * 4)),
                ("excessive_edges", replace(100, b"\xff" * 4)),
                ("excessive_cells", replace(start, b"\xff" * 4)),
                ("excessive_arcs", replace(arc_at, b"\xff" * 4)),
                ("width_33", replace(position_at, b"\x21")),
                ("invalid_supply", replace(supply_at, bytes([compact[supply_at] | 3]))),
            ]
        )
        values = [positions.get(i) for i in range(5 * len(arcs))]
        values[2] = values[3]
        bad_instance.append(
            (
                "duplicate_spin",
                compact[:position_at]
                + encode_column(values)
                + compact[position_at + position_len :],
            )
        )
        width = positions.width + 1
        nonminimal = bytes([width]) + _pack(
            [
                bit
                for i in range(5 * len(arcs))
                for bit in ((positions.get(i) >> k) & 1 for k in range(width))
            ]
        )
        alternate = (
            compact[:position_at] + nonminimal + compact[position_at + position_len :]
        )
        bad_instance.append(("nonminimal_positions", alternate))
        bad_instance.append(
            ("descriptor_before_minimality", bytes([alternate[0] ^ 1]) + alternate[1:])
        )
        if positions.count * positions.width % 8:
            last = position_at + position_len - 1
            bad_instance.append(
                ("position_padding", replace(last, bytes([compact[last] | 128])))
            )
        at = 112
        for count in (n, 2 * prefix.edge_count):
            _, length = read_column(compact[at:], count)
            at += length
        bad_instance.append(("excessive_palette", replace(at, b"\xff" * 4)))
        count = _word(compact, at)
        if count < n:
            ranks, rank_length = read_column(compact[at + 4 + count * 4 :], n)
            palette = list(struct.unpack_from(f"<{count}i", compact, at + 4))
            extra = next(v for v in range(-1000, 1001) if v not in palette)
            changed = sorted(palette + [extra])
            palette_bytes = struct.pack(f"<I{len(changed)}i", len(changed), *changed)
            palette_bytes += encode_column(
                [changed.index(palette[ranks.get(i)]) for i in range(n)]
            )
            bad_instance.append(
                (
                    "unused_palette",
                    compact[:at]
                    + palette_bytes
                    + compact[at + 4 + count * 4 + rank_length :],
                )
            )

        def failures(rows, decoder):
            output = []
            for label, payload in rows:
                try:
                    decoder(payload)
                except CodecError as error:
                    output.append(
                        {"name": label, "compact": payload.hex(), "error": error.name}
                    )
                else:
                    raise ValueError("negative golden accepted")
            return output

        result.append(
            {
                "name": name,
                "params": PARAMS.hex(),
                "canonical": instance["canonical"].hex(),
                "compact": compact.hex(),
                "witnesses": witnesses,
                "invalid_witnesses": failures(
                    bad_witness, lambda b, instance=instance: decode(instance, b)
                ),
                "invalid_instances": failures(
                    bad_instance, lambda b: decode_instance(PARAMS, b)
                ),
            }
        )
    return result
