"""Independent fields instance and witness codec checks."""

import random
import struct

import pytest

from family_oracle.build import _three_k4
from family_oracle.codec_common import CodecError, encode_column, preflight_prefix
from family_oracle.fields import generate
from family_oracle.glass import metadata
from family_oracle.instance import canonical_bytes


from family_oracle import codec_fields as codec
from family_oracle.build import _fields_params
from family_oracle.digests import descriptor_digest, params_digest


def fixture():
    params = _fields_params(0, 0, 125, 500, 16)
    return params, {
        "descriptor_digest": descriptor_digest(
            b"quip/ice-fields", 1, params_digest(params)
        ),
        "topology_order_digest": bytes([9]) * 32,
        "nonce": bytes([7]) * 32,
        "node_ids": [42],
        "edges": [],
        "h": [-321],
        "j": [],
        "offset_milli": 0,
        "metadata": bytes(
            [2, 0, 1, 0, 4, 0, 0, 0, 0, 0, 0, 0, 2, 0, 5, 0, 0, 0, 1, 0, 0, 0, 1]
        ),
    }


def test_instance_roundtrip():
    params, instance = fixture()
    data = codec.encode_instance(params, instance)
    assert codec.encode_instance(params, codec.decode_instance(params, data)) == data


def assert_roundtrip(params, instance):
    data = codec.encode_instance(params, instance)
    decoded = codec.decode_instance(params, data)
    original = instance.get("canonical")
    if original is None:
        original = canonical_bytes(**instance)
    assert decoded["canonical"] == original
    assert codec.encode_instance(params, decoded) == data
    return data


def test_fields_generated_instance_roundtrips():
    nodes, edges = _three_k4()
    successes = skips = 0
    for seed in range(1024):
        params = _fields_params(
            0,
            0,
            [1, 125, 999][seed % 3],
            [1, 500, 1000][seed % 3],
            [9, 16, 25, 65535][seed % 4],
        )
        code, instance = generate(
            b"quip/ice-fields", 1, params, nodes, edges, seed.to_bytes(32, "little")
        )
        if instance is None:
            assert code == 14
            skips += 1
        else:
            successes += 1
            assert_roundtrip(params, instance)
    assert successes > 512
    assert successes + skips == 1024
    print(f"fields generated successes={successes}, construction skips={skips}")


def test_fields_shared_gauge_roundtrips():
    for row in codec.cases():
        params = bytes.fromhex(row["params_hex"])
        instance = codec.decode_instance(params, bytes.fromhex(row["compact_hex"]))
        for mask in range(1 << len(instance["node_ids"])):
            spins = [
                1 if mask & (1 << i) else -1 for i in range(len(instance["node_ids"]))
            ]
            assert codec.decode(instance, codec.encode(instance, spins)) == spins


def test_fields_do_not_change_eligibility():
    params, instance = fixture()
    instance.update(
        node_ids=list(range(7)),
        h=[500, 0, 0, 0, 0, 0, 0],
        metadata=metadata([(0, 1, 2, 3), (0, 4, 5, 6)], [-1, 1, -1, 1, 1, -1, 1]),
    )
    aligned = [-1, 1, 1, -1, 1, 1, 1]
    opposed = [-s for s in aligned]
    for spins in (aligned, opposed):
        assert codec.encode(instance, spins)[0] & 1 == 0
    instance["h"] = [-1000, 1, 999, -321, 0, 0, 0]
    assert_roundtrip(params, instance)
    for spins in (aligned, opposed):
        assert codec.encode(instance, spins)[0] & 1 == 0
    assert codec.encode(instance, [-1, 1, -1, 1, 1, -1, 1])[0] & 1 == 1


def test_fields_decoder_properties():
    row = codec.cases()[0]
    instance = codec.decode_instance(
        bytes.fromhex(row["params_hex"]), bytes.fromhex(row["compact_hex"])
    )
    rng = random.Random(2311)
    for k in range(1024):
        spins = [rng.choice([-1, 1]) for _ in instance["node_ids"]]
        data = codec.encode(instance, spins)
        assert codec.decode(instance, data) == spins
        for cut in range(len(data)):
            with pytest.raises(CodecError):
                codec.decode(instance, data[:cut])
        random_bytes = rng.randbytes(k % 5)
        try:
            decoded = codec.decode(instance, random_bytes)
        except CodecError:
            pass
        else:
            assert codec.encode(instance, decoded) == random_bytes
    with pytest.raises(CodecError, match="spin_value"):
        codec.encode(instance, [0] * len(instance["node_ids"]))
    with pytest.raises(CodecError, match="length"):
        codec.encode(instance, [])


def test_fields_instance_decoder_properties():
    row = codec.cases()[0]
    params = bytes.fromhex(row["params_hex"])
    data = bytes.fromhex(row["compact_hex"])
    for cut in range(len(data)):
        with pytest.raises(CodecError):
            codec.decode_instance(params, data[:cut])
    rng = random.Random(1123)
    for k in range(1024):
        mutated = bytearray(data)
        mutated[k % len(data)] ^= rng.randrange(256)
        for candidate in (bytes(mutated), rng.randbytes(k % 300)):
            try:
                instance = codec.decode_instance(params, candidate)
            except CodecError:
                pass
            else:
                assert codec.encode_instance(params, instance) == candidate


def test_instance_malformed_contract():
    params, instance = fixture()
    data = codec.encode_instance(params, instance)
    start = preflight_prefix(data, codec.LIMITS).metadata_start
    bad = []
    for at in (96, 100, start):
        changed = bytearray(data)
        struct.pack_into("<I", changed, at, 0xFFFFFFFF)
        bad.append((bytes(changed), "bounds_exceeded"))
    bad.extend(
        [
            (data[:112] + b"\x21" + data[113:], "invalid_symbol"),
            (data[: start + 4] + b"\x01" + data[start + 5 :], "non_canonical"),
            (data[:-1] + b"\x81", "padding"),
            (data + b"\0", "length"),
            (bytes([data[0] ^ 1]) + data[1:], "descriptor_mismatch"),
        ]
    )
    # One node, zero edges: h palette starts after C(ids) and empty C(endpoints).
    h_at = 115
    bad.append(
        (data[:h_at] + struct.pack("<I", 2) + data[h_at + 4 :], "bounds_exceeded")
    )
    # Two h values with only palette rank zero: the second palette entry is unused.
    header = bytearray(data[:112])
    struct.pack_into("<I", header, 96, 2)
    unused = (
        bytes(header)
        + encode_column([0, 1])
        + b"\0"
        + struct.pack("<Iii", 2, -321, 0)
        + b"\0"
        + struct.pack("<I", 0)
        + b"\0"
        + struct.pack("<I", 0)
        + b"\0\0"
    )
    bad.append((unused, "non_canonical"))
    header = bytearray(data[:112])
    struct.pack_into("<I", header, 100, 1)
    bad_edge = bytes(header) + bytes([6, 42, 0])
    for coefficient in (-321, 123):
        bad_edge += struct.pack("<IiB", 1, coefficient, 0)
    bad_edge += bytes([0, 0, 0, 0, 1, 1])
    bad.append((bad_edge, "invalid_instance"))

    for candidate, error in bad:
        with pytest.raises(CodecError, match=f"^{error}$"):
            codec.decode_instance(params, candidate)
    for region in (b"\0\0", instance["metadata"][:-1] + b"\x02"):
        with pytest.raises(CodecError, match="unencodable_instance"):
            codec.encode_instance(params, instance | {"metadata": region})


def test_independent_vectors():
    rows = codec.cases()
    assert rows
    for row in rows:
        params = bytes.fromhex(row["params_hex"])
        data = bytes.fromhex(row["compact_hex"])
        instance = codec.decode_instance(params, data)
        assert instance["canonical"].hex() == row["canonical_hex"]
        for w in row["witnesses"]:
            assert codec.encode(instance, w["spins"]).hex() == w["compact_hex"]
        for key, decoder in (
            ("malformed_instances", lambda d: codec.decode_instance(params, d)),
            ("malformed_witnesses", lambda d: codec.decode(instance, d)),
        ):
            for bad in row[key]:
                with pytest.raises(CodecError, match=f"^{bad['error']}$"):
                    decoder(bytes.fromhex(bad["compact_hex"]))


def test_existing_fixtures():
    import json
    from pathlib import Path

    path = (
        Path(__file__).resolve().parents[1]
        / "crates/quip-solver-conformance/vectors/families/ice_fields.json"
    )
    count = 0
    for case in json.loads(path.read_text())["cases"]:
        params = bytes.fromhex(case["params_hex"])
        for row in case["instances"]:
            instance = {key: row[key] for key in ("node_ids", "edges", "h", "j")}
            instance.update(
                offset_milli=int(row["offset_milli"]),
                metadata=bytes.fromhex(row["metadata_hex"]),
                canonical=bytes.fromhex(row["canonical_hex"]),
            )
            assert_roundtrip(params, instance)
            count += 1
    assert count > 5
