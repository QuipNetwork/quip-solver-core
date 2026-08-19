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

interface CachedTopology {
  u: number[];
  v: number[];
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
  if (ising.edges) {
    u = ising.edges.u;
    v = ising.edges.v;
  } else if (ising.topologyHash && ising.topologyHash.length > 0) {
    const cached = topologies.get(Buffer.from(ising.topologyHash).toString("hex"));
    if (!cached) throw new MissingTopology("job references an unknown topology");
    u = cached.u;
    v = cached.v;
  } else {
    throw new MalformedProblem("job carries neither edges nor a topology hash");
  }

  if (u.length !== v.length) throw new MalformedProblem("edge halves differ in length");
  if (u.length !== j.length) {
    throw new MalformedProblem(`${u.length} edges but ${j.length} couplings`);
  }
  const edges: number[] = [];
  for (let i = 0; i < u.length; i += 1) {
    if (u[i]! >= h.length || v[i]! >= h.length) {
      throw new MalformedProblem("edge references a node outside h");
    }
    edges.push(u[i]!, v[i]!);
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

/** The solver side of the bidirectional stream. */
class Session {
  private topologies = new Map<string, CachedTopology>();
  private jobsDone = 0;
  private abandonedGeneration = 0;

  constructor(
    private readonly minerId: string,
    private readonly stream: ClientDuplexStream<MinerMsg, CoordMsg>,
  ) {}

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

    if (job.kind !== JobKind.ISING_SAMPLE) {
      await this.reject(jobId, RejectReason.UNSUPPORTED_KIND);
      return;
    }
    // deadlineMs is an absolute unix timestamp; 0 means no deadline.
    const deadline = Number(job.deadlineMs);
    if (deadline !== 0 && deadline < Date.now()) {
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
    const deviceUs = Number((process.hrtime.bigint() - started) / 1000n);

    this.jobsDone += 1;
    await this.send({
      result: {
        jobId,
        solutions: solutions.map((s) => ({
          spinsBytes: consensus.encodeSpins(s.spins),
          // The WASM scorer returns i64 as bigint; ts-proto maps int64 to
          // number. Milli-unit energies stay far inside 2^53, so the narrowing
          // is exact for any problem this solver accepts.
          energyMilli: Number(s.energy),
        })),
        meta: {
          reads: numReads,
          sweeps: job.ising?.numSweeps ?? 0,
          deviceAccessTimeUs: deviceUs,
          qpuAccessUs: 0,
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
        process.stderr.write(
          `error: unexpected protocol version in Welcome: ${msg.welcome.protocolVersion}\n`,
        );
        return 64;
      }
      return undefined;
    }

    if (msg.configure) {
      await this.send({ ready: {} });
      // Open the pipeline to the depth the coordinator asked for.
      await this.send({ jobRequest: { credits: msg.configure.queueDepth || 3 } });
      return undefined;
    }

    if (msg.topology) {
      const topo: Topology = msg.topology;
      this.topologies.set(Buffer.from(topo.hash).toString("hex"), {
        u: topo.edges?.u ?? [],
        v: topo.edges?.v ?? [],
      });
      return undefined;
    }

    // This mock ignores the difficulty target; a real solver adapts here.
    if (msg.setTarget) return undefined;

    if (msg.job) {
      await this.handleJob(msg.job);
      return undefined;
    }

    if (msg.cancel) {
      // Every job at or below maxGeneration is abandoned. Nothing is in flight
      // in this serial miner, so the acknowledgement is a Status carrying the
      // watermark -- never a Result.
      this.abandonedGeneration = Number(msg.cancel.maxGeneration);
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
      this.stream.end();
      return 0;
    }

    return undefined;
  }
}

async function runSession(endpoint: string, minerId: string): Promise<number> {
  const token = process.env["QUIP_SESSION_TOKEN"];
  if (!token) {
    process.stderr.write("error: QUIP_SESSION_TOKEN unset\n");
    return 77;
  }
  if (!endpoint.startsWith("unix://")) {
    process.stderr.write(`error: --quip-coordinator must be a unix:// path, got ${endpoint}\n`);
    return 64;
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
    return 0;
  } catch (e) {
    process.stderr.write(`error: session failed: ${e instanceof Error ? e.message : e}\n`);
    return 70;
  } finally {
    client.close();
  }
}

function runSolve(): Promise<number> {
  return new Promise((resolve) => {
    const chunks: Buffer[] = [];
    process.stdin.on("data", (c) => chunks.push(c));
    process.stdin.on("end", () => {
      try {
        const problem = JSON.parse(Buffer.concat(chunks).toString("utf8"));
        const edges: number[] = [];
        for (const [u, v] of problem.edges) edges.push(u, v);
        const solutions = sample(problem.h, problem.j, edges, problem.num_reads);
        process.stdout.write(
          `${JSON.stringify(
            solutions.map((s) => ({
              spins: Array.from(s.spins),
              energy_milli: Number(s.energy),
            })),
          )}\n`,
        );
        resolve(0);
      } catch (e) {
        process.stderr.write(`error: malformed problem JSON on stdin: ${e}\n`);
        resolve(64);
      }
    });
  });
}

async function main(): Promise<number> {
  const argv = process.argv.slice(2);
  const flag = (name: string): string | undefined => {
    const i = argv.indexOf(name);
    return i >= 0 ? argv[i + 1] : undefined;
  };

  if (argv.includes("--capabilities")) {
    process.stdout.write(`${JSON.stringify(capabilities())}\n`);
    return 0;
  }
  if (argv.includes("--check")) {
    // No device to open. A real backend probes its hardware here and returns
    // 69 when the host cannot run it.
    return 0;
  }
  if (argv.includes("--solve")) {
    return runSolve();
  }

  const endpoint = flag("--quip-coordinator");
  if (!endpoint) {
    process.stderr.write("error: --quip-coordinator required for session mode\n");
    return 64;
  }
  return runSession(endpoint, flag("--miner-id") ?? `${BACKEND}-0`);
}

main().then((code) => {
  process.exitCode = code;
});
