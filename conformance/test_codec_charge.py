"""Independent charged ice wire checks."""

import random
import struct

import pytest
from family_oracle import charge
from family_oracle import codec_charge as codec
from family_oracle.build import _charge_params
from family_oracle.codec_common import CodecError, preflight_prefix


def fixture():
    params = _charge_params(0, 0, 250, 31, 128)
    instance = charge.generate(
        b"quip/charged-ice",
        1,
        params,
        list(range(4)),
        [(a, b) for a in range(4) for b in range(a + 1, 4)],
        bytes(32),
    )
    return params, instance


def test_instance_roundtrip():
    params, instance = fixture()
    encoded = codec.encode_instance(params, instance)
    decoded = codec.decode_instance(params, encoded)
    assert decoded["canonical"] == instance["canonical"]
    assert codec.encode_instance(params, decoded) == encoded


def test_witness_roundtrip():
    _, instance = fixture()
    for mask in range(16):
        spins = [1 if mask & (1 << i) else -1 for i in range(4)]
        assert codec.decode(instance, codec.encode(instance, spins)) == spins


def test_generated_and_decoder_properties():
    rng = random.Random(2311)
    for seed in range(1024):
        params = _charge_params(0, 0, 1 + seed % 250, 1 + seed % 999, seed % 257)
        instance = charge.generate(
            b"quip/charged-ice",
            1,
            params,
            list(range(4)),
            [(a, b) for a in range(4) for b in range(a + 1, 4)],
            seed.to_bytes(32, "little"),
        )
        compact = codec.encode_instance(params, instance)
        decoded = codec.decode_instance(params, compact)
        assert decoded["canonical"] == instance["canonical"]
        assert decoded["digest"] == instance["digest"]
        assert codec.encode_instance(params, decoded) == compact
        spins = [rng.choice((-1, 1)) for _ in range(4)]
        assert codec.decode(instance, codec.encode(instance, spins)) == spins
        for data in (
            rng.randbytes(rng.randrange(250)),
            compact[: rng.randrange(len(compact))],
            compact + b"\0",
        ):
            try:
                decoded = codec.decode_instance(params, data)
            except CodecError:
                pass
            else:
                assert codec.encode_instance(params, decoded) == data
        data = rng.randbytes(rng.randrange(4))
        try:
            spins = codec.decode(instance, data)
        except CodecError:
            pass
        else:
            assert codec.encode(instance, spins) == data


def test_malformed_and_golden():
    params, instance = fixture()
    compact = codec.encode_instance(params, instance)
    for end in range(len(compact)):
        with pytest.raises(CodecError):
            codec.decode_instance(params, compact[:end])
    f = preflight_prefix(compact, codec.LIMITS).metadata_start
    for at, replacement, error in [
        (96, struct.pack("<I", 131073), "bounds_exceeded"),
        (100, struct.pack("<I", 2097153), "bounds_exceeded"),
        (f, struct.pack("<I", 2**32 - 1), "bounds_exceeded"),
        (112, b"\x21", "invalid_symbol"),
        (0, bytes([compact[0] ^ 1]), "descriptor_mismatch"),
        (f + 4, b"\x21", "invalid_symbol"),
        (len(compact) - 1, b"\x03", "invalid_symbol"),
        (len(compact) - 1, b"\x80", "padding"),
    ]:
        bad = compact[:at] + replacement + compact[at + len(replacement) :]
        with pytest.raises(CodecError, match=error):
            codec.decode_instance(params, bad)
    for row in codec.cases():
        inst = codec.decode_instance(
            bytes.fromhex(row["params"]), bytes.fromhex(row["compact"])
        )
        assert inst["canonical"].hex() == row["canonical"]
        for w in row["witnesses"]:
            assert codec.encode(inst, w["spins"]).hex() == w["compact"]
        for w in row["bad_witnesses"]:
            with pytest.raises(CodecError, match=w["error"]):
                codec.decode(inst, bytes.fromhex(w["compact"]))
        for bad in row["malformed"]:
            with pytest.raises(CodecError, match=bad["error"]):
                codec.decode_instance(
                    bytes.fromhex(row["params"]), bytes.fromhex(bad["compact"])
                )


def test_overlap_and_rank_tables():
    for q, masks in codec.MASKS.items():
        for sign in (-1, 1):
            inst = {
                "node_ids": list(range(7)),
                "metadata": charge.metadata(
                    [(0, 1, 2, 3), (3, 4, 5, 6)], [[sign] * 4, [1] * 4], [q, 0]
                ),
            }
            for mask in range(128):
                spins = [1 if mask & (1 << i) else -1 for i in range(7)]
                data = codec.encode(inst, spins)
                assert codec.decode(inst, data) == spins
                local = sum((s * sign == 1) << i for i, s in enumerate(spins[:4]))
                assert data[0] & 1 == int(local not in masks)
                if local in masks:
                    with pytest.raises(CodecError, match="non_canonical"):
                        codec.decode(
                            inst,
                            codec._pack([(1, 1)] + [(int(s == 1), 1) for s in spins]),
                        )


def test_wider_image_and_invalid_metadata():
    params, original = fixture()
    instance = codec.decode_instance(params, codec.encode_instance(params, original))
    instance["h"] = [17, -19, 23, -29]
    instance["offset_milli"] = -123
    # Explicit head fields take precedence over the old diagnostic canonical bytes.
    data = codec.encode_instance(params, instance)
    assert codec.decode_instance(params, data)["h"] == instance["h"]
    instance["metadata"] = instance["metadata"][:-1] + b"\x04"
    with pytest.raises(CodecError, match="unencodable_instance"):
        codec.encode_instance(params, instance)


def test_columns_palettes_and_error_priority():
    from family_oracle.codec_common import encode_column, encode_palette

    params, original = fixture()
    compact = codec.encode_instance(params, original)
    prefix = preflight_prefix(compact, codec.LIMITS)
    f = compact[prefix.metadata_start :]
    ids = encode_column(original["node_ids"])
    endpoints = encode_column([p for e in original["edges"] for p in e])
    h = encode_palette(original["h"])
    j = encode_palette(original["j"])
    head = compact[:112]
    unused = struct.pack("<Iii", 2, 0, 1) + b"\x00"
    bad_palettes = [
        (struct.pack("<I", 5), "bounds_exceeded"),
        (unused, "non_canonical"),
        (struct.pack("<Iii", 2, 0, 0) + b"\x00", "non_canonical"),
        (struct.pack("<Ii", 1, 0) + b"\x01\x0f", "invalid_symbol"),
    ]
    for palette, error in bad_palettes:
        with pytest.raises(CodecError, match=error):
            codec.decode_instance(params, head + ids + endpoints + palette + j + f)
    # Three bits can hold these two-bit IDs, but the wider column is nonminimal.
    wide = b"\x03" + codec._pack([(i, 3) for i in original["node_ids"]])
    with pytest.raises(CodecError, match="non_canonical"):
        codec.decode_instance(params, head + wide + endpoints + h + j + f)
    wide_f = f[:4] + b"\x03" + codec._pack([(i, 3) for i in range(4)]) + f[-2:]
    with pytest.raises(CodecError, match="non_canonical"):
        codec.decode_instance(params, head + ids + endpoints + h + j + wide_f)
    # Edge structure must precede metadata-column minimality.
    bad_edges = encode_column([0, 0] + [p for e in original["edges"][1:] for p in e])
    with pytest.raises(CodecError, match="invalid_instance"):
        codec.decode_instance(params, head + ids + bad_edges + h + j + wide_f)
    instance = codec.decode_instance(params, compact)
    instance["metadata"] = charge.metadata([], [], [])
    empty = codec.encode_instance(params, instance)
    assert codec.decode_instance(params, empty)["metadata"] == instance["metadata"]
    with pytest.raises(CodecError, match="non_canonical"):
        codec.decode(instance, b"\x01")


def test_existing_family_fixtures():
    import json
    from pathlib import Path

    path = (
        Path(__file__).resolve().parents[1]
        / "crates/quip-solver-conformance/vectors/families/ice_charge.json"
    )
    document = json.loads(path.read_text())
    for row in document["cases"]:
        params = bytes.fromhex(row["params_hex"])
        for item in row["instances"]:
            instance = {
                "node_ids": [int(v) for v in item["node_ids"]],
                "edges": [tuple(map(int, e)) for e in item["edges"]],
                "h": [int(v) for v in item["h"]],
                "j": [int(v) for v in item["j"]],
                "offset_milli": int(item["offset_milli"]),
                "metadata": bytes.fromhex(item["metadata_hex"]),
                "canonical": bytes.fromhex(item["canonical_hex"]),
            }
            data = codec.encode_instance(params, instance)
            decoded = codec.decode_instance(params, data)
            assert decoded["canonical"] == instance["canonical"]
            assert decoded["digest"].hex() == item["instance_digest"]
            assert codec.encode_instance(params, decoded) == data
