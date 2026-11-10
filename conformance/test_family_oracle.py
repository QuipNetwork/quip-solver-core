"""Tests for the family oracle stream and digest primitives."""

import json
import struct
import subprocess
import sys
import zlib
from pathlib import Path

import pytest
from family_oracle import __main__ as cli
from family_oracle import build, decisions, ice, topology_blob
from family_oracle.chacha8 import ChaCha8, shuffle, uniform
from family_oracle.digests import (
    derive_nonce,
    descriptor_material,
    lease_salts,
    params_digest,
    topology_order_digest,
)
from family_oracle.instance import (
    bound_code,
    canonical_bytes,
    default_node_order,
    edge_fault,
    energy_plain,
    example_size_bounds,
    metadata_code,
    pack_spins,
    size_bound_code,
    unpack_spins,
)
from family_oracle.params import (
    ALL_TYPES,
    UNIFORM_FIELD,
    decode_params,
    encode_params,
    family_id_code,
    label_valid,
    revision_code,
    solver_id_class,
)

VECTORS = (
    Path(__file__).resolve().parent.parent / "crates/quip-solver-conformance/vectors"
)
SPEC_PARAMS = bytes.fromhex("01010002000000020002000400000024faffff")


def _load(name: str):
    return json.loads((VECTORS / name).read_text())


class _StubRng:
    def __init__(self, words: list[int]) -> None:
        self._words = iter(words)
        self.words_used = 0

    def next_u32(self) -> int:
        self.words_used += 1
        return next(self._words)


def test_chacha8_matches_golden_vectors():
    cases = _load("golden_vectors.json")["chacha8"]
    assert cases
    for case in cases:
        rng = ChaCha8(bytes.fromhex(case["seed_hex"]))
        words = [rng.next_u32() for _ in range(case["n_words"])]
        assert words == case["words"]
        assert len(words) == case["n_words"]


def test_derive_nonce_matches_golden_vectors():
    cases = _load("golden_vectors.json")["derive_nonce"]
    assert cases
    for case in cases:
        nonce = derive_nonce(
            bytes.fromhex(case["last_proof_hex"]),
            bytes.fromhex(case["miner_hex"]),
            bytes.fromhex(case["salt_hex"]),
        )
        assert nonce.hex() == case["nonce_hex"]


def test_lease_salts_match_golden_lease():
    cases = _load("golden_lease.json")["cases"]
    assert len(cases) == 3
    for case in cases:
        result = lease_salts(
            bytes.fromhex(case["last_proof_block_hash"]),
            bytes.fromhex(case["miner_account"]),
            bytes.fromhex(case["base_salt"]),
            int(case["salt_start"]),
            case["salt_count"],
        )
        assert result is not None
        assert len(result) == len(case["salts"])
        for (counter, salt, nonce), expected in zip(result, case["salts"], strict=True):
            assert salt.hex() == expected["salt"]
            assert nonce.hex() == expected["nonce"]
            assert counter == int.from_bytes(salt[:8], "little")


def test_lease_salts_reject_overflow_and_zero():
    args = (bytes(32), bytes(32), bytes(32))
    assert lease_salts(*args, 2**64 - 3, 3) is None
    assert lease_salts(*args, 2**64 - 1, 1) is None
    assert lease_salts(*args, 0, 0) is None
    result = lease_salts(*args, 2**64 - 3, 2)
    assert result is not None
    assert len(result) == 2


def test_uniform_redraws_at_limit():
    rng = _StubRng([2**32 - 1, 7])
    assert uniform(rng, 2**31 + 1) == 7
    assert rng.words_used == 2
    one = ChaCha8(bytes(32))
    assert uniform(one, 1) == 0
    assert one.words_used == 1


def test_uniform_rejects_out_of_range():
    rng = ChaCha8(bytes(32))
    for n in (0, -1, 2**32):
        with pytest.raises(ValueError):
            uniform(rng, n)


def test_shuffle_short_lists_read_no_words():
    for items in ([], [1]):
        rng = ChaCha8(bytes(32))
        shuffle(rng, items)
        assert rng.words_used == 0


def test_shuffle_permutes():
    rng = ChaCha8(bytes(range(32)))
    items = list(range(10))
    shuffle(rng, items)
    assert sorted(items) == list(range(10))
    assert items != list(range(10))


def test_descriptor_material_matches_spec_layout():
    digest = params_digest(SPEC_PARAMS)
    material = descriptor_material(b"example/uniform-field", 1, digest)
    assert len(material) == 58
    assert material.startswith(
        bytes.fromhex("15") + b"example/uniform-field" + bytes.fromhex("01000000")
    )
    assert material.endswith(digest)


def test_spec_example_params_hex():
    data = encode_params(UNIFORM_FIELD, {"n_layers": 2, "field_milli": -1500})
    assert data.hex() == SPEC_PARAMS.hex()
    assert decode_params(UNIFORM_FIELD, data) == (
        None,
        {"n_layers": 2, "field_milli": -1500},
    )


def test_spec_negative_cases():
    cases = [
        ("010200 04000000 24faffff0100 02000000 0200", 7),
        ("010100 02000000 02000100 02000000 02000200 04000000 24faffff", 7),
        (SPEC_PARAMS.hex() + "0300 01000000 00", 7),
        ("010100 02000000 0200", 7),
        ("010100 04000000 020000000200 04000000 24faffff", 7),
        ("010100 02000000 02000200 04000000 5ff0ffff", 9),
        ("020100 02000000 02000200 04000000 24faffff", 7),
        (SPEC_PARAMS.hex() + "00" * 4078, 7),
    ]
    for text, code in cases:
        data = bytes.fromhex(text.replace(" ", ""))
        assert decode_params(UNIFORM_FIELD, data) == (code, None), text
    assert len(bytes.fromhex(cases[-1][0])) == 4097
    assert family_id_code(b"Quip/X", "lease") == 3
    assert revision_code(0) == 4
    assert revision_code(1) is None


def _all_types_values(a_bytes: bytes, a_ascii: bytes) -> dict:
    return {
        "a_u8": 0,
        "a_u16": 0,
        "a_u32": 0,
        "a_u64": 1,
        "a_i32": 0,
        "a_i64": 0,
        "a_bool": True,
        "a_bytes": a_bytes,
        "a_ascii": a_ascii,
    }


@pytest.mark.parametrize(
    ("values", "code", "size"),
    [
        pytest.param(
            _all_types_values(b"\x00" * 4013, b""), None, 4096, id="4096-bytes-ok"
        ),
        pytest.param(
            _all_types_values(b"\x00" * 4014, b""), 7, None, id="4097-bytes-too-long"
        ),
        pytest.param(
            _all_types_values(b"", b"a" * 64 + b"\x7f"), 7, None, id="bad-ascii-layout"
        ),
        pytest.param(
            _all_types_values(b"", b"a" * 65), 9, None, id="ascii-above-bound"
        ),
        # a_u64 = 0 is out of bounds (1..), but a bad bool byte is a layout fault.
        pytest.param(
            {**_all_types_values(b"", b""), "a_u64": 0}, 9, None, id="u64-out-of-bounds"
        ),
        pytest.param(
            {**_all_types_values(b"", b""), "a_u64": 0, "a_bool": 2},
            7,
            None,
            id="bad-bool-wins-over-bounds",
        ),
    ],
)
def test_all_types_decode_codes(values, code, size):
    data = encode_params(ALL_TYPES, values)
    if size is not None:
        assert len(data) == size
    assert decode_params(ALL_TYPES, data) == (code, values if code is None else None)


def test_family_id_grammar():
    for ok in [b"a/b", b"quip/" + b"a" * 59, b"quip.sub/x"]:
        assert family_id_code(ok, "lease") is None, ok
    assert family_id_code(b"quip/" + b"a" * 60, "lease") == 3
    bad = [
        b"quip./x",
        b"quip/x.y",
        b"quip/1x",
        b"quip/x/y",
        b"-quip/x",
        b"ab",
        b"quip/\xc3\xa9",
    ]
    for item in bad:
        assert family_id_code(item, "lease") == 3, item
    assert family_id_code(b"example/uniform-field", "lease") == 3
    assert family_id_code(b"example/uniform-field", "direct") is None


def test_solver_id_grammar():
    for ok in [b"quip-miner-cpu/otsuka", b"abc", b"a" * 64, b"abc/1.2-x"]:
        assert solver_id_class(ok) == "valid", ok
    bad = [
        b"ab",
        b"a" * 65,
        b"Quip-miner",
        b"1abc",
        b"abc/",
        b"abc//d",
        b"abc/.x",
        b"abc_d",
        b"abc\xc3\xa9",
    ]
    for item in bad:
        assert solver_id_class(item) == "invalid", item
    assert solver_id_class(b"") == "empty"


def test_label_limits():
    assert label_valid(b"a")
    assert label_valid(b"a" * 32)
    assert not label_valid(b"")
    assert not label_valid(b"a" * 33)
    assert not label_valid(b"fiel\x80")


SPARSE_NODES = [907, 41, 0, 4294967295, 12, 3]
SPARSE_METADATA = bytes.fromhex("020001000400000001020304030001000000ff")
SPARSE_EDGES = [(0, 1), (0, 4), (1, 3), (2, 3), (3, 4)]


def _section(kind: int, payload: bytes) -> bytes:
    return kind.to_bytes(2, "little") + len(payload).to_bytes(4, "little") + payload


def _region(*sections: bytes) -> bytes:
    return len(sections).to_bytes(2, "little") + b"".join(sections)


def test_spec_size_bounds_example():
    assert example_size_bounds(2, 100, 150, 4) == (200, 400, 5)


_SPEC_BOUNDS = (200, 400, 5)


@pytest.mark.parametrize(
    ("bounds", "limits", "expected"),
    [
        pytest.param(_SPEC_BOUNDS, (None, None, 4), 13, id="spec-third-4"),
        pytest.param(_SPEC_BOUNDS, (None, None, 0), None, id="spec-third-0"),
        pytest.param(_SPEC_BOUNDS, (None, None, None), None, id="spec-no-limits"),
        pytest.param(_SPEC_BOUNDS, (0, 0, 0), None, id="spec-zeros"),
        pytest.param(_SPEC_BOUNDS, (199, None, None), 13, id="spec-first-199"),
        pytest.param(_SPEC_BOUNDS, (None, 399, None), 13, id="spec-second-399"),
        pytest.param(_SPEC_BOUNDS, (200, 400, 5), None, id="spec-exact-limits"),
        pytest.param(example_size_bounds(2, 8388608, 0, 0), None, None, id="2-8388608"),
        pytest.param(example_size_bounds(2, 8388609, 0, 0), None, 13, id="2-8388609"),
        pytest.param(example_size_bounds(1, 2, 67108864, 1), None, None, id="1-2-64Mi"),
        pytest.param(example_size_bounds(1, 2, 67108865, 1), None, 13, id="1-2-64Mi+1"),
        pytest.param(
            example_size_bounds(16, 4294967295, 0, 0), None, 13, id="16-u32max"
        ),
        pytest.param(example_size_bounds(2**40, 2**30, 0, 0), None, 13, id="huge"),
        pytest.param((2**64, 0, 0), None, 13, id="first-2**64"),
        pytest.param((0, 0, 2**64), None, 13, id="third-2**64"),
    ],
)
def test_size_bound_code(bounds, limits, expected):
    assert size_bound_code(bounds, *(limits or (None, None, None))) == expected


def test_metadata_faults():
    kinds = [1, 3]
    assert metadata_code(SPARSE_METADATA, kinds) is None
    assert metadata_code(bytes.fromhex("0000"), kinds) is None
    one = _section(1, b"\x01")
    three = _section(3, b"\x01")
    assert metadata_code(_region(_section(6, b"")), kinds) == 16
    assert metadata_code(_region(_section(0, b"")), kinds) == 16
    assert metadata_code(_region(_section(2, b"")), kinds) == 16
    assert metadata_code(_region(three, one), kinds) == 16
    assert metadata_code(_region(one, one), kinds) == 16
    assert metadata_code(bytes.fromhex("010001000900000001"), kinds) == 16
    assert metadata_code(bytes.fromhex("0200") + one, kinds) == 16
    assert metadata_code(SPARSE_METADATA + b"\x00", kinds) == 16
    assert metadata_code(b"\x00", kinds) == 16
    assert metadata_code(b"", kinds) == 16
    assert metadata_code(bytes.fromhex("0000") + b"\x00", kinds) == 16


def test_bounds():
    for q in ("h", "j"):
        assert bound_code(q, 2**30) is None
        assert bound_code(q, -(2**30)) is None
        for v in (2**30 + 1, -(2**30) - 1, -(2**31), 2**31 - 1):
            assert bound_code(q, v) == 13
    assert bound_code("offset", 2**56) is None
    assert bound_code("offset", -(2**56)) is None
    for v in (2**56 + 1, -(2**63), 2**63 - 1):
        assert bound_code("offset", v) == 13


def test_edge_faults():
    assert edge_fault(SPARSE_EDGES, 5) is None
    defects = {
        "unsorted": ([(0, 4), (0, 1), (1, 3), (2, 3), (3, 4)], "FAM-32"),
        "duplicate": ([(0, 1), (0, 1), (1, 3), (2, 3), (3, 4)], "FAM-32"),
        "self-loop": ([(0, 0), (0, 4), (1, 3), (2, 3), (3, 4)], "FAM-32"),
        "reversed-pair": ([(1, 0), (0, 4), (1, 3), (2, 3), (3, 4)], "FAM-32"),
        "position-out-of-range": (
            [(0, 1), (0, 4), (1, 3), (2, 3), (3, 5)],
            "FAM-30",
        ),
    }
    for name, (edges, rule) in defects.items():
        assert edge_fault(edges, 5) == rule, name
    # A position fault wins over an order fault.
    assert edge_fault([(5, 0), (0, 1)], 5) == "FAM-30"


def test_spins_packing():
    assert pack_spins([1, -1, 1, -1, 1]).hex() == "15"
    assert pack_spins([1] * 10).hex() == "ff03"
    assert len(pack_spins([1, -1] * 4)) == 1
    assert unpack_spins(bytes.fromhex("15"), 5) == ([1, -1, 1, -1, 1], None)
    assert unpack_spins(bytes.fromhex("35"), 5) == (None, "padding-bit-set")
    assert unpack_spins(b"", 5) == (None, "wrong-length")
    assert unpack_spins(bytes.fromhex("1500"), 5) == (None, "wrong-length")
    assert unpack_spins(bytes.fromhex("ff07"), 10) == (None, "padding-bit-set")
    assert unpack_spins(bytes.fromhex("ff03"), 10) == ([1] * 10, None)


def test_energy_plain():
    assert energy_plain([1, 2], [3], [(0, 1)], [1, -1]) == 1 - 2 - 3
    assert energy_plain([1, 2], [3], [(0, 1)], [1, 1]) == 1 + 2 + 3


def _sparse_canonical(nonce: bytes) -> bytes:
    return canonical_bytes(
        b"\x01" * 32,
        b"\x02" * 32,
        nonce,
        [907, 41, 4294967295, 12, 3],
        SPARSE_EDGES,
        [1073741824, -1073741824, 0, 1500, -1],
        [1073741824, -1073741824, 1000, -1000, 0],
        -72057594037927936,
        SPARSE_METADATA,
    )


def test_canonical_bytes_layout():
    data = _sparse_canonical(b"\x03" * 32)
    assert data[0] == 1
    assert data[1:33] == b"\x01" * 32
    assert data[33:65] == b"\x02" * 32
    assert data[65:97] == b"\x03" * 32
    assert data[97:101] == (5).to_bytes(4, "little")
    assert data.endswith(SPARSE_METADATA)
    assert len(data) == 1 + 96 + 4 + 20 + 4 + 40 + 20 + 20 + 8 + len(SPARSE_METADATA)
    with pytest.raises(ValueError):
        _sparse_canonical(b"\x03" * 31)
    assert _sparse_canonical(b"") == _sparse_canonical(bytes(32))


def test_default_node_order():
    used = {907, 41, 4294967295, 12, 3}
    assert default_node_order(SPARSE_NODES, used) == [907, 41, 4294967295, 12, 3]


# ---- decision tables ----

D1 = params_digest(SPEC_PARAMS)
D2 = params_digest(encode_params(UNIFORM_FIELD, {"n_layers": 1, "field_milli": -1500}))

_CODE_ROWS = [
    (1, "DescriptorMissing", ["admission"], "MALFORMED", None),
    (2, "DescriptorUnexpected", ["admission"], "MALFORMED", None),
    (3, "FamilyIdInvalid", ["admission"], "MALFORMED", None),
    (4, "RevisionZero", ["admission"], "MALFORMED", None),
    (5, "FamilyUnsupported", ["admission"], "FAMILY_UNSUPPORTED", None),
    (6, "RevisionUnsupported", ["admission"], "FAMILY_UNSUPPORTED", None),
    (7, "ParamsMalformed", ["admission", "explicit-job"], "MALFORMED", "MALFORMED"),
    (
        8,
        "ParamsDigestMismatch",
        ["admission", "explicit-job"],
        "MALFORMED",
        "MALFORMED",
    ),
    (9, "ParamsOutOfBounds", ["admission", "explicit-job"], "MALFORMED", "MALFORMED"),
    (
        10,
        "ParamsRestricted",
        ["admission", "explicit-job"],
        "FAMILY_UNSUPPORTED",
        "FAMILY_UNSUPPORTED",
    ),
    (11, "TopologyOrderMismatch", ["admission"], "TOPOLOGY_MISMATCH", None),
    (12, "TopologyIncompatible", ["admission"], "FAMILY_UNSUPPORTED", None),
    (
        13,
        "BoundsExceeded",
        ["admission", "generation", "explicit-job"],
        "TOO_LARGE",
        "TOO_LARGE",
    ),
    (14, "NoInstance", ["generation"], None, None),
    (15, "InstanceDigestMismatch", ["explicit-job"], None, "MALFORMED"),
    (16, "MetadataInvalid", ["explicit-job"], None, "MALFORMED"),
]


def test_codes_table():
    expected = [
        {
            "code": code,
            "name": name,
            "phases": phases,
            "wire_admission": admission,
            "wire_generation": None,
            "wire_explicit_job": explicit,
        }
        for code, name, phases, admission, explicit in _CODE_ROWS
    ]
    assert decisions.CODES == expected


_LEASE = decisions.LEASE_ADMISSION_ORDER
_JOB = decisions.EXPLICIT_JOB_ORDER


@pytest.mark.parametrize(
    ("order", "phase", "faults", "expected"),
    [
        pytest.param(
            _LEASE,
            "admission",
            ["code:1", "session:expired"],
            ("session:expired", None, "EXPIRED"),
            id="lease-session-expired",
        ),
        pytest.param(
            _LEASE,
            "admission",
            ["code:1", "field:salt-count-zero"],
            ("field:salt-count-zero", None, "MALFORMED"),
            id="lease-salt-count-zero",
        ),
        pytest.param(
            _LEASE,
            "admission",
            ["session:topology-hash-mismatch", "code:10"],
            ("code:10", 10, "FAMILY_UNSUPPORTED"),
            id="lease-code-10-before-topology-hash",
        ),
        pytest.param(
            _LEASE,
            "admission",
            ["code:11", "session:topology-hash-mismatch"],
            ("session:topology-hash-mismatch", None, "TOPOLOGY_MISMATCH"),
            id="lease-topology-hash-before-code-11",
        ),
        pytest.param(
            _LEASE,
            "admission",
            ["code:13", "code:11"],
            ("code:11", 11, "TOPOLOGY_MISMATCH"),
            id="lease-code-11-before-13",
        ),
        pytest.param(
            _LEASE,
            "admission",
            ["code:5"],
            ("code:5", 5, "FAMILY_UNSUPPORTED"),
            id="lease-code-5",
        ),
        pytest.param(
            _JOB,
            "explicit-job",
            ["tier:not-listed", "code:15"],
            ("tier:not-listed", None, None),
            id="job-tier-not-listed",
        ),
        pytest.param(
            _JOB,
            "explicit-job",
            ["code:15", "code:13"],
            ("code:13", 13, "TOO_LARGE"),
            id="job-code-13-before-15",
        ),
        pytest.param(
            _JOB,
            "explicit-job",
            ["code:16", "code:8"],
            ("code:8", 8, "MALFORMED"),
            id="job-code-8-before-16",
        ),
        pytest.param(
            _JOB,
            "explicit-job",
            ["code:16"],
            ("code:16", 16, "MALFORMED"),
            id="job-code-16",
        ),
    ],
)
def test_winner_outcomes(order, phase, faults, expected):
    step = decisions.winner(order, faults)
    code, wire = decisions.step_outcome(step, phase)
    assert (step, code, wire) == expected


@pytest.mark.parametrize("faults", [[], ["code:99"]], ids=["empty", "unknown-fault"])
def test_winner_rejects_bad_input(faults):
    with pytest.raises(ValueError):
        decisions.winner(decisions.LEASE_ADMISSION_ORDER, faults)


def _entry(family_id, revisions, digests=()):
    return {
        "family_id": family_id,
        "revisions": list(revisions),
        "params_digests": [d.hex() if isinstance(d, bytes) else d for d in digests],
    }


def test_entry_claims():
    t = "quip/twisted-ice-dimer"
    cases = [
        ([_entry(t, [1])], [True]),
        ([_entry(t, [1], [D1])], [True]),
        ([_entry("example/foo", [1])], [False]),
        ([_entry(t, [])], [False]),
        ([_entry(t, [0, 1])], [False]),
        ([_entry(t, [1], ["00" * 31])], [False]),
        ([_entry("Quip/X", [1])], [False]),
        (
            [_entry(t, [1]), _entry(t, [2]), _entry("quip/other", [1])],
            [False, False, True],
        ),
    ]
    for entries, expected in cases:
        assert decisions.entry_claims(entries) == expected


def test_lease_codes():
    entries = [
        _entry("quip/twisted-ice-dimer", [1]),
        _entry("quip/charged-ice", [2], [D1]),
    ]
    cases = [
        ("quip/twisted-ice-dimer", 1, D1, None),
        ("quip/twisted-ice-dimer", 2, D1, 6),
        ("quip/charged-ice", 1, D1, 6),
        ("quip/routing", 1, D1, 5),
        ("quip/charged-ice", 2, D1, None),
        ("quip/charged-ice", 2, D2, 10),
    ]
    for family_id, revision, digest, expected in cases:
        assert decisions.lease_code(entries, family_id, revision, digest) == expected


def test_field_length_codes():
    f = decisions.field_length_code
    d = "FamilyDescriptor.params_digest"
    assert [f(d, n, None) for n in (32, 31, 33, 0)] == [None, 8, 8, 8]
    g = "IsingProblemGenerator.topology_order_digest"
    assert [f(g, n, None) for n in (32, 0, 31)] == [None, 1, 1]
    n = "FamilyBinding.nonce"
    assert [f(n, k, None) for k in (0, 32, 31, 33)] == [None, None, 15, 15]
    i = "FamilyBinding.instance_digest"
    assert [f(i, k, None) for k in (32, 0)] == [None, 15]
    t = "FamilyBinding.topology_order_digest"
    assert [f(t, k, None) for k in (32, 31)] == [None, 15]
    ids = "FamilyBinding.node_ids"
    assert [f(ids, k, 5) for k in (5, 4, 6)] == [None, 15, 15]
    with pytest.raises(ValueError):
        f("Nope.field", 1, None)


def test_layered_p_spec_skips_match_the_session_capture():
    from family_oracle import layered
    from family_oracle.digests import lease_salts

    shared = topology_blob.decode(
        (VECTORS / "topologies" / "t-sparse.bin.gz").read_bytes()
    )
    nodes = shared["nodes"]
    edges = [tuple(edge) for edge in shared["edges"]]
    params = bytes.fromhex("01010002000000020002000400000024faffff")
    lease = lease_salts(
        bytes([0x11]) * 32,
        bytes([0x22]) * 32,
        bytes(8) + bytes([0x33]) * 24,
        5,
        16,
    )
    assert lease is not None
    skipped = set()
    for index, (_, _, nonce) in enumerate(lease):
        code, instance = layered.generate(
            b"vectors/uniform-field", 1, params, nodes, edges, nonce
        )
        if code is not None:
            assert instance is None
            skipped.add(index)
        else:
            assert instance is not None
    assert skipped == {0, 3, 9, 13}


# ---- ice structure oracle ----


@pytest.mark.parametrize("m", [1, 2, 3])
@pytest.mark.parametrize("t", [1, 2, 3, 4])
def test_ice_lattice_counts(m, t):
    nodes, edges = ice.lattice(m, t)
    assert len(nodes) == 4 * t * m * (2 * m + 1)
    expected = (
        2 * t * (8 * t + 3) if m == 1 else 2 * t * ((8 * t + 8) * m * m - 2 * m - 3)
    )
    assert len(edges) == expected
    assert edges == sorted(set(edges))
    assert all(a < b for a, b in edges)


def test_ice_coordinates_invert_the_linear_id():
    m, t = 2, 3
    for q in range(ice.node_count(m, t)):
        assert ice.linear_id(m, t, *ice.zephyr_coordinates(m, t, q)) == q
    assert ice.zephyr_coordinates(m, t, ice.node_count(m, t)) is None


def test_ice_live_blob_gives_the_pinned_counts_and_digest():
    live_cases = json.loads(build.ICE_STRUCTURE_PATH.read_text())["live"]
    paths = {(build.FAMILIES_DIR / case["topology"]).resolve() for case in live_cases}
    assert paths == {build.LIVE_BLOB_PATH.resolve()}
    body = topology_blob.decode(paths.pop().read_bytes())
    assert body["kind"] == "zephyr-removed"
    live = topology_blob.expand(body)
    nodes, edges = live["nodes"], live["edges"]
    assert (len(nodes), len(edges)) == (4575, 41474)
    assert topology_order_digest(nodes, edges).hex() == (
        "e12cffc9392cc502c5d065d6e1a3e2eabcc08247ffd55d2fe1b41988d2bc37e3"
    )
    rebuilt_nodes, rebuilt_edges = ice.live_topology(
        body["removed_nodes"], body["removed_edges"]
    )
    assert nodes == rebuilt_nodes
    assert edges == [list(edge) for edge in rebuilt_edges]


def test_ice_both_modes_agree_and_select_is_a_matching():
    nodes, edges = ice.lattice(2, 2)
    zephyr = ice.candidates(2, 2, nodes, edges)
    assert zephyr == ice.candidates(0, 0, nodes, edges)
    tiles, spines = zephyr
    coords = [ice.zephyr_coordinates(2, 2, n) for n in nodes]
    left, right = ice.sides(2, coords, spines)
    for seed in (bytes(range(32)), bytes(range(32, 64))):
        orders = ice.seeded_orders(seed, len(left), len(right))
        chosen = ice.select(tiles, spines, left, right, *orders)
        used = [e for tile in chosen for e in ice._tile_edges(tile)]
        assert len(used) == len(set(used))
        assert len(chosen) == min(len(left), len(right))


def test_ice_ties_follow_the_orders():
    nodes, edges = build._graph(build._tile_ring(4))
    tiles, spines = ice.candidates(0, 0, nodes, edges)
    left, right = ice.sides(0, [], spines)
    assert left == [(0, 1), (4, 5)] and right == [(2, 3), (6, 7)]
    assert ice.select(tiles, spines, left, right, [0, 1], [0, 1]) == [
        (0, 1, 6, 7),
        (2, 3, 4, 5),
    ]
    assert ice.select(tiles, spines, left, right, [1, 0], [0, 1]) == [
        (0, 1, 2, 3),
        (4, 5, 6, 7),
    ]


# ---- topology blob codec ----

BLOB_HEAD = b"QTOP\x01\x00"
BLOB_FILE_HEAD = bytes([0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 2, 0xFF])


def _varint(value):
    out = bytearray()
    topology_blob.put_varint(out, value)
    return bytes(out)


def _frame(payload, crc=None, size=None, header=BLOB_FILE_HEAD):
    compressor = zlib.compressobj(9, zlib.DEFLATED, -15)
    stream = compressor.compress(payload) + compressor.flush()
    crc = zlib.crc32(payload) if crc is None else crc
    size = len(payload) if size is None else size
    return header + stream + struct.pack("<II", crc, size)


def _explicit(nodes, edges):
    return {"kind": "explicit", "nodes": nodes, "edges": edges}


def _zephyr(m, t, nodes, edges):
    return {
        "kind": "zephyr-removed",
        "m": m,
        "t": t,
        "removed_nodes": nodes,
        "removed_edges": edges,
    }


def _payload_error(payload):
    with pytest.raises(topology_blob.BlobError) as info:
        topology_blob.decode_payload(payload)
    return info.value.code


def _file_error(data):
    with pytest.raises(topology_blob.BlobError) as info:
        topology_blob.decode(data)
    return info.value.code


@pytest.mark.parametrize(
    ("body", "code"),
    [
        pytest.param(_explicit([True], []), "shape", id="bool-node"),
        pytest.param(_explicit([1], [[1, False]]), "shape", id="bool-edge-end"),
        pytest.param(_explicit((1,), []), "shape", id="tuple-nodes"),
        pytest.param(_explicit([1], {"a": 1}), "shape", id="dict-edges"),
        pytest.param(_explicit([1], [[1, 2, 3]]), "shape", id="triple-edge"),
        pytest.param(_explicit([1], [5]), "shape", id="scalar-edge"),
        pytest.param(_zephyr(True, 1, [], []), "shape", id="bool-m"),
        pytest.param(_zephyr(1, True, [], []), "shape", id="bool-t"),
        pytest.param(_zephyr(1, 1, "ab", []), "shape", id="string-removed-nodes"),
        pytest.param(_zephyr(1, 1, [], {}), "shape", id="dict-removed-edges"),
        pytest.param(_zephyr(1, 1, [], [[1]]), "shape", id="short-removed-edge"),
        pytest.param(_explicit([2**32], []), None, id="node-above-u32"),
        pytest.param(_explicit([-1], []), None, id="negative-node"),
        pytest.param(_explicit([1], [[1, 2**32]]), None, id="edge-end-above-u32"),
        pytest.param(_zephyr(0, 1, [], []), None, id="m-0"),
        pytest.param(_zephyr(33, 1, [], []), None, id="m-33"),
        pytest.param(_zephyr(1, 9, [], []), None, id="t-9"),
        pytest.param(_zephyr(1, 1, [12], []), None, id="removed-not-a-node"),
        pytest.param(_zephyr(1, 1, [3, 3], []), None, id="removed-duplicate"),
        pytest.param(_zephyr(1, 1, [4, 3], []), None, id="removed-descending"),
        pytest.param(_zephyr(1, 1, [], [[0, 2]]), None, id="not-a-coupler"),
        pytest.param(_zephyr(1, 1, [], [[1, 0]]), None, id="reversed-edge"),
        pytest.param(_zephyr(1, 1, [], [[0, 6], [0, 1]]), None, id="edges-unsorted"),
        pytest.param(_zephyr(1, 1, [], [[0, 1], [0, 1]]), None, id="edges-duplicate"),
        pytest.param(_zephyr(1, 1, [0], [[0, 1]]), None, id="edge-at-removed-node"),
        pytest.param({"kind": "other"}, None, id="unknown-kind"),
    ],
)
def test_blob_encoder_rejects(body, code):
    with pytest.raises(topology_blob.BlobError) as info:
        topology_blob.encode(body)
    if code is not None:
        assert info.value.code == code


def test_blob_payload_bytes_are_pinned():
    payload = topology_blob.encode_payload(_explicit([1, 2], [[1, 2]]))
    assert payload.hex() == "51544f5001" + "00" + "020202010202"
    assert topology_blob.encode_payload(_zephyr(2, 2, [5], [[0, 1]])) == (
        BLOB_HEAD[:5] + b"\x01" + bytes([2, 2, 1, 5, 1, 0, 1])
    )


def test_blob_round_trips():
    body = _explicit(
        [4294967295, 0, 7, 3],
        [[4294967295, 0], [0, 4294967295], [7, 3], [7, 3], [3, 3]],
    )
    data = topology_blob.encode(body)
    assert data[:10] == BLOB_FILE_HEAD
    assert topology_blob.decode(data) == body
    nodes, edges = ice.lattice(2, 2)
    zephyr = _zephyr(2, 2, [0, 5], [list(edges[40])])
    first = topology_blob.encode(zephyr)
    assert first == topology_blob.encode(zephyr)
    assert first == topology_blob.encode(json.loads(json.dumps(zephyr)))
    explicit = _explicit(nodes, [list(e) for e in edges])
    assert topology_blob.encode(explicit) == topology_blob.encode(explicit)
    assert topology_blob.decode(topology_blob.encode(_explicit([], []))) == _explicit(
        [], []
    )
    payload = topology_blob.encode_payload(_explicit([1, 2], [[1, 2]]))
    header = bytes([0x1F, 0x8B, 8, 0, 1, 2, 3, 4, 0, 3])
    assert topology_blob.decode(_frame(payload, header=header)) == _explicit(
        [1, 2], [[1, 2]]
    )


def test_blob_zephyr_expands_to_the_lattice_minus_removals():
    nodes, edges = ice.lattice(2, 2)
    edge = next(e for e in edges if not {0, 5} & set(e))
    body = _zephyr(2, 2, [0, 5], [list(edge)])
    assert topology_blob.decode(topology_blob.encode(body)) == body
    expanded = topology_blob.expand(body)
    assert expanded["nodes"] == [n for n in nodes if n not in (0, 5)]
    assert [list(edge)] not in [[e] for e in expanded["edges"]]
    assert all(
        a < b and a not in (0, 5) and b not in (0, 5) for a, b in expanded["edges"]
    )
    assert expanded["edges"] == sorted(expanded["edges"])
    full = topology_blob.expand(_zephyr(2, 2, [], []))
    assert full == _explicit(nodes, [list(e) for e in edges])
    z11 = topology_blob.expand(_zephyr(1, 1, [], []))
    assert len(z11["nodes"]) == 12 and len(z11["edges"]) == 22


_GOOD_PAYLOAD = topology_blob.encode_payload(_explicit([1, 2], [[1, 2]]))
_GOOD_FILE = topology_blob.encode(_explicit([1, 2], [[1, 2]]))


def _patched(data, index, value):
    bad = bytearray(data)
    bad[index] = value
    return bytes(bad)


@pytest.mark.parametrize(
    ("data", "code"),
    [
        pytest.param(b"", "gzip", id="empty"),
        pytest.param(_GOOD_FILE[:-1], "gzip", id="short-trailer"),
        pytest.param(_GOOD_FILE[:12], "gzip", id="truncated-stream"),
        pytest.param(_GOOD_FILE + b"\x00", "gzip", id="trailing-byte"),
        pytest.param(_GOOD_FILE * 2, "gzip", id="two-members"),
        pytest.param(_GOOD_FILE + _GOOD_FILE[:5], "gzip", id="partial-second-member"),
        pytest.param(_patched(_GOOD_FILE, 0, 0), "gzip", id="magic-byte-0"),
        pytest.param(_patched(_GOOD_FILE, 1, 0), "gzip", id="magic-byte-1"),
        pytest.param(_patched(_GOOD_FILE, 2, 7), "gzip", id="method-7"),
        pytest.param(_patched(_GOOD_FILE, 3, 8), "gzip", id="flags-8"),
        # Garbage in place of the DEFLATE stream.
        pytest.param(_GOOD_FILE[:10] + b"\xff" * 12, "gzip", id="garbage-stream"),
        pytest.param(
            _frame(_GOOD_PAYLOAD, crc=zlib.crc32(_GOOD_PAYLOAD) ^ 1), "crc", id="crc"
        ),
        pytest.param(
            _frame(_GOOD_PAYLOAD, size=len(_GOOD_PAYLOAD) + 1), "isize", id="isize-high"
        ),
        pytest.param(
            _frame(_GOOD_PAYLOAD, size=len(_GOOD_PAYLOAD) - 1), "isize", id="isize-low"
        ),
    ],
)
def test_blob_file_errors(data, code):
    assert _file_error(data) == code


_Z11 = b"QTOP\x01\x01\x01\x01"
_EDGES = BLOB_HEAD + b"\x00\x01"


@pytest.mark.parametrize(
    ("payload", "code"),
    [
        pytest.param(b"", "magic", id="header-empty"),
        pytest.param(b"QTO", "magic", id="header-short-magic"),
        pytest.param(b"QTOQ\x01\x00\x00\x00", "magic", id="header-wrong-magic"),
        pytest.param(b"QTOP\x02\x00\x00\x00", "version", id="header-version-2"),
        pytest.param(b"QTOP\x00\x00\x00\x00", "version", id="header-version-0"),
        pytest.param(b"QTOP\x01\x02\x00\x00", "kind", id="header-kind-2"),
        pytest.param(b"QTOP\x01", "truncated", id="header-no-kind"),
        pytest.param(b"QTOP\x01\x00", "truncated", id="header-no-count"),
        pytest.param(BLOB_HEAD + b"\x80\x00", "varint-non-minimal", id="varint-80-00"),
        pytest.param(
            BLOB_HEAD + b"\xff\x80\x00", "varint-non-minimal", id="varint-ff-80-00"
        ),
        pytest.param(BLOB_HEAD + b"\x80", "truncated", id="varint-cut-off"),
        pytest.param(
            BLOB_HEAD + b"\xff" * 10 + b"\x01", "varint-overflow", id="varint-11-bytes"
        ),
        pytest.param(
            BLOB_HEAD + b"\xff" * 9 + b"\x02", "varint-overflow", id="varint-above-u64"
        ),
        pytest.param(
            BLOB_HEAD + b"\x80" * 10 + b"\x00", "varint-overflow", id="varint-padded"
        ),
        # u64::MAX itself is a legal varint, so it reaches the count check instead.
        pytest.param(
            BLOB_HEAD + _varint(2**64 - 1), "count-too-large", id="varint-u64-max"
        ),
        pytest.param(
            BLOB_HEAD + b"\x01" + _varint(2 * 2**32), "id-range", id="node-above-u32"
        ),
        pytest.param(BLOB_HEAD + b"\x01" + _varint(1), "id-range", id="node-negative"),
        # A zigzag delta below zero from 0.
        pytest.param(
            BLOB_HEAD + b"\x01\x01\x00\x00", "id-range", id="node-delta-below-zero"
        ),
        pytest.param(
            _EDGES + _varint(2 * 2**32) + b"\x00", "id-range", id="edge-first-above-u32"
        ),
        pytest.param(
            _EDGES + b"\x02" + _varint(2 * 2**32),
            "id-range",
            id="edge-second-above-u32",
        ),
        pytest.param(_EDGES + b"\x01" + b"\x01", "id-range", id="edge-negative"),
        # zigzag(i64::MAX) and zigzag(i64::MIN) as deltas overflow the id range.
        pytest.param(
            BLOB_HEAD + b"\x01" + _varint(2**64 - 2), "id-range", id="delta-i64-max"
        ),
        pytest.param(
            BLOB_HEAD + b"\x01" + _varint(2**64 - 1), "id-range", id="delta-i64-min"
        ),
        pytest.param(BLOB_HEAD + b"\x05\x00\x00", "count-too-large", id="count-5"),
        pytest.param(BLOB_HEAD + b"\x00\x01\x00", "count-too-large", id="edges-1"),
        pytest.param(
            BLOB_HEAD + b"\x00\x02\x00\x00\x00", "count-too-large", id="edges-2"
        ),
        pytest.param(BLOB_HEAD + _varint(2**40), "count-too-large", id="count-2**40"),
        pytest.param(_Z11 + b"\x03\x00\x01", "count-too-large", id="zephyr-nodes-3"),
        pytest.param(_Z11 + b"\x00\x01\x00", "count-too-large", id="zephyr-edges-1"),
        pytest.param(BLOB_HEAD + b"\x00\x00\x00", "trailing-bytes", id="trailing"),
        pytest.param(_Z11 + b"\x00\x00\x00", "trailing-bytes", id="zephyr-trailing"),
        pytest.param(b"QTOP\x01\x01\x00\x01", "m-range", id="m-0"),
        pytest.param(b"QTOP\x01\x01\x21\x01", "m-range", id="m-33"),
        pytest.param(b"QTOP\x01\x01\x01\x00", "t-range", id="t-0"),
        pytest.param(b"QTOP\x01\x01\x01\x09", "t-range", id="t-9"),
        # z(1, 1) has nodes 0..=11.
        pytest.param(
            b"QTOP\x01\x01\x01\x01\x01\x0c\x00", "not-a-node", id="removed-node-12"
        ),
        pytest.param(_Z11 + b"\x02\x03\x00\x00", "order", id="nodes-descending"),
        # Ascending nodes, then bad edges. (0, 1) and (0, 6) are z(1, 1) couplers.
        pytest.param(
            _Z11 + b"\x00\x02\x00\x00\x00\x01", "order", id="edges-descending-ends"
        ),
        pytest.param(_Z11 + b"\x00\x02\x00\x06\x00\x01", "order", id="edges-unsorted"),
        pytest.param(_Z11 + b"\x00\x02\x00\x01\x00\x01", "order", id="edges-duplicate"),
        pytest.param(
            _Z11 + b"\x00\x02\x00\x01\x00\x00", "order", id="edges-second-below"
        ),
        # (0, 2) is not a coupler of z(1, 1), (0, 1) is.
        pytest.param(_Z11 + b"\x00\x01\x00\x02", "not-a-coupler", id="edge-0-2"),
        # The lattice has 12 nodes, so an edge end of 12 is not a node.
        pytest.param(_Z11 + b"\x00\x01\x00\x0c", "not-a-node", id="edge-end-12"),
        # Removed edge (0, 1) while node 1 is removed.
        pytest.param(
            b"QTOP\x01\x01\x01\x01\x01\x01\x01\x00\x01",
            "edge-at-removed-node",
            id="edge-at-removed-node-1",
        ),
        pytest.param(
            b"QTOP\x01\x01\x01\x01\x01\x00\x01\x00\x01",
            "edge-at-removed-node",
            id="edge-at-removed-node-0",
        ),
        # Two edges. The first end is above the 12 lattice nodes.
        pytest.param(
            _Z11 + b"\x00\x02\x00" + _varint(2**40) + b"\x00\x01",
            "not-a-node",
            id="edge-end-beyond-lattice",
        ),
    ],
)
def test_blob_payload_errors(payload, code):
    assert _payload_error(payload) == code


def test_blob_payload_accepts():
    assert topology_blob.decode_payload(BLOB_HEAD + b"\x00\x00") == _explicit([], [])
    decoded = topology_blob.decode_payload(_Z11 + b"\x00\x01\x00\x01")
    assert decoded["removed_edges"] == [[0, 1]]


def test_blob_oversize_payload_is_rejected(monkeypatch):
    limit = 1024
    monkeypatch.setattr(topology_blob, "MAX_PAYLOAD", limit)
    assert _file_error(_frame(b"\x00" * (limit + 1))) == "too-large"
    # A payload of exactly the limit passes the framing and fails on its magic.
    assert _payload_error(topology_blob.decompress(_frame(b"\x00" * limit))) == "magic"


def test_blob_cli_round_trip(tmp_path, capsys):
    source = tmp_path / "in.json"
    blob = tmp_path / "out.bin.gz"
    body = _zephyr(2, 2, [0, 5], [])
    source.write_text(json.dumps(body))
    root = Path(__file__).parent

    def run(*args):
        code = cli.main(["topology", *args])
        captured = capsys.readouterr()
        return code, captured.out, captured.err

    assert run("encode", str(source), str(blob))[0] == 0
    assert blob.read_bytes() == topology_blob.encode(body)
    code, out, _ = run("decode", str(blob))
    assert json.loads(out) == body
    code, out, _ = run("decode", "--expand", str(blob))
    assert json.loads(out) == topology_blob.expand(body)
    back = tmp_path / "back.json"
    assert run("decode", str(blob), str(back))[0] == 0
    assert json.loads(back.read_text()) == body
    blob.write_bytes(blob.read_bytes()[:-1])
    code, _, err = run("decode", str(blob))
    assert code == 1 and "gzip" in err
    assert run("topology")[0] == 2
    for text in ("[1, 2]", "3", "null", '"x"'):
        source.write_text(text)
        code, _, err = run("encode", str(source), str(blob))
        assert code == 1, text
        assert "shape" in err, text
    source.write_text("[1, 2]")
    failed = subprocess.run(
        [
            sys.executable,
            "-m",
            "family_oracle",
            "topology",
            "encode",
            str(source),
            str(blob),
        ],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert failed.returncode == 1
    assert "shape" in failed.stderr and "Traceback" not in failed.stderr


def test_glass_rejects_a_tile_pair_that_is_not_an_edge(monkeypatch):
    from family_oracle import glass

    nodes, edges = ice.lattice(2, 2)
    params = build._ice_glass_params(2, 2, 125)
    monkeypatch.setattr(
        glass,
        "select",
        lambda *_args: [(0, 1, len(nodes) - 2, len(nodes) - 1)],
    )
    with pytest.raises(ValueError, match="not topology edges"):
        glass.generate(b"quip/ice-glass", 1, params, nodes, edges, bytes(32))


# ---- quip/twisted-ice-dimer ----


def _twisted(m, t, weak, pairs, nodes, edges, nonce=bytes(range(32))):
    from family_oracle import twisted

    params = build._twisted_params(m, t, weak, pairs)
    return twisted.generate(b"quip/twisted-ice-dimer", 1, params, nodes, edges, nonce)


def test_twisted_instance_pairs_and_shortfall():
    nodes, edges = ice.lattice(2, 2)
    code, inst = _twisted(2, 2, 62, 8, nodes, edges)
    assert code is None
    in_tile = {e for tile in inst["tiles"] for e in ice._tile_edges(tile)}
    at = {e: i for i, e in enumerate(inst["edges"])}
    assert len(inst["pairs"]) == 8
    ends = [x for pair in inst["pairs"] for x in pair]
    assert len(set(ends)) == 16
    for pair in inst["pairs"]:
        assert pair not in in_tile
        assert inst["j"][at[pair]] == -1000
    for tile, gauge in zip(inst["tiles"], inst["gauges"], strict=True):
        local = dict(zip(tile, gauge, strict=True))
        for a, b in ice._tile_edges(tile):
            assert inst["j"][at[(a, b)]] == 1000 * local[a] * local[b]
    for edge, value in zip(inst["edges"], inst["j"], strict=True):
        if edge not in in_tile and edge not in inst["pairs"]:
            assert abs(value) == 62
    code, inst = _twisted(2, 2, 62, 0, nodes, edges)
    assert code is None and inst["pairs"] == []
    code, inst = _twisted(2, 2, 62, len(nodes) // 2, nodes, edges)
    assert (code, inst) == (14, None)


def test_twisted_admission_codes():
    from family_oracle import twisted

    nodes, edges = ice.lattice(2, 2)
    half = len(nodes) // 2
    assert (
        twisted.admission_code(build._twisted_params(2, 2, 62, half), nodes, edges)
        is None
    )
    assert (
        twisted.admission_code(build._twisted_params(2, 2, 62, half + 1), nodes, edges)
        == 12
    )
    assert (
        twisted.admission_code(build._twisted_params(2, 2, 1000, 8), nodes, edges) == 9
    )
    assert (
        twisted.admission_code(build._twisted_params(2, 0, 62, 8), nodes, edges) == 12
    )
    assert twisted.admission_code(b"\x01", nodes, edges) == 7


def test_twisted_obstructions_ring_and_errors():
    from family_oracle import twisted

    ring = [[0, 1, 2, 3], [3, 4, 5, 6], [6, 7, 8, 0]]
    ones = [[1] * 4] * 3
    assert twisted.gauge_obstructions(ring, ones, 9)["gauge_equivalent"]
    flipped = [[1, 1, 1, -1], [1] * 4, [1] * 4]
    assert twisted.gauge_obstructions(ring, flipped, 9)["frustrated_cycles"] == 1
    with pytest.raises(ice.StructureError) as err:
        twisted.gauge_obstructions([[0, 1, 2, 3], [3, 4, 5, 6], [3, 7, 8, 9]], ones, 10)
    assert (err.value.name, err.value.detail) == ("membership-above-two", [3])
    with pytest.raises(ice.StructureError, match="overflow"):
        twisted.gauge_obstructions(ring, ones[:2], 9)


# ---- quip/charged-ice ----


def _charged(m, t, scale, background, charged, nodes, edges, nonce=bytes(range(32))):
    from family_oracle import charge

    params = build._charge_params(m, t, scale, background, charged)
    return charge.generate(b"quip/charged-ice", 1, params, nodes, edges, nonce)


def test_charged_instance_and_direct_energy_follow_the_contract_rules():
    nodes, edges = ice.lattice(2, 2)
    inst = _charged(2, 2, 250, 31, 128, nodes, edges)
    in_tile = {e for tile in inst["tiles"] for e in ice._tile_edges(tile)}
    at = {e: i for i, e in enumerate(inst["edges"])}
    expected_h = [0] * len(nodes)
    for tile, gauge, q in zip(
        inst["tiles"], inst["gauges"], inst["charges"], strict=True
    ):
        local = dict(zip(tile, gauge, strict=True))
        assert q in (0, 2, -2)
        for a, b in ice._tile_edges(tile):
            assert inst["j"][at[(a, b)]] == 250 * local[a] * local[b]
        for node in tile:
            expected_h[node] += -250 * q * local[node]
    assert inst["h"] == expected_h
    assert inst["offset_milli"] == sum(125 * (4 + q * q) for q in inst["charges"])
    for edge, value in zip(inst["edges"], inst["j"], strict=True):
        if edge not in in_tile:
            assert abs(value) == 31
    from family_oracle import charge

    spins = charge.energy_states(bytes(range(32)), len(nodes))[3][1]
    direct = charge.direct_energy(inst, spins)
    encoded = charge.encoded_energy(inst, spins)
    assert direct == encoded
    (u, v), _ = next(iter(inst["background_signs"].items()))
    at = inst["edges"].index((u, v))
    broken = dict(inst, j=list(inst["j"]))
    broken["j"][at] = -broken["j"][at]
    assert charge.direct_energy(broken, spins) == direct
    assert charge.encoded_energy(broken, spins) != direct


def test_charged_extremes_and_encoded_equals_direct_for_every_charge():
    from family_oracle import charge

    nodes, edges = ice.lattice(1, 2)
    none = _charged(1, 2, 250, 31, 0, nodes, edges)
    assert set(none["charges"]) == {0}
    full = _charged(0, 0, 100, 45, 256, nodes, edges)
    assert 0 not in full["charges"]
    assert set(full["charges"]) <= {2, -2}
    for inst in (none, full):
        for _, spins in charge.energy_states(bytes(range(32)), len(nodes)):
            assert charge.encoded_energy(inst, spins) == charge.direct_energy(
                inst, spins
            )


def test_charged_single_tile_penalty_for_all_sixteen_states():
    from itertools import product

    from family_oracle import charge

    for q in (0, 2, -2):
        for gauge in ([1, 1, 1, 1], [1, -1, 1, -1]):
            inst = {
                "tiles": [(0, 1, 2, 3)],
                "gauges": [gauge],
                "charges": [q],
                "scale": 250,
                "background_milli": 31,
                "background_signs": {},
                "edges": [(a, b) for a in range(4) for b in range(a + 1, 4)],
                "j": [
                    250 * gauge[a] * gauge[b] for a in range(4) for b in range(a + 1, 4)
                ],
                "h": [-250 * q * g for g in gauge],
                "offset_milli": 125 * (4 + q * q),
            }
            for spins in product((1, -1), repeat=4):
                assert charge.encoded_energy(inst, list(spins)) == charge.direct_energy(
                    inst, list(spins)
                )


def test_charged_admission_codes():
    from family_oracle import charge

    nodes, edges = ice.lattice(2, 2)
    assert (
        charge.admission_code(build._charge_params(2, 2, 250, 31, 128), nodes, edges)
        is None
    )
    assert (
        charge.admission_code(build._charge_params(0, 0, 251, 45, 128), nodes, edges)
        == 9
    )
    assert (
        charge.admission_code(build._charge_params(2, 2, 250, 31, 257), nodes, edges)
        == 9
    )
    assert (
        charge.admission_code(build._charge_params(2, 0, 250, 31, 128), nodes, edges)
        == 12
    )
    assert charge.admission_code(b"\x01", nodes, edges) == 7


_K4 = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)]


def _ice_admission_codes(nodes, edges):
    from family_oracle import charge, twisted

    return (
        charge.admission_code(build._charge_params(0, 0, 250, 31, 128), nodes, edges),
        twisted.admission_code(build._twisted_params(0, 0, 62, 8), nodes, edges),
    )


_STAR = [(0, leaf) for leaf in range(1, 34)]


@pytest.mark.parametrize(
    ("nodes", "edges", "expected"),
    [
        pytest.param(list(range(131_072)), _K4, (None, None), id="node-limit"),
        pytest.param(list(range(131_073)), _K4, (13, 13), id="node-limit-plus-one"),
        pytest.param(list(range(34)), _STAR, (13, 13), id="degree-above-limit"),
        pytest.param(
            list(range(34)), [*_STAR, (0, 0)], (12, 12), id="self-loop-wins-over-degree"
        ),
    ],
)
def test_ice_admission_boundaries(nodes, edges, expected):
    assert _ice_admission_codes(nodes, edges) == expected


# ---- quip/ice-fields ----


def _fields(m, t, glass, field, divisor, nodes, edges, nonce=bytes(range(32))):
    from family_oracle import fields

    params = build._fields_params(m, t, glass, field, divisor)
    return fields.generate(b"quip/ice-fields", 1, params, nodes, edges, nonce)


def test_fields_instance_follows_the_contract_rules():
    nodes, edges = ice.lattice(2, 2)
    code, inst = _fields(2, 2, 125, 500, 16, nodes, edges)
    assert code == 0
    gauge = inst["gauge"]
    in_tile = {e for tile in inst["tiles"] for e in ice._tile_edges(tile)}
    for (a, b), value in zip(inst["edges"], inst["j"], strict=True):
        product = value * gauge[a] * gauge[b]
        assert product == (1000 if (a, b) in in_tile else product)
        if (a, b) not in in_tile:
            assert abs(product) == 125
    seen = [0] * len(nodes)
    for tile in inst["tiles"]:
        for node in tile:
            seen[node] += 1
    nonzero = [i for i, v in enumerate(inst["h"]) if v]
    assert nonzero == inst["selected"]
    assert len(nonzero) == len(nodes) // 16
    assert all(seen[i] == 2 and abs(inst["h"][i]) == 500 for i in nonzero)
    assert inst["offset_milli"] == 0
    _, other = _fields(2, 2, 125, 1000, 8, nodes, edges)
    # The parameters differ, so the descriptor digest and every stream differ. The
    # field count and size follow the parameters.
    assert len(other["selected"]) == 2 * len(inst["selected"])
    assert {abs(v) for v in other["h"] if v} == {1000}


def test_fields_no_fields_and_shortfall():
    nodes, edges = ice.lattice(1, 2)
    code, inst = _fields(1, 2, 125, 500, 25, nodes, edges)
    assert code == 0
    assert set(inst["h"]) == {0}
    k4_nodes, k4_edges = build._three_k4()
    codes = [
        _fields(0, 0, 125, 500, 9, k4_nodes, k4_edges, bytes([i]) * 32)[0]
        for i in range(16)
    ]
    assert set(codes) == {0, 14}


def test_fields_admission_codes():
    from family_oracle import fields

    nodes, edges = ice.lattice(2, 2)

    def code(m, t, glass, field, divisor):
        return fields.admission_code(
            build._fields_params(m, t, glass, field, divisor), nodes, edges
        )

    assert code(2, 2, 125, 500, 4) is None
    assert code(2, 2, 125, 500, 3) == 12
    assert code(2, 2, 125, 1001, 16) == 9
    assert code(2, 2, 1000, 500, 16) == 9
    assert code(2, 2, 125, 500, 0) == 9
    assert code(2, 0, 125, 500, 16) == 12
    assert fields.admission_code(b"\x01", nodes, edges) == 7
    k4_nodes, k4_edges = build._three_k4()
    for divisor, want in ((9, None), (4, 12)):
        params = build._fields_params(0, 0, 125, 500, divisor)
        assert fields.admission_code(params, k4_nodes, k4_edges) == want


# ---- quip/two-commodity-routing ----


def _routing(m, t, divisor, nodes, edges, nonce=bytes(range(32))):
    from family_oracle import routing

    params = build._routing_params(m, t, divisor)
    return routing.generate(
        b"quip/two-commodity-routing", 1, params, nodes, edges, nonce
    )


def test_routing_instance_follows_the_contract_rules():
    nodes, edges = ice.lattice(3, 4)
    code, inst = _routing(3, 4, 8, nodes, edges)
    assert code == 0
    cells, arcs, supply = inst["cells"], inst["arcs"], inst["supply"]
    assert inst["node_ids"] == sorted(inst["node_ids"], key=nodes.index)
    assert len(inst["node_ids"]) == 3 * len(arcs)
    pairs = len(cells) // 8
    for k in (0, 1):
        assert sum(1 for s in supply if s[k] == 1) == pairs
        assert sum(1 for s in supply if s[k] == -1) == pairs
    assert all(not (s[0] and s[1]) for s in supply)
    assert {c for a in arcs for c in a["cost"]} <= {62, 124}
    assert 1 <= inst["strength"] <= 1000
    # The loads are the sum of |J| and |h| of each spin without the chain couplers.
    chain_edge = {(min(a["chain"]), max(a["chain"])) for a in arcs}
    load = [abs(v) for v in inst["h"]]
    for edge, value in zip(inst["edges"], inst["j"], strict=True):
        if edge in chain_edge:
            continue
        load[edge[0]] += abs(value)
        load[edge[1]] += abs(value)
    chain_spins = [s for a in arcs for s in a["chain"]]
    assert inst["strength"] == 1 + max(load[s] for s in chain_spins)
    coupling = dict(zip(inst["edges"], inst["j"], strict=True))
    for edge in chain_edge:
        assert coupling[edge] == -inst["strength"]


def test_routing_energy_is_the_structured_energy():
    from family_oracle import routing

    nodes, edges = ice.lattice(2, 4)
    code, inst = _routing(2, 4, 4, nodes, edges)
    assert code == 0
    arcs, supply = inst["arcs"], inst["supply"]
    n = len(inst["node_ids"])
    import random

    rng = random.Random(7)
    for _ in range(200):
        spins = [rng.choice((-1, 1)) for _ in range(n)]
        x = [(1 - s) // 2 for s in spins]
        total = 0
        for v, b in enumerate(supply):
            for k in (0, 1):
                net = sum(x[routing._spin(a, v, k)] for a in arcs if a["tail"] == v)
                net -= sum(x[routing._spin(a, v, k)] for a in arcs if a["head"] == v)
                total += 248 * (net - b[k]) ** 2
        for a in arcs:
            total += 496 * x[a["x1"]] * x[a["chain"][0]]
            total += a["cost"][0] * x[a["x1"]] + a["cost"][1] * x[a["chain"][0]]
            total -= inst["strength"] * spins[a["chain"][0]] * spins[a["chain"][1]]
        energy = (
            energy_plain(inst["h"], inst["j"], inst["edges"], spins)
            + inst["offset_milli"]
        )
        assert energy == total


def test_routing_admission_codes():
    from family_oracle import routing

    nodes, edges = ice.lattice(3, 4)

    def code(m, t, divisor, nodes=nodes, edges=edges):
        return routing.admission_code(
            build._routing_params(m, t, divisor), nodes, edges
        )

    assert code(3, 4, 8) is None
    assert code(3, 4, 3) == 9
    assert code(3, 2, 8) == 9
    assert code(2, 4, 8) == 12
    assert routing.admission_code(b"\x01", nodes, edges) == 7
    no_arc = build._routing_no_arc()
    lattice = routing._lattice_edge_set(3, 4)
    assert all((min(e), max(e)) in lattice for e in no_arc[1])
    assert routing.template(3, 4, *no_arc) is None
    assert code(3, 4, 8, *no_arc) == 12
    looped = build._routing_self_loop()
    nodes24, edges24 = ice.lattice(2, 4)
    params24 = build._routing_params(2, 4, 4)
    assert routing.admission_code(params24, nodes24, edges24) is None
    assert routing.admission_code(params24, *looped) == 12
    nodes28, edges28 = ice.lattice(2, 8)
    assert (
        routing.admission_code(build._routing_params(2, 8, 4), nodes28, edges28) is None
    )


def test_routing_retry_and_no_instance_cases_exist():
    cases, terminals = build._routing_cases_and_terminals()
    by_name = {c["name"]: c for c in cases}
    assert not by_name["routing-no-instance"]["instances"]
    assert {s["skip_code"] for s in by_name["routing-no-instance"]["skipped"]} == {14}
    assert any(t["try"] > 1 for t in terminals)


# ---- quip/polariton-static ----


def _polariton(m, t, side, pairs, nodes, edges, nonce=bytes(range(32))):
    from family_oracle import polariton

    params = build._polariton_params(m, t, side, pairs)
    return polariton.generate(b"quip/polariton-static", 1, params, nodes, edges, nonce)


def test_polariton_energy_is_the_charge_loss_for_every_l2_state():
    from family_oracle import polariton

    nodes, edges = ice.lattice(2, 4)
    code, inst = _polariton(2, 4, 2, 0, nodes, edges)
    assert code == 0
    assert len(inst["node_ids"]) == 12
    for bits in range(4096):
        spins = [1 if bits >> i & 1 else -1 for i in range(12)]
        seen = polariton.observables(inst, spins)
        assert polariton.encoded_energy(inst, spins) == seen["loss_milli"]
        assert (seen["violations"] == 0) == (seen["loss_milli"] == 0)


def test_polariton_instance_and_witnesses():
    nodes, edges = ice.lattice(3, 4)
    code, inst = _polariton(3, 4, 3, 2, nodes, edges)
    assert code == 0
    labels = inst["labels"]
    assert sum(1 for q in labels if q == 2) == 2
    assert sum(1 for q in labels if q == -2) == 2
    side, width = 3, 4
    for v, q in enumerate(labels):
        x, y = v % width, v // width
        on_border = x in (0, side) or y in (0, side)
        assert (q is None) == on_border
    assert inst["node_ids"] == sorted(inst["node_ids"], key=nodes.index)
    assert sorted(inst["positions"]) == list(range(2 * side * (side + 1)))
    assert inst["window"] == (1, 1)
    assert max(abs(x) for x in (*inst["h"], *inst["j"])) <= 1000
    native = {(min(a, b), max(a, b)) for a, b in edges}
    for a, b in inst["edges"]:
        pair = (inst["node_ids"][a], inst["node_ids"][b])
        assert (min(pair), max(pair)) in native
    from family_oracle import polariton

    good, bad = polariton.witnesses(inst)
    assert polariton.observables(inst, good)["violations"] == 0
    assert polariton.encoded_energy(inst, good) == 0
    seen = polariton.observables(inst, bad)
    assert seen["violations"] > 0
    assert polariton.encoded_energy(inst, bad) == seen["loss_milli"] == 500


def test_polariton_feasibility_finds_the_known_obstruction():
    from family_oracle import polariton

    lattice = polariton.Lattice(7)
    labels = [None] * len(lattice.incident)
    # A 2 by 3 block of +2 needs charge 12 from 10 leaving links.
    for y in (2, 3):
        for x in (2, 3, 4):
            labels[y * lattice.width + x] = 2
    assert polariton.check_request(lattice, labels) is None
    labels[3 * lattice.width + 4] = 0
    assert polariton.check_request(lattice, labels) is not None


def test_polariton_admission_codes():
    from family_oracle import polariton

    z34 = ice.lattice(3, 4)

    def code(m, t, side, pairs, nodes, edges):
        return polariton.admission_code(
            build._polariton_params(m, t, side, pairs), nodes, edges
        )

    assert code(3, 4, 3, 2, *z34) is None
    assert code(3, 4, 3, 3, *z34) == 12
    assert code(2, 4, 3, 0, *ice.lattice(2, 4)) == 12
    assert code(2, 4, 1, 0, *ice.lattice(2, 4)) == 9
    assert code(2, 4, 65, 0, *ice.lattice(2, 4)) == 9
    assert code(2, 4, 2, 2049, *ice.lattice(2, 4)) == 9
    assert polariton.admission_code(b"\x01", *z34) == 7
    assert code(3, 1, 2, 0, *build._polariton_no_window()) == 12
    assert code(3, 1, 3, 2, *build._polariton_dead_cell()) is None
    looped = build._routing_self_loop()
    assert code(3, 4, 3, 2, *looped) == 12


def test_polariton_dead_cell_moves_the_window_and_the_retry_case_retries():
    from family_oracle import polariton

    placed = polariton.place(3, 1, 3, *build._polariton_dead_cell())
    assert (placed["c0"], placed["r0"]) == (1, 2)
    cases, witnesses = build._polariton_cases_and_witnesses()
    by_name = {c["name"]: c for c in cases}
    assert {s["skip_code"] for s in by_name["polariton-retry"]["skipped"]} == {14}
    assert len(witnesses) == 2 * len(cases)
