//! Parameter sets of `quip/defects-static` revision 1 for the conformance
//! gate and the registered vectors. The contract itself is
//! `ising_generators::defects::DEFECTS_V1`.

/// Zephyr z(2, 4) with `side` 2, no pairs, no obstacles, 2 pins, `field_x` 300, and `field_y` -200.
pub const P_DEFECTS_Z24_L2: [u8; 68] = [
    1, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 2, 0, 4, 0, 2, 0, 0, 0, 0, 0,
    5, 0, 2, 0, 0, 0, 0, 0, 6, 0, 2, 0, 0, 0, 2, 0, 7, 0, 4, 0, 0, 0, 44, 1, 0, 0, 8, 0, 4, 0, 0,
    0, 56, 255, 255, 255,
];

/// Zephyr z(3, 4) with `side` 3, 1 pair, 1 obstacle, 2 pins, `field_x` 1000, and `field_y` -1000.
pub const P_DEFECTS_Z34_L3: [u8; 68] = [
    1, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 3, 0, 4, 0, 2, 0, 0, 0, 1, 0,
    5, 0, 2, 0, 0, 0, 1, 0, 6, 0, 2, 0, 0, 0, 2, 0, 7, 0, 4, 0, 0, 0, 232, 3, 0, 0, 8, 0, 4, 0, 0,
    0, 24, 252, 255, 255,
];

/// Zephyr z(3, 4) with `side` 4 and the fields of `P_DEFECTS_Z34_L3`. It decodes and is out of bounds (code 9).
pub const P_DEFECTS_SIDE4: [u8; 68] = [
    1, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 4, 0, 4, 0, 2, 0, 0, 0, 0, 0,
    5, 0, 2, 0, 0, 0, 0, 0, 6, 0, 2, 0, 0, 0, 0, 0, 7, 0, 4, 0, 0, 0, 232, 3, 0, 0, 8, 0, 4, 0, 0,
    0, 24, 252, 255, 255,
];
