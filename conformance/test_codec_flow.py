"""Independent flow codec contract tests."""

import copy
import random
import struct

import pytest

from family_oracle import codec_flow as codec
from family_oracle import flow
from family_oracle.build import _flow_params
from family_oracle.codec_common import CodecError, preflight_prefix, read_column
from family_oracle.ice import lattice


@pytest.fixture(scope="module")
def fixtures():
    return list(codec.fixtures())


def test_golden_roundtrip(fixtures):
    rows = codec.cases()
    assert rows
    instances = {name: instance for name, _, instance in fixtures}
    for row in rows:
        compact = bytes.fromhex(row["compact"])
        if row["kind"] == "instance":
            params = bytes.fromhex(row["params"])
            decoded = codec.decode_instance(params, compact)
            assert decoded["canonical"].hex() == row["canonical"]
            assert decoded["digest"].hex() == row["digest"]
            assert codec.encode_instance(params, decoded) == compact
        elif row["kind"] == "witness":
            instance = instances[row["instance"]]
            assert codec.decode(instance, compact) == row["spins"]
            assert codec.encode(instance, row["spins"]) == compact
        else:
            with pytest.raises(CodecError) as error:
                if row["kind"] == "instance_error":
                    codec.decode_instance(bytes.fromhex(row["params"]), compact)
                else:
                    codec.decode(instances[row["instance"]], compact)
            assert error.value.name == row["error"]


def test_all_small_feasible_roundtrips(fixtures):
    instance = fixtures[0][2]
    count = 0
    for mask in range(4096):
        spins = [1 if mask & (1 << b) else -1 for b in range(12)]
        if flow.feasible(instance, spins):
            compact = codec.encode(instance, spins)
            assert codec.decode(instance, compact) == spins
            count += 1
        else:
            with pytest.raises(CodecError, match="infeasible"):
                codec.encode(instance, spins)
    assert count > 0


def test_instance_generated_roundtrips():
    nodes, edges = lattice(2, 4)
    params = _flow_params(2, 4, 2, 30)
    skips = 0
    for seed in range(1024):
        code, instance = flow.generate(
            flow.FAMILY_ID.encode(),
            1,
            params,
            nodes,
            edges,
            seed.to_bytes(32, "little"),
        )
        if code:
            skips += 1
            continue
        assert instance is not None
        compact = codec.encode_instance(params, instance)
        decoded = codec.decode_instance(params, compact)
        assert decoded["canonical"] == instance["canonical"]
        assert decoded["digest"] == instance["digest"]
        assert codec.encode_instance(params, decoded) == compact
    assert skips == 0


def test_malformed_properties(fixtures):
    _, params, instance = fixtures[0]
    compact = codec.encode_instance(params, instance)
    for length in range(len(compact)):
        with pytest.raises(CodecError):
            codec.decode_instance(params, compact[:length])
    rng = random.Random(2311)
    for _ in range(1024):
        arbitrary = rng.randbytes(rng.randrange(256))
        changed = bytearray(compact)
        changed[rng.randrange(len(changed))] ^= rng.randrange(1, 256)
        for payload in (arbitrary, bytes(changed)):
            try:
                decoded = codec.decode_instance(params, payload)
            except CodecError:
                continue
            assert codec.encode_instance(params, decoded) == payload
        payload = rng.randbytes(rng.randrange(5))
        try:
            spins = codec.decode(instance, payload)
        except CodecError:
            continue
        assert flow.feasible(instance, spins)
        assert codec.encode(instance, spins) == payload
    for spins, error in (([], "length"), ([0] * 12, "spin_value")):
        with pytest.raises(CodecError, match=error):
            codec.encode(instance, spins)


def test_structural_image_and_errors(fixtures):
    _, params, instance = fixtures[0]
    wider = copy.deepcopy(instance)
    wider["h"][0] += 7
    wider["metadata"] = bytearray(wider["metadata"])
    start = len(wider["metadata"]) - 8 - 5 * len(wider["h"])
    struct.pack_into("<Q", wider["metadata"], start, 2**64 - 1)
    struct.pack_into("<I", wider["metadata"], start + 9, 2**32 - 1)
    wider["metadata"] = bytes(wider["metadata"])
    compact = codec.encode_instance(params, wider)
    decoded = codec.decode_instance(params, compact)
    assert decoded["h"] == wider["h"]
    assert decoded["metadata"] == wider["metadata"]
    assert codec.encode_instance(params, decoded) == compact
    wider["metadata"] = wider["metadata"][:10] + b"\x01" + wider["metadata"][11:]
    with pytest.raises(CodecError, match="unencodable_instance"):
        codec.encode_instance(params, wider)
    compact = codec.encode_instance(params, instance)
    for at, value in ((96, 8321), (100, 24575)):
        changed = bytearray(compact)
        struct.pack_into("<I", changed, at, value)
        with pytest.raises(CodecError, match="bounds_exceeded"):
            codec.decode_instance(params, bytes(changed))
    changed = bytearray(compact)
    changed[112] = 33
    with pytest.raises(CodecError, match="invalid_symbol"):
        codec.decode_instance(params, bytes(changed))
    changed = bytearray(compact)
    changed[0] ^= 1
    with pytest.raises(CodecError, match="descriptor_mismatch"):
        codec.decode_instance(params, bytes(changed))
    prefix = preflight_prefix(compact, codec.LIMITS)
    boundary_start = prefix.metadata_start + 4
    changed = bytearray(compact)
    changed[boundary_start] |= 0x80
    with pytest.raises(CodecError, match="padding"):
        codec.decode_instance(params, bytes(changed))
    positions_start = boundary_start + 1
    _, used = read_column(compact[positions_start:], 12)
    target_start = positions_start + used + 8
    changed = bytearray(compact)
    changed[target_start + 1] |= 0x80
    with pytest.raises(CodecError, match="padding"):
        codec.decode_instance(params, bytes(changed))
    with pytest.raises(CodecError, match="length"):
        codec.decode_instance(params, compact + b"\x00")


def test_cycle_sizes_targets_and_padding(fixtures):
    for _, _, instance in fixtures:
        spins = [0] * len(instance["positions"])
        for edge, p in enumerate(instance["positions"]):
            spins[p] = instance["orientation"][edge]
        compact = codec.encode(instance, spins)
        assert len(compact) == (instance["side"] ** 2 + 7) // 8
        if instance["side"] == 2:
            with pytest.raises(CodecError, match="padding"):
                codec.decode(instance, bytes([compact[0] | 0x80]))
        if instance["params"]["cost_half_max"] == 0:
            changed = copy.deepcopy(instance)
            data = bytearray(changed["metadata"])
            start = len(data) - 5 * len(spins)
            for edge in range(len(spins)):
                data[start + 5 * edge] = 255 if edge % 2 else 1
            changed["metadata"] = bytes(data)
            assert codec.encode(changed, spins) == compact
            assert codec.decode(changed, compact) == spins
