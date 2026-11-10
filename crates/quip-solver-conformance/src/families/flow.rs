//! Parameter sets of `quip/flow-static` revision 1 for the conformance gate and
//! the registered vectors. The contract itself is
//! `ising_generators::flow::FLOW_V1`.

/// Zephyr z(2, 4) with `side` 2 and `cost_half_max` 30.
pub const P_FLOW_Z24_L2: [u8; 32] = [
    1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 2, 0, 4, 0, 2, 0, 0, 0, 30, 0,
];

/// Zephyr z(3, 4) with `side` 3 and `cost_half_max` 8.
pub const P_FLOW_Z34_L3: [u8; 32] = [
    1, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 3, 0, 4, 0, 2, 0, 0, 0, 8, 0,
];

/// Zephyr z(2, 4) with `side` 1 and `cost_half_max` 30. It decodes and is out of bounds (code 9).
pub const P_FLOW_SIDE1: [u8; 32] = [
    1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 1, 0, 4, 0, 2, 0, 0, 0, 30, 0,
];
