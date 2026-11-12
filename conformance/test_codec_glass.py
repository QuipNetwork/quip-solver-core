"""Independent glass codec checks."""

import random
import struct
from itertools import combinations

import pytest

from family_oracle.codec_common import CodecError, preflight_prefix
from family_oracle.glass import metadata


from family_oracle import codec_glass as codec, glass
from family_oracle.build import _ice_glass_params


def test_roundtrip():
    params = _ice_glass_params(0, 0, 125)
    instance = glass.generate(
        b"quip/ice-glass",
        1,
        params,
        [0, 1, 2, 3],
        [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)],
        bytes(32),
    )
    packed = codec.encode_instance(params, instance)
    assert codec.decode_instance(params, packed)["canonical"] == instance["canonical"]
    for mask in range(16):
        spins = [1 if mask & (1 << i) else -1 for i in range(4)]
        assert codec.decode(instance, codec.encode(instance, spins)) == spins


def sample(n=7):
    params = _ice_glass_params(0, 0, 125)
    instance = glass.generate(
        b"quip/ice-glass",
        1,
        params,
        list(range(n)),
        list(combinations(range(4), 2)),
        bytes(32),
    )
    return params, instance


def test_seeded_instance_roundtrips():
    params, _ = sample()
    for seed in range(1024):
        n = 4 + seed % 5
        instance = glass.generate(
            b"quip/ice-glass",
            1,
            params,
            list(reversed(range(n))),
            list(combinations(range(4), 2)),
            seed.to_bytes(32, "little"),
        )
        compact = codec.encode_instance(params, instance)
        decoded = codec.decode_instance(params, compact)
        assert decoded["canonical"] == instance["canonical"]
        assert decoded["digest"] == instance["digest"]
        assert codec.encode_instance(params, decoded) == compact


def test_instance_malformed_and_arbitrary():
    params, instance = sample()
    valid = codec.encode_instance(params, instance)
    for end in range(len(valid)):
        with pytest.raises(CodecError):
            codec.decode_instance(params, valid[:end])
    rng = random.Random(2311)
    corpus = [rng.randbytes(rng.randrange(300)) for _ in range(1024)]
    for _ in range(1024):
        mutated = bytearray(valid)
        at = rng.randrange(len(mutated))
        mutated[at] ^= 1 << rng.randrange(8)
        corpus.append(bytes(mutated))
    for data in corpus:
        try:
            decoded = codec.decode_instance(params, data)
        except CodecError:
            continue
        assert codec.encode_instance(params, decoded) == data
    prefix = preflight_prefix(valid, codec.LIMITS)
    at = prefix.metadata_start
    faults = [
        (valid + b"\0", "length"),
        (valid[:96] + struct.pack("<I", 131073) + valid[100:], "bounds_exceeded"),
        (valid[:100] + struct.pack("<I", 2097153) + valid[104:], "bounds_exceeded"),
        (valid[:at] + b"\xff" * 4 + valid[at + 4 :], "bounds_exceeded"),
        (valid[: at + 4] + b"\x21" + valid[at + 5 :], "invalid_symbol"),
        (bytes([valid[0] ^ 1]) + valid[1:], "descriptor_mismatch"),
        (valid[:-1] + bytes([valid[-1] | 128]), "padding"),
    ]
    for data, error in faults:
        with pytest.raises(CodecError) as caught:
            codec.decode_instance(params, data)
        assert caught.value.name == error


def test_witness_properties_and_overlap():
    _, instance = sample()
    instance["metadata"] = metadata(
        [(0, 1, 2, 3), (2, 3, 4, 5)], [1, -1, 1, -1, 1, -1, 1]
    )
    for mask in range(128):
        spins = [1 if mask & (1 << i) else -1 for i in range(7)]
        assert codec.decode(instance, codec.encode(instance, spins)) == spins
    for byte in range(256):
        data = bytes([byte])
        try:
            spins = codec.decode(instance, data)
        except CodecError:
            continue
        assert codec.encode(instance, spins) == data
    rng = random.Random(2311)
    for _ in range(1024):
        data = rng.randbytes(rng.randrange(4))
        try:
            spins = codec.decode(instance, data)
        except CodecError:
            continue
        assert codec.encode(instance, spins) == data
    for data, error in [
        (b"\x0c", "invalid_symbol"),
        (b"\x0e", "invalid_symbol"),
        (b"\x80", "padding"),
    ]:
        with pytest.raises(CodecError) as caught:
            codec.decode(instance, data)
        assert caught.value.name == error
    spins = codec.decode(instance, b"\0")
    forced = bytes([1 + sum((s == 1) << (i + 1) for i, s in enumerate(spins))])
    with pytest.raises(CodecError, match="non_canonical"):
        codec.decode(instance, forced)
    with pytest.raises(CodecError, match="spin_value"):
        codec.encode(instance, [0] * 7)


def test_wider_image_and_schema_rejection():
    params, instance = sample()
    instance["h"][0] = 37
    instance["j"][0] = -71
    instance["metadata"] = metadata([(3, 1, 0, 2), (0, 2, 4, 6)], [1] * 7)
    compact = codec.encode_instance(params, instance)
    decoded = codec.decode_instance(params, compact)
    assert decoded["h"] == instance["h"]
    assert decoded["metadata"] == instance["metadata"]
    instance["metadata"] = instance["metadata"][:-1] + b"\x02"
    with pytest.raises(CodecError, match="unencodable_instance"):
        codec.encode_instance(params, instance)


def test_golden_cases():
    from family_oracle.codec_vectors import document
    import json
    from pathlib import Path

    path = (
        Path(__file__).resolve().parents[1]
        / "crates/quip-solver-conformance/vectors/families/codecs/glass.json"
    )
    assert json.loads(path.read_text()) == document("glass")


def test_empty_metadata_image_and_width_minimality():
    from family_oracle.digests import descriptor_digest, params_digest

    params, instance = sample()
    for n in (0, 1, 8):
        instance = dict(
            descriptor_digest=descriptor_digest(
                b"quip/ice-glass", 1, params_digest(params)
            ),
            topology_order_digest=bytes(32),
            nonce=bytes(32),
            node_ids=list(range(n)),
            edges=[],
            h=[0] * n,
            j=[],
            offset_milli=0,
            metadata=metadata([], [1] * n),
        )
        compact = codec.encode_instance(params, instance)
        assert (
            codec.encode_instance(params, codec.decode_instance(params, compact))
            == compact
        )
        assert codec.decode(instance, codec.encode(instance, [1] * n)) == [1] * n
        forced = (2 ** (n + 1) - 1).to_bytes((n + 8) // 8, "little")
        with pytest.raises(CodecError, match="non_canonical"):
            codec.decode(instance, forced)
        at = preflight_prefix(compact, codec.LIMITS).metadata_start
        changed = compact[: at + 4] + b"\x01" + compact[at + 5 :]
        with pytest.raises(CodecError, match="non_canonical"):
            codec.decode_instance(params, changed)
