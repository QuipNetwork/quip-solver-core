"""Defects compact codec contracts."""

import pytest
from family_oracle import defects
from family_oracle.build import _defects_instances, _defects_rows
from family_oracle.codec_common import CodecError
from family_oracle.codec_defects import decode, decode_instance, encode, encode_instance


def fixtures():
    for _, params, count, (nodes, edges), _ in _defects_rows():
        for _, _, _, instance in _defects_instances(params, nodes, edges, count):
            if instance is not None:
                yield params, instance


def test_instance_roundtrips():
    for params, instance in fixtures():
        data = encode_instance(params, instance)
        decoded = decode_instance(params, data)
        assert decoded["canonical"] == instance["canonical"]
        assert decoded["digest"] == instance["digest"]
        assert encode_instance(params, decoded) == data
        for end in range(len(data)):
            with pytest.raises(CodecError):
                decode_instance(params, data[:end])
        with pytest.raises(CodecError, match="length"):
            decode_instance(params, data + b"\0")


def test_witness_roundtrips():
    for _, instance in fixtures():
        spins = defects.node_spins(instance, instance["orientation"])
        data = encode(instance, spins)
        assert decode(instance, data) == spins
        with pytest.raises(CodecError, match="spin_value"):
            encode(instance, [0] * len(spins))
        with pytest.raises(CodecError, match="length"):
            decode(instance, data + b"\0")


def test_negative_vectors():
    from family_oracle.codec_defects import cases

    for row in cases():
        params = bytes.fromhex(row["params"])
        instance = decode_instance(params, bytes.fromhex(row["compact"]))
        # The witness oracle validates against original generated diagnostic data.
        original = next(
            i
            for p, i in fixtures()
            if p == params and i["canonical"] == instance["canonical"]
        )
        for bad in row["invalid_instances"]:
            with pytest.raises(CodecError) as caught:
                decode_instance(params, bytes.fromhex(bad["data"]))
            assert caught.value.name == bad["error"]
        for bad in row["invalid_witnesses"]:
            with pytest.raises(CodecError) as caught:
                decode(original, bytes.fromhex(bad["data"]))
            assert caught.value.name == bad["error"]


def test_arbitrary_bytes_are_canonical():
    import random

    rng = random.Random(2311)
    params, instance = next(fixtures())
    valid = encode_instance(params, instance)
    for i in range(1024):
        data = bytearray(valid) if i % 2 else bytearray(rng.randbytes(i % 500))
        if data:
            data[rng.randrange(len(data))] ^= rng.randrange(256)
        try:
            decoded = decode_instance(params, bytes(data))
        except CodecError:
            pass
        else:
            assert encode_instance(params, decoded) == data
        witness = rng.randbytes(i % 5)
        try:
            spins = decode(instance, witness)
        except CodecError:
            pass
        else:
            assert encode(instance, spins) == witness
            assert defects.feasible(instance, spins)


def test_structural_metadata_preserves_order_and_opaque_fields():
    from family_oracle.codec_defects import _sections
    from family_oracle.instance import canonical_bytes

    params, original = next(fixtures())
    changed = dict(original)
    changed.pop("canonical")
    changed["h"] = list(changed["h"])
    changed["h"][0] += 1
    charges, incidence, rest = _sections(original["metadata"])
    fixed = bytes(range(48))
    # The first fixture has zero obstacles and two pins. Preserve reversed pins.
    rest = fixed + rest[48:52] + rest[56:60] + rest[52:56]
    changed["metadata"] = b"\x03\x00" + b"".join(
        __import__("struct").pack("<HI", kind, len(payload)) + payload
        for kind, payload in ((4, charges), (5, incidence), (8, rest))
    )
    # Extract head values through the prefix, independent of diagnostic fields.
    head = decode_instance(params, encode_instance(params, original))
    for key in ("descriptor_digest", "topology_order_digest", "nonce"):
        changed[key] = head[key]
    data = encode_instance(params, changed)
    decoded = decode_instance(params, data)
    expected = canonical_bytes(
        changed["descriptor_digest"],
        changed["topology_order_digest"],
        changed["nonce"],
        changed["node_ids"],
        changed["edges"],
        changed["h"],
        changed["j"],
        changed["offset_milli"],
        changed["metadata"],
    )
    assert decoded["canonical"] == expected
    assert encode_instance(params, decoded) == data
