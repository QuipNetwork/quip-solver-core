"""Tests for the family oracle stream and digest primitives."""

import json
from pathlib import Path

import blake3
import pytest
from family_oracle import decisions
from family_oracle.chacha8 import ChaCha8, shuffle, uniform
from family_oracle.digests import (
    derive_key,
    derive_nonce,
    descriptor_material,
    lease_salts,
    params_digest,
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


def test_uniform_n_one_reads_one_word():
    rng = ChaCha8(bytes(32))
    assert uniform(rng, 1) == 0
    assert rng.words_used == 1


def test_uniform_redraws_at_limit():
    rng = _StubRng([2**32 - 1, 7])
    assert uniform(rng, 2**31 + 1) == 7
    assert rng.words_used == 2


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


def test_derive_key_differs_from_plain_hash():
    m = b"material"
    assert derive_key("quip family params v1", m) != blake3.blake3(m).digest()


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


def test_all_types_length_boundary():
    data = encode_params(ALL_TYPES, _all_types_values(b"\x00" * 4013, b""))
    assert len(data) == 4096
    code, values = decode_params(ALL_TYPES, data)
    assert code is None
    assert values == _all_types_values(b"\x00" * 4013, b"")
    data = encode_params(ALL_TYPES, _all_types_values(b"\x00" * 4014, b""))
    assert decode_params(ALL_TYPES, data) == (7, None)


def test_layout_fault_wins_over_bounds():
    bad = encode_params(ALL_TYPES, _all_types_values(b"", b"a" * 64 + b"\x7f"))
    assert decode_params(ALL_TYPES, bad) == (7, None)
    long_ok = encode_params(ALL_TYPES, _all_types_values(b"", b"a" * 65))
    assert decode_params(ALL_TYPES, long_ok) == (9, None)
    # a_u64 = 0 is out of bounds (1..), but a bad bool byte is a layout fault.
    values = _all_types_values(b"", b"")
    values["a_u64"] = 0
    assert decode_params(ALL_TYPES, encode_params(ALL_TYPES, values)) == (9, None)
    values["a_bool"] = 2
    assert decode_params(ALL_TYPES, encode_params(ALL_TYPES, values)) == (7, None)


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
    bounds = example_size_bounds(2, 100, 150, 4)
    assert bounds == (200, 400, 5)
    assert size_bound_code(bounds, None, None, 4) == 13
    assert size_bound_code(bounds, None, None, 0) is None
    assert size_bound_code(bounds, None, None, None) is None
    assert size_bound_code(bounds, 0, 0, 0) is None
    assert size_bound_code(bounds, 199, None, None) == 13
    assert size_bound_code(bounds, None, 399, None) == 13
    assert size_bound_code(bounds, 200, 400, 5) is None


def test_size_bounds_instance_limits():
    def code(*args):
        return size_bound_code(example_size_bounds(*args), None, None, None)

    assert code(2, 8388608, 0, 0) is None
    assert code(2, 8388609, 0, 0) == 13
    assert code(1, 2, 67108864, 1) is None
    assert code(1, 2, 67108865, 1) == 13
    assert code(16, 4294967295, 0, 0) == 13


def test_size_bounds_u64_overflow():
    huge = example_size_bounds(2**40, 2**30, 0, 0)
    assert size_bound_code(huge, None, None, None) == 13
    assert size_bound_code((2**64, 0, 0), None, None, None) == 13
    assert size_bound_code((0, 0, 2**64), None, None, None) == 13


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


def test_archived_nonce_is_zero_filled():
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


def test_order_lengths():
    assert len(decisions.LEASE_ADMISSION_ORDER) == 22
    assert len(decisions.EXPLICIT_JOB_ORDER) == 8


def _lease(faults: list[str]) -> tuple[str, int | None, str | None]:
    step = decisions.winner(decisions.LEASE_ADMISSION_ORDER, faults)
    code, wire = decisions.step_outcome(step, "admission")
    return step, code, wire


def _job(faults: list[str]) -> tuple[str, int | None, str | None]:
    step = decisions.winner(decisions.EXPLICIT_JOB_ORDER, faults)
    code, wire = decisions.step_outcome(step, "explicit-job")
    return step, code, wire


def test_lease_admission_winners():
    assert _lease(["code:1", "session:expired"]) == (
        "session:expired",
        None,
        "EXPIRED",
    )
    assert _lease(["code:1", "field:salt-count-zero"]) == (
        "field:salt-count-zero",
        None,
        "MALFORMED",
    )
    assert _lease(["session:topology-hash-mismatch", "code:10"]) == (
        "code:10",
        10,
        "FAMILY_UNSUPPORTED",
    )
    assert _lease(["code:11", "session:topology-hash-mismatch"]) == (
        "session:topology-hash-mismatch",
        None,
        "TOPOLOGY_MISMATCH",
    )
    assert _lease(["code:13", "code:11"])[1] == 11
    assert _lease(["code:5"]) == ("code:5", 5, "FAMILY_UNSUPPORTED")


def test_explicit_job_winners():
    assert _job(["tier:not-listed", "code:15"]) == (
        "tier:not-listed",
        None,
        None,
    )
    assert _job(["code:15", "code:13"]) == ("code:13", 13, "TOO_LARGE")
    assert _job(["code:16", "code:8"])[1] == 8
    assert _job(["code:16"]) == ("code:16", 16, "MALFORMED")


def test_winner_rejects_bad_input():
    with pytest.raises(ValueError):
        decisions.winner(decisions.LEASE_ADMISSION_ORDER, [])
    with pytest.raises(ValueError):
        decisions.winner(decisions.LEASE_ADMISSION_ORDER, ["code:99"])


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
