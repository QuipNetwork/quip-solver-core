// Bit-packed spins share the initial_spins layout: spin i is bit i % 8 of
// byte i / 8, LSB first, 1 = +1 and 0 = -1. One, eight, and nine spins cover
// a partial byte, a full byte, and a one-spin tail.
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import test from "node:test";
import assert from "node:assert/strict";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const { consensus } = require(join(here, "..", "dist", "index.js"));

const cases = [
  { spins: [1], bytes: [0x01] },
  { spins: [-1], bytes: [0x00] },
  { spins: [1, -1, 1, -1, 1, -1, 1, -1], bytes: [0x55] },
  { spins: [1, -1, 1, -1, 1, -1, 1, -1, 1], bytes: [0x55, 0x01] },
];

test("packed spins round-trip for 1, 8, and 9 spins", () => {
  const widths = new Set(cases.map((c) => c.spins.length));
  assert.deepEqual([...widths], [1, 8, 9]);
  for (const { spins, bytes } of cases) {
    const encoded = consensus.encodeSpinsPacked(Int8Array.from(spins));
    assert.deepEqual(Array.from(encoded), bytes, `encode ${spins.length}`);
    const decoded = consensus.decodeSpinsPacked(encoded, spins.length);
    assert.deepEqual(Array.from(decoded), spins, `decode ${spins.length}`);
  }
});

test("packed spin decode rejects a wrong byte length", () => {
  assert.throws(() => consensus.decodeSpinsPacked(Uint8Array.from([]), 1), /packed spins are \d+ bytes, expected \d+/);
  assert.throws(() => consensus.decodeSpinsPacked(Uint8Array.from([0x01, 0x00]), 1), /packed spins are \d+ bytes, expected \d+/);
  assert.throws(() => consensus.decodeSpinsPacked(Uint8Array.from([]), 8), /packed spins are \d+ bytes, expected \d+/);
  assert.throws(() => consensus.decodeSpinsPacked(Uint8Array.from([0x55, 0x00]), 8), /packed spins are \d+ bytes, expected \d+/);
  assert.throws(() => consensus.decodeSpinsPacked(Uint8Array.from([0x55]), 9), /packed spins are \d+ bytes, expected \d+/);
  assert.throws(() => consensus.decodeSpinsPacked(Uint8Array.from([0x55, 0x01, 0x00]), 9), /packed spins are \d+ bytes, expected \d+/);
});

test("packed spin decode rejects nonzero padding bits", () => {
  assert.throws(() => consensus.decodeSpinsPacked(Uint8Array.from([0x81]), 1), /padding/);
});
