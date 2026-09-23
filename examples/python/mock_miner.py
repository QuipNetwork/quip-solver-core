#!/usr/bin/env python3
"""A complete mock Quip solver in Python.

The ``quip-solver-core`` wheel carries the consensus primitives and the
generated gRPC stubs, but not the session loop, so this file writes the state
machine itself as SPEC section 9 describes. Everything consensus-critical --
energy scoring and the spin wire encoding -- comes from the wheel's compiled
Rust, never from a local reimplementation, because the network recomputes both
and rejects a mismatch.

The session runs on asyncio through ``grpc.aio``. Awaiting each write is what
makes the shutdown path correct with no extra machinery: by the time
``Shutdown`` arrives, every earlier Reject and JobRequest has already gone out,
so half-closing the stream cannot drop one.

The sampler is deliberately trivial: it returns ``num_reads`` copies of the
all-(+1) configuration.

Modes, per SPEC section 2:

    --capabilities   print the capabilities JSON and exit
    --solve          read one problem as JSON on stdin, write solutions on stdout
    --check          probe the device and exit
    (session)        --quip-coordinator unix://<path> --miner-id <id>
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
import time
from dataclasses import dataclass, field

import grpc
from google.protobuf.json_format import MessageToDict
from quip_solver_core import ExitCode, miner_pb2, miner_pb2_grpc, scoring, session, wire

BACKEND = "mock-python"
MAX_NODES = 100_000
MAX_EDGES = 1_000_000
PROTOCOL_VERSION = 2


def capabilities_message() -> miner_pb2.Capabilities:
    """What this solver supports. Must answer without touching the device."""
    return miner_pb2.Capabilities(
        backend=miner_pb2.BACKEND_MOCK,
        algorithm=miner_pb2.ALGORITHM_SA,
        supported_kinds=[miner_pb2.ISING_SAMPLE],
        max_nodes=MAX_NODES,
        max_edges=MAX_EDGES,
        features=[],
        protocol_version=PROTOCOL_VERSION,
        stream_width=1,
        encodings=[miner_pb2.COEFFICIENT_ENCODING_I32],
    )


class MissingTopology(Exception):
    """Job names a topology this session never received."""


def sample(
    h: list[float],
    j: list[float],
    edges: list[tuple[int, int]],
    num_reads: int,
) -> list[tuple[list[int], int]]:
    """Return ``num_reads`` solutions, each scored by the consensus scorer."""
    spins = [1] * len(h)
    energy = scoring.energy_milli(spins, h, j, edges)
    return [(spins, energy) for _ in range(num_reads)]


@dataclass
class Topology:
    """A cached ``Topology`` a job may reference by hash instead of by edges.

    ``pos`` maps each native (possibly sparse) node id to its dense position in
    ``nodes``, matching Rust ``TopologyCache``. Scoring always uses dense
    indices into ``h``.
    """

    nodes: list[int] = field(default_factory=list)
    u: list[int] = field(default_factory=list)
    v: list[int] = field(default_factory=list)
    pos: dict[int, int] = field(default_factory=dict)

    @classmethod
    def from_proto(cls, topo: miner_pb2.Topology) -> Topology:
        nodes = list(topo.nodes)
        edges = topo.edges
        return cls(
            nodes=nodes,
            u=list(edges.u),
            v=list(edges.v),
            pos={node: i for i, node in enumerate(nodes)},
        )

    def dense_edges(self) -> list[tuple[int, int]]:
        """Remap native ``u``/``v`` ids onto dense positions in ``h``."""
        out: list[tuple[int, int]] = []
        for a, b in zip(self.u, self.v, strict=True):
            try:
                out.append((self.pos[a], self.pos[b]))
            except KeyError as exc:
                raise ValueError(
                    "edge references a node not in topology.nodes"
                ) from exc
        return out


def validate_ising(
    h: list[float], j: list[float], edges: list[tuple[int, int]]
) -> None:
    """Length rules shared by session jobs and ``--solve`` JSON.

    A silent truncation here would submit a confidently wrong energy.
    """
    if len(edges) != len(j):
        raise ValueError(f"{len(edges)} edges but {len(j)} couplings")
    n = len(h)
    for a, b in edges:
        if a < 0 or b < 0 or a >= n or b >= n:
            raise ValueError("edge references a node outside h")


def decode_problem(ising, topologies: dict[bytes, Topology]):
    """Decode an ``IsingProblem`` into ``(h, j, edges)`` in float units.

    Raises ``ValueError`` for anything malformed, which the caller turns into
    ``Reject{MALFORMED}``. Raises ``MissingTopology`` when the job names a
    hash this session never cached.
    """
    # I32 at scale 1000 is the only coefficient form this sample decodes.
    if ising.encoding != miner_pb2.COEFFICIENT_ENCODING_I32 or ising.scale != 1000:
        raise ValueError("coefficients must be I32 at scale 1000")
    if len(ising.h) % 4 != 0:
        raise ValueError("h length is not a multiple of 4")
    if len(ising.j) % 4 != 0:
        raise ValueError("j length is not a multiple of 4")

    # Decoding through the wheel keeps Python byte-identical with Rust.
    h = [v / 1000.0 for v in wire.decode_i32_le(ising.h)]
    j = [v / 1000.0 for v in wire.decode_i32_le(ising.j)]

    which = ising.WhichOneof("graph")
    if which == "edges":
        # Inline edges are already dense 0..n-1 ids, same as the Rust path.
        u_list, v_list = list(ising.edges.u), list(ising.edges.v)
        if len(u_list) != len(v_list):
            raise ValueError("edge list halves differ in length")
        edges = list(zip(u_list, v_list, strict=True))
    elif which == "topology_hash":
        topo = topologies.get(bytes(ising.topology_hash))
        if topo is None:
            raise MissingTopology("job references a topology this session never sent")
        if len(topo.u) != len(topo.v):
            raise ValueError("edge list halves differ in length")
        edges = topo.dense_edges()
    else:
        raise ValueError("job carries neither edges nor a topology hash")

    validate_ising(h, j, edges)
    return h, j, edges


def build_solution(spins: list[int], energy_milli: int) -> miner_pb2.Solution:
    """Pack one solution. ``encode_spins_packed`` is the wheel's Rust codec."""
    return miner_pb2.Solution(
        spins=bytes(wire.encode_spins_packed(spins)),
        energy_milli=energy_milli,
    )


class Session:
    """The solver side of the bidirectional stream.

    Every send is awaited, so ordering and flushing need no separate queue.
    """

    def __init__(self, miner_id: str, call) -> None:
        self.miner_id = miner_id
        self.call = call
        self.topologies: dict[bytes, Topology] = {}
        self.config = None
        self.jobs_done = 0
        self.abandoned_generation = 0
        self.got_welcome = False
        self.got_shutdown = False

    async def send_hello(self) -> None:
        hello = session.build_hello(self.miner_id, capabilities_message())
        await self.call.write(miner_pb2.MinerMsg(hello=hello))

    async def send_fatal(self, exit_code: int, reason: str) -> None:
        """Emit Fatal on the stream before a non-clean process exit."""
        await self.call.write(
            miner_pb2.MinerMsg(
                fatal=miner_pb2.Fatal(exit_code=exit_code, reason=reason)
            )
        )
        await self.call.done_writing()

    async def send_status(self) -> None:
        await self.call.write(
            miner_pb2.MinerMsg(
                status=miner_pb2.Status(
                    miner_id=self.miner_id,
                    utilization=0.0,
                    jobs_done=self.jobs_done,
                    abandoned_generation=self.abandoned_generation,
                )
            )
        )

    async def send_credit(self, credits: int = 1) -> None:
        await self.call.write(
            miner_pb2.MinerMsg(job_request=miner_pb2.JobRequest(credits=credits))
        )

    async def reject(self, job_id: bytes, reason) -> None:
        """Decline one job and refund the credit it consumed."""
        await self.call.write(
            miner_pb2.MinerMsg(reject=miner_pb2.Reject(job_id=job_id, reason=reason))
        )
        await self.send_credit()

    async def handle_job(self, job: miner_pb2.Job) -> None:
        """Dispose of exactly one job: a Result, or a Reject with a reason."""
        job_id = bytes(job.job_id)

        # Generation 0 is a mempool job: Cancel never abandons it (SPEC §5).
        # A later job whose generation is at or below the watermark must
        # produce no Result — only a credit refund so the coordinator pool
        # does not leak a slot.
        if job.generation != 0 and job.generation <= self.abandoned_generation:
            await self.send_credit()
            return

        if job.kind != miner_pb2.ISING_SAMPLE:
            await self.reject(job_id, miner_pb2.UNSUPPORTED_KIND)
            return

        # deadline_ms is an absolute unix timestamp; 0 means no deadline.
        if job.deadline_ms and job.deadline_ms < int(time.time() * 1000):
            await self.reject(job_id, miner_pb2.EXPIRED)
            return

        try:
            h, j, edges = decode_problem(job.ising, self.topologies)
        except MissingTopology:
            await self.reject(job_id, miner_pb2.TOPOLOGY_MISSING)
            return
        except ValueError:
            await self.reject(job_id, miner_pb2.MALFORMED)
            return

        if len(h) > MAX_NODES or len(edges) > MAX_EDGES:
            await self.reject(job_id, miner_pb2.TOO_LARGE)
            return

        num_reads = job.ising.num_reads or 1
        started = time.monotonic()
        # Sampling is CPU-bound, so it goes to a worker thread rather than
        # blocking the event loop and stalling heartbeats. A backend that
        # reaches a device over the network would await that call directly.
        solutions = await asyncio.to_thread(sample, h, j, edges, num_reads)
        device_us = int((time.monotonic() - started) * 1_000_000)

        self.jobs_done += 1
        await self.call.write(
            miner_pb2.MinerMsg(
                result=miner_pb2.Result(
                    job_id=job_id,
                    solutions=[build_solution(s, e) for s, e in solutions],
                    meta=miner_pb2.SamplerMeta(
                        reads=num_reads,
                        # A per-job pin wins; otherwise the session-wide budget
                        # the coordinator set through Configure.backend_toml.
                        sweeps=job.ising.num_sweeps
                        or (
                            self.config.num_sweeps
                            if self.config
                            else session.DEFAULT_NUM_SWEEPS
                        ),
                        device_access_time_us=device_us,
                        qpu_access_us=0,
                    ),
                )
            )
        )
        await self.send_credit()

    async def handle(self, msg: miner_pb2.CoordMsg) -> int | None:
        """Dispatch one coordinator message. Returns an exit code to stop."""
        which = msg.WhichOneof("msg")

        if which == "welcome":
            try:
                session.check_welcome(msg.welcome)
            except session.BadWelcome as exc:
                print(f"error: {exc}", file=sys.stderr)
                await self.send_fatal(ExitCode.CONFIG_INVALID, str(exc))
                return ExitCode.CONFIG_INVALID
            self.got_welcome = True
            return None

        if which == "configure":
            self.config = session.session_config_from_configure(
                self.miner_id, msg.configure
            )
            await self.call.write(miner_pb2.MinerMsg(ready=miner_pb2.Ready()))
            # Open the pipeline to the depth the coordinator asked for.
            await self.send_credit(self.config.queue_depth)
            return None

        if which == "topology":
            topo = msg.topology
            self.topologies[bytes(topo.hash)] = Topology.from_proto(topo)
            return None

        if which == "set_target":
            # Adapt support needs a PyO3 binding that does not exist.
            # Reimplementing consensus adapt math in Python would drift from
            # the Rust reference, so this mock ignores the target and keeps
            # the trivial all-(+1) sampler.
            return None

        if which == "job":
            await self.handle_job(msg.job)
            return None

        if which == "cancel":
            # Every job at or below max_generation is abandoned. Nothing is in
            # flight in this serial miner, so the acknowledgement is a Status
            # carrying the watermark -- never a Result.
            self.abandoned_generation = msg.cancel.max_generation
            await self.send_status()
            return None

        if which == "ping":
            await self.send_status()
            return None

        if which == "get_capabilities":
            await self.call.write(
                miner_pb2.MinerMsg(capabilities=capabilities_message())
            )
            return None

        if which == "shutdown":
            # Every earlier write was awaited, so half-closing here cannot drop
            # a queued Reject or JobRequest.
            self.got_shutdown = True
            await self.call.done_writing()
            return ExitCode.CLEAN

        return None


async def run_session(endpoint: str, miner_id: str) -> int:
    """Dial the coordinator and run the stream to completion."""
    if not endpoint.startswith("unix://"):
        print(
            f"error: --quip-coordinator must be a unix:// path, got {endpoint}",
            file=sys.stderr,
        )
        return ExitCode.CONFIG_INVALID

    # grpc wants unix:/path (one slash) for an absolute socket path.
    target = "unix:" + endpoint[len("unix://") :]
    # A Unix socket has no hostname, so grpc would otherwise derive an
    # :authority from the socket path. Tonic rejects that as a malformed header
    # and answers RST_STREAM(PROTOCOL_ERROR) before the handshake completes.
    options = [("grpc.default_authority", "localhost")]

    state: Session | None = None
    try:
        async with grpc.aio.insecure_channel(target, options=options) as channel:
            stub = miner_pb2_grpc.MinerServiceStub(channel)
            call = stub.Session()
            state = Session(miner_id, call)
            await state.send_hello()

            async for msg in call:
                code = await state.handle(msg)
                if code is not None:
                    return code
    except session.MissingToken:
        print("error: QUIP_SESSION_TOKEN unset", file=sys.stderr)
        return ExitCode.TOKEN_REJECTED
    except grpc.aio.AioRpcError as e:
        print(f"error: session failed: {e.code()}: {e.details()}", file=sys.stderr)
        return ExitCode.INTERNAL_FATAL

    # The coordinator closed the stream. Shutdown is the only clean close.
    # Closing before Welcome means the token was rejected.
    if state is None:
        return ExitCode.INTERNAL_FATAL
    if state.got_shutdown:
        return ExitCode.CLEAN
    if not state.got_welcome:
        print(
            "error: coordinator closed before Welcome (token rejected)", file=sys.stderr
        )
        return ExitCode.TOKEN_REJECTED
    print("error: coordinator closed the stream without Shutdown", file=sys.stderr)
    return ExitCode.INTERNAL_FATAL


def run_solve() -> int:
    """Read one problem as JSON on stdin and write solutions on stdout."""
    try:
        problem = json.load(sys.stdin)
        h = problem["h"]
        j = problem["j"]
        edges = [tuple(e) for e in problem["edges"]]
        num_reads = problem["num_reads"]
        validate_ising(h, j, edges)
    except (json.JSONDecodeError, KeyError, TypeError, ValueError) as e:
        print(f"error: malformed problem JSON on stdin: {e}", file=sys.stderr)
        return ExitCode.CONFIG_INVALID

    solutions = sample(h, j, edges, num_reads)
    json.dump([{"spins": s, "energy_milli": e} for s, e in solutions], sys.stdout)
    sys.stdout.write("\n")
    return ExitCode.CLEAN


def main() -> int:
    parser = argparse.ArgumentParser(add_help=True)
    parser.add_argument("--quip-coordinator")
    parser.add_argument("--miner-id")
    parser.add_argument("--capabilities", action="store_true")
    parser.add_argument("--solve", action="store_true")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--log-level", default="info")
    parser.add_argument("--sweeps-per-beta", type=int)
    args = parser.parse_args()

    if args.capabilities:
        # SPEC section 8: the protobuf JSON mapping, so field names are
        # lowerCamelCase. backend and algorithm are the stable lowercase
        # names from the SDK maps, not the protobuf enum labels.
        message = capabilities_message()
        payload = MessageToDict(
            message,
            always_print_fields_with_no_presence=True,
            preserving_proto_field_name=False,
        )
        payload["backend"] = session.BACKEND_NAMES[message.backend]
        payload["algorithm"] = session.ALGORITHM_NAMES[message.algorithm]
        print(json.dumps(payload))
        return ExitCode.CLEAN

    if args.check:
        # No device to open; a real backend probes its hardware here and
        # returns ENV_INCOMPATIBLE (69) when the host cannot run it.
        return ExitCode.CLEAN

    if args.solve:
        return run_solve()

    if not args.quip_coordinator:
        print("error: --quip-coordinator required for session mode", file=sys.stderr)
        return ExitCode.CONFIG_INVALID

    miner_id = args.miner_id or f"{BACKEND}-0"
    return asyncio.run(run_session(args.quip_coordinator, miner_id))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        # SPEC exit codes only. An unexpected exception must not become the
        # interpreter's default exit 1.
        print(f"error: unexpected failure: {exc}", file=sys.stderr)
        raise SystemExit(ExitCode.INTERNAL_FATAL) from exc
