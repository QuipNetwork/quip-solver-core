"""Routing compact codec checks."""

from family_oracle import codec_routing as codec
from family_oracle.digests import descriptor_digest, params_digest
from family_oracle.instance import canonical_bytes
from family_oracle.routing import metadata

PARAMS = bytes.fromhex("010100020000000200020001000000040300020000002000")


def fixture():
    arcs = [{"tail": 0, "head": 1, "x1": 2, "chain": [0, 1], "cost": [0, 65535]}]
    meta = metadata([(65535, 0), (0, 2)], [[-1, 1], [0, 0]], arcs, 65535)
    instance: dict = {
        "descriptor_digest": descriptor_digest(
            b"quip/two-commodity-routing", 1, params_digest(PARAMS)
        ),
        "topology_order_digest": bytes(range(32)),
        "nonce": bytes(32),
        "node_ids": [9, 3, 7],
        "edges": [(0, 2)],
        "h": [-999, 0, 999],
        "j": [-1],
        "offset_milli": -123,
        "metadata": meta,
    }
    instance["canonical"] = canonical_bytes(**instance)
    return instance


def test_instance_roundtrip():
    instance = fixture()
    data = codec.encode_instance(PARAMS, instance)
    assert codec.decode_instance(PARAMS, data)["canonical"] == instance["canonical"]


def test_witness_roundtrip():
    instance = fixture()
    for spins in [[-1, -1, 1], [1, 1, -1], [-1, 1, -1], [1, -1, 1]]:
        assert codec.decode(instance, codec.encode(instance, spins)) == spins


def reject(name, call):
    import pytest
    from family_oracle.codec_common import CodecError

    with pytest.raises(CodecError) as error:
        call()
    assert error.value.name == name


def test_chain_assignments_and_redundant_fallback():
    from itertools import product

    instance = fixture()
    for spins in product((-1, 1), repeat=3):
        data = codec.encode(instance, list(spins))
        assert data[0] & 1 == int(spins[0] != spins[1])
        assert codec.decode(instance, data) == list(spins)
    reject("non_canonical", lambda: codec.decode(instance, bytes([15])))
    reject("padding", lambda: codec.decode(instance, bytes([128])))
    reject("length", lambda: codec.decode(instance, b"\0\0"))
    reject("spin_value", lambda: codec.encode(instance, [0, 1, 1]))
    reject("length", lambda: codec.encode(instance, []))


def test_decoder_properties():
    import random

    from family_oracle.codec_common import CodecError

    rng = random.Random(2311)
    instance = fixture()
    valid = codec.encode_instance(PARAMS, instance)
    for _ in range(1024):
        spins = [rng.choice((-1, 1)) for _ in range(3)]
        assert codec.decode(instance, codec.encode(instance, spins)) == spins
        for data in [rng.randbytes(rng.randrange(4)), bytes([rng.randrange(256)])]:
            try:
                decoded = codec.decode(instance, data)
            except CodecError:
                continue
            assert codec.encode(instance, decoded) == data
        mutated = bytearray(valid)
        mutated[rng.randrange(len(mutated))] ^= rng.randrange(256)
        for data in [rng.randbytes(rng.randrange(len(valid) + 10)), bytes(mutated)]:
            try:
                decoded = codec.decode_instance(PARAMS, data)
            except CodecError:
                continue
            assert codec.encode_instance(PARAMS, decoded) == data
    for end in range(len(valid)):
        reject("length", lambda end=end: codec.decode_instance(PARAMS, valid[:end]))


def test_instance_faults_and_priority():
    import struct

    from family_oracle.codec_common import preflight_prefix, read_column

    instance = fixture()
    valid = codec.encode_instance(PARAMS, instance)
    start = preflight_prefix(valid, codec.LIMITS).metadata_start

    def altered(at, value):
        return valid[:at] + value + valid[at + len(value) :]

    for at in (96, 100, start):
        reject(
            "bounds_exceeded",
            lambda at=at: codec.decode_instance(PARAMS, altered(at, b"\xff" * 4)),
        )
    reject(
        "invalid_symbol", lambda: codec.decode_instance(PARAMS, altered(112, b"\x21"))
    )
    reject(
        "invalid_symbol",
        lambda: codec.decode_instance(PARAMS, altered(start + 4, b"\x21")),
    )
    _, length = read_column(valid[start + 4 :], 4)
    supply = start + 4 + length
    reject(
        "invalid_symbol",
        lambda: codec.decode_instance(PARAMS, altered(supply, b"\x03")),
    )
    reject(
        "bounds_exceeded",
        lambda: codec.decode_instance(PARAMS, altered(supply + 1, b"\xff" * 4)),
    )
    reject(
        "descriptor_mismatch",
        lambda: codec.decode_instance(PARAMS, altered(0, bytes([valid[0] ^ 1]))),
    )
    reject("length", lambda: codec.decode_instance(PARAMS, valid + b"\0"))
    malformed = dict(instance, metadata=instance["metadata"][:-1] + b"\0\0")
    reject("unencodable_instance", lambda: codec.encode_instance(PARAMS, malformed))
    # Retain a structurally valid coefficient palette, but include an unused value.
    at = 112
    for count in (3, 2):
        _, length = read_column(valid[at:], count)
        at += length
    palette = struct.pack("<I4i", 4, -999, 0, 999, 1000) + b"\x02\x24"
    # K > N is rejected before reading the palette bytes.
    reject(
        "bounds_exceeded",
        lambda: codec.decode_instance(PARAMS, valid[:at] + palette + valid[at + 18 :]),
    )


def test_nonminimal_and_unused_palettes():
    import struct

    from family_oracle.codec_common import preflight_prefix

    instance = fixture()
    instance["h"] = [0, 0, 0]
    instance.pop("canonical")
    valid = codec.encode_instance(PARAMS, instance)
    # Node ids are width four. Width five adds an all-zero high bit to every id.
    alternate_ids = bytes([5, 9 | (3 << 5) & 255, ((3 << 5) | (7 << 10)) >> 8])
    reject(
        "non_canonical",
        lambda: codec.decode_instance(
            PARAMS, valid[:112] + alternate_ids + valid[115:]
        ),
    )
    # ids=3 bytes, endpoints=2 bytes, then a one-value zero palette and width zero.
    at = 117
    alternate_palette = struct.pack("<Iii", 2, 0, 1) + b"\x00"
    reject(
        "non_canonical",
        lambda: codec.decode_instance(
            PARAMS, valid[:at] + alternate_palette + valid[at + 9 :]
        ),
    )
    prefix = preflight_prefix(valid, codec.LIMITS)
    assert prefix.node_count == 3


def test_golden_cases():
    for row in codec.cases():
        params = bytes.fromhex(row["params"])
        instance = codec.decode_instance(params, bytes.fromhex(row["compact"]))
        assert instance["canonical"].hex() == row["canonical"]
        for witness in row["witnesses"]:
            assert codec.encode(instance, witness["spins"]).hex() == witness["compact"]
        for negative in row["invalid_instances"]:
            reject(
                negative["error"],
                lambda params=params, negative=negative: codec.decode_instance(
                    params, bytes.fromhex(negative["compact"])
                ),
            )


def test_generated_instance_roundtrips():
    from family_oracle.ice import lattice
    from family_oracle.routing import generate

    nodes, edges = lattice(2, 4)
    successes = skips = 0
    for seed in range(1024):
        nonce = seed.to_bytes(4, "little") + bytes(28)
        code, instance = generate(
            b"quip/two-commodity-routing", 1, PARAMS, nodes, edges, nonce
        )
        if code:
            assert code in (13, 14)
            skips += 1
            continue
        assert instance is not None
        compact = codec.encode_instance(PARAMS, instance)
        decoded = codec.decode_instance(PARAMS, compact)
        assert decoded["canonical"] == instance["canonical"]
        assert decoded["digest"] == instance["digest"]
        assert codec.encode_instance(PARAMS, decoded) == compact
        successes += 1
    assert successes + skips == 1024
    assert successes > 0
    print(f"generated successes={successes}, construction skips={skips}")


def test_zero_width_empty_instance():
    instance: dict = {
        "descriptor_digest": descriptor_digest(
            b"quip/two-commodity-routing", 1, params_digest(PARAMS)
        ),
        "topology_order_digest": bytes(32),
        "nonce": bytes(32),
        "node_ids": [],
        "edges": [],
        "h": [],
        "j": [],
        "offset_milli": 0,
        "metadata": metadata([], [], [], 0),
    }
    data = codec.encode_instance(PARAMS, instance)
    decoded = codec.decode_instance(PARAMS, data)
    assert decoded["canonical"] == canonical_bytes(**instance)
    assert codec.encode(instance, []) == b"\0"
    assert codec.decode(instance, b"\0") == []
    reject("non_canonical", lambda: codec.decode(instance, b"\1"))
