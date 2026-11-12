"""Independent compact instance and soft-block witness codecs for twisted ice."""

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
from .twisted import metadata

LIMITS = {
    "nodes": 131072,
    "edges": 2097152,
    "coefficient": 1000,
    "offset": 0,
    "metadata": 8388608,
}
MASKS = (3, 5, 6, 9, 10, 12)


def _u32(data, at=0):
    if at + 4 > len(data):
        raise CodecError("length")
    return struct.unpack_from("<I", data, at)[0]


def _size(tiles, pairs):
    size = 32 + 20 * tiles + 8 * pairs
    if size > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    return size


def _positions(count, arity, get, n):
    previous = None
    for i in range(count):
        row = tuple(get(arity * i + j) for j in range(arity))
        if (
            any(a >= b for a, b in zip(row, row[1:]))
            or row[-1] >= n
            or (previous is not None and previous >= row)
        ):
            raise CodecError("invalid_instance")
        previous = row


def _metadata(instance):
    data = instance["metadata"]
    if len(data) > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    if data[:2] != b"\x03\x00":
        raise CodecError("unencodable_instance")
    at, sections = 2, []
    for kind in (1, 2, 3):
        if at + 6 > len(data):
            raise CodecError("unencodable_instance")
        found, length = struct.unpack_from("<HI", data, at)
        at += 6
        if found != kind or length < 4 or at + length > len(data):
            raise CodecError("unencodable_instance")
        sections.append(data[at : at + length])
        at += length
    tiles, gauges, pairs = sections
    t, d = _u32(tiles), _u32(pairs)
    _size(t, d)
    if (
        at != len(data)
        or len(tiles) != 4 + 16 * t
        or _u32(gauges) != t
        or len(gauges) != 4 + 4 * t
        or len(pairs) != 4 + 8 * d
        or any(g > 1 for g in gauges[4:])
    ):
        raise CodecError("unencodable_instance")
    n = len(instance["node_ids"])
    try:
        _positions(t, 4, lambda i: _u32(tiles, 4 + 4 * i), n)
        _positions(d, 2, lambda i: _u32(pairs, 4 + 4 * i), n)
    except CodecError as err:
        raise CodecError("unencodable_instance") from err
    return (
        list(struct.iter_unpack("<4I", tiles[4:])),
        [
            [1 if g == 0 else -1 for g in gauges[i : i + 4]]
            for i in range(4, len(gauges), 4)
        ],
        list(struct.iter_unpack("<2I", pairs[4:])),
    )


def _blocks(instance):
    tiles, gauges, pairs = _metadata(instance)
    candidates: list[tuple[tuple[int, ...], list[int], tuple[int, ...], int]] = [
        (p, [1, 1], (0, 3), 1) for p in pairs
    ]
    candidates += [(t, g, MASKS, 3) for t, g in zip(tiles, gauges, strict=True)]
    used, selected = set(), []
    for block in candidates:
        positions = block[0]
        if used.isdisjoint(positions):
            selected.append(block)
            used.update(positions)
    return selected, [p for p in range(len(instance["node_ids"])) if p not in used]


def _ranks(blocks, spins):
    ranks = []
    for positions, gauges, masks, _ in blocks:
        mask = sum(
            (spins[p] * g == 1) << i
            for i, (p, g) in enumerate(zip(positions, gauges, strict=True))
        )
        if mask not in masks:
            return None
        ranks.append(masks.index(mask))
    return ranks


def encode(instance: dict, spins: list[int]) -> bytes:
    n = len(instance["node_ids"])
    if len(spins) != n:
        raise CodecError("length")
    if any(s not in (-1, 1) for s in spins):
        raise CodecError("spin_value")
    blocks, uncovered = _blocks(instance)
    ranks = _ranks(blocks, spins)
    value, bits = (1 if ranks is None else 0), 1
    if ranks is None:
        fields = [(int(s == 1), 1) for s in spins]
    else:
        fields = [(rank, block[3]) for rank, block in zip(ranks, blocks, strict=True)]
        fields += [(int(spins[p] == 1), 1) for p in uncovered]
    for field, width in fields:
        value |= field << bits
        bits += width
    return value.to_bytes((bits + 7) // 8, "little")


def decode(instance: dict, data: bytes) -> list[int]:
    n = len(instance["node_ids"])
    if not data or len(data) > (n + 8) // 8:
        raise CodecError("length")
    blocks, uncovered = _blocks(instance)
    fallback = data[0] & 1
    bits = 1 + (n if fallback else sum(b[3] for b in blocks) + len(uncovered))
    check_bit_array(data, bits)
    value = int.from_bytes(data, "little") >> 1
    if fallback:
        spins = [1 if value >> i & 1 else -1 for i in range(n)]
        if _ranks(blocks, spins) is not None:
            raise CodecError("non_canonical")
        return spins
    spins = [-1] * n
    for positions, gauges, masks, width in blocks:
        rank = value & ((1 << width) - 1)
        value >>= width
        if rank >= len(masks):
            raise CodecError("invalid_symbol")
        for i, (p, g) in enumerate(zip(positions, gauges, strict=True)):
            spins[p] = g * (1 if masks[rank] >> i & 1 else -1)
    for p in uncovered:
        spins[p] = 1 if value & 1 else -1
        value >>= 1
    return spins


def _descriptor(params):
    return descriptor_digest(b"quip/twisted-ice-dimer", 1, params_digest(params))


def encode_instance(params: bytes, instance: dict) -> bytes:
    prefix = encode_prefix(instance)
    if prefix[:32] != _descriptor(params):
        raise CodecError("descriptor_mismatch")
    tiles, gauges, pairs = _metadata(instance)
    gauge_bits = sum(
        (g == -1) << i for i, g in enumerate(g for row in gauges for g in row)
    )
    result = (
        prefix
        + struct.pack("<I", len(tiles))
        + encode_column([p for row in tiles for p in row])
        + gauge_bits.to_bytes((4 * len(tiles) + 7) // 8, "little")
        + struct.pack("<I", len(pairs))
        + encode_column([p for row in pairs for p in row])
    )
    try:
        decode_instance(params, result)
    except CodecError as err:
        if err.name == "invalid_instance":
            raise CodecError("unencodable_instance") from err
        raise
    return result


def decode_instance(params: bytes, data: bytes) -> dict:
    prefix = preflight_prefix(data, LIMITS)
    at = prefix.metadata_start
    t = _u32(data, at)
    _size(t, 0)
    at += 4
    tiles, length = read_column(memoryview(data)[at:], 4 * t)
    at += length
    gauge_len = (4 * t + 7) // 8
    gauges = memoryview(data)[at : at + gauge_len]
    check_bit_array(gauges, 4 * t)
    at += gauge_len
    d = _u32(data, at)
    _size(t, d)
    at += 4
    pairs, length = read_column(memoryview(data)[at:], 2 * d)
    if at + length != len(data):
        raise CodecError("length")
    if prefix.descriptor_digest != _descriptor(params):
        raise CodecError("descriptor_mismatch")
    _positions(t, 4, tiles.get, prefix.node_count)
    _positions(d, 2, pairs.get, prefix.node_count)
    # The common decoder checks edge structure before prefix minimality.
    region = metadata(
        [tuple(tiles.get(4 * i + j) for j in range(4)) for i in range(t)],
        [
            [
                -1 if gauges[(4 * i + j) // 8] >> ((4 * i + j) % 8) & 1 else 1
                for j in range(4)
            ]
            for i in range(t)
        ],
        [(pairs.get(2 * i), pairs.get(2 * i + 1)) for i in range(d)],
    )
    instance = decode_prefix(prefix, region, LIMITS, (1, 2, 3))
    tiles.minimal()
    pairs.minimal()
    return instance


def cases() -> list[dict]:
    from .build import _twisted_params
    from .ice import lattice
    from .twisted import generate

    rows = []
    for pair_count in (0, 1):
        params = _twisted_params(1, 1, 62, pair_count)
        nodes, edges = lattice(1, 1)
        for seed in range(2):
            code, instance = generate(
                b"quip/twisted-ice-dimer",
                1,
                params,
                nodes,
                edges,
                seed.to_bytes(32, "little"),
            )
            if code is not None:
                continue
            assert instance is not None
            compact = encode_instance(params, instance)
            blocks, uncovered = _blocks(instance)
            structured = [-1] * len(nodes)
            for positions, gauges, masks, _ in blocks:
                for i, (p, g) in enumerate(zip(positions, gauges, strict=True)):
                    structured[p] = g * (1 if masks[0] >> i & 1 else -1)
            witnesses: list[dict] = []
            for spins in (structured, [1] * len(nodes)):
                encoded = encode(instance, spins)
                bits = (
                    1 + len(nodes)
                    if encoded[0] & 1
                    else 1 + sum(b[3] for b in blocks) + len(uncovered)
                )
                witnesses.append(
                    {"spins": spins, "compact": encoded.hex(), "bits": bits}
                )
            bad = []
            for name, payload in (
                ("truncated", compact[:-1]),
                ("appended", compact + b"\0"),
                ("descriptor", bytes([compact[0] ^ 1]) + compact[1:]),
            ):
                try:
                    decode_instance(params, payload)
                except CodecError as err:
                    bad.append(
                        {"name": name, "compact": payload.hex(), "error": err.name}
                    )
            witness_bad = []
            for payload in (b"", bytes.fromhex(witnesses[0]["compact"]) + b"\0"):
                try:
                    decode(instance, payload)
                except CodecError as err:
                    witness_bad.append({"compact": payload.hex(), "error": err.name})
            rows.append(
                {
                    "name": f"pairs-{pair_count}-seed-{seed}",
                    "params": params.hex(),
                    "canonical": instance["canonical"].hex(),
                    "compact": compact.hex(),
                    "witnesses": witnesses,
                    "invalid_instances": bad,
                    "invalid_witnesses": witness_bad,
                }
            )
    return rows
