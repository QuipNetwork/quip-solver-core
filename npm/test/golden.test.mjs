// Runs the conformance golden vectors through the WebAssembly build.
//
// The npm package ships the Rust consensus math compiled to WASM rather than a
// TypeScript reimplementation, and this is what proves it: the same fixtures the
// Rust tests assert against must produce identical answers here.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import test from "node:test";
import assert from "node:assert/strict";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const { consensus } = require(join(here, "..", "dist", "index.js"));

const golden = JSON.parse(
  readFileSync(
    join(here, "..", "..", "crates", "quip-solver-conformance", "vectors", "golden_vectors.json"),
    "utf8",
  ),
);

test("energy matches the golden vectors", () => {
  for (const [index, c] of golden.energy.entries()) {
    const got = consensus.energyMilli(
      Int8Array.from(c.spins),
      Float64Array.from(c.h_milli.map((v) => v / 1000)),
      Float64Array.from(c.j_milli.map((v) => v / 1000)),
      Uint32Array.from(c.edges.flat()),
    );
    assert.equal(got, BigInt(c.energy_milli), `energy case ${index}`);
  }
});

test("diversity matches the golden vectors", () => {
  for (const [index, c] of golden.diversity.entries()) {
    const width = c.solutions[0].length;
    const got = consensus.setDiversity(Int8Array.from(c.solutions.flat()), width);
    assert.ok(Math.abs(got - c.diversity) < 1e-9, `diversity case ${index}: ${got} != ${c.diversity}`);
  }
});

test("wire codecs round-trip", () => {
  const spins = Int8Array.from([1, -1, 1, 1, -1, -1, 1, -1, 1]);
  assert.deepEqual(Array.from(consensus.decodeSpins(consensus.encodeSpins(spins))), Array.from(spins));

  const values = Int32Array.from([-2147483648, -1, 0, 1, 2147483647]);
  assert.deepEqual(Array.from(consensus.decodeI32Le(consensus.encodeI32Le(values))), Array.from(values));
});

test("malformed input is rejected rather than silently scored", () => {
  // An odd-length edge array cannot describe vertex pairs. Returning a number
  // here would let a solver submit a confidently wrong energy.
  assert.throws(() =>
    consensus.energyMilli(
      Int8Array.from([1]),
      Float64Array.from([0]),
      Float64Array.from([0]),
      Uint32Array.from([0]),
    ),
  );
  assert.throws(() => consensus.setDiversity(Int8Array.from([1, 1, 1]), 2));
});

test("exit codes match the Python bindings", () => {
  assert.equal(consensus.ExitCode.CLEAN, 0);
  assert.equal(consensus.ExitCode.CONFIG_INVALID, 64);
  assert.equal(consensus.ExitCode.ENV_INCOMPATIBLE, 69);
  assert.equal(consensus.ExitCode.INTERNAL_FATAL, 70);
  assert.equal(consensus.ExitCode.TOKEN_REJECTED, 77);
});

test("gRPC stubs expose a bidirectional session", () => {
  const { MinerServiceService, MinerServiceClient } = require(join(here, "..", "dist", "index.js"));
  assert.equal(typeof MinerServiceClient, "function");
  assert.ok(MinerServiceService.session.requestStream);
  assert.ok(MinerServiceService.session.responseStream);
});
