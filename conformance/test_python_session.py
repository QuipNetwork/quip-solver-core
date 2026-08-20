import pytest
from quip.v1 import miner_pb2
from quip_solver_core import session
from quip_solver_core._core import ExitCode


def test_hello_requires_token(monkeypatch):
    monkeypatch.delenv("QUIP_SESSION_TOKEN", raising=False)
    with pytest.raises(session.MissingToken):
        session.build_hello(
            "qpu-0", "dwave-qpu", "quantum-anneal", [miner_pb2.ISING_SAMPLE], 100, 200
        )
    monkeypatch.setenv("QUIP_SESSION_TOKEN", "tok-abc")
    h = session.build_hello(
        "qpu-0", "dwave-qpu", "quantum-anneal", [miner_pb2.ISING_SAMPLE], 100, 200
    )
    assert h.session_token == "tok-abc"
    assert h.protocol_version == 1
    assert h.max_nodes == 100
    assert h.max_edges == 200
    assert list(h.features) == []
    assert not h.HasField("native_topology_hash")


def test_hello_optional_features_and_topology_hash(monkeypatch):
    monkeypatch.setenv("QUIP_SESSION_TOKEN", "tok-abc")
    h = session.build_hello(
        "qpu-0",
        "cpu",
        "sa",
        [miner_pb2.ISING_SAMPLE],
        0,
        0,
        features=["streaming"],
        native_topology_hash=b"\x01\x02",
    )
    assert list(h.features) == ["streaming"]
    assert h.native_topology_hash == b"\x01\x02"


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


def test_welcome_rejects_non_v1_protocol_version():
    session.check_welcome(miner_pb2.Welcome(protocol_version=1))
    with pytest.raises(session.BadWelcome) as ei:
        session.check_welcome(miner_pb2.Welcome(protocol_version=2))
    assert ei.value.version == 2


def test_session_exit_codes_match_core():
    assert session.EXIT_CLEAN == ExitCode.CLEAN == 0
    assert session.EXIT_CONFIG_INVALID == ExitCode.CONFIG_INVALID == 64
    assert session.EXIT_ENV_INCOMPATIBLE == ExitCode.ENV_INCOMPATIBLE == 69
    assert session.EXIT_INTERNAL_FATAL == ExitCode.INTERNAL_FATAL == 70
    assert session.EXIT_TOKEN_REJECTED == ExitCode.TOKEN_REJECTED == 77
