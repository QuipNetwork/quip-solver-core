"""Independent polariton codec contract checks."""

import random

import pytest
from family_oracle import codec_polariton as codec
from family_oracle import polariton
from family_oracle.build import _polariton_params
from family_oracle.codec_common import CodecError
from family_oracle.ice import lattice


@pytest.fixture
def case():
    params = _polariton_params(2, 1, 2, 0)
    nodes, edges = lattice(2, 1)
    code, instance = polariton.generate(
        b"quip/polariton-static", 1, params, nodes, edges, bytes(32)
    )
    assert code == 0
    return params, instance


def test_roundtrip(case):
    params, instance = case
    compact = codec.encode_instance(params, instance)
    decoded = codec.decode_instance(params, compact)
    assert decoded["canonical"] == instance["canonical"]
    assert codec.encode_instance(params, decoded) == compact
    count = 0
    for bits in range(4096):
        spins = [1 if bits >> i & 1 else -1 for i in range(12)]
        if polariton.observables(instance, spins)["violations"] == 0:
            count += 1
            assert codec.decode(instance, codec.encode(instance, spins)) == spins
    assert count == 1536


def test_properties(case):
    params, instance = case
    compact = codec.encode_instance(params, instance)
    for end in range(len(compact)):
        with pytest.raises(CodecError):
            codec.decode_instance(params, compact[:end])
    rng = random.Random(2311)
    for _ in range(1024):
        data = rng.randbytes(2)
        try:
            spins = codec.decode(instance, data)
        except CodecError:
            pass
        else:
            assert codec.encode(instance, spins) == data
        mutated = bytearray(compact)
        mutated[rng.randrange(len(mutated))] ^= 1 << rng.randrange(8)
        try:
            decoded = codec.decode_instance(params, bytes(mutated))
        except CodecError:
            pass
        else:
            assert codec.encode_instance(params, decoded) == mutated


def test_goldens():
    for row in codec.cases():
        params, data = (
            bytes.fromhex(row["params"]),
            bytes.fromhex(row["compact_instance"]),
        )
        instance = codec.decode_instance(params, data)
        assert instance["canonical"].hex() == row["canonical"]
        assert codec.encode(instance, row["spins"]).hex() == row["witness"]
        assert codec.decode(instance, bytes.fromhex(row["witness"])) == row["spins"]
        for negative in row["negative_instance"]:
            with pytest.raises(CodecError, match=negative["error"]):
                codec.decode_instance(params, bytes.fromhex(negative["bytes"]))


def test_rejections(case):
    params, instance = case
    compact = codec.encode_instance(params, instance)
    prefix = codec.preflight_prefix(compact, codec.LIMITS)
    at = prefix.metadata_start
    mutations = []
    for offset in (96, 100):
        data = bytearray(compact)
        data[offset : offset + 4] = bytes([255]) * 4
        mutations.append((bytes(data), "bounds_exceeded"))
    data = bytearray(compact)
    data[112] = 33
    mutations.append((bytes(data), "invalid_symbol"))
    data = bytearray(compact)
    data[at + 4] = 3
    mutations.append((bytes(data), "invalid_symbol"))
    data = bytearray(compact)
    data[at + 4] |= 128
    mutations.append((bytes(data), "padding"))
    data = bytearray(compact)
    data[at + 5] = 33
    mutations.append((bytes(data), "invalid_symbol"))
    for data, error in mutations:
        with pytest.raises(CodecError, match=error):
            codec.decode_instance(params, data)
    good, bad = polariton.witnesses(instance)
    for spins, error in (
        (bad, "infeasible"),
        ([0] * 12, "spin_value"),
        (good[:-1], "length"),
    ):
        with pytest.raises(CodecError, match=error):
            codec.encode(instance, spins)
    data = bytearray(codec.encode(instance, good))
    data[-1] |= 128
    with pytest.raises(CodecError, match="padding"):
        codec.decode(instance, bytes(data))
    wider = dict(instance, h=[13] * 12, j=[-17], edges=[(0, 1)], offset_milli=-123)
    wider.pop("canonical")
    wider.update(
        descriptor_digest=instance["canonical"][1:33],
        topology_order_digest=bytes([9]) * 32,
        nonce=bytes(32),
    )
    data = codec.encode_instance(params, wider)
    assert codec.encode_instance(params, codec.decode_instance(params, data)) == data
    metadata = bytearray(instance["metadata"])
    metadata[10] = 0
    with pytest.raises(CodecError, match="unencodable_instance"):
        codec.encode_instance(params, dict(instance, metadata=bytes(metadata)))


def test_palette_and_column_canonicality(case):
    import struct

    params, instance = case
    data = codec.encode_instance(params, instance)
    prefix = codec.preflight_prefix(data, codec.LIMITS)
    _, used_ids = codec.read_column(data[112:], 12)
    endpoints = 112 + used_ids
    _, used_edges = codec.read_column(data[endpoints:], len(instance["edges"]) * 2)
    palette = endpoints + used_edges
    excessive = data[:palette] + struct.pack("<I", 13) + data[palette + 4 :]
    with pytest.raises(CodecError, match="bounds_exceeded"):
        codec.decode_instance(params, excessive)
    # The original h palette contains only zero. Add an unused coefficient.
    unused = (
        data[:palette] + struct.pack("<Iii", 2, 0, 1) + b"\x00" + data[palette + 9 :]
    )
    with pytest.raises(CodecError, match="non_canonical"):
        codec.decode_instance(params, unused)
    at = prefix.metadata_start + 5
    positions, used = codec.read_column(data[at:], 12)
    width = positions.width + 1
    packed = bytearray((12 * width + 7) // 8)
    for i in range(12):
        value = positions.get(i)
        for bit in range(width):
            index = i * width + bit
            packed[index // 8] |= (value >> bit & 1) << (index % 8)
    nonminimal = data[:at] + bytes([width]) + packed + data[at + used :]
    with pytest.raises(CodecError, match="non_canonical"):
        codec.decode_instance(params, nonminimal)
    bad_descriptor = bytes([nonminimal[0] ^ 1]) + nonminimal[1:]
    with pytest.raises(CodecError, match="descriptor_mismatch"):
        codec.decode_instance(params, bad_descriptor)
