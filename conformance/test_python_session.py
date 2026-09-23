import pytest
from quip.v1 import miner_pb2
from quip_solver_core import session
from quip_solver_core._core import ExitCode


def _sample_capabilities() -> miner_pb2.Capabilities:
    return miner_pb2.Capabilities(
        backend=miner_pb2.BACKEND_DWAVE_QPU,
        algorithm=miner_pb2.ALGORITHM_QUANTUM_ANNEAL,
        supported_kinds=[miner_pb2.ISING_SAMPLE],
        max_nodes=100,
        max_edges=200,
        protocol_version=2,
    )


def test_hello_requires_token(monkeypatch):
    caps = _sample_capabilities()
    monkeypatch.delenv("QUIP_SESSION_TOKEN", raising=False)
    with pytest.raises(session.MissingToken):
        session.build_hello("qpu-0", caps)
    monkeypatch.setenv("QUIP_SESSION_TOKEN", "tok-abc")
    h = session.build_hello("qpu-0", caps)
    assert h.miner_id == "qpu-0"
    assert h.session_token == "tok-abc"
    assert h.capabilities.protocol_version == 2
    assert h.capabilities.max_nodes == 100
    assert h.capabilities.max_edges == 200
    assert list(h.capabilities.features) == []
    assert not h.capabilities.HasField("native_topology_hash")


def test_hello_optional_features_and_topology_hash(monkeypatch):
    monkeypatch.setenv("QUIP_SESSION_TOKEN", "tok-abc")
    caps = miner_pb2.Capabilities(
        backend=miner_pb2.BACKEND_CPU,
        algorithm=miner_pb2.ALGORITHM_SA,
        supported_kinds=[miner_pb2.ISING_SAMPLE],
        features=["streaming"],
        protocol_version=2,
        native_topology_hash=b"\x01\x02",
    )
    h = session.build_hello("qpu-0", caps)
    assert list(h.capabilities.features) == ["streaming"]
    assert h.capabilities.native_topology_hash == b"\x01\x02"
    assert h.capabilities.protocol_version == 2


def test_configure_defaults():
    c = miner_pb2.Configure()  # all zero
    cfg = session.session_config_from_configure("qpu-0", c)
    assert (
        cfg.queue_depth,
        cfg.idle_timeout_s,
        cfg.heartbeat_s,
        cfg.reconnect_window_s,
    ) == (
        3,
        300,
        15,
        60,
    )
    assert session.DEFAULT_QUEUE_DEPTH == 3
    assert session.DEFAULT_IDLE_TIMEOUT_S == 300
    assert session.DEFAULT_HEARTBEAT_S == 15
    assert session.DEFAULT_RECONNECT_WINDOW_S == 60


def test_welcome_rejects_other_protocol_version():
    session.check_welcome(miner_pb2.Welcome(protocol_version=2))
    with pytest.raises(session.BadWelcome) as ei:
        session.check_welcome(miner_pb2.Welcome(protocol_version=1))
    assert ei.value.version == 1


def test_identity_names_match_the_rust_map():
    assert session.PROTOCOL_VERSION == 2
    assert session.BACKEND_NAMES == {
        miner_pb2.BACKEND_UNSPECIFIED: "unspecified",
        miner_pb2.BACKEND_CPU: "cpu",
        miner_pb2.BACKEND_CUDA: "cuda",
        miner_pb2.BACKEND_METAL: "metal",
        miner_pb2.BACKEND_ANE: "ane",
        miner_pb2.BACKEND_DWAVE_QPU: "dwave-qpu",
        miner_pb2.BACKEND_EXEC: "exec",
        miner_pb2.BACKEND_MOCK: "mock",
    }
    assert session.ALGORITHM_NAMES == {
        miner_pb2.ALGORITHM_UNSPECIFIED: "unspecified",
        miner_pb2.ALGORITHM_SA: "sa",
        miner_pb2.ALGORITHM_GIBBS: "gibbs",
        miner_pb2.ALGORITHM_QUANTUM_ANNEAL: "quantum-anneal",
        miner_pb2.ALGORITHM_FSA: "fsa",
        miner_pb2.ALGORITHM_MSA: "msa",
        miner_pb2.ALGORITHM_FLATIRON: "flatiron",
        miner_pb2.ALGORITHM_MPS: "mps",
        miner_pb2.ALGORITHM_MFA: "mfa",
        miner_pb2.ALGORITHM_SB: "sb",
        miner_pb2.ALGORITHM_BSB: "bsb",
        miner_pb2.ALGORITHM_GBSB: "gbsb",
        miner_pb2.ALGORITHM_GDSB: "gdsb",
        miner_pb2.ALGORITHM_GGDSB: "ggdsb",
        miner_pb2.ALGORITHM_HBSB: "hbsb",
        miner_pb2.ALGORITHM_HDSB: "hdsb",
        miner_pb2.ALGORITHM_SBQA: "sbqa",
        miner_pb2.ALGORITHM_TEDSB: "tedsb",
        miner_pb2.ALGORITHM_EXTERNAL: "external",
    }


def test_session_exit_codes_match_core():
    assert session.EXIT_CLEAN == ExitCode.CLEAN == 0
    assert session.EXIT_CONFIG_INVALID == ExitCode.CONFIG_INVALID == 64
    assert session.EXIT_ENV_INCOMPATIBLE == ExitCode.ENV_INCOMPATIBLE == 69
    assert session.EXIT_INTERNAL_FATAL == ExitCode.INTERNAL_FATAL == 70
    assert session.EXIT_TOKEN_REJECTED == ExitCode.TOKEN_REJECTED == 77
