/**
 * Quip solver SDK for JavaScript and TypeScript.
 *
 * Re-exports the generated gRPC stubs alongside `consensus`, the WebAssembly
 * build of the Rust primitives in `quip-protocol`. The consensus math is the
 * same compiled code the Rust and Python packages call, not a reimplementation,
 * so a JavaScript solver cannot disagree with the network about an energy score.
 */

/**
 * Generated gRPC stubs. `int64`/`uint64` fields are `bigint` (ts-proto
 * `forceLong=bigint`) so generation and cancellation watermarks above
 * `Number.MAX_SAFE_INTEGER` round-trip on the wire.
 */
export * from "./generated/quip/v1/miner";

/**
 * Golden-pinned consensus primitives, compiled from Rust to WebAssembly.
 *
 * Loaded with `require` so the `.wasm` binary resolves relative to `dist/` at
 * runtime. The `import type` is erased at compile time and only supplies types.
 */
// eslint-disable-next-line @typescript-eslint/no-require-imports
export const consensus: typeof import("../wasm/quip_protocol_wasm") = require("../wasm/quip_protocol_wasm");

/**
 * Process exit codes from SPEC section 2. Same values as `Fatal.exitCode`.
 */
export const ExitCode = consensus.ExitCode;
