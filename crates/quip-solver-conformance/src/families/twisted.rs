//! Parameter sets of `quip/twisted-ice-dimer` revision 1 for the conformance
//! gate and the registered vectors. The contract itself is
//! `ising_generators::ice::TWISTED_DIMER_V1`.

/// Zephyr mode on z(2, 2) (`zephyr_m` 2, `zephyr_t` 2) with `weak_milli` 62
/// and `pairs` 8.
pub const P_TWISTED_Z22_W62_P8: [u8; 34] = [
    1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 2, 3, 0, 2, 0, 0, 0, 0x3e, 0, 4, 0, 4, 0, 0, 0, 8,
    0, 0, 0,
];

/// Adjacency mode (`zephyr_m` 0, `zephyr_t` 0) with `weak_milli` 45 and
/// `pairs` 6.
pub const P_TWISTED_ADJ_W45_P6: [u8; 34] = [
    1, 1, 0, 2, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 3, 0, 2, 0, 0, 0, 0x2d, 0, 4, 0, 4, 0, 0, 0, 6,
    0, 0, 0,
];

/// Adjacency mode with `weak_milli` 1000 and `pairs` 6. It decodes and is out
/// of bounds (code 9).
pub const P_TWISTED_W1000: [u8; 34] = [
    1, 1, 0, 2, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 3, 0, 2, 0, 0, 0, 0xe8, 3, 4, 0, 4, 0, 0, 0, 6,
    0, 0, 0,
];
