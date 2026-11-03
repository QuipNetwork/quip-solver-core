//! Parameter sets of `quip/ice-glass` revision 1 for the conformance gate and
//! the registered vectors. The contract itself is
//! `ising_generators::ice::ICE_GLASS_V1`.

/// Adjacency mode (`zephyr_m` 0, `zephyr_t` 0) with `glass_milli` 125.
pub const P_ICE_ADJ_125: [u8; 24] = [
    1, 1, 0, 2, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 3, 0, 2, 0, 0, 0, 0x7d, 0,
];

/// Zephyr mode on z(1, 2) (`zephyr_m` 1, `zephyr_t` 2) with `glass_milli` 200.
pub const P_ICE_Z12_200: [u8; 24] = [
    1, 1, 0, 2, 0, 0, 0, 1, 0, 2, 0, 1, 0, 0, 0, 2, 3, 0, 2, 0, 0, 0, 0xc8, 0,
];

/// Adjacency mode with `glass_milli` 1000. It decodes and is out of bounds
/// (code 9).
pub const P_ICE_GLASS_1000: [u8; 24] = [
    1, 1, 0, 2, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 3, 0, 2, 0, 0, 0, 0xe8, 3,
];
