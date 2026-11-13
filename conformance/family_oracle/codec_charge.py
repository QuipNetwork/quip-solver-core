"""Independent charged ice compact witness and instance codecs."""

import itertools
import struct

from . import charge
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
    "offset": 1073741824,
    "metadata": 8388608,
}
MASKS = {0: [3, 5, 6, 9, 10, 12], 2: [7, 11, 13, 14], -2: [1, 2, 4, 8]}


def _schema(instance):
    data = instance["metadata"]
    if len(data) < 2 or data[:2] != b"\x03\x00":
        raise CodecError("unencodable_instance")
    at, payloads = 2, []
    for kind in (1, 2, 6):
        if at + 6 > len(data):
            raise CodecError("unencodable_instance")
        actual, size = struct.unpack_from("<HI", data, at)
        at += 6
        if actual != kind or at + size > len(data) or size < 4:
            raise CodecError("unencodable_instance")
        payloads.append(data[at : at + size])
        at += size
    t = int.from_bytes(payloads[0][:4], "little")
    if at != len(data) or any(
        int.from_bytes(p[:4], "little") != t or len(p) != 4 + t * w
        for p, w in zip(payloads, (16, 4, 1), strict=True)
    ):
        raise CodecError("unencodable_instance")
    tiles = list(struct.iter_unpack("<4I", payloads[0][4:]))
    gauges = list(payloads[1][4:])
    charges = [v[0] for v in struct.iter_unpack("<b", payloads[2][4:])]
    if any(g > 1 for g in gauges) or any(q not in MASKS for q in charges):
        raise CodecError("unencodable_instance")
    _tiles(tiles, len(instance["node_ids"]), "unencodable_instance")
    return tiles, gauges, charges


def _tiles(tiles, n, error):
    previous = None
    for tile in tiles:
        if (
            any(a >= b for a, b in itertools.pairwise(tile))
            or tile[-1] >= n
            or (previous is not None and previous >= tile)
        ):
            raise CodecError(error)
        previous = tile


def _blocks(instance):
    tiles, gauges, charges = _schema(instance)
    used, blocks = set(), []
    for i, (tile, q) in enumerate(zip(tiles, charges, strict=True)):
        if used.intersection(tile):
            continue
        used.update(tile)
        blocks.append((tile, [1 - 2 * g for g in gauges[4 * i : 4 * i + 4]], q))
    return blocks, [i for i in range(len(instance["node_ids"])) if i not in used]


def _pack(values):
    bits = []
    for value, width in values:
        bits.extend((value >> i) & 1 for i in range(width))
    data = bytearray((len(bits) + 7) // 8)
    for i, bit in enumerate(bits):
        data[i // 8] |= bit << (i % 8)
    return bytes(data)


def encode(instance: dict, spins: list[int]) -> bytes:
    if len(spins) != len(instance["node_ids"]):
        raise CodecError("length")
    if any(s not in (-1, 1) for s in spins):
        raise CodecError("spin_value")
    blocks, uncovered = _blocks(instance)
    ranks = []
    for tile, gauge, q in blocks:
        mask = sum(
            (spins[p] * g == 1) << i
            for i, (p, g) in enumerate(zip(tile, gauge, strict=True))
        )
        if mask not in MASKS[q]:
            return _pack([(1, 1)] + [(int(s == 1), 1) for s in spins])
        ranks.append((MASKS[q].index(mask), 3 if q == 0 else 2))
    return _pack([(0, 1)] + ranks + [(int(spins[p] == 1), 1) for p in uncovered])


def decode(instance: dict, data: bytes) -> list[int]:
    blocks, uncovered = _blocks(instance)
    if not data:
        raise CodecError("length")
    n = len(instance["node_ids"])
    mode = data[0] & 1
    bits = (
        1 + n
        if mode
        else 1 + sum(3 if q == 0 else 2 for _, _, q in blocks) + len(uncovered)
    )
    check_bit_array(data, bits)
    at = 1

    def read(width):
        nonlocal at
        value = sum(
            ((data[(at + i) // 8] >> ((at + i) % 8)) & 1) << i for i in range(width)
        )
        at += width
        return value

    spins = [-1] * n
    if mode:
        spins = [2 * read(1) - 1 for _ in range(n)]
    else:
        for tile, gauge, q in blocks:
            rank = read(3 if q == 0 else 2)
            if rank >= len(MASKS[q]):
                raise CodecError("invalid_symbol")
            for i, (p, g) in enumerate(zip(tile, gauge, strict=True)):
                spins[p] = g * (2 * ((MASKS[q][rank] >> i) & 1) - 1)
        for p in uncovered:
            spins[p] = 2 * read(1) - 1
    if encode(instance, spins) != data:
        raise CodecError("non_canonical")
    return spins


def _descriptor(params):
    return descriptor_digest(b"quip/charged-ice", 1, params_digest(params))


def encode_instance(params: bytes, instance: dict) -> bytes:
    prefix = encode_prefix(instance)
    if prefix[:32] != _descriptor(params):
        raise CodecError("descriptor_mismatch")
    if (
        len(instance["node_ids"]) > LIMITS["nodes"]
        or len(instance["edges"]) > LIMITS["edges"]
        or len(instance["metadata"]) > LIMITS["metadata"]
        or abs(instance["offset_milli"]) > LIMITS["offset"]
        or any(abs(v) > 1000 for v in instance["h"] + instance["j"])
    ):
        raise CodecError("bounds_exceeded")
    tiles, gauges, charges = _schema(instance)
    return (
        prefix
        + struct.pack("<I", len(tiles))
        + encode_column([p for t in tiles for p in t])
        + _pack([(g, 1) for g in gauges])
        + _pack([({0: 0, 2: 1, -2: 2}[q], 2) for q in charges])
    )


def decode_instance(params: bytes, data: bytes) -> dict:
    prefix = preflight_prefix(data, LIMITS)
    at = prefix.metadata_start
    if at + 4 > len(data):
        raise CodecError("length")
    t = int.from_bytes(data[at : at + 4], "little")
    if 32 + 21 * t > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    positions, size = read_column(memoryview(data)[at + 4 :], 4 * t)
    at += 4 + size
    end = at + (4 * t + 7) // 8
    if end > len(data):
        raise CodecError("length")
    gauges = memoryview(data)[at:end]
    check_bit_array(gauges, 4 * t)
    ranks = memoryview(data)[end:]
    check_bit_array(ranks, 2 * t)
    for i in range(t):
        if (ranks[i // 4] >> (2 * (i % 4))) & 3 == 3:
            raise CodecError("invalid_symbol")
    if prefix.descriptor_digest != _descriptor(params):
        raise CodecError("descriptor_mismatch")
    _tiles(
        (tuple(positions.get(4 * i + j) for j in range(4)) for i in range(t)),
        prefix.node_count,
        "invalid_instance",
    )
    metadata = charge.metadata(
        [tuple(positions.get(4 * i + j) for j in range(4)) for i in range(t)],
        [
            [
                1 - 2 * ((gauges[(4 * i + j) // 8] >> ((4 * i + j) % 8)) & 1)
                for j in range(4)
            ]
            for i in range(t)
        ],
        [(0, 2, -2)[(ranks[i // 4] >> (2 * (i % 4))) & 3] for i in range(t)],
    )
    instance = decode_prefix(prefix, metadata, LIMITS, (1, 2, 6))
    positions.minimal()
    return instance


def cases() -> list[dict]:
    from .build import _charge_params

    rows = []
    for chance in (0, 128, 256):
        params = _charge_params(0, 0, 250, 31, chance)
        for seed, n in enumerate((4, 7, 16)):
            tiles = (
                [(0, 1, 2, 3), (3, 4, 5, 6)]
                if n == 7
                else [tuple(range(i, i + 4)) for i in range(0, n, 4)]
            )
            edges = sorted(
                {(a, b) for tile in tiles for a in tile for b in tile if a < b}
            )
            inst = charge.generate(
                b"quip/charged-ice",
                1,
                params,
                list(range(n)),
                edges,
                bytes([seed]) * 32,
            )
            compact = encode_instance(params, inst)
            witnesses = []
            for mask in range(16):
                spins = [1 if mask & (1 << i) else -1 for i in range(n)]
                witnesses.append({"spins": spins, "compact": encode(inst, spins).hex()})
            spins = [-1] * n
            blocks, _ = _blocks(inst)
            for tile, gauge, q in blocks:
                for i, (p, g) in enumerate(zip(tile, gauge, strict=True)):
                    spins[p] = g * (2 * ((MASKS[q][0] >> i) & 1) - 1)
            witnesses.append(
                {"spins": spins.copy(), "compact": encode(inst, spins).hex()}
            )
            spins[0] *= -1
            witnesses.append({"spins": spins, "compact": encode(inst, spins).hex()})
            malformed = []
            for payload in (b"", compact[:-1], compact + b"\0", compact[:-1] + b"\x03"):
                try:
                    decode_instance(params, payload)
                except CodecError as error:
                    malformed.append({"compact": payload.hex(), "error": error.name})
            bad_witnesses = []
            for payload in (b"", b"\x80", b"\x00\x00", b"\x0e", b"\x0f"):
                try:
                    decode(inst, payload)
                except CodecError as error:
                    bad_witnesses.append(
                        {"compact": payload.hex(), "error": error.name}
                    )
            rows.append(
                {
                    "name": f"charge-{chance}-{seed}",
                    "params": params.hex(),
                    "canonical": inst["canonical"].hex(),
                    "compact": compact.hex(),
                    "witnesses": witnesses,
                    "malformed": malformed,
                    "bad_witnesses": bad_witnesses,
                }
            )
    return rows
