"""Independent compact ice-fields codecs with stored-order tile selection."""

import itertools
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

LIMITS = {
    "nodes": 131072,
    "edges": 2097152,
    "coefficient": 1000,
    "offset": 0,
    "metadata": 8388608,
}
MASKS = (3, 5, 6, 9, 10, 12)


def _size(n, count):
    size = 22 + 16 * count + n
    if size > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    return size


def _tiles(count, n, get):
    previous = None
    for i in range(count):
        tile = tuple(get(4 * i + j) for j in range(4))
        if (
            any(p >= n for p in tile)
            or any(a >= b for a, b in itertools.pairwise(tile))
            or (previous is not None and previous >= tile)
        ):
            raise CodecError("invalid_instance")
        previous = tile


def _metadata(instance):
    data = instance["metadata"]
    n = len(instance["node_ids"])
    if len(data) < 22 or data[:4] != b"\x02\x00\x01\x00":
        raise CodecError("unencodable_instance")
    length, count = struct.unpack_from("<II", data, 4)
    _size(n, count)
    at = 12 + 16 * count
    if (
        length != 4 + 16 * count
        or len(data) != at + 10 + n
        or data[at : at + 2] != b"\x02\x00"
        or struct.unpack_from("<II", data, at + 2) != (4 + n, n)
    ):
        raise CodecError("unencodable_instance")
    gauge = data[at + 10 :]
    if any(g > 1 for g in gauge):
        raise CodecError("unencodable_instance")

    def get(i):
        return struct.unpack_from("<I", data, 12 + 4 * i)[0]

    try:
        _tiles(count, n, get)
    except CodecError as error:
        raise CodecError("unencodable_instance") from error
    return [tuple(get(4 * i + j) for j in range(4)) for i in range(count)], gauge


def _selected(instance):
    tiles, gauge = _metadata(instance)
    used = set()
    selected = []
    for tile in tiles:
        if used.isdisjoint(tile):
            selected.append(tile)
            used.update(tile)
    return selected, gauge, [p for p in range(len(gauge)) if p not in used]


def encode(instance: dict, spins: list[int]) -> bytes:
    tiles, gauge, uncovered = _selected(instance)
    if len(spins) != len(gauge):
        raise CodecError("length")
    if any(s not in (-1, 1) for s in spins):
        raise CodecError("spin_value")
    masks = [
        sum((spins[p] * (1 - 2 * gauge[p]) == 1) << j for j, p in enumerate(tile))
        for tile in tiles
    ]
    if all(mask in MASKS for mask in masks):
        bits = [0]
        for mask in masks:
            rank = MASKS.index(mask)
            bits.extend((rank >> j) & 1 for j in range(3))
        bits.extend(int(spins[p] == 1) for p in uncovered)
    else:
        bits = [1] + [int(s == 1) for s in spins]
    out = bytearray((len(bits) + 7) // 8)
    for i, bit in enumerate(bits):
        out[i // 8] |= bit << (i % 8)
    return bytes(out)


def decode(instance: dict, data: bytes) -> list[int]:
    tiles, gauge, uncovered = _selected(instance)
    if not data:
        raise CodecError("length")
    mode = data[0] & 1
    n = len(gauge)
    check_bit_array(data, 1 + n if mode else 1 + n - len(tiles))

    def bit(i):
        return (data[i // 8] >> (i % 8)) & 1

    if mode:
        spins = [2 * bit(i + 1) - 1 for i in range(n)]
        if encode(instance, spins) != data:
            raise CodecError("non_canonical")
        return spins
    spins = [-1] * n
    at = 1
    for tile in tiles:
        rank = sum(bit(at + j) << j for j in range(3))
        at += 3
        if rank >= len(MASKS):
            raise CodecError("invalid_symbol")
        for j, p in enumerate(tile):
            spins[p] = (1 - 2 * gauge[p]) * (2 * ((MASKS[rank] >> j) & 1) - 1)
    for p in uncovered:
        spins[p] = 2 * bit(at) - 1
        at += 1
    return spins


def encode_instance(params: bytes, instance: dict) -> bytes:
    tiles, gauge = _metadata(instance)
    data = encode_prefix(instance)
    packed = bytearray((len(gauge) + 7) // 8)
    for i, g in enumerate(gauge):
        packed[i // 8] |= g << (i % 8)
    data += (
        struct.pack("<I", len(tiles))
        + encode_column([p for tile in tiles for p in tile])
        + packed
    )
    # The independent decoder checks the common canonical model and contract bounds.
    try:
        decode_instance(params, data)
    except CodecError as error:
        if error.name == "invalid_instance":
            raise CodecError("unencodable_instance") from error
        raise
    return bytes(data)


def decode_instance(params: bytes, data: bytes) -> dict:
    prefix = preflight_prefix(data, LIMITS)
    n = prefix.node_count
    at = prefix.metadata_start
    if len(data) < at + 4:
        raise CodecError("length")
    count = struct.unpack_from("<I", data, at)[0]
    _size(n, count)
    positions, used = read_column(memoryview(data)[at + 4 :], 4 * count)
    gauge = memoryview(data)[at + 4 + used :]
    check_bit_array(gauge, n)
    if prefix.descriptor_digest != descriptor_digest(
        b"quip/ice-fields", 1, params_digest(params)
    ):
        raise CodecError("descriptor_mismatch")
    _tiles(count, n, positions.get)
    metadata = bytearray(struct.pack("<HHII", 2, 1, 4 + 16 * count, count))
    for i in range(4 * count):
        metadata += struct.pack("<I", positions.get(i))
    metadata += struct.pack("<HII", 2, 4 + n, n)
    metadata.extend((gauge[i // 8] >> (i % 8)) & 1 for i in range(n))
    instance = decode_prefix(prefix, bytes(metadata), LIMITS, (1, 2))
    positions.minimal()
    return instance


def cases() -> list[dict]:
    from .build import _fields_params, _three_k4
    from .fields import generate

    params = _fields_params(0, 0, 125, 500, 9)
    nodes, edges = _three_k4()
    rows = []
    for seed in range(8):
        _, instance = generate(
            b"quip/ice-fields", 1, params, nodes, edges, seed.to_bytes(32, "little")
        )
        if instance is None:
            continue
        compact = encode_instance(params, instance)
        tiles, gauge, _ = _selected(instance)
        spins = [1] * len(nodes)
        for tile in tiles:
            for j, p in enumerate(tile):
                spins[p] = (1 if j < 2 else -1) * (1 - 2 * gauge[p])
        witnesses = []
        for name, vector in [
            ("structured", spins),
            ("fallback", [1 - 2 * g for g in gauge]),
        ]:
            payload = encode(instance, vector)
            witnesses.append(
                {
                    "name": name,
                    "spins": vector,
                    "compact_hex": payload.hex(),
                    "bits": 1
                    + len(nodes)
                    - (len(tiles) if name == "structured" else 0),
                }
            )
        malformed = []
        for name, payload in [
            ("truncated", compact[:-1]),
            ("appended", compact + b"\0"),
            ("descriptor", bytes([compact[0] ^ 1]) + compact[1:]),
        ]:
            try:
                decode_instance(params, payload)
            except CodecError as error:
                malformed.append(
                    {"name": name, "compact_hex": payload.hex(), "error": error.name}
                )
        bad_witnesses = []
        valid = encode(instance, spins)
        redundant = bytearray((len(spins) + 8) // 8)
        redundant[0] = 1
        for i, spin in enumerate(spins, 1):
            redundant[i // 8] |= int(spin == 1) << (i % 8)
        for name, payload in [
            ("rank6", bytes([(valid[0] & 0xF1) | 12]) + valid[1:]),
            ("rank7", bytes([(valid[0] & 0xF1) | 14]) + valid[1:]),
            ("padding", valid[:-1] + bytes([valid[-1] | 128])),
            ("extra", valid + b"\0"),
            ("redundant-fallback", bytes(redundant)),
        ]:
            try:
                decode(instance, payload)
            except CodecError as error:
                bad_witnesses.append(
                    {"name": name, "compact_hex": payload.hex(), "error": error.name}
                )
        rows.append(
            {
                "name": f"shared-fields-{seed}",
                "params_hex": params.hex(),
                "canonical_hex": instance["canonical"].hex(),
                "compact_hex": compact.hex(),
                "witnesses": witnesses,
                "malformed_instances": malformed,
                "malformed_witnesses": bad_witnesses,
            }
        )
    return rows
