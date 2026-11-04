//! Parameter sets of `quip/charged-ice` revision 1 for the conformance gate
//! and the registered vectors. The contract itself is
//! `ising_generators::ice::CHARGED_ICE_V1`.

/// Zephyr mode on z(2, 2) (`zephyr_m` 2, `zephyr_t` 2) with `scale_milli` 250,
/// `background_milli` 31, and `charged_per_256` 128.
pub const P_CHARGED_Z22_A250_B31_C128: [u8; 40] = [
    1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 2, 3, 0, 2, 0, 0, 0, 0xfa, 0, 4, 0, 2, 0, 0, 0,
    0x1f, 0, 5, 0, 2, 0, 0, 0, 0x80, 0,
];

/// Adjacency mode (`zephyr_m` 0, `zephyr_t` 0) with `scale_milli` 100,
/// `background_milli` 45, and `charged_per_256` 256.
pub const P_CHARGED_ADJ_A100_B45_C256: [u8; 40] = [
    1, 1, 0, 2, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 3, 0, 2, 0, 0, 0, 0x64, 0, 4, 0, 2, 0, 0, 0,
    0x2d, 0, 5, 0, 2, 0, 0, 0, 0, 1,
];

/// Adjacency mode with `scale_milli` 251. It decodes and is out of bounds
/// (code 9).
pub const P_CHARGED_A251: [u8; 40] = [
    1, 1, 0, 2, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 3, 0, 2, 0, 0, 0, 0xfb, 0, 4, 0, 2, 0, 0, 0,
    0x2d, 0, 5, 0, 2, 0, 0, 0, 0x80, 0,
];
