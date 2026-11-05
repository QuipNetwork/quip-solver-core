//! Parameter sets of `quip/ice-fields` revision 1 for the conformance gate and
//! the registered vectors. The contract itself is
//! `ising_generators::ice::ICE_FIELDS_V1`.

/// Zephyr mode on z(2, 2) (`zephyr_m` 2, `zephyr_t` 2) with `glass_milli` 125,
/// `field_milli` 500, and `field_divisor` 16.
pub const P_FIELDS_Z22_G125_F500_D16: [u8; 40] = [
    1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 2, 3, 0, 2, 0, 0, 0, 0x7d, 0, 4, 0, 2, 0, 0, 0,
    0xf4, 1, 5, 0, 2, 0, 0, 0, 16, 0,
];

/// Adjacency mode (`zephyr_m` 0, `zephyr_t` 0) with `glass_milli` 45,
/// `field_milli` 1000, and `field_divisor` 16.
pub const P_FIELDS_ADJ_G45_F1000_D16: [u8; 40] = [
    1, 1, 0, 2, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 3, 0, 2, 0, 0, 0, 0x2d, 0, 4, 0, 2, 0, 0, 0,
    0xe8, 3, 5, 0, 2, 0, 0, 0, 16, 0,
];

/// Zephyr mode on z(2, 2) with `glass_milli` 125, `field_milli` 1001, and
/// `field_divisor` 16. It decodes and is out of bounds (code 9).
pub const P_FIELDS_F1001: [u8; 40] = [
    1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 2, 3, 0, 2, 0, 0, 0, 0x7d, 0, 4, 0, 2, 0, 0, 0,
    0xe9, 3, 5, 0, 2, 0, 0, 0, 16, 0,
];
