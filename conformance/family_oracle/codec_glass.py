"""Independent compact glass instance and witness codecs."""

import struct
from itertools import combinations

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
from .glass import metadata

LIMITS = dict(nodes=131072, edges=2097152, coefficient=1000, offset=0, metadata=8388608)
RANKS = (3, 5, 6, 9, 10, 12)


def _schema(instance: dict) -> tuple[list[tuple[int, ...]], list[int]]:
    data = instance["metadata"]
    n = len(instance["node_ids"])
    if len(data) < 22 or data[:4] != b"\x02\0\x01\0":
        raise CodecError("unencodable_instance")
    size, count = struct.unpack_from("<II", data, 4)
    at = 12 + 16 * count
    if size != 4 + 16 * count or len(data) != at + 10 + n:
        raise CodecError("unencodable_instance")
    if struct.unpack_from("<HII", data, at) != (2, 4 + n, n):
        raise CodecError("unencodable_instance")
    tiles = list(struct.iter_unpack("<4I", data[12:at]))
    gauge = list(data[at + 10 :])
    if any(p >= n for tile in tiles for p in tile) or any(g > 1 for g in gauge):
        raise CodecError("unencodable_instance")
    return tiles, gauge


def _selected(instance: dict) -> tuple[list[tuple[int, ...]], list[int], list[int]]:
    tiles, gauge = _schema(instance)
    used = set()
    selected = []
    for tile in tiles:
        if len(set(tile)) != 4:
            raise CodecError("invalid_instance")
        if not used.intersection(tile):
            selected.append(tile)
            used.update(tile)
    return selected, gauge, [i for i in range(len(gauge)) if i not in used]


def encode(instance: dict, spins: list[int]) -> bytes:
    tiles, gauge, uncovered = _selected(instance)
    if len(spins) != len(gauge):
        raise CodecError("length")
    if any(s not in (-1, 1) for s in spins):
        raise CodecError("spin_value")
    masks = [
        sum((spins[p] * (-1 if gauge[p] else 1) == 1) << i for i, p in enumerate(tile))
        for tile in tiles
    ]
    if all(mask in RANKS for mask in masks):
        fields = [(0, 1)] + [(RANKS.index(mask), 3) for mask in masks]
        fields += [(int(spins[p] == 1), 1) for p in uncovered]
    else:
        fields = [(1, 1)] + [(int(s == 1), 1) for s in spins]
    value = 0
    bits = 0
    for field, width in fields:
        value |= field << bits
        bits += width
    return value.to_bytes((bits + 7) // 8, "little")


def decode(instance: dict, data: bytes) -> list[int]:
    tiles, gauge, uncovered = _selected(instance)
    if not data:
        raise CodecError("length")
    fallback = data[0] & 1
    bits = 1 + len(gauge) - (0 if fallback else len(tiles))
    check_bit_array(data, bits)
    value = int.from_bytes(data, "little") >> 1
    if fallback:
        spins = [1 if value & (1 << i) else -1 for i in range(len(gauge))]
    else:
        spins = [-1] * len(gauge)
        for tile in tiles:
            rank = value & 7
            value >>= 3
            if rank >= len(RANKS):
                raise CodecError("invalid_symbol")
            for i, p in enumerate(tile):
                spins[p] = (1 if RANKS[rank] & (1 << i) else -1) * (
                    -1 if gauge[p] else 1
                )
        for p in uncovered:
            spins[p] = 1 if value & 1 else -1
            value >>= 1
    if encode(instance, spins) != data:
        raise CodecError("non_canonical")
    return spins


def encode_instance(params: bytes, instance: dict) -> bytes:
    tiles, gauge = _schema(instance)
    prefix = encode_prefix(instance)
    packed_gauge = sum(g << i for i, g in enumerate(gauge)).to_bytes(
        (len(gauge) + 7) // 8, "little"
    )
    data = (
        prefix
        + struct.pack("<I", len(tiles))
        + encode_column([p for tile in tiles for p in tile])
        + packed_gauge
    )
    decode_instance(params, data)
    return data


def decode_instance(params: bytes, data: bytes) -> dict:
    prefix = preflight_prefix(data, LIMITS)
    at = prefix.metadata_start
    if len(data) < at + 4:
        raise CodecError("length")
    count = struct.unpack_from("<I", data, at)[0]
    n = prefix.node_count
    if 22 + 16 * count + n > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    column, size = read_column(memoryview(data)[at + 4 :], 4 * count)
    gauge = memoryview(data)[at + 4 + size :]
    check_bit_array(gauge, n)
    if prefix.descriptor_digest != descriptor_digest(
        b"quip/ice-glass", 1, params_digest(params)
    ):
        raise CodecError("descriptor_mismatch")
    if any(column.get(i) >= n for i in range(4 * count)):
        raise CodecError("invalid_instance")
    tiles = [tuple(column.get(4 * t + i) for i in range(4)) for t in range(count)]
    signs = [-1 if gauge[i // 8] & (1 << (i % 8)) else 1 for i in range(n)]
    instance = decode_prefix(prefix, metadata(tiles, signs), LIMITS, (1, 2))
    column.minimal()
    return instance


def cases() -> list[dict]:
    from .build import _ice_glass_cases, _ice_glass_params
    from .glass import generate

    fixtures: list[tuple[str, bytes, dict]] = []
    for case in _ice_glass_cases():
        for row in case["instances"]:
            fixtures.append(
                (
                    case["name"] + "-" + row["index"],
                    bytes.fromhex(case["params_hex"]),
                    {
                        **row,
                        "metadata": bytes.fromhex(row["metadata_hex"]),
                        "canonical": bytes.fromhex(row["canonical_hex"]),
                        "offset_milli": int(row["offset_milli"]),
                    },
                )
            )
    params = _ice_glass_params(0, 0, 125)
    small = generate(
        b"quip/ice-glass",
        1,
        params,
        [9, 4, 7, 2, 6],
        list(combinations([9, 4, 7, 2], 2)),
        bytes(32),
    )
    fixtures.append(("small-uncovered", params, small))
    result = []
    for name, params, instance in fixtures:
        tiles, gauge, _ = _selected(instance)
        structured = [1] * len(gauge)
        for tile in tiles:
            for i, p in enumerate(tile):
                structured[p] = (1 if i < 2 else -1) * (-1 if gauge[p] else 1)
        spins_rows = [structured, [1] * len(gauge), [-1 if g else 1 for g in gauge]]
        compact = encode_instance(params, instance)
        malformed = [("truncated", compact[:-1]), ("appended", compact + b"\0")]
        prefix = preflight_prefix(compact, LIMITS)
        at = prefix.metadata_start
        malformed += [
            ("tile-count-cap", compact[:at] + b"\xff" * 4 + compact[at + 4 :]),
            ("width-33", compact[: at + 4] + b"\x21" + compact[at + 5 :]),
            ("descriptor", bytes([compact[0] ^ 1]) + compact[1:]),
        ]
        column, consumed = read_column(
            memoryview(compact)[at + 4 :], 4 * len(_schema(instance)[0])
        )
        if column.count:
            width = column.width + 1
            bits = sum(column.get(i) << (i * width) for i in range(column.count))
            wider = bytes([width]) + bits.to_bytes(
                (column.count * width + 7) // 8, "little"
            )
            malformed.append(
                (
                    "nonminimal-tile-column",
                    compact[: at + 4] + wider + compact[at + 4 + consumed :],
                )
            )
        palette_at = 112
        _, consumed = read_column(memoryview(compact)[palette_at:], len(gauge))
        palette_at += consumed
        _, consumed = read_column(
            memoryview(compact)[palette_at:], 2 * len(instance["edges"])
        )
        palette_at += consumed
        malformed += [
            (
                "palette-count-cap",
                compact[:palette_at]
                + struct.pack("<I", len(gauge) + 1)
                + compact[palette_at + 4 :],
            ),
            (
                "unused-field-palette",
                compact[:palette_at]
                + struct.pack("<Iii", 2, 0, 1)
                + compact[palette_at + 8 :],
            ),
        ]
        negatives = []
        for label, payload in malformed:
            try:
                decode_instance(params, payload)
            except CodecError as error:
                negatives.append(dict(name=label, hex=payload.hex(), error=error.name))
        witness_bad = []
        witness_payloads = [b"", encode(instance, structured) + b"\0"]
        if tiles:
            rank_bad = bytearray(encode(instance, structured))
            rank_bad[0] = (rank_bad[0] & 0xF1) | 12
            witness_payloads.append(bytes(rank_bad))
            fallback = 1 + sum(
                (spin == 1) << (i + 1) for i, spin in enumerate(structured)
            )
            witness_payloads.append(fallback.to_bytes((len(gauge) + 8) // 8, "little"))
        for payload in witness_payloads:
            try:
                decode(instance, payload)
            except CodecError as error:
                witness_bad.append(dict(hex=payload.hex(), error=error.name))
        result.append(
            dict(
                name=name,
                params_hex=params.hex(),
                canonical_hex=instance["canonical"].hex(),
                compact_hex=compact.hex(),
                witnesses=[
                    dict(spins=s, hex=encode(instance, s).hex()) for s in spins_rows
                ],
                invalid_instances=negatives,
                invalid_witnesses=witness_bad,
                nodes=len(gauge),
                selected_tiles=len(tiles),
            )
        )
    return result
