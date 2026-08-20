"""Golden-vector parity for the PyO3-backed quip_solver_core primitives.

Runs the same crates/quip-solver-conformance/vectors cases through the
compiled quip_solver_core.scoring / wire that the Rust golden tests consume.
Because the binding is the Rust code, the two cannot diverge.
"""

import json
from pathlib import Path

import pytest
from quip_solver_core._core import scoring, wire

VECTORS = Path(__file__).resolve().parents[1] / "crates/quip-solver-conformance/vectors"
GOLDEN = json.loads((VECTORS / "golden_vectors.json").read_text())


def test_energy_matches_golden():
    for c in GOLDEN["energy"]:
        spins = c["spins"]
        h = [x / 1000 for x in c["h_milli"]]
        j = [x / 1000 for x in c["j_milli"]]
        edges = [tuple(e) for e in c["edges"]]
        assert scoring.energy_milli(spins, h, j, edges) == c["energy_milli"]


def test_diversity_matches_golden():
    for c in GOLDEN["diversity"]:
        assert abs(scoring.set_diversity(c["solutions"]) - c["diversity"]) < 1e-12


def test_wire_roundtrip():
    assert wire.decode_i32_le(wire.encode_i32_le([1, -2, 3])) == [1, -2, 3]
    assert wire.decode_spins(wire.encode_spins([1, -1, 1])) == [1, -1, 1]


def test_decode_i32_bad_length_raises():
    with pytest.raises(ValueError):
        wire.decode_i32_le(b"\x00\x00\x00")


def test_decode_spins_bad_byte_raises():
    # 0x00 is neither 0x01 (+1) nor 0xFF (-1): the PyO3 wire.decode_spins must
    # surface WireError::BadSpinByte as a Python ValueError, not silently accept.
    with pytest.raises(ValueError):
        wire.decode_spins(b"\x00")


def test_energy_milli_j_edges_mismatch_raises():
    # WASM rejects a j/edges length mismatch rather than silently dropping
    # extra couplings. The PyO3 binding must raise the same class of error.
    with pytest.raises(ValueError):
        scoring.energy_milli([1, -1], [1.0, 1.0], [1.0], [(0, 1), (0, 0)])


def test_set_diversity_zero_width_raises():
    # WASM rejects width 0 (it cannot split a flat buffer by zero). Nested
    # Python lists make that an empty inner vector.
    with pytest.raises(ValueError):
        scoring.set_diversity([[], []])


def test_set_diversity_jagged_raises():
    # WASM rejects a solutions buffer that is not a whole multiple of width.
    # Nested Python lists make that a jagged array.
    with pytest.raises(ValueError):
        scoring.set_diversity([[1, -1], [1]])


def test_wire_empty_payload_roundtrip():
    assert wire.decode_i32_le(wire.encode_i32_le([])) == []
    assert wire.decode_spins(wire.encode_spins([])) == []


def test_wire_i32_bounds_roundtrip():
    # i32::MIN / i32::MAX are load-bearing edge values for the LE codec.
    vals = [-2147483648, -1, 0, 1, 2147483647]
    assert wire.decode_i32_le(wire.encode_i32_le(vals)) == vals


def test_positive_sign_convention():
    # spins [+1,-1]; h=[1.0,-0.5]; edge (0,1) J=2.0 -> E = 1 + 0.5 - 2.0 = -0.5 -> -500
    assert scoring.energy_milli([1, -1], [1.0, -0.5], [2.0], [(0, 1)]) == -500


def test_wire_le32_and_spin_encoding():
    # Exact little-endian and spin-byte layouts; the round-trip tests above
    # would still pass a swapped codec as long as encode/decode agree.
    assert wire.encode_i32_le([-1000, 0, 1000]) == bytes(
        [0x18, 0xFC, 0xFF, 0xFF, 0, 0, 0, 0, 0xE8, 3, 0, 0]
    )
    assert wire.encode_spins([1, -1, 1]) == bytes([0x01, 0xFF, 0x01])
    assert wire.decode_spins(bytes([0x01, 0xFF, 0x01])) == [1, -1, 1]


def test_truncation_matches_golden():
    # The golden `truncation` section pins truncation toward zero
    # cross-language; cases are chosen so int() != round(). Python's
    # int(e*1000) must match the committed (truncated) energy_milli, and the
    # matching Rust test (golden_scoring.rs) asserts the identical values.
    for c in GOLDEN["truncation"]:
        assert int(c["energy"] * 1000) == c["energy_milli"]


def test_energy_oob_edge_is_skipped_not_raising():
    # edge (0, 5) references node 5, out of range for a 2-spin problem; must be
    # skipped like a length-mismatched h/j entry, not raise IndexError.
    # E = (1*1) + (1*-1) = 0 -> 0 milli
    assert scoring.energy_milli([1, -1], [1.0, 1.0], [1.0], [(0, 5)]) == 0


def test_energy_rounds_sub_milli_input_to_nearest():
    # E = 0.0015 -> 2 milli. energy_milli recovers each coefficient's integer
    # milli by rounding to nearest, which is what makes it agree with the
    # coordinator's integer re-score; it no longer truncates a f64 accumulator.
    # Mirrors Rust's energy_rounds_sub_milli_input_to_nearest.
    assert scoring.energy_milli([1], [0.0015], [], []) == 2


def test_energy_rounding_matches_golden():
    # Same section the Rust energy_rounding_matches_golden test pins; the
    # binding calls the identical Rust entry point, so the two cannot drift.
    for c in GOLDEN["energy_rounding"]:
        assert scoring.energy_milli([1], [c["energy"]], [], []) == c["energy_milli"]


def test_sentinel_matches_golden():
    # Non-finite coefficients yield the shared 1 << 62 sentinel; the fixture
    # spells inf/-inf/nan as strings because JSON has no literal for them.
    for c in GOLDEN["sentinel"]:
        h = [float(v) if isinstance(v, str) else v for v in c["h"]]
        j = [float(v) if isinstance(v, str) else v for v in c["j"]]
        edges = [tuple(e) for e in c["edges"]]
        assert scoring.energy_milli(c["spins"], h, j, edges) == c["energy_milli"], c[
            "name"
        ]


def test_energy_milli_saturates_at_i64_boundary():
    # Python's int(e*1000) is unbounded and even raises OverflowError for very
    # large e; Rust's `(e * 1000.0) as i64` saturates. Python must replicate
    # Rust's saturating cast so cross-language scores agree at the boundary.
    i64_max = (1 << 63) - 1
    assert scoring.energy_milli([1], [1e16], [], []) == i64_max
    # Must not raise (Python's bare int() would raise OverflowError here).
    assert scoring.energy_milli([1], [1e308], [], []) == i64_max


def test_energy_milli_saturates_at_negative_i64_boundary():
    i64_min = -(1 << 63)
    assert scoring.energy_milli([1], [-1e16], [], []) == i64_min


def test_energy_milli_rejects_negative_edge_index():
    # scoring is now the PyO3 binding to Rust, whose edge indices are usize and
    # cannot be negative. A negative index is a type error, not a silent skip —
    # stricter than the old pure-Python guard and unreachable from the wire
    # (which decodes u32 -> non-negative). This enforces Rust's contract.
    with pytest.raises((OverflowError, ValueError)):
        scoring.energy_milli([1, -1], [], [1.0], [(-1, 0)])
