//! The dipolar coefficient tables: hand values, ties, validator faults, the
//! commitment, the calibration status, the bounds, the truncation bound, and
//! the frozen oracle table.

#![expect(
    clippy::cast_possible_truncation,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    reason = "test code: a failed lookup is a test failure"
)]

use ising_generators::dipolar::{
    physical_status, quantize, truncation_bound, validate, Axis, Calibration, CalibrationRecord,
    DipolarError, Entry, Geometry, Island, PhysicalStatus, Table, MAX_R2,
};
use ising_generators::square::{Axis as LinkAxis, Boundary, SquareLattice};

const fn isl(x: u16, y: u16, axis: Axis, moment: i8) -> Island {
    Island { x, y, axis, moment }
}

fn geometry(islands: &[Island], d: i32, scale: u32, r2: u32) -> Geometry {
    Geometry::new(islands.to_vec(), d, scale, r2).expect("admitted geometry")
}

/// The only coefficient of a two island geometry with a generous radius.
fn q_of(a: Island, b: Island, d: i32, scale: u32) -> i32 {
    let g = geometry(&[a, b], d, scale, 16);
    let entries = quantize(&g).expect("quantize");
    assert_eq!(entries.len(), 1);
    validate(&g, &entries).expect("the quantized entry validates");
    entries[0].q
}

#[test]
fn two_island_couplings_match_hand_values() {
    use Axis::{MinusX, PlusX, PlusY};
    // S = 1024 and D = 3, so S * D = 3072. N = K (u_i . u_j) - 3 (u_i . r)(u_j . r).
    let o = isl(0, 0, PlusX, 1);
    // K = 1, head to tail, r = (1, 0): N = 1 - 3 = -2, J = -2 D, q = -2 * 3072.
    assert_eq!(q_of(o, isl(1, 0, PlusX, 1), 3, 1024), -6144);
    // K = 1, side by side, r = (0, 1): N = 1, q = 3072.
    assert_eq!(q_of(o, isl(0, 1, PlusX, 1), 3, 1024), 3072);
    // K = 1, head to head, r = (1, 0), axes +x and -x: N = -1 + 3 = 2, q = 6144.
    assert_eq!(q_of(o, isl(1, 0, MinusX, 1), 3, 1024), 6144);
    // K = 1, perpendicular on an axis: u_i . r = 0 or u_j . r = 0, so N = 0, q = 0.
    assert_eq!(q_of(o, isl(1, 0, PlusY, 1), 3, 1024), 0);
    // K = 2, r = (1, 1), both +x: N = 2 - 3 = -1. K^(5/2) = 5.657, so
    // q = -3072 / 5.657 = -543.04, which rounds to -543.
    assert_eq!(q_of(o, isl(1, 1, PlusX, 1), 3, 1024), -543);
    // K = 2, r = (1, 1), +x and +y: N = 0 - 3 = -3. q = -9216 / 5.657 = -1629.2.
    assert_eq!(q_of(o, isl(1, 1, PlusY, 1), 3, 1024), -1629);
    // K = 4, head to tail, r = (2, 0): N = 4 - 12 = -8, K^(5/2) = 32,
    // J = -D / 4, q = -768. Side by side, r = (0, 2): N = 4, J = D / 8, q = 384.
    assert_eq!(q_of(o, isl(2, 0, PlusX, 1), 3, 1024), -768);
    assert_eq!(q_of(o, isl(0, 2, PlusX, 1), 3, 1024), 384);
    // K = 5, r = (2, 1), both +x: N = 5 - 12 = -7. K^(5/2) = 55.90.
    // q = -21504 / 55.90 = -384.7, which rounds to -385.
    assert_eq!(q_of(o, isl(2, 1, PlusX, 1), 3, 1024), -385);
    // The moments multiply: m_i m_j = -6 turns K = 1 head to tail into +36864.
    assert_eq!(
        q_of(isl(0, 0, PlusX, 2), isl(1, 0, PlusX, -3), 3, 1024),
        36864
    );
}

#[test]
fn ties_round_away_from_zero() {
    use Axis::PlusX;
    // K = 4 is a perfect square, so K^(5/2) = 32 and a tie needs 2x = 32 (2q + 1).
    // Side by side (N = 4) with S D = 4: x = 16, x / 32 = 0.5, so q = 1.
    let two = |b: Island, d: i32, s: u32| geometry(&[isl(0, 0, PlusX, 1), b], d, s, 4);
    let side = isl(0, 2, PlusX, 1);
    let g = two(side, 1, 4);
    assert_eq!(
        quantize(&g).expect("quantize"),
        [Entry { i: 0, j: 1, q: 1 }]
    );
    for wrong in [0, 2, -1] {
        let e = [Entry {
            i: 0,
            j: 1,
            q: wrong,
        }];
        assert_eq!(
            validate(&g, &e),
            Err(DipolarError::WrongValue { i: 0, j: 1 })
        );
    }
    // x = 48 gives 1.5, which rounds to 2 and not 1.
    let g = two(side, 1, 12);
    assert_eq!(
        quantize(&g).expect("quantize"),
        [Entry { i: 0, j: 1, q: 2 }]
    );
    // Head to tail (N = -8) with S D = 2: x = -16, so -0.5 rounds to -1, not 0.
    let g = two(isl(2, 0, PlusX, 1), 1, 2);
    assert_eq!(
        quantize(&g).expect("quantize"),
        [Entry { i: 0, j: 1, q: -1 }]
    );
    let e = [Entry { i: 0, j: 1, q: 0 }];
    assert_eq!(
        validate(&g, &e),
        Err(DipolarError::WrongValue { i: 0, j: 1 })
    );
    // S D = 6 gives x = -48, so -1.5 rounds to -2.
    let g = two(isl(2, 0, PlusX, 1), 1, 6);
    assert_eq!(
        quantize(&g).expect("quantize"),
        [Entry { i: 0, j: 1, q: -2 }]
    );
    // A negative D mirrors the sign.
    let g = two(side, -1, 4);
    assert_eq!(
        quantize(&g).expect("quantize"),
        [Entry { i: 0, j: 1, q: -1 }]
    );
}

fn three() -> Vec<Island> {
    vec![
        isl(0, 0, Axis::PlusX, 1),
        isl(1, 0, Axis::PlusX, 1),
        isl(0, 1, Axis::PlusX, 1),
    ]
}

#[test]
fn validator_rejects_each_fault() {
    // R2 = 2 holds the pairs (0, 1), (0, 2) at K = 1 and (1, 2) at K = 2.
    let g = geometry(&three(), 1, 1024, 2);
    let good = quantize(&g).expect("quantize");
    assert_eq!(good.len(), 3);
    validate(&g, &good).expect("valid");

    let mut off = good.clone();
    off[1].q += 1;
    assert_eq!(
        validate(&g, &off),
        Err(DipolarError::WrongValue { i: 0, j: 2 })
    );

    let mut missing = good.clone();
    let _gone = missing.remove(1);
    assert_eq!(
        validate(&g, &missing),
        Err(DipolarError::MissingPair { i: 0, j: 2 })
    );
    let mut last = good.clone();
    let _gone = last.pop();
    assert_eq!(
        validate(&g, &last),
        Err(DipolarError::MissingPair { i: 1, j: 2 })
    );
    assert_eq!(
        validate(&g, &[]),
        Err(DipolarError::MissingPair { i: 0, j: 1 })
    );

    // R2 = 1 leaves (1, 2) outside the radius.
    let tight = geometry(&three(), 1, 1024, 1);
    let mut extra = quantize(&tight).expect("quantize");
    assert_eq!(extra.len(), 2);
    extra.push(good[2]);
    assert_eq!(
        validate(&tight, &extra),
        Err(DipolarError::ExtraPair { i: 1, j: 2 })
    );

    let mut mirrored = good.clone();
    mirrored.push(Entry {
        i: 2,
        j: 1,
        q: good[2].q,
    });
    assert_eq!(
        validate(&g, &mirrored),
        Err(DipolarError::AsymmetricPair { i: 2, j: 1 })
    );
    let mut own = good.clone();
    own.insert(1, Entry { i: 1, j: 1, q: 0 });
    assert_eq!(validate(&g, &own), Err(DipolarError::SelfPair { i: 1 }));
    let mut twice = good.clone();
    twice.insert(1, good[0]);
    assert_eq!(
        validate(&g, &twice),
        Err(DipolarError::NotAscending { i: 0, j: 1 })
    );
    let mut again = good.clone();
    again.push(good[0]);
    assert_eq!(
        validate(&g, &again),
        Err(DipolarError::NotAscending { i: 0, j: 1 })
    );
    let mut swapped = good.clone();
    swapped.swap(0, 1);
    // Entries (0, 2), (0, 1), (1, 2): the pair (0, 1) comes after (0, 2).
    assert_eq!(
        validate(&g, &swapped),
        Err(DipolarError::NotAscending { i: 0, j: 1 })
    );
    let mut far = good;
    far.push(Entry { i: 0, j: 3, q: 0 });
    assert_eq!(validate(&g, &far), Err(DipolarError::IndexOutOfRange));

    // Table::new runs the same validation.
    assert_eq!(
        Table::new(g, off, Calibration::Oracle),
        Err(DipolarError::WrongValue { i: 0, j: 2 })
    );
}

fn commitment(islands: &[Island], d: i32, s: u32, r2: u32) -> [u8; 32] {
    let g = geometry(islands, d, s, r2);
    let entries = quantize(&g).expect("quantize");
    Table::new(g, entries, Calibration::Oracle)
        .expect("table")
        .commitment()
}

#[test]
fn commitment_binds_every_input() {
    let base = commitment(&three(), 1, 1024, 2);
    assert_eq!(base, commitment(&three(), 1, 1024, 2));
    let mut seen = vec![base];
    let mut variants: Vec<[u8; 32]> = Vec::new();
    let mut coordinate = three();
    coordinate[2].y = 2;
    variants.push(commitment(&coordinate, 1, 1024, 2));
    let mut axis = three();
    axis[1].axis = Axis::MinusX;
    variants.push(commitment(&axis, 1, 1024, 2));
    let mut moment = three();
    moment[0].moment = 2;
    variants.push(commitment(&moment, 1, 1024, 2));
    variants.push(commitment(&three(), 1, 1025, 2));
    variants.push(commitment(&three(), 2, 1024, 2));
    variants.push(commitment(&three(), 1, 1024, 1));
    variants.push(commitment(&three(), 1, 1024, 3));
    for v in variants {
        assert!(!seen.contains(&v));
        seen.push(v);
    }
    // The entry bytes, the calibration, and the record fields are bound too.
    let g = geometry(&three(), 1, 1024, 2);
    let entries = quantize(&g).expect("quantize");
    let record = CalibrationRecord {
        material: "permalloy".into(),
        lattice_spacing_pm: 500_000,
        normalization: "D in units of m_i m_j".into(),
        approximation_error_ppm: 1000,
        source_digest: [7; 32],
    };
    let commit = |c: Calibration| {
        Table::new(g.clone(), entries.clone(), c)
            .expect("table")
            .commitment()
    };
    let mut calibrations = vec![commit(Calibration::Oracle)];
    assert_eq!(calibrations[0], base);
    let edits: [fn(&mut CalibrationRecord); 6] = [
        |_| {},
        |r| r.material.push('x'),
        |r| r.lattice_spacing_pm += 1,
        |r| r.normalization.push('x'),
        |r| r.approximation_error_ppm += 1,
        |r| r.source_digest[31] ^= 1,
    ];
    for edit in edits {
        let mut r = record.clone();
        edit(&mut r);
        calibrations.push(commit(Calibration::Recorded(r)));
    }
    for (n, c) in calibrations.iter().enumerate() {
        assert!(!calibrations[..n].contains(c), "calibration {n}");
    }
    // The geometry digest moves with every geometry field and the table
    // commitment covers it.
    assert_ne!(g.digest(), geometry(&three(), 1, 1024, 1).digest());
}

#[test]
fn oracle_tables_have_no_physical_success() {
    let g = geometry(&three(), 1, 1024, 2);
    let entries = quantize(&g).expect("quantize");
    let oracle = Table::new(g.clone(), entries.clone(), Calibration::Oracle).expect("table");
    assert_eq!(
        physical_status(&oracle),
        PhysicalStatus::InsufficientPhysicalEvidence
    );
    let record = CalibrationRecord {
        material: "permalloy".into(),
        lattice_spacing_pm: 500_000,
        normalization: "unit".into(),
        approximation_error_ppm: 1000,
        source_digest: [1; 32],
    };
    let recorded = Table::new(
        g.clone(),
        entries.clone(),
        Calibration::Recorded(record.clone()),
    )
    .expect("table");
    assert_eq!(
        physical_status(&recorded),
        PhysicalStatus::RecordedUnreviewed
    );
    // Both variants of the status are the whole enumeration: there is no success.
    for status in [physical_status(&oracle), physical_status(&recorded)] {
        match status {
            PhysicalStatus::InsufficientPhysicalEvidence | PhysicalStatus::RecordedUnreviewed => {}
            _ => unreachable!("no other status exists"),
        }
    }
    let mut empty = record.clone();
    empty.material.clear();
    assert_eq!(
        Table::new(g.clone(), entries.clone(), Calibration::Recorded(empty)),
        Err(DipolarError::CalibrationText)
    );
    let mut long = record;
    long.normalization = "x".repeat(257);
    assert_eq!(
        Table::new(g, entries, Calibration::Recorded(long)),
        Err(DipolarError::CalibrationText)
    );
}

#[test]
fn bounds_hold_at_the_limits() {
    use Axis::{MinusX, MinusY, PlusX, PlusY};
    let one = |x, y| isl(x, y, PlusX, 1);
    let new = |islands: Vec<Island>, d, s, r2| Geometry::new(islands, d, s, r2);
    // Island count: 8320 pass and 8321 fail.
    let grid = |count: usize| -> Vec<Island> {
        (0..count)
            .map(|n| one((n % 257) as u16, (n / 257) as u16))
            .collect()
    };
    assert!(new(grid(8320), 1, 1, 1).is_ok());
    assert_eq!(new(grid(8321), 1, 1, 1), Err(DipolarError::IslandCount));
    assert_eq!(
        new(vec![one(257, 0)], 1, 1, 1),
        Err(DipolarError::Coordinate)
    );
    assert_eq!(
        new(vec![one(0, 257)], 1, 1, 1),
        Err(DipolarError::Coordinate)
    );
    assert!(new(vec![one(256, 256)], 1, 1, 1).is_ok());
    for m in [-16, 16] {
        assert!(new(vec![isl(0, 0, PlusX, m)], 1, 1, 1).is_ok());
    }
    for m in [-17, 17, i8::MIN, i8::MAX] {
        assert_eq!(
            new(vec![isl(0, 0, PlusX, m)], 1, 1, 1),
            Err(DipolarError::Moment)
        );
    }
    assert_eq!(new(vec![], 1, 0, 1), Err(DipolarError::Scale));
    assert_eq!(new(vec![], 0, 1, 1), Err(DipolarError::Constant));
    assert!(new(vec![], 1, 1 << 20, 1).is_ok());
    assert!(new(vec![], -1, 1 << 20, 1).is_ok());
    assert!(new(vec![], 1 << 20, 1, 1).is_ok());
    assert!(new(vec![], 2, 1 << 19, 1).is_ok());
    assert_eq!(
        new(vec![], 1, (1 << 20) + 1, 1),
        Err(DipolarError::ScaleProduct)
    );
    assert_eq!(
        new(vec![], -1, (1 << 20) + 1, 1),
        Err(DipolarError::ScaleProduct)
    );
    assert_eq!(
        new(vec![], 2, (1 << 19) + 1, 1),
        Err(DipolarError::ScaleProduct)
    );
    assert_eq!(new(vec![], i32::MIN, 1, 1), Err(DipolarError::ScaleProduct));
    assert_eq!(new(vec![], 1, u32::MAX, 1), Err(DipolarError::ScaleProduct));
    assert!(new(vec![], 1, 1, MAX_R2).is_ok());
    assert_eq!(new(vec![], 1, 1, MAX_R2 + 1), Err(DipolarError::Radius));
    assert_eq!(
        new(vec![one(3, 3), one(3, 3)], 1, 1, 1),
        Err(DipolarError::CoincidentIslands)
    );

    // The extreme pairs quantize and validate: S |D| = 2^20, |m| = 16, every
    // axis pair, at the nearest and the farthest distances.
    let axes = [PlusX, MinusX, PlusY, MinusY];
    let spots = [
        ((0, 0), (1, 0)),
        ((0, 0), (0, 1)),
        ((0, 0), (256, 256)),
        ((256, 0), (0, 256)),
        ((0, 0), (256, 0)),
        ((10, 20), (11, 21)),
    ];
    for ((x0, y0), (x1, y1)) in spots {
        for a in axes {
            for b in axes {
                for (d, s, mi, mj) in [
                    (1, 1 << 20, 16, 16),
                    (-1, 1 << 20, 16, -16),
                    (1 << 20, 1, -16, 16),
                    (1024, 1024, 16, 16),
                ] {
                    let g = geometry(&[isl(x0, y0, a, mi), isl(x1, y1, b, mj)], d, s, MAX_R2);
                    let entries = quantize(&g).expect("quantize");
                    validate(&g, &entries).expect("the extreme entry validates");
                    for bad in [entries[0].q.wrapping_add(1), entries[0].q.wrapping_sub(1)] {
                        let e = [Entry { i: 0, j: 1, q: bad }];
                        assert_eq!(
                            validate(&g, &e),
                            Err(DipolarError::WrongValue { i: 0, j: 1 })
                        );
                    }
                }
            }
        }
    }
    // An entry at the edge of i32 at the largest admitted K = 2^17 gets the
    // wrong-coefficient error. The limit rejects it before any squaring.
    let g = geometry(&[one(0, 0), one(256, 256)], 1, 1 << 20, MAX_R2);
    for q in [i32::MAX, i32::MIN] {
        assert_eq!(
            validate(&g, &[Entry { i: 0, j: 1, q }]),
            Err(DipolarError::WrongValue { i: 0, j: 1 })
        );
    }
}

#[test]
fn truncation_bound_is_exact_on_a_tiny_case() {
    use Axis::PlusX;
    // S = 1 and D = 1, so the terms are ceil(|N| / (K^2 isqrt(K))).
    // Three islands in a row: (0,0), (1,0), (3,0), all +x, moment 1, R2 = 1.
    // Outside the radius: (0,2) K = 9, N = 9 - 27 = -18, den = 81 * 3 = 243,
    //   so the term is ceil(18 / 243) = 1.
    // (1,2) K = 4, N = 4 - 12 = -8, den = 16 * 2 = 32, so the term is
    //   ceil(8 / 32) = 1.
    // The per-pair ceilings add to 2. The exact sum 35 / 108 would round up to 1.
    // (0,1) at K = 1 is inside the radius and adds nothing.
    let g = geometry(
        &[
            isl(0, 0, PlusX, 1),
            isl(1, 0, PlusX, 1),
            isl(3, 0, PlusX, 1),
        ],
        1,
        1,
        1,
    );
    assert_eq!(truncation_bound(&g), Ok(2));
    // A scale of 1000 turns the terms into ceil(18000 / 243) = 75 and
    // ceil(8000 / 32) = 250. The rounding bound counts the one entry.
    let g = geometry(
        &[
            isl(0, 0, PlusX, 1),
            isl(1, 0, PlusX, 1),
            isl(3, 0, PlusX, 1),
        ],
        1,
        1000,
        1,
    );
    assert_eq!(truncation_bound(&g), Ok(325));
    let entries = quantize(&g).expect("quantize");
    let table = Table::new(g, entries, Calibration::Oracle).expect("table");
    assert_eq!(table.truncation_bound(), 325);
    assert_eq!(table.rounding_bound(), 1);
    // The bound covers the real omitted magnitude: S * |N| / K^(5/2) is
    // 1000 * 18 / 243 = 74.07 and 1000 * 8 / 5.657^2 ... both stay under their terms.
}

/// The 12 links of the open L = 2 lattice as islands at doubled midpoints.
fn oracle_islands() -> Vec<Island> {
    let lattice = SquareLattice::new(2, Boundary::Open).expect("lattice");
    (0..lattice.link_count())
        .map(|e| {
            let link = lattice.link(e).expect("link");
            let (x, y) = ((link.tail % 3) as u16, (link.tail / 3) as u16);
            match link.axis {
                LinkAxis::X => isl(2 * x + 1, 2 * y, Axis::PlusX, 1),
                LinkAxis::Y => isl(2 * x, 2 * y + 1, Axis::PlusY, 1),
            }
        })
        .collect()
}

#[test]
fn the_oracle_table_is_frozen() {
    let islands = oracle_islands();
    assert_eq!(islands.len(), 12);
    let g = geometry(&islands, 1, 1024, 8);
    let entries = quantize(&g).expect("quantize");
    let pinned: Vec<(u32, u32, i32)> = entries.iter().map(|e| (e.i, e.j, e.q)).collect();
    assert_eq!(pinned, ORACLE_ENTRIES);
    let table = Table::new(g, entries, Calibration::Oracle).expect("table");
    assert_eq!(
        physical_status(&table),
        PhysicalStatus::InsufficientPhysicalEvidence
    );
    assert_eq!(hex(&table.geometry().digest()), ORACLE_GEOMETRY_DIGEST);
    assert_eq!(hex(&table.commitment()), ORACLE_COMMITMENT);
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::new(), |acc, b| acc + &format!("{b:02x}"))
}

/// The entries of the oracle table. A K = 2 diagonal is 543 or -543, a K = 4
/// head to tail pair is -256, a K = 4 side pair is 128, and a K = 8 diagonal pair is -23 (N = -4, 4096 / 181.02).
const ORACLE_ENTRIES: [(u32, u32, i32); 38] = [
    (0, 1, 543),
    (0, 2, -256),
    (0, 3, -543),
    (0, 5, 128),
    (0, 7, -23),
    (1, 3, 128),
    (1, 5, -543),
    (1, 6, -256),
    (1, 8, -23),
    (2, 3, 543),
    (2, 4, -543),
    (2, 5, -23),
    (2, 7, 128),
    (3, 4, 128),
    (3, 5, 543),
    (3, 6, -23),
    (3, 7, -543),
    (3, 8, -256),
    (3, 9, -23),
    (4, 7, 543),
    (4, 8, -23),
    (4, 9, -256),
    (5, 6, 543),
    (5, 7, -256),
    (5, 8, -543),
    (5, 10, 128),
    (5, 11, -23),
    (6, 8, 128),
    (6, 10, -543),
    (7, 8, 543),
    (7, 9, -543),
    (7, 10, -23),
    (7, 11, 128),
    (8, 9, 128),
    (8, 10, 543),
    (8, 11, -543),
    (9, 11, 543),
    (10, 11, -256),
];
const ORACLE_GEOMETRY_DIGEST: &str =
    "45a43f9d9641ae8e2100274213b46e2738fa0ad78e5efc9dd14db9f30ae5b692";
const ORACLE_COMMITMENT: &str = "efe917918f6d04ba87c27941caf2341ec3cb8625c3c7668fefa217bd292e365f";

/// Findings of a small Rust lexer: the identifiers `f32` and `f64`, and the
/// numeric literals with a decimal point followed by a digit or with an
/// exponent. It skips comments, nested block comments, strings, raw strings,
/// byte strings, and char literals.
fn float_findings(src: &str) -> Vec<String> {
    let c: Vec<char> = src.chars().collect();
    let at = |i: usize| c.get(i).copied().unwrap_or('\0');
    let is_ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut found = Vec::new();
    let mut prev = '\0';
    let mut i = 0;
    while i < c.len() {
        let ch = at(i);
        if ch == '/' && at(i + 1) == '/' {
            while i < c.len() && at(i) != '\n' {
                i += 1;
            }
        } else if ch == '/' && at(i + 1) == '*' {
            let mut depth = 1;
            i += 2;
            while i < c.len() && depth > 0 {
                if at(i) == '/' && at(i + 1) == '*' {
                    depth += 1;
                    i += 2;
                } else if at(i) == '*' && at(i + 1) == '/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if ch == '"' {
            i = skip_string(&c, i + 1);
            prev = '"';
        } else if ch == '\'' {
            // A char literal closes after one char or one escape. Anything
            // else is a lifetime.
            if at(i + 1) == '\\' {
                i += 2;
                while i < c.len() && at(i) != '\'' {
                    i += 1;
                }
                i += 1;
            } else if at(i + 2) == '\'' {
                i += 3;
            } else {
                i += 1;
            }
            prev = '\'';
        } else if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while is_ident(at(i)) {
                i += 1;
            }
            let ident: String = c[start..i].iter().collect();
            let raw_prefix = matches!(ident.as_str(), "r" | "br" | "cr");
            if raw_prefix && (at(i) == '#' || at(i) == '"') {
                let mut hashes = 0;
                while at(i) == '#' {
                    hashes += 1;
                    i += 1;
                }
                i += 1;
                while i < c.len() {
                    if at(i) == '"' && (1..=hashes).all(|n| at(i + n) == '#') {
                        i += 1 + hashes;
                        break;
                    }
                    i += 1;
                }
            } else if matches!(ident.as_str(), "b" | "c") && at(i) == '"' {
                i = skip_string(&c, i + 1);
            } else if ident == "b" && at(i) == '\'' {
                i += 1;
                if at(i) == '\\' {
                    i += 1;
                }
                i += 2;
            } else if ident == "f32" || ident == "f64" {
                found.push(ident);
            }
            prev = 'a';
        } else if ch.is_ascii_digit() {
            let start = i;
            let tuple_index = prev == '.';
            let radix = ch == '0' && matches!(at(i + 1), 'x' | 'b' | 'o');
            if radix {
                while is_ident(at(i)) {
                    i += 1;
                }
            } else {
                while at(i).is_ascii_digit() || at(i) == '_' {
                    i += 1;
                }
                if !tuple_index {
                    if at(i) == '.' && at(i + 1).is_ascii_digit() {
                        i += 1;
                        while at(i).is_ascii_digit() || at(i) == '_' {
                            i += 1;
                        }
                        found.push(c[start..i].iter().collect());
                    }
                    let signed = matches!(at(i + 1), '+' | '-') && at(i + 2).is_ascii_digit();
                    if matches!(at(i), 'e' | 'E') && (at(i + 1).is_ascii_digit() || signed) {
                        i += 2;
                        while at(i).is_ascii_digit() || at(i) == '_' {
                            i += 1;
                        }
                        found.push(c[start..i].iter().collect());
                    }
                }
                let suffix = i;
                while is_ident(at(i)) {
                    i += 1;
                }
                let suffix: String = c[suffix..i].iter().collect();
                if suffix == "f32" || suffix == "f64" {
                    found.push(c[start..i].iter().collect());
                }
            }
            prev = '0';
        } else {
            if !ch.is_whitespace() {
                prev = ch;
            }
            i += 1;
        }
    }
    found
}

/// The index after the closing quote of a string whose body starts at `i`.
fn skip_string(c: &[char], mut i: usize) -> usize {
    while i < c.len() {
        match c.get(i).copied() {
            Some('\\') => i += 2,
            Some('"') => return i + 1,
            _ => i += 1,
        }
    }
    i
}

#[test]
fn the_float_lexer_flags_floats_and_skips_text() {
    for flagged in [
        "1.5 + 0.5",
        "1e3",
        "2.5E-1",
        "let x: f64 = y;",
        "1f64",
        "2_0.5_0",
        "fn f(a: f32) {}",
    ] {
        assert!(!float_findings(flagged).is_empty(), "missed {flagged}");
    }
    for clean in [
        "x.0",
        "x.0.1",
        "0..n",
        "0..=9",
        "0x1e3 + 0b10",
        "1usize",
        "// f64",
        "\"f64 1.5\"",
        "/* f64 /* 1.5 */ 2.5 */ 7",
        "r#\"raw \"f64\" 1.5\"#",
        "b\"f64\"",
        "'f' + '\\'' + b'x'",
        "fn f<'a>(x: &'a str) {}",
        "let n = 10;",
    ] {
        assert_eq!(
            float_findings(clean),
            Vec::<String>::new(),
            "flagged {clean}"
        );
    }
}

#[test]
fn the_module_uses_no_float() {
    for (name, source) in [
        ("src/dipolar.rs", include_str!("../src/dipolar.rs")),
        ("tests/dipolar.rs", include_str!("dipolar.rs")),
    ] {
        assert_eq!(float_findings(source), Vec::<String>::new(), "{name}");
    }
}

#[test]
fn a_recorded_commitment_follows_the_documented_bytes() {
    // The table: three +x islands, D = 1, S = 1024, R2 = 2, moment 1 each.
    // (0,1): K = 1, N = -2, q = -2048. (0,2): K = 1, N = 1, q = 1024.
    // (1,2): K = 2, N = -1, q = -1024 / 5.657 = -181.02, so -181.
    let material = "Permalloy Ni\u{2088}\u{2080}Fe\u{2082}\u{2080}";
    let normalization = "D in units of m_i m_j";
    assert_ne!(material.len(), material.chars().count());
    let record = CalibrationRecord {
        material: material.into(),
        lattice_spacing_pm: 500_000,
        normalization: normalization.into(),
        approximation_error_ppm: 1000,
        source_digest: [7; 32],
    };
    let g = geometry(&three(), 1, 1024, 2);
    let entries =
        [(0_u32, 1_u32, -2048_i32), (0, 2, 1024), (1, 2, -181)].map(|(i, j, q)| Entry { i, j, q });
    let table = Table::new(g, entries.to_vec(), Calibration::Recorded(record)).expect("table");

    let derive = |context: &str, bytes: &[u8]| -> [u8; 32] {
        let mut h = blake3::Hasher::new_derive_key(context);
        let _updated = h.update(bytes);
        *h.finalize().as_bytes()
    };
    // The geometry bytes: count, then x, y, axis code, moment, then D, S, R2.
    let mut geo = 3_u32.to_le_bytes().to_vec();
    for (x, y) in [(0_u16, 0_u16), (1, 0), (0, 1)] {
        geo.extend(x.to_le_bytes());
        geo.extend(y.to_le_bytes());
        geo.extend([0_u8, 1]);
    }
    geo.extend(1_i32.to_le_bytes());
    geo.extend(1024_u32.to_le_bytes());
    geo.extend(2_u32.to_le_bytes());
    let geo_digest = derive("quip dipolar geometry v1", &geo);
    // The table bytes: the geometry digest, the entry count, each entry, then
    // the record: tag 1, two length-prefixed UTF-8 strings, two u32 values,
    // and the source digest.
    let mut bytes = geo_digest.to_vec();
    bytes.extend(3_u64.to_le_bytes());
    for e in entries {
        bytes.extend(e.i.to_le_bytes());
        bytes.extend(e.j.to_le_bytes());
        bytes.extend(e.q.to_le_bytes());
    }
    bytes.push(1);
    bytes.extend(u32::try_from(material.len()).expect("short").to_le_bytes());
    bytes.extend(material.as_bytes());
    bytes.extend(
        u32::try_from(normalization.len())
            .expect("short")
            .to_le_bytes(),
    );
    bytes.extend(normalization.as_bytes());
    bytes.extend(500_000_u32.to_le_bytes());
    bytes.extend(1000_u32.to_le_bytes());
    bytes.extend([7_u8; 32]);
    let expected = derive("quip dipolar table v1", &bytes);
    assert_eq!(table.commitment(), expected);
    assert_eq!(hex(&table.commitment()), RECORDED_COMMITMENT);
}

const RECORDED_COMMITMENT: &str =
    "9c03ec9b2e45866a48dc3752ced737e533d0a3e29c56f83b07d0b9c246c9dab3";
