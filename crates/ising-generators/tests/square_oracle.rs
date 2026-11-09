//! The square-link lattice: incidence, direct charges, and charge-request
//! compatibility against brute force on the L = 2 lattices.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code: a failed lookup is a test failure"
)]

use ising_generators::square::{
    charges, check_request, symmetries, Axis, Boundary, LatticeError, Request, RequestError,
    SquareLattice,
};
use ising_generators::witness::{
    canonical, check_witness, constraints, pack, unpack, Constraint, WitnessError,
};

fn lattice(l: u16, boundary: Boundary) -> SquareLattice {
    SquareLattice::new(l, boundary).expect("lattice builds")
}

fn request(charges: Vec<Option<i8>>, fixed: Vec<(u32, i8)>) -> Request {
    Request {
        charges,
        fixed,
        fields: Vec::new(),
    }
}

/// The spin vector of `state`: bit `i` set means spin +1 on link `i`.
fn spins_of(state: u32, links: u32) -> Vec<i8> {
    (0..links)
        .map(|i| if (state >> i) & 1 == 1 { 1 } else { -1 })
        .collect()
}

/// Charges by counting arrows: arrows into the vertex minus arrows out.
fn arrow_charges(lattice: &SquareLattice, spins: &[i8]) -> Vec<i8> {
    let mut q = vec![0i8; lattice.vertex_count() as usize];
    for e in 0..lattice.link_count() {
        let link = lattice.link(e).expect("link exists");
        let (into, out) = if spins[e as usize] == 1 {
            (link.head, link.tail)
        } else {
            (link.tail, link.head)
        };
        q[into as usize] += 1;
        q[out as usize] -= 1;
    }
    q
}

#[test]
fn periodic_l2_has_parallel_links() {
    let lat = lattice(2, Boundary::Periodic);
    assert_eq!(lat.vertex_count(), 4);
    assert_eq!(lat.link_count(), 8);
    let a = lat.link(0).expect("link 0");
    let b = lat.link(2).expect("link 2");
    assert_eq!((a.tail, a.head, a.axis), (0, 1, Axis::X));
    assert_eq!((b.tail, b.head, b.axis), (1, 0, Axis::X));
    assert_eq!(lat.link(1).expect("link 1").head, 2);
    assert!(lat.link(8).is_none());
    for v in 0..4 {
        assert_eq!(lat.degree(v), Some(4));
    }
    assert_eq!(lat.degree(4), None);
    assert_eq!(lat.incident(0), &[(0, -1), (1, -1), (2, 1), (5, 1)][..]);
    assert!(lat.incident(4).is_empty());
}

#[test]
fn open_l2_has_12_links_and_boundary_degrees() {
    let lat = lattice(2, Boundary::Open);
    assert_eq!(lat.vertex_count(), 9);
    assert_eq!(lat.link_count(), 12);
    let degrees: Vec<u8> = (0..9).map(|v| lat.degree(v).expect("vertex")).collect();
    assert_eq!(degrees, [2, 3, 2, 3, 4, 3, 2, 3, 2]);
    // Vertex 2 is (2, 0): it owns only a +y link. Link order is by tail.
    let first = lat.link(0).expect("link 0");
    assert_eq!((first.tail, first.head, first.axis), (0, 1, Axis::X));
    let second = lat.link(1).expect("link 1");
    assert_eq!((second.tail, second.head, second.axis), (0, 3, Axis::Y));
    let third = lat.link(3).expect("link 3");
    assert_eq!((third.tail, third.head, third.axis), (1, 4, Axis::Y));
    let last = lat.link(11).expect("link 11");
    assert_eq!((last.tail, last.head, last.axis), (7, 8, Axis::X));
}

#[test]
fn lattice_bounds() {
    for l in [0, 1, 65] {
        assert_eq!(
            SquareLattice::new(l, Boundary::Periodic).err(),
            Some(LatticeError::SizeOutOfRange)
        );
        assert_eq!(
            SquareLattice::new(l, Boundary::Open).err(),
            Some(LatticeError::SizeOutOfRange)
        );
    }
    let big = lattice(64, Boundary::Periodic);
    assert_eq!((big.vertex_count(), big.link_count()), (4096, 8192));
    let big = lattice(64, Boundary::Open);
    assert_eq!(
        (big.vertex_count(), big.link_count()),
        (65 * 65, 2 * 64 * 65)
    );
    assert_eq!(lattice(2, Boundary::Periodic).vertex_count(), 4);
}

fn check_all_states(lat: &SquareLattice) {
    let links = lat.link_count();
    for state in 0..(1u32 << links) {
        let spins = spins_of(state, links);
        let q = charges(lat, &spins).expect("valid spins");
        assert_eq!(q, arrow_charges(lat, &spins), "state {state}");
        assert_eq!(q.iter().map(|&c| i32::from(c)).sum::<i32>(), 0);
        for (v, &c) in q.iter().enumerate() {
            let deg = lat
                .degree(u32::try_from(v).expect("small"))
                .expect("vertex");
            assert_eq!((i32::from(c) - i32::from(deg)) % 2, 0, "parity");
            assert!(c.unsigned_abs() <= deg, "degree bound");
        }
    }
}

#[test]
fn every_periodic_l2_state_has_reproducible_charges() {
    check_all_states(&lattice(2, Boundary::Periodic));
}

#[test]
fn every_open_l2_state_has_reproducible_charges() {
    check_all_states(&lattice(2, Boundary::Open));
}

#[test]
fn charges_rejects_bad_spin_vectors() {
    let lat = lattice(2, Boundary::Periodic);
    assert!(charges(&lat, &[1; 7]).is_none());
    assert!(charges(&lat, &[1; 9]).is_none());
    let mut spins = vec![1i8; 8];
    spins[3] = 0;
    assert!(charges(&lat, &spins).is_none());
    spins[3] = 2;
    assert!(charges(&lat, &spins).is_none());
}

/// Every charge vector some state of `lat` produces, with its spins.
fn all_states(lat: &SquareLattice) -> Vec<(Vec<i8>, Vec<i8>)> {
    let links = lat.link_count();
    (0..(1u32 << links))
        .map(|state| {
            let spins = spins_of(state, links);
            (charges(lat, &spins).expect("valid"), spins)
        })
        .collect()
}

fn brute_force(states: &[(Vec<i8>, Vec<i8>)], req: &Request) -> bool {
    states.iter().any(|(q, spins)| {
        req.charges
            .iter()
            .zip(q)
            .all(|(label, &c)| label.is_none_or(|t| t == c))
            && req.fixed.iter().all(|&(e, s)| spins[e as usize] == s)
    })
}

#[test]
fn compatibility_matches_brute_force_on_l2() {
    let lat = lattice(2, Boundary::Periodic);
    let states = all_states(&lat);
    let labels: Vec<Option<i8>> = core::iter::once(None).chain((-4..=4).map(Some)).collect();
    let mut compatible = 0;
    let mut total = 0;
    for &a in &labels {
        for &b in &labels {
            for &c in &labels {
                for &d in &labels {
                    let req = request(vec![a, b, c, d], Vec::new());
                    let ok = check_request(&lat, &req).is_ok();
                    assert_eq!(ok, brute_force(&states, &req), "{req:?}");
                    total += 1;
                    compatible += usize::from(ok);
                }
            }
        }
    }
    assert_eq!(total, 10_000);
    assert!(compatible > 100 && compatible < total);

    // Periodic with fixed spins.
    let fixed_sets: [&[(u32, i8)]; 4] = [
        &[(0, 1)],
        &[(0, -1), (2, -1)],
        &[(1, 1), (5, -1)],
        &[(3, 1), (4, 1), (7, -1)],
    ];
    for fixed in fixed_sets {
        for &a in &labels {
            for &b in &labels {
                let req = request(vec![a, b, Some(2), None], fixed.to_vec());
                let ok = check_request(&lat, &req).is_ok();
                assert_eq!(ok, brute_force(&states, &req), "{req:?}");
            }
        }
    }
}

#[test]
fn compatibility_matches_brute_force_on_open_l2() {
    let lat = lattice(2, Boundary::Open);
    let states = all_states(&lat);
    let labels: Vec<Option<i8>> = core::iter::once(None).chain((-4..=4).map(Some)).collect();
    let fixed_sets: [&[(u32, i8)]; 5] = [
        &[],
        &[(0, 1)],
        &[(0, -1), (1, -1)],
        &[(5, 1), (9, -1)],
        &[(2, 1), (3, -1), (11, 1)],
    ];
    let mut cases = 0;
    let mut compatible = 0;
    let mut check = |req: Request| {
        let ok = check_request(&lat, &req).is_ok();
        assert_eq!(ok, brute_force(&states, &req), "{req:?}");
        cases += 1;
        compatible += usize::from(ok);
    };

    // Three chosen vertices: a corner, an edge, and the centre.
    for fixed in fixed_sets {
        for &a in &labels {
            for &b in &labels {
                for &c in &labels {
                    let mut q = vec![None; 9];
                    q[0] = a;
                    q[1] = b;
                    q[4] = c;
                    check(request(q, fixed.to_vec()));
                }
            }
        }
    }

    // Tight cases: charges of a real state, some perturbed, some freed.
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize
    };
    for _ in 0..600 {
        let (q, spins) = &states[next() % states.len()];
        let mut labels: Vec<Option<i8>> = q.iter().map(|&c| Some(c)).collect();
        for slot in &mut labels {
            match next() % 5 {
                0 => *slot = None,
                1 => *slot = slot.map(|c| c - 2),
                _ => {}
            }
        }
        let fixed = match next() % 3 {
            0 => Vec::new(),
            1 => vec![(0, spins[0])],
            _ => vec![(4, spins[4]), (7, -spins[7])],
        };
        check(request(labels, fixed));
    }
    assert!(cases >= 5000);
    assert!(compatible > 100 && compatible < cases);
}

#[test]
fn incompatible_requests_fail_directly() {
    let lat = lattice(2, Boundary::Periodic);
    let free = || vec![None; 4];
    let with = |idx: &[(usize, i8)]| {
        let mut q = free();
        for &(i, c) in idx {
            q[i] = Some(c);
        }
        q
    };
    let err = |req: &Request| check_request(&lat, req).expect_err("must fail");

    assert!(check_request(&lat, &request(free(), Vec::new())).is_ok());
    assert!(check_request(&lat, &request(vec![Some(0); 4], Vec::new())).is_ok());

    assert_eq!(
        err(&request(vec![None; 3], Vec::new())),
        RequestError::ChargesLength
    );
    let mut req = request(free(), Vec::new());
    req.fields = vec![0; 7];
    assert_eq!(err(&req), RequestError::FieldsLength);
    req.fields = vec![0; 8];
    assert!(check_request(&lat, &req).is_ok());

    assert_eq!(
        err(&request(free(), vec![(8, 1)])),
        RequestError::FixedOutOfRange
    );
    assert_eq!(
        err(&request(free(), vec![(2, 1), (2, 1)])),
        RequestError::FixedNotAscending
    );
    assert_eq!(
        err(&request(free(), vec![(3, 1), (2, 1)])),
        RequestError::FixedNotAscending
    );
    assert_eq!(err(&request(free(), vec![(2, 0)])), RequestError::FixedSpin);
    assert_eq!(err(&request(free(), vec![(2, 2)])), RequestError::FixedSpin);

    assert_eq!(
        err(&request(with(&[(0, 6)]), Vec::new())),
        RequestError::DegreeBound
    );
    assert_eq!(
        err(&request(with(&[(0, -6)]), Vec::new())),
        RequestError::DegreeBound
    );
    assert_eq!(
        err(&request(with(&[(0, 1)]), Vec::new())),
        RequestError::Parity
    );
    assert_eq!(
        err(&request(
            vec![Some(2), Some(0), Some(0), Some(0)],
            Vec::new()
        )),
        RequestError::Total
    );

    // Precedence: length before fixed, fixed before degree, degree before
    // parity, parity before total.
    assert_eq!(
        err(&request(vec![Some(9); 3], vec![(9, 0)])),
        RequestError::ChargesLength
    );
    assert_eq!(
        err(&request(with(&[(0, 9)]), vec![(9, 0)])),
        RequestError::FixedOutOfRange
    );
    assert_eq!(
        err(&request(with(&[(0, 9)]), vec![(2, 0)])),
        RequestError::FixedSpin
    );
    assert_eq!(
        err(&request(with(&[(0, 6), (1, 1)]), Vec::new())),
        RequestError::DegreeBound
    );
    assert_eq!(
        err(&request(
            vec![Some(1), Some(0), Some(0), Some(0)],
            Vec::new()
        )),
        RequestError::Parity
    );

    // Review focus 2: local tests pass, no orientation exists. Vertex 0 takes
    // all four arrows in, so vertex 1 cannot take all four in as well.
    let both = request(with(&[(0, 4), (1, 4)]), Vec::new());
    assert_eq!(err(&both), RequestError::Infeasible);
    // Two sinks and a total that holds, still impossible.
    let sinks = request(vec![Some(4), Some(4), Some(-4), Some(-4)], Vec::new());
    assert_eq!(err(&sinks), RequestError::Infeasible);

    // Review focus 3: link 0 points from vertex 0 to vertex 1 with spin +1,
    // which contradicts four arrows into vertex 0.
    let clash = request(with(&[(0, 4)]), vec![(0, 1)]);
    assert_eq!(err(&clash), RequestError::Infeasible);
    let agree = request(with(&[(0, 4)]), vec![(0, -1)]);
    assert!(check_request(&lat, &agree).is_ok());

    // Open boundary: a corner has degree 2, so a charge of 4 is a bound error.
    let open = lattice(2, Boundary::Open);
    let mut q = vec![None; 9];
    q[0] = Some(4);
    assert_eq!(
        check_request(&open, &request(q, Vec::new())).expect_err("bound"),
        RequestError::DegreeBound
    );
}

fn empty_request(lat: &SquareLattice) -> Request {
    request(vec![None; lat.vertex_count() as usize], Vec::new())
}

fn packed_state(state: u32, links: u32) -> Vec<u8> {
    pack(&spins_of(state, links)).expect("valid spins")
}

#[test]
fn packing_round_trips_and_rejects_malformed() {
    // Link i is bit i % 8 of byte i / 8, and bit 1 is spin +1.
    let mut spins = vec![-1i8; 10];
    spins[0] = 1;
    spins[9] = 1;
    let bytes = pack(&spins).expect("valid");
    assert_eq!(bytes, [0b0000_0001, 0b0000_0010]);
    assert_eq!(unpack(&bytes, 10).expect("valid"), spins);
    assert_eq!(pack(&[]).expect("empty"), Vec::<u8>::new());
    for state in 0..256u32 {
        let spins = spins_of(state, 8);
        assert_eq!(
            unpack(&pack(&spins).expect("valid"), 8).expect("valid"),
            spins
        );
    }

    assert_eq!(pack(&[1, 0, -1]), Err(WitnessError::Value));
    assert_eq!(pack(&[2]), Err(WitnessError::Value));
    assert_eq!(unpack(&[0], 9), Err(WitnessError::Length));
    assert_eq!(unpack(&[0, 0], 8), Err(WitnessError::Length));
    assert_eq!(unpack(&[0, 0b0000_0100], 10), Err(WitnessError::Padding));
    assert_eq!(unpack(&[0, 0b0000_0011], 10).expect("valid").len(), 10);

    // Malformed bytes fail before any constraint is read.
    let lat = lattice(2, Boundary::Periodic);
    let mut req = empty_request(&lat);
    req.charges[0] = Some(4);
    assert_eq!(
        check_witness(&lat, &req, &[0, 0]),
        Err(WitnessError::Length)
    );
    assert_eq!(canonical(&lat, &req, &[0, 0]), Err(WitnessError::Length));
    let open = lattice(2, Boundary::Open);
    assert_eq!(
        check_witness(&open, &empty_request(&open), &[0, 0b0001_0000]),
        Err(WitnessError::Padding)
    );
}

#[test]
fn witness_reports_labeled_violations() {
    let lat = lattice(2, Boundary::Periodic);
    let mut req = empty_request(&lat);
    req.charges[0] = Some(4);
    req.fixed = vec![(0, 1)];
    assert_eq!(
        constraints(&req),
        [
            Constraint::Fixed { link: 0, spin: 1 },
            Constraint::Charge {
                vertex: 0,
                charge: 4
            },
        ]
    );
    // All spins +1: link 0 is as fixed, but vertex 0 has charge 0.
    let all_up = pack(&[1; 8]).expect("valid");
    assert_eq!(check_witness(&lat, &req, &all_up), Ok(vec![1]));
    // Review focus 3: the contradicting fixed spin and the charge both fail.
    let into_zero = packed_state(0b0010_0100, 8); // links 2 and 5 up, 0 and 1 down
    assert_eq!(check_witness(&lat, &req, &into_zero), Ok(vec![0]));
    req.fixed = vec![(0, -1)];
    assert_eq!(check_witness(&lat, &req, &all_up), Ok(vec![0, 1]));
}

#[test]
fn a_valid_witness_has_no_violation() {
    let lat = lattice(2, Boundary::Periodic);
    let mut req = empty_request(&lat);
    req.charges[0] = Some(4);
    req.fixed = vec![(0, -1)];
    let witness = packed_state(0b0010_0100, 8);
    assert_eq!(check_witness(&lat, &req, &witness), Ok(Vec::new()));
    assert_eq!(
        check_witness(&lat, &empty_request(&lat), &witness),
        Ok(Vec::new())
    );
    assert!(constraints(&empty_request(&lat)).is_empty());
}

#[test]
fn symmetry_group_sizes() {
    let periodic = lattice(2, Boundary::Periodic);
    // 4 translations, 8 D4 elements, and with or without reversal.
    assert_eq!(symmetries(&periodic, &empty_request(&periodic)).len(), 64);
    let open = lattice(2, Boundary::Open);
    // No translation: 8 D4 elements, with or without reversal.
    assert_eq!(symmetries(&open, &empty_request(&open)).len(), 16);

    // Labels +2 at corner 0 and -2 at corner 8. Only four candidates keep the
    // two labels: the identity and the main diagonal reflection (charges
    // unchanged), and the half turn and the anti-diagonal reflection (the two
    // corners swap, so the charges negate and reversal is required).
    let mut pair = empty_request(&open);
    pair.charges[0] = Some(2);
    pair.charges[8] = Some(-2);
    let group = symmetries(&open, &pair);
    assert_eq!(group.len(), 4);
    assert_eq!(group.iter().filter(|g| g.is_reversal()).count(), 2);

    let mut one_corner = empty_request(&open);
    one_corner.charges[0] = Some(2);
    assert_eq!(symmetries(&open, &one_corner).len(), 2);

    let mut torus_pair = empty_request(&periodic);
    torus_pair.charges[0] = Some(2);
    torus_pair.charges[3] = Some(-2);
    let group = symmetries(&periodic, &torus_pair);
    assert!(!group.is_empty() && group.len() < 64);
}

#[test]
fn parallel_links_map_by_direction() {
    let lat = lattice(2, Boundary::Periodic);
    let group = symmetries(&lat, &empty_request(&lat));
    // Order: translation, then D4 element, then reversal. Translation 1 is
    // (1, 0), and element 0 is the identity.
    let shift = &group[(8) * 2];
    assert_eq!(shift.link_image(&lat, 0), Some((2, 1)));
    assert_eq!(shift.link_image(&lat, 2), Some((0, 1)));
    // Element 4 reflects x. Links 0 and 2 join the same two vertices, and the
    // reflection swaps them with sign -1, by direction.
    let flip = &group[4 * 2];
    assert_eq!(flip.link_image(&lat, 0), Some((2, -1)));
    assert_eq!(flip.link_image(&lat, 2), Some((0, -1)));
    // Every candidate permutes the links.
    for g in &group {
        let mut seen = [false; 8];
        for e in 0..8 {
            let (image, sign) = g.link_image(&lat, e).expect("image");
            assert!(sign == 1 || sign == -1);
            assert!(!seen[image as usize]);
            seen[image as usize] = true;
        }
    }
}

#[test]
fn canonical_bytes_are_invariant() {
    let lat = lattice(2, Boundary::Periodic);
    let req = empty_request(&lat);
    let group = symmetries(&lat, &req);
    for state in 0..256u32 {
        let spins = spins_of(state, 8);
        let canon = canonical(&lat, &req, &pack(&spins).expect("valid")).expect("valid");
        let q = charges(&lat, &spins).expect("valid");
        let mut least = pack(&spins).expect("valid");
        for g in &group {
            let image = g.apply(&lat, &spins).expect("image");
            let image_bytes = pack(&image).expect("valid");
            assert_eq!(
                canonical(&lat, &req, &image_bytes).expect("valid"),
                canon,
                "state {state}"
            );
            // Charges follow the vertex map, negated under reversal.
            let r = if g.is_reversal() { -1 } else { 1 };
            let q_image = charges(&lat, &image).expect("valid");
            for v in 0..4 {
                let w = g.vertex_image(&lat, v).expect("vertex");
                assert_eq!(q_image[w as usize], r * q[v as usize]);
            }
            least = least.min(image_bytes);
        }
        assert_eq!(canon, least);
    }
    // The open lattice too.
    let open = lattice(2, Boundary::Open);
    let req = empty_request(&open);
    let group = symmetries(&open, &req);
    for state in (0..4096u32).step_by(7) {
        let spins = spins_of(state, 12);
        let canon = canonical(&open, &req, &pack(&spins).expect("valid")).expect("valid");
        for g in &group {
            let image = pack(&g.apply(&open, &spins).expect("image")).expect("valid");
            assert_eq!(canonical(&open, &req, &image).expect("valid"), canon);
        }
    }
}

#[test]
fn fields_and_boundary_labels_break_symmetry() {
    let lat = lattice(2, Boundary::Periodic);
    let all_up = pack(&[1; 8]).expect("valid");
    let all_down = pack(&[-1; 8]).expect("valid");
    // With no request, reversal is a symmetry and the two states merge.
    let plain = empty_request(&lat);
    assert_eq!(
        canonical(&lat, &plain, &all_up),
        canonical(&lat, &plain, &all_down)
    );
    // A field on link 0 forbids plain reversal. A half turn with reversal
    // stays, because both negate the field and cancel.
    let mut biased = empty_request(&lat);
    biased.fields = vec![0; 8];
    biased.fields[0] = 3;
    let group = symmetries(&lat, &biased);
    assert!(!group.is_empty() && group.len() < 64);
    assert!(group
        .iter()
        .all(|g| !(g.is_reversal() && g.element() == 0 && g.translation() == (0, 0))));
    assert_ne!(
        canonical(&lat, &biased, &all_up),
        canonical(&lat, &biased, &all_down)
    );
    // A request that no candidate preserves except the identity returns the
    // input unchanged.
    let mut rigid = empty_request(&lat);
    rigid.fields = vec![1, 2, 3, 4, 5, 6, 7, 8];
    assert_eq!(symmetries(&lat, &rigid).len(), 1);
    let state = packed_state(0b1011_0110, 8);
    assert_eq!(canonical(&lat, &rigid, &state), Ok(state));

    // A boundary label on the open lattice breaks the corner symmetry.
    let open = lattice(2, Boundary::Open);
    let mut corner = empty_request(&open);
    corner.charges[0] = Some(2);
    let states = [
        pack(&spins_of(0b0000_0000_0101, 12)).expect("valid"),
        pack(
            &spins_of(0b0000_0000_0101, 12)
                .iter()
                .map(|s| -s)
                .collect::<Vec<_>>(),
        )
        .expect("valid"),
    ];
    assert_ne!(
        canonical(&open, &corner, &states[0]),
        canonical(&open, &corner, &states[1])
    );
}
