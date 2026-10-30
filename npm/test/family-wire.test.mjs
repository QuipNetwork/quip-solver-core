// Wire checks for the problem-family extension. The reference bytes come from
// the family wire spec and were produced by an independent encoder.
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import test from "node:test";
import assert from "node:assert/strict";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const { IsingProblemGenerator, Capabilities, IsingProblem } = require(
  join(here, "..", "dist", "index.js"),
);

const DESCRIPTOR_HEX = "0a06717569702f7810011a01012220" + "aa".repeat(32);
const FAMILY_LEASE_HEX = "0802422f" + DESCRIPTOR_HEX + "4a20" + "bb".repeat(32);
const BINDING_HEX = "8a010f3202050738fdffffffffffffffff01";
const EMPTY_BINDING_HEX = "8a0100";
const CAPS_HEX =
  "8a010361626392010c0a06717569702f78120201029a012d0a06717569702f78" +
  "1201011a20" +
  "cc".repeat(32);

// ts-proto writes empty packed repeated fields (for example 1a00), which the
// reference encoder omits, so only some messages are byte-identical. Every
// message must still decode to an equal value after a re-encode.
const reencodeEqual = (codec, hex) => {
  const decoded = codec.decode(Buffer.from(hex, "hex"));
  const again = codec.decode(codec.encode(decoded).finish());
  assert.deepEqual(codec.toJSON(again), codec.toJSON(decoded));
};

const roundTrip = (codec, hex) => {
  const decoded = codec.decode(Buffer.from(hex, "hex"));
  return Buffer.from(codec.encode(decoded).finish()).toString("hex");
};

test("family lease round-trips byte for byte", () => {
  assert.equal(roundTrip(IsingProblemGenerator, FAMILY_LEASE_HEX), FAMILY_LEASE_HEX);
  const lease = IsingProblemGenerator.decode(Buffer.from(FAMILY_LEASE_HEX, "hex"));
  assert.equal(lease.algorithm, 2);
  assert.equal(lease.family.familyId, "quip/x");
  assert.equal(lease.family.revision, 1);
});

test("capabilities with family support round-trip by value", () => {
  reencodeEqual(Capabilities, CAPS_HEX);
  const caps = Capabilities.decode(Buffer.from(CAPS_HEX, "hex"));
  assert.equal(caps.solverId, "abc");
  assert.deepEqual(caps.generatedFamilies[0].revisions, [1, 2]);
});

test("binding round-trips and an empty binding stays present", () => {
  assert.equal(roundTrip(IsingProblem, BINDING_HEX), BINDING_HEX);
  reencodeEqual(IsingProblem, EMPTY_BINDING_HEX);
  const empty = IsingProblem.decode(Buffer.from(EMPTY_BINDING_HEX, "hex"));
  assert.notEqual(empty.family, undefined);
});

test("an unknown algorithm number survives decode and encode", () => {
  const lease = IsingProblemGenerator.decode(Buffer.from("0807", "hex"));
  assert.equal(lease.algorithm, 7);
  assert.equal(Buffer.from(IsingProblemGenerator.encode(lease).finish()).toString("hex"), "0807");
});

test("an unknown generator number in capabilities survives", () => {
  reencodeEqual(Capabilities, "5a03010207");
  const caps = Capabilities.decode(Buffer.from("5a03010207", "hex"));
  assert.deepEqual(caps.generators, [1, 2, 7]);
});
