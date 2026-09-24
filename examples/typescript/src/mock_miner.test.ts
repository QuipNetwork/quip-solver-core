import assert from "node:assert/strict";
import test from "node:test";
import { CoefficientEncoding, IsingProblem, consensus } from "@quip.network/quip-solver-core";
import { decodeProblem } from "./mock_miner.js";

function problem(scale: number, value: number): IsingProblem {
  return IsingProblem.fromPartial({
    encoding: CoefficientEncoding.COEFFICIENT_ENCODING_I32, scale,
    h: consensus.encodeI32Le(Int32Array.from([value, -value])),
    j: consensus.encodeI32Le(Int32Array.from([value])),
    edges: { u: [0], v: [1] },
  });
}

test("accepts exact I32 scales", () => {
  for (const [scale, stored, milli] of [[1, 1, 1000], [2000, 2, 1], [3, 3, 1000]] as const) {
    assert.deepEqual(decodeProblem(problem(scale, stored), new Map()), {
      h: [milli / 1000, -milli / 1000], j: [milli / 1000], edges: [0, 1],
    });
  }
});

test("rejects non-dividing scales, zero scales, and milli overflow", () => {
  for (const [scale, stored] of [[3, 1], [0, 1], [1, 2147484], [1, -2147484]] as const) {
    assert.throws(() => decodeProblem(problem(scale, stored), new Map()));
  }
});
