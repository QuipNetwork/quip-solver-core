//! Parameter sets of `quip/two-commodity-routing` revision 1 for the
//! conformance gate and the registered vectors. The contract itself is
//! `ising_generators::routing::ROUTING_V1`.

/// Zephyr z(3, 4) (`zephyr_m` 3, `zephyr_t` 4) with `pairs_divisor` 8.
pub const P_ROUTING_Z34_D8: [u8; 24] = [
    1, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 8, 0,
];

/// Zephyr z(3, 4) with `pairs_divisor` 6.
pub const P_ROUTING_Z34_D6: [u8; 24] = [
    1, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 6, 0,
];

/// Zephyr z(3, 4) with `pairs_divisor` 3. It decodes and is out of bounds
/// (code 9).
pub const P_ROUTING_D3: [u8; 24] = [
    1, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 1, 0, 0, 0, 4, 3, 0, 2, 0, 0, 0, 3, 0,
];
