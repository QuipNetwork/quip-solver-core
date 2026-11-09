//! Parameter sets of `quip/polariton-static` revision 1 for the conformance
//! gate and the registered vectors. The contract itself is
//! `ising_generators::polariton::POLARITON_V1`.

/// Zephyr z(2, 4) (`zephyr_m` 2, `zephyr_t` 4) with `side` 2 and `pairs` 0.
pub const P_POLARITON_Z24_L2_P0: [u8; 32] = [
    1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 2, 0, 4, 0, 2, 0, 0, 0, 0, 0,
];

/// Zephyr z(3, 4) with `side` 3 and `pairs` 2.
pub const P_POLARITON_Z34_L3_P2: [u8; 32] = [
    1, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 3, 0, 4, 0, 2, 0, 0, 0, 2, 0,
];

/// Zephyr z(2, 4) with `side` 1. It decodes and is out of bounds (code 9).
pub const P_POLARITON_SIDE1: [u8; 32] = [
    1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 1, 0, 4, 0, 2, 0, 0, 0, 0, 0,
];
