"""Tests for the family oracle stream and digest primitives."""

import json
from pathlib import Path

import blake3
import pytest
from family_oracle.chacha8 import ChaCha8, shuffle, uniform
from family_oracle.digests import (
    derive_key,
    derive_nonce,
    descriptor_material,
    lease_salts,
    params_digest,
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
