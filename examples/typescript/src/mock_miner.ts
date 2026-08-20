#!/usr/bin/env node
/**
 * A complete mock Quip solver in TypeScript.
 *
 * The npm package carries the consensus primitives as WebAssembly and the
 * generated gRPC stubs, but not the session loop, so this file writes the state
 * machine itself. Everything consensus-critical -- energy scoring and the spin
 * wire encoding -- runs the same compiled Rust the crates.io and PyPI releases
 * run, never a TypeScript reimplementation, because the network recomputes both
 * and rejects a mismatch.
 *
 * The session is promise-based: writes are awaited and the inbound stream is
 * consumed with `for await`. Awaiting each write is what makes the shutdown
 * path correct with no extra machinery, because by the time `Shutdown` arrives
 * every earlier Reject and JobRequest has already gone out.
 *
 * Modes, per SPEC section 2:
 *
 *   --capabilities   print the capabilities JSON and exit
 *   --solve          read one problem as JSON on stdin, write solutions on stdout
 *   --check          probe the device and exit
 *   (session)        --quip-coordinator unix://<path> --miner-id <id>
 */

import { credentials, type ClientDuplexStream } from "@grpc/grpc-js";
import {
  consensus,
  ExitCode,
  JobKind,
  RejectReason,
  MinerServiceClient,
  type CoordMsg,
  type IsingProblem,
  type Job,
  type MinerMsg,
  type Topology,
} from "@quip.network/quip-solver-core";

const BACKEND = "mock-typescript";
const ALGORITHM = "sa";
const MAX_NODES = 100_000;
const MAX_EDGES = 1_000_000;
const PROTOCOL_VERSION = 1;
// Mirrors the Rust session loop's DEFAULT_NUM_SWEEPS in quip-solver-core.
const DEFAULT_NUM_SWEEPS = 64;

// Read the session-wide `num_sweeps` from `Configure.backend_toml`, matching
// the Rust session loop: only a top-level key counts (one under a backend's
// own `[table]` belongs to that backend), and anything absent or unusable
// falls back to the default.
function numSweepsFromToml(backendToml: string): number {
  for (const line of backendToml.split("\n")) {
    const stripped = (line.split("#", 1)[0] ?? "").trim();
    if (stripped.startsWith("[")) {
      // First table header ends the top-level scope.
      break;
    }
    const eq = stripped.indexOf("=");
    if (eq === -1 || stripped.slice(0, eq).trim() !== "num_sweeps") {
      continue;
    }
    const n = Number(stripped.slice(eq + 1).trim());
    if (Number.isInteger(n) && n > 0) {
      return n;
    }
    break;
  }
  return DEFAULT_NUM_SWEEPS;
}

/** What this solver supports. Must answer without touching the device. */
function capabilities() {
  return {
    backend: BACKEND,
    algorithm: ALGORITHM,
    supportedKinds: ["ISING_SAMPLE"],
    maxNodes: MAX_NODES,
    maxEdges: MAX_EDGES,
    features: [] as string[],
    protocolVersion: PROTOCOL_VERSION,
    streamWidth: 1,
  };
}

/** Thrown for anything malformed on the wire; the caller rejects the job. */
class MalformedProblem extends Error {}
/** Thrown when a job names a topology this session never received. */
class MissingTopology extends Error {}

/**
 * Session-cached topology. `pos` maps each native (possibly sparse) node id to
 * its dense position in received order, matching Rust `TopologyCache`.
 */
interface CachedTopology {
  u: number[];
  v: number[];
  pos: Map<number, number>;
}

/** Decodes little-endian int32 milli-units into floats via the WASM codec. */
function decodeMilli(bytes: Uint8Array): number[] {
  return Array.from(consensus.decodeI32Le(bytes), (v) => v / 1000);
}

/**
 * Decodes an IsingProblem into `h`, `j` and a flat edge array.
 *
 * Every length rule below is a real wire invariant. Silently truncating here
 * would submit a confidently wrong energy rather than a clean rejection.
 * Topology-hash jobs remap native node ids through `pos` onto dense 0..n-1
 * indices that line up with `h`.
 */
function decodeProblem(
  ising: IsingProblem | undefined,
  topologies: Map<string, CachedTopology>,
): { h: number[]; j: number[]; edges: number[] } {
  if (!ising) throw new MalformedProblem("job carries no ising problem");
  if (ising.hMilliLe32.length % 4 !== 0) {
    throw new MalformedProblem("h_milli_le32 length is not a multiple of 4");
  }
  if (ising.jMilliLe32.length % 4 !== 0) {
    throw new MalformedProblem("j_milli_le32 length is not a multiple of 4");
  }

  const h = decodeMilli(ising.hMilliLe32);
  const j = decodeMilli(ising.jMilliLe32);

  let u: number[];
  let v: number[];
  let pos: Map<number, number> | undefined;
  if (ising.edges) {
    // Inline edges use dense 0..n-1 ids straight off the wire.
    u = ising.edges.u;
    v = ising.edges.v;
  } else if (ising.topologyHash && ising.topologyHash.length > 0) {
    const cached = topologies.get(Buffer.from(ising.topologyHash).toString("hex"));
    if (!cached) throw new MissingTopology("job references an unknown topology");
    u = cached.u;
    v = cached.v;
    pos = cached.pos;
  } else {
    throw new MalformedProblem("job carries neither edges nor a topology hash");
  }

  if (u.length !== v.length) throw new MalformedProblem("edge halves differ in length");
  if (u.length !== j.length) {
    throw new MalformedProblem(`${u.length} edges but ${j.length} couplings`);
  }
  const edges: number[] = [];
  for (let i = 0; i < u.length; i += 1) {
    const nu = u[i];
    const nv = v[i];
    if (nu === undefined || nv === undefined) {
      throw new MalformedProblem("edge halves differ in length");
    }
    let pu = nu;
    let pv = nv;
    if (pos) {
      const mappedU = pos.get(nu);
      const mappedV = pos.get(nv);
      if (mappedU === undefined || mappedV === undefined) {
        throw new MalformedProblem("edge references a node not in topology.nodes");
      }
      pu = mappedU;
      pv = mappedV;
    }
    if (pu >= h.length || pv >= h.length) {
      throw new MalformedProblem("edge references a node outside h");
    }
    edges.push(pu, pv);
  }
  return { h, j, edges };
}

/** Returns `numReads` all-(+1) solutions, scored by the WASM consensus scorer. */
function sample(h: number[], j: number[], edges: number[], numReads: number) {
  const spins = new Int8Array(h.length).fill(1);
  const energy = consensus.energyMilli(
    spins,
    Float64Array.from(h),
    Float64Array.from(j),
    Uint32Array.from(edges),
  );
  return Array.from({ length: numReads }, () => ({ spins, energy }));
}

/**
 * Writes one message and resolves once the stream has accepted it.
 *
 * grpc-js `write` is callback-based and returns a backpressure boolean.
 * Wrapping it in a promise lets the session await ordering rather than track
 * it, which is what removes the need for an outbound queue.
 */
function write(
  stream: ClientDuplexStream<MinerMsg, CoordMsg>,
  msg: Partial<MinerMsg>,
): Promise<void> {
  return new Promise((resolve, reject) => {
    stream.write(msg as MinerMsg, (e: Error | null | undefined) =>
      e ? reject(e) : resolve(),
    );
  });
}

/** True when this job's generation has already been abandoned by Cancel. */
function isCancelled(generation: bigint, abandonedGeneration: bigint): boolean {
  // Generation 0 is mempool: SPEC section 5, never cancelled.
  return generation !== 0n && abandonedGeneration !== 0n && generation <= abandonedGeneration;
}

/**
 * Reads `--name <value>` from argv. A value that is missing or itself starts
 * with `--` is invalid (so `--miner-id --capabilities` is not a miner id).
 */
function readFlag(
  argv: string[],
  name: string,
): { kind: "missing" } | { kind: "invalid" } | { kind: "ok"; value: string } {
  const i = argv.indexOf(name);
  if (i < 0) return { kind: "missing" };
  const value = argv[i + 1];
  if (value === undefined || value.startsWith("--")) return { kind: "invalid" };
  return { kind: "ok", value };
}

/** The solver side of the bidirectional stream. */
class Session {
  private topologies = new Map<string, CachedTopology>();
  private sessionSweeps = DEFAULT_NUM_SWEEPS;
  private jobsDone = 0n;
  private abandonedGeneration = 0n;
  private sawWelcome = false;
  private sawShutdown = false;

  constructor(
    private readonly minerId: string,
    private readonly stream: ClientDuplexStream<MinerMsg, CoordMsg>,
  ) {}

  get receivedWelcome(): boolean {
    return this.sawWelcome;
  }

  get receivedShutdown(): boolean {
    return this.sawShutdown;
  }

  private send(msg: Partial<MinerMsg>): Promise<void> {
    return write(this.stream, msg);
  }

  async hello(token: string): Promise<void> {
    await this.send({
      hello: {
        minerId: this.minerId,
        sessionToken: token,
        protocolVersion: PROTOCOL_VERSION,
        backend: BACKEND,
        algorithm: ALGORITHM,
        supportedKinds: [JobKind.ISING_SAMPLE],
        maxNodes: MAX_NODES,
        maxEdges: MAX_EDGES,
        features: [],
      },
    });
  }

  private async status(): Promise<void> {
    await this.send({
      status: {
        minerId: this.minerId,
        utilization: 0,
        jobsDone: this.jobsDone,
        abandonedGeneration: this.abandonedGeneration,
        samplerStats: {},
      },
    });
  }

  /** Rejects one job and refunds the credit it consumed. */
  private async reject(jobId: Uint8Array, reason: RejectReason): Promise<void> {
    await this.send({ reject: { jobId, reason } });
    await this.send({ jobRequest: { credits: 1 } });
  }

  private async handleJob(job: Job): Promise<void> {
    const jobId = job.jobId;

    if (isCancelled(job.generation, this.abandonedGeneration)) {
      // SPEC section 5: no Result for a generation already cancelled. Refund
      // the credit so the coordinator's consume-on-dispatch pool does not leak.
      await this.send({ jobRequest: { credits: 1 } });
      return;
    }

    if (job.kind !== JobKind.ISING_SAMPLE) {
      await this.reject(jobId, RejectReason.UNSUPPORTED_KIND);
      return;
    }
    // deadlineMs is an absolute unix timestamp; 0 means no deadline.
    const deadline = job.deadlineMs;
    if (deadline !== 0n && deadline < BigInt(Date.now())) {
      await this.reject(jobId, RejectReason.EXPIRED);
      return;
    }

    let decoded;
    try {
      decoded = decodeProblem(job.ising, this.topologies);
    } catch (e) {
      await this.reject(
        jobId,
        e instanceof MissingTopology ? RejectReason.TOPOLOGY_MISSING : RejectReason.MALFORMED,
      );
      return;
    }

    if (decoded.h.length > MAX_NODES || decoded.j.length > MAX_EDGES) {
      await this.reject(jobId, RejectReason.TOO_LARGE);
      return;
    }

    const numReads = job.ising?.numReads || 1;
    const started = process.hrtime.bigint();
    const solutions = sample(decoded.h, decoded.j, decoded.edges, numReads);
    const deviceUs = (process.hrtime.bigint() - started) / 1000n;

    this.jobsDone += 1n;
    await this.send({
      result: {
        jobId,
        solutions: solutions.map((s) => ({
          spinsBytes: consensus.encodeSpins(s.spins),
          energyMilli: s.energy,
        })),
        meta: {
          reads: numReads,
          // A per-job pin wins; otherwise the session-wide budget the
          // coordinator set through Configure.backend_toml.
          sweeps: job.ising?.numSweeps || this.sessionSweeps,
          deviceAccessTimeUs: deviceUs,
          qpuAccessUs: 0n,
          extra: {},
        },
      },
    });
    await this.send({ jobRequest: { credits: 1 } });
  }

  /** Dispatches one coordinator message. Returns an exit code to stop. */
  async handle(msg: CoordMsg): Promise<number | undefined> {
    if (msg.welcome) {
      if (msg.welcome.protocolVersion !== PROTOCOL_VERSION) {
        const reason = `unexpected protocol version in Welcome: ${msg.welcome.protocolVersion}`;
        process.stderr.write(`error: ${reason}\n`);
        await this.send({
          fatal: {
            exitCode: ExitCode.CONFIG_INVALID,
            reason,
            restartRequired: false,
          },
        });
        this.stream.end();
        return ExitCode.CONFIG_INVALID;
      }
      this.sawWelcome = true;
      return undefined;
    }

    if (msg.configure) {
      this.sessionSweeps = numSweepsFromToml(msg.configure.backendToml ?? "");
      await this.send({ ready: {} });
      // Open the pipeline to the depth the coordinator asked for.
      await this.send({ jobRequest: { credits: msg.configure.queueDepth || 3 } });
      return undefined;
    }

    if (msg.topology) {
      const topo: Topology = msg.topology;
      const pos = new Map<number, number>();
      const nodes = topo.nodes ?? [];
      for (let i = 0; i < nodes.length; i += 1) {
        const id = nodes[i];
        if (id === undefined) continue;
        pos.set(id, i);
      }
      this.topologies.set(Buffer.from(topo.hash).toString("hex"), {
        u: topo.edges?.u ?? [],
        v: topo.edges?.v ?? [],
        pos,
      });
      return undefined;
    }

    // SetTarget/adapt has no JavaScript binding in the wasm package, so this
    // mock cannot resolve a sampling budget from max_energy_milli. A real
    // solver would call the same adapt math the Rust path uses.
    if (msg.setTarget) return undefined;

    if (msg.job) {
      await this.handleJob(msg.job);
      return undefined;
    }

    if (msg.cancel) {
      // Every job at or below maxGeneration is abandoned. The watermark is
      // monotonic, so a later lower Cancel cannot revive an earlier one.
      if (msg.cancel.maxGeneration > this.abandonedGeneration) {
        this.abandonedGeneration = msg.cancel.maxGeneration;
      }
      await this.status();
      return undefined;
    }

    if (msg.ping) {
      await this.status();
      return undefined;
    }

    if (msg.getCapabilities) {
      await this.send({
        capabilities: {
          backend: BACKEND,
          algorithm: ALGORITHM,
          supportedKinds: [JobKind.ISING_SAMPLE],
          maxNodes: MAX_NODES,
          maxEdges: MAX_EDGES,
          features: [],
          protocolVersion: PROTOCOL_VERSION,
          streamWidth: 1,
        },
      });
      return undefined;
    }

    if (msg.shutdown) {
      // Every earlier write was awaited, so half-closing here cannot drop a
      // queued Reject or JobRequest.
      this.sawShutdown = true;
      this.stream.end();
      return ExitCode.CLEAN;
    }

    return undefined;
  }
}

async function runSession(endpoint: string, minerId: string): Promise<number> {
  const token = process.env["QUIP_SESSION_TOKEN"];
  if (!token) {
    process.stderr.write("error: QUIP_SESSION_TOKEN unset\n");
    return ExitCode.TOKEN_REJECTED;
  }
  if (!endpoint.startsWith("unix://")) {
    process.stderr.write(`error: --quip-coordinator must be a unix:// path, got ${endpoint}\n`);
    return ExitCode.CONFIG_INVALID;
  }

  const client = new MinerServiceClient(
    `unix:${endpoint.slice("unix://".length)}`,
    credentials.createInsecure(),
    {
      // A Unix socket has no hostname, so grpc would otherwise derive an
      // :authority from the socket path, which tonic rejects as malformed.
      "grpc.default_authority": "localhost",
    },
  );

  const stream = client.session();
  const session = new Session(minerId, stream);

  try {
    await session.hello(token);
    // ClientDuplexStream is a Node readable, so it iterates directly. This
    // replaces a data/end/error callback trio with ordinary control flow, and
    // an exception from handle() propagates instead of being swallowed by an
    // event handler.
    for await (const msg of stream) {
      const code = await session.handle(msg as CoordMsg);
      if (code !== undefined) return code;
    }
    if (!session.receivedWelcome) return ExitCode.TOKEN_REJECTED;
    if (!session.receivedShutdown) return ExitCode.INTERNAL_FATAL;
    return ExitCode.CLEAN;
  } catch (e) {
    process.stderr.write(`error: session failed: ${e instanceof Error ? e.message : e}\n`);
    return ExitCode.INTERNAL_FATAL;
  } finally {
    client.close();
  }
}

function runSolve(): Promise<number> {
  return new Promise((resolve) => {
    const chunks: Buffer[] = [];
    process.stdin.on("data", (c) => chunks.push(c));
    process.stdin.on("end", () => {
      let h: number[];
      let j: number[];
      let edges: number[];
      let numReads: number;
      try {
        const parsed: unknown = JSON.parse(Buffer.concat(chunks).toString("utf8"));
        if (parsed === null || typeof parsed !== "object") {
          throw new Error("problem JSON must be an object");
        }
        const rec = parsed as Record<string, unknown>;
        if (!Array.isArray(rec.h) || !Array.isArray(rec.j) || !Array.isArray(rec.edges)) {
          throw new Error("problem JSON must contain h, j, and edges arrays");
        }
        if (typeof rec.num_reads !== "number") {
          throw new Error("problem JSON must contain numeric num_reads");
        }
        h = rec.h as number[];
        j = rec.j as number[];
        numReads = rec.num_reads;
        edges = [];
        for (const pair of rec.edges) {
          if (!Array.isArray(pair) || pair.length !== 2) {
            throw new Error("each edge must be a [u, v] pair");
          }
          const u = pair[0];
          const v = pair[1];
          if (typeof u !== "number" || typeof v !== "number") {
            throw new Error("edge endpoints must be numbers");
          }
          edges.push(u, v);
        }
      } catch (e) {
        process.stderr.write(`error: malformed problem JSON on stdin: ${e}\n`);
        resolve(ExitCode.CONFIG_INVALID);
        return;
      }
      try {
        const solutions = sample(h, j, edges, numReads);
        process.stdout.write(
          `${JSON.stringify(
            solutions.map((s) => ({
              spins: Array.from(s.spins),
              energy_milli: Number(s.energy),
            })),
          )}\n`,
        );
        resolve(ExitCode.CLEAN);
      } catch (e) {
        process.stderr.write(`error: ${e instanceof Error ? e.message : e}\n`);
        resolve(ExitCode.INTERNAL_FATAL);
      }
    });
  });
}

async function main(): Promise<number> {
  const argv = process.argv.slice(2);

  if (argv.includes("--capabilities")) {
    process.stdout.write(`${JSON.stringify(capabilities())}\n`);
    return ExitCode.CLEAN;
  }
  if (argv.includes("--check")) {
    // No device to open. A real backend probes its hardware here and returns
    // 69 when the host cannot run it.
    return ExitCode.CLEAN;
  }
  if (argv.includes("--solve")) {
    return runSolve();
  }

  const coordinator = readFlag(argv, "--quip-coordinator");
  if (coordinator.kind === "invalid") {
    process.stderr.write("error: --quip-coordinator value must not start with --\n");
    return ExitCode.CONFIG_INVALID;
  }
  if (coordinator.kind === "missing") {
    process.stderr.write("error: --quip-coordinator required for session mode\n");
    return ExitCode.CONFIG_INVALID;
  }

  const miner = readFlag(argv, "--miner-id");
  if (miner.kind === "invalid") {
    process.stderr.write("error: --miner-id value must not start with --\n");
    return ExitCode.CONFIG_INVALID;
  }
  return runSession(coordinator.value, miner.kind === "ok" ? miner.value : `${BACKEND}-0`);
}

main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((e: unknown) => {
    process.stderr.write(`error: ${e instanceof Error ? e.message : e}\n`);
    process.exitCode = ExitCode.INTERNAL_FATAL;
  });
