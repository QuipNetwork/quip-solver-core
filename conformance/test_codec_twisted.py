"""Independent twisted codec conformance checks."""

import itertools
import random
import struct

import pytest
from family_oracle import codec_twisted as codec
from family_oracle.build import _twisted_params
from family_oracle.codec_common import CodecError, encode_prefix
from family_oracle.digests import descriptor_digest, params_digest
from family_oracle.ice import lattice
from family_oracle.twisted import generate, metadata


def test_instance_roundtrip():
    params = _twisted_params(1, 1, 62, 0)
    nodes, edges = lattice(1, 1)
    code, instance = generate(
        b"quip/twisted-ice-dimer", 1, params, nodes, edges, bytes(32)
    )
    assert code is None
    assert instance is not None
    compact = codec.encode_instance(params, instance)
    assert codec.decode_instance(params, compact)["canonical"] == instance["canonical"]
    spins = [1] * len(nodes)
    assert codec.decode(instance, codec.encode(instance, spins)) == spins


def fixture(n, tiles=(), gauges=(), pairs=()):
    params = _twisted_params(0, 0, 62, len(pairs))
    return params, {
        "descriptor_digest": descriptor_digest(
            b"quip/twisted-ice-dimer", 1, params_digest(params)
        ),
        "topology_order_digest": bytes(range(32)),
        "nonce": bytes(32),
        "node_ids": list(reversed(range(n))),
        "edges": [],
        "h": [0] * n,
        "j": [],
        "offset_milli": 0,
        "metadata": metadata(list(tiles), list(gauges), list(pairs)),
    }


def test_exact_bytes_and_canonical_modes():
    _, pair = fixture(3, pairs=[(0, 1)])
    assert codec.encode(pair, [1, 1, -1]) == b"\x02"
    assert codec.encode(pair, [1, -1, -1]) == b"\x03"
    _, tile = fixture(4, [(0, 1, 2, 3)], [[1] * 4])
    assert codec.encode(tile, [1, 1, -1, -1]) == b"\x00"
    assert codec.encode(tile, [1] * 4) == b"\x1f"
    for payload, error in [
        (b"\x07", "non_canonical"),
        (b"\x0c", "invalid_symbol"),
        (b"\x0e", "invalid_symbol"),
        (b"\x80", "padding"),
    ]:
        with pytest.raises(CodecError, match=error):
            codec.decode(tile, payload)


def test_overlap_priority_and_exhaustive_spins():
    fixtures = [
        fixture(0)[1],
        fixture(3)[1],
        fixture(6, [(0, 1, 2, 3), (2, 3, 4, 5)], [[1] * 4, [-1] * 4])[1],
        fixture(6, [(0, 1, 2, 3)], [[-1, 1, -1, 1]], [(0, 4)])[1],
    ]
    for inst in fixtures:
        for spins in itertools.product((-1, 1), repeat=len(inst["node_ids"])):
            spins = list(spins)
            assert codec.decode(inst, codec.encode(inst, spins)) == spins
    # The pair skips the tile, so even a tile violation uses the short form.
    assert codec.encode(fixtures[-1], [1] * 6)[0] & 1 == 0
    # The first tile wins and the second tile imposes no condition.
    assert codec.encode(fixtures[-2], [1, 1, -1, -1, 1, 1])[0] & 1 == 0


def test_instance_roundtrips_generated():
    nodes, edges = lattice(1, 1)
    skipped = 0
    for seed in range(1024):
        params = _twisted_params(1, 1, 1 + seed % 999, seed % 3)
        code, inst = generate(
            b"quip/twisted-ice-dimer",
            1,
            params,
            nodes,
            edges,
            seed.to_bytes(32, "little"),
        )
        if code is not None:
            skipped += 1
            continue
        assert inst is not None
        wire = codec.encode_instance(params, inst)
        decoded = codec.decode_instance(params, wire)
        assert decoded["canonical"] == inst["canonical"]
        assert decoded["digest"] == inst["digest"]
        assert codec.encode_instance(params, decoded) == wire
    assert skipped == 0


def test_instance_malformed_properties():
    params, inst = fixture(6, [(0, 1, 2, 3)], [[-1, 1, -1, 1]], [(0, 4)])
    wire = codec.encode_instance(params, inst)
    for end in range(len(wire)):
        with pytest.raises(CodecError):
            codec.decode_instance(params, wire[:end])
    rng = random.Random(2311)
    for _ in range(1024):
        arbitrary = rng.randbytes(rng.randrange(300))
        mutated = bytearray(wire)
        mutated[rng.randrange(len(wire))] ^= rng.randrange(1, 256)
        for payload in (arbitrary, bytes(mutated)):
            try:
                decoded = codec.decode_instance(params, payload)
            except CodecError:
                continue
            assert codec.encode_instance(params, decoded) == payload
    prefix = encode_prefix(inst)
    for suffix, error in [
        (struct.pack("<I", 2**32 - 1), "bounds_exceeded"),
        (b"\0" * 4 + b"\x21", "invalid_symbol"),
        (b"\0" * 4 + b"\x01" + b"\0" * 5, "non_canonical"),
    ]:
        with pytest.raises(CodecError, match=error):
            codec.decode_instance(params, prefix + suffix)
    invalid = dict(inst, metadata=metadata([(0, 0, 2, 3)], [[1] * 4], []))
    with pytest.raises(CodecError, match="unencodable_instance"):
        codec.encode_instance(params, invalid)


def test_witness_malformed_properties():
    _, inst = fixture(6, [(0, 1, 2, 3)], [[-1, 1, -1, 1]])
    rng = random.Random(2311)
    for _ in range(1024):
        spins = [rng.choice((-1, 1)) for _ in range(6)]
        assert codec.decode(inst, codec.encode(inst, spins)) == spins
        payload = rng.randbytes(rng.randrange(5))
        try:
            decoded = codec.decode(inst, payload)
        except CodecError:
            continue
        assert codec.encode(inst, decoded) == payload
    for spins, error in [([1], "length"), ([0] * 6, "spin_value")]:
        with pytest.raises(CodecError, match=error):
            codec.encode(inst, spins)


def test_golden_rows():
    for row in codec.cases():
        params, wire = bytes.fromhex(row["params"]), bytes.fromhex(row["compact"])
        inst = codec.decode_instance(params, wire)
        assert inst["canonical"].hex() == row["canonical"]
        for witness in row["witnesses"]:
            assert codec.encode(inst, witness["spins"]).hex() == witness["compact"]
        for bad in row["invalid_instances"]:
            with pytest.raises(CodecError, match=bad["error"]):
                codec.decode_instance(params, bytes.fromhex(bad["compact"]))


def test_existing_family_fixtures():
    import json
    from pathlib import Path

    path = (
        Path(__file__).parents[1]
        / "crates/quip-solver-conformance/vectors/families/twisted_dimer.json"
    )
    count = 0
    for case in json.loads(path.read_text())["cases"]:
        params = bytes.fromhex(case["params_hex"])
        for row in case["instances"]:
            inst = dict(
                row,
                canonical=bytes.fromhex(row["canonical_hex"]),
                metadata=bytes.fromhex(row["metadata_hex"]),
                offset_milli=int(row["offset_milli"]),
            )
            compact = codec.encode_instance(params, inst)
            decoded = codec.decode_instance(params, compact)
            assert decoded["canonical"] == inst["canonical"]
            assert decoded["digest"].hex() == row["instance_digest"]
            assert codec.encode_instance(params, decoded) == compact
            count += 1
    assert count > 0


def test_malformed_field_priorities():
    params, inst = fixture(0)
    wire = codec.encode_instance(params, inst)
    for at, value, error in [
        (96, b"\xff" * 4, "bounds_exceeded"),
        (100, b"\xff" * 4, "bounds_exceeded"),
        (114, b"\xff" * 4, "bounds_exceeded"),
        (124, b"\xff" * 4, "bounds_exceeded"),
        (129, b"\xff" * 4, "bounds_exceeded"),
        (128, b"\x21", "invalid_symbol"),
        (133, b"\x01", "non_canonical"),
    ]:
        bad = bytearray(wire)
        bad[at : at + len(value)] = value
        with pytest.raises(CodecError, match=error):
            codec.decode_instance(params, bytes(bad))
    bad = bytearray(wire)
    bad[0] ^= 1
    bad[128] = 1
    with pytest.raises(CodecError, match="descriptor_mismatch"):
        codec.decode_instance(params, bytes(bad))
    params, inst = fixture(6, [(0, 1, 2, 3)], [[1] * 4])
    inst["h"] = [17] * 6
    wire = codec.encode_instance(params, inst)
    bad = bytearray(wire)
    bad[117:121] = struct.pack("<I", 2)
    bad[125:125] = struct.pack("<i", 18)
    with pytest.raises(CodecError, match="non_canonical"):
        codec.decode_instance(params, bytes(bad))
    bad = bytearray(wire)
    bad[115] |= 128
    with pytest.raises(CodecError, match="padding"):
        codec.decode_instance(params, bytes(bad))
