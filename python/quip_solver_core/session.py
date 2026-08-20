import os
from collections.abc import Iterable, Sequence
from dataclasses import dataclass

from quip.v1 import miner_pb2

# Protocol version this SDK speaks. Welcome.protocol_version must equal this.
PROTOCOL_VERSION = 1

# Configure zeros mean "use the SDK default". These match
# quip_protocol::session::SessionConfig::from_configure.
DEFAULT_QUEUE_DEPTH = 3
DEFAULT_IDLE_TIMEOUT_S = 300
DEFAULT_HEARTBEAT_S = 15
DEFAULT_RECONNECT_WINDOW_S = 60
# Mirrors the Rust session loop's DEFAULT_NUM_SWEEPS in quip-solver-core.
DEFAULT_NUM_SWEEPS = 64


class MissingToken(Exception):
    pass


class BadWelcome(Exception):
    """Welcome.protocol_version is not what this SDK speaks."""

    def __init__(self, version: int):
        self.version = version
        super().__init__(f"unexpected protocol version in Welcome: {version}")


# Sysexits-style process exit codes (also carried on Fatal.exit_code).
EXIT_CLEAN = 0
EXIT_CONFIG_INVALID = 64
EXIT_ENV_INCOMPATIBLE = 69
EXIT_INTERNAL_FATAL = 70
EXIT_TOKEN_REJECTED = 77


@dataclass
class SessionConfig:
    miner_id: str
    queue_depth: int
    idle_timeout_s: int
    heartbeat_s: int
    reconnect_window_s: int
    # Session-wide sampling budget from Configure.backend_toml, applied when a
    # job does not pin its own num_sweeps. Mirrors the Rust session loop's
    # num_sweeps_from_toml (default 64).
    num_sweeps: int


def build_hello(
    miner_id: str,
    backend: str,
    algorithm: str,
    supported_kinds: Iterable[int],
    max_nodes: int,
    max_edges: int,
    features: Sequence[str] | None = None,
    native_topology_hash: bytes | None = None,
) -> miner_pb2.Hello:
    """Build the miner Hello, reading QUIP_SESSION_TOKEN from the env.

    `max_nodes` / `max_edges` of 0 mean unlimited, matching Rust `BackendCaps`.
    """
    token = os.environ.get("QUIP_SESSION_TOKEN")
    if not token:
        raise MissingToken("QUIP_SESSION_TOKEN unset")
    hello = miner_pb2.Hello(
        miner_id=miner_id,
        session_token=token,
        protocol_version=PROTOCOL_VERSION,
        backend=backend,
        algorithm=algorithm,
        supported_kinds=list(supported_kinds),
        max_nodes=max_nodes,
        max_edges=max_edges,
        features=list(features or ()),
    )
    if native_topology_hash is not None:
        hello.native_topology_hash = native_topology_hash
    return hello


def check_welcome(welcome: miner_pb2.Welcome) -> None:
    """Reject a Welcome whose protocol_version is not what this SDK speaks."""
    version = welcome.protocol_version
    if version != PROTOCOL_VERSION:
        raise BadWelcome(version)


def num_sweeps_from_toml(backend_toml: str) -> int:
    """Read the session-wide `num_sweeps` from `Configure.backend_toml`.

    Matches the Rust session loop: only a top-level key counts (a `num_sweeps`
    under a backend's own `[table]` belongs to that backend), and anything
    absent or unusable falls back to DEFAULT_NUM_SWEEPS.
    """
    for line in backend_toml.splitlines():
        stripped = line.split("#", 1)[0].strip()
        if stripped.startswith("["):
            # First table header ends the top-level scope.
            break
        key, sep, value = stripped.partition("=")
        if sep and key.strip() == "num_sweeps":
            try:
                n = int(value.strip())
            except ValueError:
                break
            if n > 0:
                return n
            break
    return DEFAULT_NUM_SWEEPS


def session_config_from_configure(
    miner_id: str, configure: miner_pb2.Configure
) -> SessionConfig:
    def d(v: int, default: int) -> int:
        return default if v == 0 else v

    return SessionConfig(
        miner_id=miner_id,
        queue_depth=d(configure.queue_depth, DEFAULT_QUEUE_DEPTH),
        idle_timeout_s=d(configure.idle_timeout_s, DEFAULT_IDLE_TIMEOUT_S),
        heartbeat_s=d(configure.heartbeat_s, DEFAULT_HEARTBEAT_S),
        reconnect_window_s=d(configure.reconnect_window_s, DEFAULT_RECONNECT_WINDOW_S),
        num_sweeps=num_sweeps_from_toml(configure.backend_toml),
    )
