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


def test_validate_ising_rejects_length_mismatch():
    with pytest.raises(ValueError, match="edges"):
        mock_miner.validate_ising([1.0, -1.0], [1.0], [(0, 1), (0, 0)])


def test_missing_topology_is_not_keyerror():
    ising = miner_pb2.IsingProblem(
        encoding=miner_pb2.COEFFICIENT_ENCODING_I32,
        scale=1000,
        h=bytes(wire.encode_i32_le([1000, -1000])),
        j=bytes(wire.encode_i32_le([500])),
        topology_hash=b"unknown-hash",
        num_reads=1,
    )
    with pytest.raises(mock_miner.MissingTopology):
        mock_miner.decode_problem(ising, {})


def test_sparse_topology_remaps_native_ids():
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
    h, j, edges = mock_miner.decode_problem(ising, {b"sparse": topo})
    assert h == [1.0, 0.0, -1.0]
    assert j == [0.5, 0.25]
    assert edges == [(0, 1), (1, 2)]


def test_inline_edges_stay_dense():
    ising = _ising_from_dense([1000, -1000], [500], [(0, 1)])
    h, j, edges = mock_miner.decode_problem(ising, {})
    assert edges == [(0, 1)]
    assert h == [1.0, -1.0]
    assert j == [0.5]


@pytest.mark.asyncio
async def test_stale_generation_drops_job_without_result():
    call = FakeCall()
    state = mock_miner.Session("mock-0", call)
    state.abandoned_generation = 3
    job = miner_pb2.Job(
        job_id=b"stale",
        kind=miner_pb2.ISING_SAMPLE,
        generation=2,
        ising=_ising_from_dense([1000, -1000], [500], [(0, 1)]),
    )
    await state.handle_job(job)
    kinds = [
        msg.WhichOneof("msg") if msg != "done_writing" else msg for msg in call.written
    ]
    assert "result" not in kinds
    assert "reject" not in kinds
    assert kinds == ["job_request"]


@pytest.mark.asyncio
async def test_generation_zero_is_never_cancelled():
    call = FakeCall()
    state = mock_miner.Session("mock-0", call)
    state.abandoned_generation = 99
    job = miner_pb2.Job(
        job_id=b"mempool",
        kind=miner_pb2.ISING_SAMPLE,
        generation=0,
        ising=_ising_from_dense([1000, -1000], [500], [(0, 1)]),
    )
    await state.handle_job(job)
    kinds = [msg.WhichOneof("msg") for msg in call.written]
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
async def test_hello_advertises_v2_capabilities(monkeypatch):
    monkeypatch.setenv("QUIP_SESSION_TOKEN", "tok-abc")
    call = FakeCall()
    state = mock_miner.Session("mock-0", call)
    await state.send_hello()
    hello = call.written[0].hello
    assert hello.capabilities.protocol_version == 2
    assert hello.capabilities.backend == miner_pb2.BACKEND_MOCK
    assert hello.capabilities.algorithm == miner_pb2.ALGORITHM_SA
    assert list(hello.capabilities.encodings) == [miner_pb2.COEFFICIENT_ENCODING_I32]


@pytest.mark.asyncio
async def test_result_spins_are_bit_packed():
    call = FakeCall()
    state = mock_miner.Session("mock-0", call)
    job = miner_pb2.Job(
        job_id=b"packed",
        kind=miner_pb2.ISING_SAMPLE,
        generation=0,
        ising=_ising_from_dense([1000, -1000], [500], [(0, 1)]),
    )
    await state.handle_job(job)
    result = next(
        msg.result
        for msg in call.written
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
    call = FakeCall()
    state = mock_miner.Session("mock-0", call)
    ising = _ising_from_dense([1000, -1000], [500], [(0, 1)])
    setattr(ising, field, value)
    job = miner_pb2.Job(
        job_id=b"bad-coeff",
        kind=miner_pb2.ISING_SAMPLE,
        generation=0,
        ising=ising,
    )
    await state.handle_job(job)
    rejects = [
        msg.reject
        for msg in call.written
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


@pytest.mark.parametrize(
    "scale,stored,milli", [(1, 1, 1000), (2000, 2, 1), (3, 3, 1000)]
)
def test_accepts_exact_i32_scales(scale, stored, milli):
    ising = _ising_from_dense([stored, -stored], [stored], [(0, 1)])
    ising.scale = scale
    h, j, edges = mock_miner.decode_problem(ising, {})
    assert h == [milli / 1000, -milli / 1000]
    assert j == [milli / 1000]
    assert edges == [(0, 1)]


@pytest.mark.parametrize("stored", [2147484, -2147484])
def test_rejects_milli_overflow(stored):
    ising = _ising_from_dense([stored], [], [])
    ising.scale = 1
    with pytest.raises(ValueError):
        mock_miner.decode_problem(ising, {})
