//! Twisted ice: local gauges, obstructions, and pairs.

use ising_generators::ice::{gauge_obstructions, Obstructions, StructureError};

/// Three tiles of four positions in a ring. Tiles 0 and 1 share position 3,
/// tiles 1 and 2 share position 6, and tiles 2 and 0 share position 0.
const RING: [[u32; 4]; 3] = [[0, 1, 2, 3], [3, 4, 5, 6], [6, 7, 8, 0]];
const RING_NODES: usize = 9;

#[test]
fn obstructions_of_no_tiles_are_zero() {
    let result = gauge_obstructions(&[], &[], 0);
    assert_eq!(
        result,
        Ok(Obstructions {
            components: 0,
            cycle_rank: 0,
            frustrated_cycles: 0,
            gauge_equivalent: true,
        })
    );
}

#[test]
fn a_consistent_ring_is_gauge_equivalent() {
    let gauges = [[1_i8; 4]; 3];
    let result = gauge_obstructions(&RING, &gauges, RING_NODES);
    assert_eq!(
        result,
        Ok(Obstructions {
            components: 1,
            cycle_rank: 1,
            frustrated_cycles: 0,
            gauge_equivalent: true,
        })
    );
}

#[test]
fn a_flipped_ring_has_one_frustrated_cycle() {
    let mut gauges = [[1_i8; 4]; 3];
    // Flip tile 0 at position 3 only, which flips one link relation.
    gauges[0][3] = -1;
    let result = gauge_obstructions(&RING, &gauges, RING_NODES);
    assert_eq!(
        result,
        Ok(Obstructions {
            components: 1,
            cycle_rank: 1,
            frustrated_cycles: 1,
            gauge_equivalent: false,
        })
    );
}

#[test]
fn a_node_in_three_tiles_is_an_error() {
    let tiles = [[0, 1, 2, 3], [3, 4, 5, 6], [3, 7, 8, 9]];
    let gauges = [[1_i8; 4]; 3];
    assert_eq!(
        gauge_obstructions(&tiles, &gauges, 10),
        Err(StructureError::MembershipAboveTwo(3))
    );
}

#[test]
fn bad_gauge_inputs_are_errors() {
    let gauges = [[1_i8; 4]; 3];
    assert_eq!(
        gauge_obstructions(&RING, &gauges[..2], RING_NODES),
        Err(StructureError::Overflow)
    );
    let mut zero = gauges;
    zero[1][0] = 0;
    assert_eq!(
        gauge_obstructions(&RING, &zero, RING_NODES),
        Err(StructureError::Overflow)
    );
    assert_eq!(
        gauge_obstructions(&RING, &gauges, RING_NODES - 1),
        Err(StructureError::Overflow)
    );
}
