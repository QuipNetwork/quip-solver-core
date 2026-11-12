"""Golden parity for lease salts, nonce derivation, and the legacy draw.

Runs the golden_lease.json and golden_vectors.json cases through the PyO3
`lease` module. The Rust golden_lease and golden_chacha tests read the same
fixtures, so a pass here means Python and Rust give the same bytes.
"""

import json
from pathlib import Path

import blake3
import pytest
from quip.v1 import miner_pb2
from quip_solver_core import LeaseError, lease, wire

VECTORS = Path(__file__).resolve().parents[1] / "crates/quip-solver-conformance/vectors"
GOLDEN_LEASE = json.loads((VECTORS / "golden_lease.json").read_text())["cases"]
GOLDEN = json.loads((VECTORS / "golden_vectors.json").read_text())

LEGACY = miner_pb2.GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1
FAMILY = miner_pb2.GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1


def _generator(**fields) -> bytes:
    base = {
        "algorithm": LEGACY,
        "last_proof_block_hash": b"\x11" * 32,
        "miner_account": b"\x22" * 32,
        "base_salt": b"\x00" * 8 + b"\x33" * 24,
        "salt_start": 5,
        "salt_count": 4,
    }
    base.update(fields)
    return miner_pb2.IsingProblemGenerator(**base).SerializeToString()


def _golden_generator(case) -> bytes:
    return _generator(
        last_proof_block_hash=bytes.fromhex(case["last_proof_block_hash"]),
        miner_account=bytes.fromhex(case["miner_account"]),
        base_salt=bytes.fromhex(case["base_salt"]),
        salt_start=int(case["salt_start"]),
        salt_count=case["salt_count"],
    )


def _hash(values) -> str:
    return blake3.blake3(wire.encode_i32_le(values)).hexdigest()


@pytest.mark.parametrize("case", GOLDEN_LEASE, ids=lambda c: c["name"])
def test_lease_matches_golden(case):
    spec = lease.Lease(_golden_generator(case))
    assert spec.salt_count == len(case["salts"])
    # The Rust golden_lease test builds the same ring of edges over dense ids.
    n, m = case["n_nodes"], case["n_edges"]
    nodes = list(range(n))
    edges = [(k % n, (k + 1) % n) for k in range(m)]
    for index, vector in enumerate(case["salts"]):
        salt = bytes.fromhex(vector["salt"])
        nonce = bytes.fromhex(vector["nonce"])
        assert spec.salt(index) == salt
        assert spec.index_of(salt) == index
        assert spec.nonce(index) == nonce
        h, j = lease.draw(
            nonce, nodes, edges, case["allowed_h_milli"], case["allowed_j_milli"]
        )
        assert (len(h), len(j)) == (n, m)
        assert _hash(h) == vector["h_blake3"]
        assert _hash(j) == vector["j_blake3"]


@pytest.mark.parametrize("case", GOLDEN["derive_nonce"])
def test_derive_nonce_matches_golden(case):
    nonce = lease.derive_nonce(
        bytes.fromhex(case["last_proof_hex"]),
        bytes.fromhex(case["miner_hex"]),
        bytes.fromhex(case["salt_hex"]),
    )
    assert nonce == bytes.fromhex(case["nonce_hex"])


@pytest.mark.parametrize("case", GOLDEN["ising"])
def test_draw_matches_golden(case):
    h, j = lease.draw(
        bytes.fromhex(case["nonce_hex"]),
        case["nodes"],
        [tuple(e) for e in case["edges"]],
        case["allowed_h_milli"],
        case["allowed_j_milli"],
    )
    assert h == case["h_milli"]
    assert j == case["j_milli"]


def test_draw_maps_native_ids_in_registration_order():
    # Sparse native ids draw the same values as dense positions: only the
    # counts and the order reach the stream.
    case = GOLDEN["ising"][0]
    ids = [100 + 7 * n for n in case["nodes"]]
    edges = [(ids[u], ids[v]) for u, v in case["edges"]]
    args = (case["allowed_h_milli"], case["allowed_j_milli"])
    nonce = bytes.fromhex(case["nonce_hex"])
    assert lease.draw(nonce, ids, edges, *args) == (case["h_milli"], case["j_milli"])


def test_lease_nonce_is_derive_nonce_of_the_salt():
    spec = lease.Lease(_generator())
    for index in range(spec.salt_count):
        assert spec.nonce(index) == lease.derive_nonce(
            b"\x11" * 32, b"\x22" * 32, spec.salt(index)
        )


def test_family_lease_uses_the_legacy_salt_and_nonce_rules():
    family = miner_pb2.FamilyDescriptor(family_id="x", revision=1)
    legacy = lease.Lease(_generator())
    spec = lease.Lease(
        _generator(algorithm=FAMILY, family=family, topology_order_digest=b"\x44" * 32)
    )
    assert [spec.nonce(i) for i in range(4)] == [legacy.nonce(i) for i in range(4)]


def test_salts_outside_the_lease():
    spec = lease.Lease(_generator())
    with pytest.raises(IndexError):
        spec.salt(4)
    with pytest.raises(IndexError):
        spec.nonce(4)
    assert spec.index_of(b"\x09" + b"\x00" * 7 + b"\x33" * 24) is None  # past the end
    assert (
        spec.index_of(b"\x04" + b"\x00" * 7 + b"\x33" * 24) is None
    )  # before the start
    foreign = bytearray(spec.salt(0))
    foreign[20] ^= 1
    assert spec.index_of(bytes(foreign)) is None
    with pytest.raises(ValueError, match="32 bytes"):
        spec.index_of(b"\x05")


@pytest.mark.parametrize(
    ("fields", "name"),
    [
        pytest.param({"salt_count": 0}, "zero_count", id="zero-count"),
        pytest.param(
            {"salt_start": 2**64 - 1, "salt_count": 2},
            "counter_overflow",
            id="overflow",
        ),
        pytest.param({"miner_account": b"\x22" * 31}, "bad_length", id="bad-length"),
        pytest.param({"algorithm": 0}, "unknown_generator", id="unspecified"),
        pytest.param(
            {"topology_order_digest": b"\x44" * 32},
            "descriptor_unexpected",
            id="legacy-with-digest",
        ),
        pytest.param({"algorithm": FAMILY}, "descriptor_missing", id="family-bare"),
    ],
)
def test_invalid_leases_raise_lease_error(fields, name):
    with pytest.raises(LeaseError) as raised:
        lease.Lease(_generator(**fields))
    assert raised.value.name == name
    assert isinstance(raised.value, ValueError)


def test_undecodable_generator_raises_lease_error():
    with pytest.raises(LeaseError) as raised:
        lease.Lease(b"\xff")
    assert raised.value.name == "malformed"


def test_counter_limit_is_accepted():
    spec = lease.Lease(_generator(salt_start=2**64 - 3, salt_count=2))
    assert spec.salt(1)[:8] == (2**64 - 2).to_bytes(8, "little")


@pytest.mark.parametrize(
    ("nodes", "edges", "allowed_h", "name"),
    [
        pytest.param([0, 0], [], [1000], "duplicate_node", id="duplicate"),
        pytest.param([0, 1], [(0, 2)], [1000], "unknown_node", id="unknown"),
        pytest.param([0, 1], [(0, 1)], [], "empty_allowed_values", id="empty"),
    ],
)
def test_draw_faults_raise_lease_error(nodes, edges, allowed_h, name):
    with pytest.raises(LeaseError) as raised:
        lease.draw(b"\x00" * 32, nodes, edges, allowed_h, [1000])
    assert raised.value.name == name


@pytest.mark.parametrize("arg", [0, 1, 2])
def test_derive_nonce_needs_32_byte_inputs(arg):
    inputs = [b"\x00" * 32] * 3
    inputs[arg] = b"\x00" * 31
    with pytest.raises(ValueError, match="32 bytes"):
        lease.derive_nonce(*inputs)
