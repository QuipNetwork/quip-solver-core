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
