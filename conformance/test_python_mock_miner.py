"""Unit tests for examples/python/mock_miner.py helpers.

Loads the example by path so it does not put ``python/`` on sys.path and
shadow an installed wheel.
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest
from quip.v1 import miner_pb2
from quip_solver_core import wire

ROOT = Path(__file__).resolve().parents[1]
_SPEC = importlib.util.spec_from_file_location(
    "quip_mock_miner", ROOT / "examples" / "python" / "mock_miner.py"
)
assert _SPEC is not None and _SPEC.loader is not None
mock_miner = importlib.util.module_from_spec(_SPEC)
sys.modules["quip_mock_miner"] = mock_miner
_SPEC.loader.exec_module(mock_miner)


class FakeCall:
    def __init__(self) -> None:
        self.written: list = []

    async def write(self, msg) -> None:
        self.written.append(msg)

    async def done_writing(self) -> None:
        self.written.append("done_writing")


def _ising_from_dense(
    h_milli: list[int], j_milli: list[int], edges: list[tuple[int, int]]
):
    return miner_pb2.IsingProblem(
        encoding=miner_pb2.COEFFICIENT_ENCODING_I32,
        scale=1000,
        h=bytes(wire.encode_i32_le(h_milli)),
        j=bytes(wire.encode_i32_le(j_milli)),
        edges=miner_pb2.EdgeList(u=[u for u, _ in edges], v=[v for _, v in edges]),
        num_reads=1,
    )


async def _run_job(job, abandoned_generation=None) -> list:
    call = FakeCall()
    state = mock_miner.Session("mock-0", call)
    if abandoned_generation is not None:
        state.abandoned_generation = abandoned_generation
    await state.handle_job(job)
    return call.written


def test_validate_ising_rejects_length_mismatch():
    with pytest.raises(ValueError, match="edges"):
        mock_miner.validate_ising([1.0, -1.0], [1.0], [(0, 1), (0, 0)])


def _scaled_ising(scale, h, j, edges):
    ising = _ising_from_dense(h, j, edges)
    ising.scale = scale
    return ising


def _unknown_topology_ising():
    return miner_pb2.IsingProblem(
        encoding=miner_pb2.COEFFICIENT_ENCODING_I32,
        scale=1000,
        h=bytes(wire.encode_i32_le([1000, -1000])),
        j=bytes(wire.encode_i32_le([500])),
        topology_hash=b"unknown-hash",
        num_reads=1,
    )


@pytest.mark.parametrize(
    ("ising", "error"),
    [
        pytest.param(
            _unknown_topology_ising(), mock_miner.MissingTopology, id="missing-topology"
        ),
        pytest.param(_scaled_ising(1, [2147484], [], []), ValueError, id="milli-high"),
        pytest.param(_scaled_ising(1, [-2147484], [], []), ValueError, id="milli-low"),
    ],
)
def test_decode_problem_rejects(ising, error):
    with pytest.raises(error):
        mock_miner.decode_problem(ising, {})


def _sparse_case():
    # Native ids [0, 12, 2400] must index h at dense positions 0, 1, 2.
    topo = mock_miner.Topology.from_proto(
        miner_pb2.Topology(
            hash=b"sparse",
            nodes=[0, 12, 2400],
            edges=miner_pb2.EdgeList(u=[0, 12], v=[12, 2400]),
        )
    )
    ising = miner_pb2.IsingProblem(
        encoding=miner_pb2.COEFFICIENT_ENCODING_I32,
        scale=1000,
        h=bytes(wire.encode_i32_le([1000, 0, -1000])),
        j=bytes(wire.encode_i32_le([500, 250])),
        topology_hash=b"sparse",
        num_reads=1,
    )
    return ising, {b"sparse": topo}


@pytest.mark.parametrize(
    ("ising", "topologies", "expected"),
    [
        pytest.param(
            *_sparse_case(),
            ([1.0, 0.0, -1.0], [0.5, 0.25], [(0, 1), (1, 2)]),
            id="sparse-topology-remaps-native-ids",
        ),
        pytest.param(
            _scaled_ising(1, [1, -1], [1], [(0, 1)]),
            {},
            ([1.0, -1.0], [1.0], [(0, 1)]),
            id="scale-1",
        ),
        pytest.param(
            _scaled_ising(2000, [2, -2], [2], [(0, 1)]),
            {},
            ([0.001, -0.001], [0.001], [(0, 1)]),
            id="scale-2000",
        ),
        pytest.param(
            _scaled_ising(3, [3, -3], [3], [(0, 1)]),
            {},
            ([1.0, -1.0], [1.0], [(0, 1)]),
            id="scale-3",
        ),
    ],
)
def test_decode_problem(ising, topologies, expected):
    assert mock_miner.decode_problem(ising, topologies) == expected


@pytest.mark.asyncio
async def test_stale_generation_drops_job_without_result():
    job = miner_pb2.Job(
        job_id=b"stale",
        kind=miner_pb2.ISING_SAMPLE,
        generation=2,
        ising=_ising_from_dense([1000, -1000], [500], [(0, 1)]),
    )
    written = await _run_job(job, abandoned_generation=3)
    kinds = [msg.WhichOneof("msg") if msg != "done_writing" else msg for msg in written]
    assert "result" not in kinds
    assert "reject" not in kinds
    assert kinds == ["job_request"]


@pytest.mark.asyncio
async def test_generation_zero_is_never_cancelled():
    job = miner_pb2.Job(
        job_id=b"mempool",
        kind=miner_pb2.ISING_SAMPLE,
        generation=0,
        ising=_ising_from_dense([1000, -1000], [500], [(0, 1)]),
    )
    written = await _run_job(job, abandoned_generation=99)
    kinds = [msg.WhichOneof("msg") for msg in written]
    assert "result" in kinds


@pytest.mark.asyncio
async def test_bad_welcome_sends_fatal_then_exits_64():
    call = FakeCall()
    state = mock_miner.Session("mock-0", call)
    msg = miner_pb2.CoordMsg(welcome=miner_pb2.Welcome(protocol_version=1))
    code = await state.handle(msg)
    assert code == mock_miner.ExitCode.CONFIG_INVALID
    assert call.written[0].WhichOneof("msg") == "fatal"
    assert call.written[0].fatal.exit_code == 64
    assert call.written[-1] == "done_writing"


@pytest.mark.asyncio
async def test_get_capabilities_and_ping_are_answered():
    call = FakeCall()
    state = mock_miner.Session("mock-0", call)
    await state.handle(miner_pb2.CoordMsg(get_capabilities=miner_pb2.GetCapabilities()))
    await state.handle(miner_pb2.CoordMsg(ping=miner_pb2.Ping()))
    kinds = [msg.WhichOneof("msg") for msg in call.written]
    assert kinds == ["capabilities", "status"]
    caps = call.written[0].capabilities
    assert caps.protocol_version == 2
    assert caps.backend == miner_pb2.BACKEND_MOCK
    assert caps.algorithm == miner_pb2.ALGORITHM_SA
    assert list(caps.encodings) == [miner_pb2.COEFFICIENT_ENCODING_I32]


@pytest.mark.asyncio
async def test_result_spins_are_bit_packed():
    job = miner_pb2.Job(
        job_id=b"packed",
        kind=miner_pb2.ISING_SAMPLE,
        generation=0,
        ising=_ising_from_dense([1000, -1000], [500], [(0, 1)]),
    )
    result = next(
        msg.result
        for msg in await _run_job(job)
        if msg != "done_writing" and msg.WhichOneof("msg") == "result"
    )
    # The sampler returns all-+1. Two spins, LSB first, are bits 0 and 1.
    assert result.solutions[0].spins == bytes([0b0000_0011])


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("encoding", miner_pb2.COEFFICIENT_ENCODING_F64),
        ("scale", 3),
        ("scale", 0),
    ],
)
async def test_rejects_malformed_coefficients(field, value):
    ising = _ising_from_dense([1000, -1000], [500], [(0, 1)])
    setattr(ising, field, value)
    job = miner_pb2.Job(
        job_id=b"bad-coeff",
        kind=miner_pb2.ISING_SAMPLE,
        generation=0,
        ising=ising,
    )
    rejects = [
        msg.reject
        for msg in await _run_job(job)
        if msg != "done_writing" and msg.WhichOneof("msg") == "reject"
    ]
    assert [r.reason for r in rejects] == [miner_pb2.MALFORMED]


def test_capabilities_flag_prints_lowercase_identity_names():
    proc = subprocess.run(
        [
            sys.executable,
            str(ROOT / "examples" / "python" / "mock_miner.py"),
            "--capabilities",
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    assert proc.returncode == 0, proc.stderr
    data = json.loads(proc.stdout)
    assert data == {
        "supportedKinds": ["ISING_SAMPLE"],
        "maxNodes": 100_000,
        "maxEdges": 1_000_000,
        "protocolVersion": 2,
        "streamWidth": 1,
        "encodings": ["COEFFICIENT_ENCODING_I32"],
        "backend": "mock",
        "algorithm": "sa",
        "features": [],
        "generators": [],
    }


def _solve_problem(**overrides):
    problem = {
        "h": [1.0, -1.0],
        "j": [1.0],
        "edges": [[0, 1]],
        "num_reads": 2,
        "num_sweeps": 8,
        "sweeps_per_beta": 1,
        "beta_range": None,
        "seed": 7,
    }
    problem.update(overrides)
    return problem


def test_solve_problem_parses_and_defaults_to_geometric():
    parsed = mock_miner.parse_solve_problem(_solve_problem())
    assert parsed["edges"] == [(0, 1)]
    assert parsed["beta_schedule_type"] == "geometric"


@pytest.mark.parametrize(
    ("overrides", "missing"),
    [
        pytest.param({"edges": [[1, 1]]}, None, id="self-loop"),
        pytest.param({"num_sweep": 1}, None, id="unknown-key"),
        pytest.param({"num_reads": 0}, None, id="zero-reads"),
        pytest.param({"sweeps_per_beta": 0}, None, id="zero-sweeps-per-beta"),
        pytest.param({"seed": -1}, None, id="negative-seed"),
        pytest.param({"beta_schedule_type": "cosine"}, None, id="cosine-schedule"),
        pytest.param({"beta_schedule_type": "linear"}, None, id="linear-schedule"),
        pytest.param({}, "seed", id="missing-seed"),
    ],
)
def test_solve_problem_rejects_malformed_input(overrides, missing):
    problem = _solve_problem(**overrides)
    if missing:
        del problem[missing]
    with pytest.raises(ValueError, match=missing):
        mock_miner.parse_solve_problem(problem)
