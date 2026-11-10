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


def _energy_args(c):
    h = [x / 1000 for x in c["h_milli"]]
    j = [x / 1000 for x in c["j_milli"]]
    return c["spins"], h, j, [tuple(e) for e in c["edges"]]


def _rounding_args(c):
    return [1], [c["energy"]], [], []


def _sentinel_args(c):
    h = [float(v) if isinstance(v, str) else v for v in c["h"]]
    j = [float(v) if isinstance(v, str) else v for v in c["j"]]
    return c["spins"], h, j, [tuple(e) for e in c["edges"]]


# The rounding section is the one the Rust energy_rounding_matches_golden test pins;
# the binding calls the identical Rust entry point, so the two cannot drift. The
# sentinel section yields the shared 1 << 62 sentinel for non-finite coefficients; the
# fixture spells inf/-inf/nan as strings because JSON has no literal for them.
@pytest.mark.parametrize(
    ("section", "args"),
    [
        pytest.param("energy", _energy_args, id="energy"),
        pytest.param("energy_rounding", _rounding_args, id="rounding"),
        pytest.param("sentinel", _sentinel_args, id="sentinel"),
    ],
)
def test_energy_matches_golden(section, args):
    for c in GOLDEN[section]:
        assert scoring.energy_milli(*args(c)) == c["energy_milli"], c.get("name")


def test_diversity_matches_golden():
    for c in GOLDEN["diversity"]:
        assert abs(scoring.set_diversity(c["solutions"]) - c["diversity"]) < 1e-12


@pytest.mark.parametrize(
    ("encode", "decode", "values"),
    [
        pytest.param(wire.encode_i32_le, wire.decode_i32_le, [1, -2, 3], id="i32"),
        pytest.param(wire.encode_spins, wire.decode_spins, [1, -1, 1], id="spins"),
        pytest.param(wire.encode_i32_le, wire.decode_i32_le, [], id="i32-empty"),
        pytest.param(wire.encode_spins, wire.decode_spins, [], id="spins-empty"),
        # i32::MIN / i32::MAX are load-bearing edge values for the LE codec.
        pytest.param(
            wire.encode_i32_le,
            wire.decode_i32_le,
            [-2147483648, -1, 0, 1, 2147483647],
            id="i32-bounds",
        ),
    ],
)
def test_wire_round_trips(encode, decode, values):
    assert decode(encode(values)) == values


def test_decode_i32_bad_length_raises():
    with pytest.raises(ValueError):
        wire.decode_i32_le(b"\x00\x00\x00")


def test_decode_spins_bad_byte_raises():
    # 0x00 is neither 0x01 (+1) nor 0xFF (-1): the PyO3 wire.decode_spins must
    # surface WireError::BadSpinByte as a Python ValueError, not silently accept.
    with pytest.raises(ValueError):
        wire.decode_spins(b"\x00")


def test_packed_spins_round_trip_one_eight_and_nine():
    for spins in (
        [1],
        [1, -1, -1, -1, -1, -1, -1, 1],
        [1, -1, -1, -1, -1, -1, -1, 1, -1],
    ):
        assert (
            wire.decode_spins_packed(wire.encode_spins_packed(spins), len(spins))
            == spins
        )


def test_decode_spins_packed_wrong_length_raises():
    # 8 spins occupy one byte. Two bytes is WireError::BadPackedLength.
    with pytest.raises(ValueError):
        wire.decode_spins_packed(b"\x00\x00", 8)


@pytest.mark.parametrize(
    ("args", "errors"),
    [
        # WASM rejects a j/edges length mismatch rather than silently dropping
        # extra couplings. The PyO3 binding must raise the same class of error.
        pytest.param(
            ([1, -1], [1.0, 1.0], [1.0], [(0, 1), (0, 0)]),
            ValueError,
            id="j-edges-mismatch",
        ),
        # scoring is now the PyO3 binding to Rust, whose edge indices are usize and
        # cannot be negative. A negative index is a type error, not a silent skip,
        # stricter than the old pure-Python guard and unreachable from the wire
        # (which decodes u32 -> non-negative). This enforces Rust's contract.
        pytest.param(
            ([1, -1], [], [1.0], [(-1, 0)]),
            (OverflowError, ValueError),
            id="negative-edge-index",
        ),
    ],
)
def test_energy_milli_rejects_malformed_input(args, errors):
    with pytest.raises(errors):
        scoring.energy_milli(*args)


@pytest.mark.parametrize(
    "solutions",
    [
        # WASM rejects width 0 (it cannot split a flat buffer by zero). Nested
        # Python lists make that an empty inner vector.
        pytest.param([[], []], id="zero-width"),
        # WASM rejects a solutions buffer that is not a whole multiple of width.
        # Nested Python lists make that a jagged array.
        pytest.param([[1, -1], [1]], id="jagged"),
    ],
)
def test_set_diversity_rejects_malformed_solutions(solutions):
    with pytest.raises(ValueError):
        scoring.set_diversity(solutions)


@pytest.mark.parametrize(
    ("spins", "h", "j", "edges", "expected"),
    [
        # spins [+1,-1]; h=[1.0,-0.5]; edge (0,1) J=2.0 -> E = 1 + 0.5 - 2.0 = -0.5
        pytest.param([1, -1], [1.0, -0.5], [2.0], [(0, 1)], -500, id="sign-convention"),
        # edge (0, 5) references node 5, out of range for a 2-spin problem; must be
        # skipped like a length-mismatched h/j entry, not raise IndexError.
        # E = (1*1) + (1*-1) = 0 -> 0 milli
        pytest.param([1, -1], [1.0, 1.0], [1.0], [(0, 5)], 0, id="out-of-range-edge"),
        # E = 0.0015 -> 2 milli. energy_milli recovers each coefficient's integer
        # milli by rounding to nearest, which is what makes it agree with the
        # coordinator's integer re-score; it no longer truncates a f64 accumulator.
        # Mirrors Rust's energy_rounds_sub_milli_input_to_nearest.
        pytest.param([1], [0.0015], [], [], 2, id="sub-milli-rounds-to-nearest"),
    ],
)
def test_energy_milli_small_cases(spins, h, j, edges, expected):
    assert scoring.energy_milli(spins, h, j, edges) == expected


def test_wire_le32_and_spin_encoding():
    # Exact little-endian and spin-byte layouts; the round-trip tests above
    # would still pass a swapped codec as long as encode/decode agree.
    assert wire.encode_i32_le([-1000, 0, 1000]) == bytes(
        [0x18, 0xFC, 0xFF, 0xFF, 0, 0, 0, 0, 0xE8, 3, 0, 0]
    )
    assert wire.encode_spins([1, -1, 1]) == bytes([0x01, 0xFF, 0x01])
    assert wire.decode_spins(bytes([0x01, 0xFF, 0x01])) == [1, -1, 1]


# Python's int(e*1000) is unbounded and even raises OverflowError for very large e;
# Rust's `(e * 1000.0) as i64` saturates. Python must replicate Rust's saturating
# cast so cross-language scores agree at the boundary. The 1e308 row must not raise
# (Python's bare int() would raise OverflowError there).
@pytest.mark.parametrize(
    ("h", "expected"),
    [
        pytest.param(1e16, (1 << 63) - 1, id="above-i64-max"),
        pytest.param(1e308, (1 << 63) - 1, id="far-above-i64-max"),
        pytest.param(-1e16, -(1 << 63), id="below-i64-min"),
    ],
)
def test_energy_milli_saturates(h, expected):
    assert scoring.energy_milli([1], [h], [], []) == expected
