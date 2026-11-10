//! The `quip/polariton-static@1` construction: the energy against the direct
//! charge loss, feasibility, windows, admission codes, the charge draw, the
//! metadata check, observables, and the registered recipes.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    clippy::panic,
    clippy::print_stdout,
    clippy::too_many_lines,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;

use ising_generators::descriptor::{admit, params_digest, Admitted, AdvertisedLimits, SizeBounds};
use ising_generators::family::{
    admit_topology, feasible, generate_instance, BaseTopology, SaltFault, Streams,
};
use ising_generators::ice::zephyr_lattice;
use ising_generators::instance::{Instance, InstanceParts};
use ising_generators::polariton::{
    polariton_observables, POLARITON, POLARITON_RECIPES, POLARITON_SCHEMA, POLARITON_V1,
};
use ising_generators::rng::shuffle;
use ising_generators::square::{charges, check_request, Boundary, Request, SquareLattice};
use ising_generators::Code;

#[expect(dead_code, reason = "this test uses only the live topology loader")]
mod common;

const CONTRACTS: &[ising_generators::descriptor::Contract] = &[POLARITON_V1];

/// The labels value of a free vertex.
const FREE: i8 = 127;

/// Canonical parameter bytes: format, then `(tag, len, payload)` per field.
fn params(m: u16, t: u8, side: u16, pairs: u16) -> Vec<u8> {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(&[1, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&m.to_le_bytes());
    bytes.extend_from_slice(&[2, 0, 1, 0, 0, 0, t]);
    bytes.extend_from_slice(&[3, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&side.to_le_bytes());
    bytes.extend_from_slice(&[4, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&pairs.to_le_bytes());
    bytes
}

fn admitted(bytes: &[u8]) -> Result<Admitted<'_>, Code> {
    admit(
        CONTRACTS,
        b"quip/polariton-static",
        1,
        bytes,
        &params_digest(bytes),
    )
}

fn bounds(admitted: &Admitted<'_>, topology: &BaseTopology) -> Result<SizeBounds, Code> {
    admit_topology(
        admitted,
        topology,
        topology.order_digest(),
        &AdvertisedLimits::default(),
    )
}

fn lattice_topology(m: u16, t: u8) -> BaseTopology {
    let (nodes, edges) = zephyr_lattice(m, t).expect("lattice");
    BaseTopology::new(nodes, &edges).expect("topology")
}

/// The Zephyr id of the segment `(u, w, k, p)` of `z(m, t)`.
fn seg_id(m: u32, t: u32, u: u32, w: u32, k: u32, p: u32) -> u32 {
    (((u * (2 * m + 1) + w) * t + k) * 2 + p % 2) * m + p / 2
}

/// `z(m, t)` without the nodes in `dead` and without the edges in `cut`.
fn damaged(m: u16, t: u8, dead: &[u32], cut: &[(u32, u32)]) -> BaseTopology {
    let (nodes, edges) = zephyr_lattice(m, t).expect("lattice");
    let nodes: Vec<u32> = nodes.into_iter().filter(|id| !dead.contains(id)).collect();
    let edges: Vec<(u32, u32)> = edges
        .into_iter()
        .filter(|&(a, b)| {
            !dead.contains(&a) && !dead.contains(&b) && !cut.contains(&(a.min(b), a.max(b)))
        })
        .collect();
    BaseTopology::new(nodes, &edges).expect("topology")
}

fn nonce_of(i: u32) -> [u8; 32] {
    let mut nonce = [9u8; 32];
    nonce[..4].copy_from_slice(&i.to_le_bytes());
    nonce
}

fn try_build(
    bytes: &[u8],
    topology: &BaseTopology,
    nonce: [u8; 32],
) -> Result<Instance, SaltFault> {
    let admitted = admitted(bytes).expect("admit");
    let bounds = bounds(&admitted, topology).expect("admit topology");
    generate_instance(&admitted, topology, &bounds, nonce)
}

fn build(bytes: &[u8], topology: &BaseTopology, nonce: [u8; 32]) -> Instance {
    try_build(bytes, topology, nonce)
        .map_err(|fault| format!("{fault:?}"))
        .expect("generate")
}

/// The decoded metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Meta {
    side: u16,
    labels: Vec<i8>,
    c0: u16,
    r0: u16,
    positions: Vec<u32>,
}

fn decode(instance: &Instance) -> Meta {
    let region = instance.metadata();
    assert_eq!(&region[..4], &[2, 0, 4, 0], "kind 4 first");
    let len = u32::from_le_bytes(region[4..8].try_into().expect("four")) as usize;
    let charges = &region[8..8 + len];
    let rest = &region[8 + len..];
    assert_eq!(&rest[..2], &[5, 0], "kind 5 second");
    let len5 = u32::from_le_bytes(rest[2..6].try_into().expect("four")) as usize;
    let incidence = &rest[6..];
    assert_eq!(incidence.len(), len5, "no trailing bytes");
    let side = u16::from_le_bytes(charges[..2].try_into().expect("two"));
    let u16_at =
        |bytes: &[u8], at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().expect("two"));
    Meta {
        side,
        labels: charges[2..].iter().map(|&b| b as i8).collect(),
        c0: u16_at(incidence, 0),
        r0: u16_at(incidence, 2),
        positions: incidence[4..]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().expect("four")))
            .collect(),
    }
}

fn encode(meta: &Meta) -> Vec<u8> {
    let mut charges = meta.side.to_le_bytes().to_vec();
    charges.extend(meta.labels.iter().map(|&l| l as u8));
    let mut incidence = meta.c0.to_le_bytes().to_vec();
    incidence.extend_from_slice(&meta.r0.to_le_bytes());
    for p in &meta.positions {
        incidence.extend_from_slice(&p.to_le_bytes());
    }
    let mut region = vec![2, 0];
    for (kind, payload) in [(4u16, charges), (5, incidence)] {
        region.extend_from_slice(&kind.to_le_bytes());
        region.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        region.extend_from_slice(&payload);
    }
    region
}

/// A copy of `instance` with new fields and metadata.
fn rebuilt(
    instance: &Instance,
    edges: Vec<(u32, u32)>,
    h: Vec<i32>,
    j: Vec<i32>,
    offset_milli: i64,
    metadata: Vec<u8>,
) -> Instance {
    let parts = InstanceParts {
        descriptor_digest: *instance.descriptor_digest(),
        topology_order_digest: *instance.topology_order_digest(),
        nonce: instance.nonce().copied(),
        node_ids: instance.node_ids().to_vec(),
        edges,
        h,
        j,
        offset_milli,
        metadata,
    };
    Instance::new(
        parts,
        &POLARITON_V1.limits,
        POLARITON_V1.metadata_kinds,
        None,
    )
    .expect("well formed instance")
}

fn same_fields(instance: &Instance, metadata: Vec<u8>) -> Instance {
    rebuilt(
        instance,
        instance.edges().to_vec(),
        instance.h().to_vec(),
        instance.j().to_vec(),
        instance.offset_milli(),
        metadata,
    )
}

/// The coefficients of `125 * sum (Q_v - q_v)^2` over the labeled vertices,
/// in milli, written from the definition and not from the generator:
/// `(h, edges, j, offset)` over `n` instance positions.
type Coefficients = (Vec<i32>, Vec<(u32, u32)>, Vec<i32>, i64);

fn expected_energy(meta: &Meta) -> Coefficients {
    let lattice = SquareLattice::new(meta.side, Boundary::Open).expect("lattice");
    let n = meta.positions.len();
    let mut h = vec![0i64; n];
    let mut pair: std::collections::BTreeMap<(u32, u32), i64> = std::collections::BTreeMap::new();
    let mut offset = 0i64;
    for v in 0..lattice.vertex_count() {
        let label = meta.labels[v as usize];
        if label == FREE {
            continue;
        }
        let q = i64::from(label);
        let at = lattice.incident(v);
        offset += 125 * (at.len() as i64 + q * q);
        for (a, &(e, ie)) in at.iter().enumerate() {
            let pe = meta.positions[e as usize];
            h[pe as usize] -= 250 * q * i64::from(ie);
            for &(f, jf) in &at[a + 1..] {
                let pf = meta.positions[f as usize];
                *pair.entry((pe.min(pf), pe.max(pf))).or_default() +=
                    250 * i64::from(ie) * i64::from(jf);
            }
        }
    }
    let edges: Vec<(u32, u32)> = pair
        .iter()
        .filter(|(_, &v)| v != 0)
        .map(|(&k, _)| k)
        .collect();
    let j: Vec<i32> = pair
        .values()
        .filter(|&&v| v != 0)
        .map(|&v| v as i32)
        .collect();
    (h.into_iter().map(|v| v as i32).collect(), edges, j, offset)
}

/// The energy of `spins` (one per instance node) in milli.
fn energy(instance: &Instance, spins: &[i8]) -> i64 {
    let mut total = instance.offset_milli();
    for (&h, &s) in instance.h().iter().zip(spins) {
        total += i64::from(h) * i64::from(s);
    }
    for (&(a, b), &j) in instance.edges().iter().zip(instance.j()) {
        total += i64::from(j) * i64::from(spins[a as usize]) * i64::from(spins[b as usize]);
    }
    total
}

/// The charge loss `125 * sum (Q_v - q_v)^2` in milli, from `square::charges`.
fn direct_loss(meta: &Meta, spins: &[i8]) -> i64 {
    let lattice = SquareLattice::new(meta.side, Boundary::Open).expect("lattice");
    let link_spins: Vec<i8> = meta.positions.iter().map(|&p| spins[p as usize]).collect();
    let q = charges(&lattice, &link_spins).expect("charges");
    q.iter()
        .zip(&meta.labels)
        .filter(|(_, &label)| label != FREE)
        .map(|(&got, &label)| 125 * (i64::from(got) - i64::from(label)).pow(2))
        .sum()
}

fn state(bits: u64, n: usize) -> Vec<i8> {
    (0..n)
        .map(|i| if bits >> i & 1 == 1 { 1 } else { -1 })
        .collect()
}

/// A tiny deterministic generator for spin states.
fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33
}

/// A zero loss witness of the instance, found by descent from random starts.
fn feasible_state(instance: &Instance, meta: &Meta) -> Vec<i8> {
    let n = instance.node_ids().len();
    let mut seed = 5u64;
    for _ in 0..200 {
        let mut spins: Vec<i8> = (0..n)
            .map(|_| if next(&mut seed) & 1 == 1 { 1 } else { -1 })
            .collect();
        let mut loss = direct_loss(meta, &spins);
        for _ in 0..40 * n {
            if loss == 0 {
                return spins;
            }
            let i = next(&mut seed) as usize % n;
            spins[i] = -spins[i];
            let after = direct_loss(meta, &spins);
            if after <= loss {
                loss = after;
            } else {
                spins[i] = -spins[i];
            }
        }
        if loss == 0 {
            return spins;
        }
    }
    panic!("no zero loss state found");
}

// ---- tests ----

#[test]
fn every_l2_state_matches_the_direct_charge_loss() {
    let bytes = params(2, 4, 2, 0);
    let instance = build(&bytes, &lattice_topology(2, 4), nonce_of(1));
    let meta = decode(&instance);
    let admitted = admitted(&bytes).expect("admit");
    assert_eq!(instance.node_ids().len(), 12);
    assert_eq!(meta.labels.iter().filter(|&&l| l != FREE).count(), 1);
    for bits in 0..4096u64 {
        let spins = state(bits, 12);
        let loss = direct_loss(&meta, &spins);
        assert_eq!(energy(&instance, &spins), loss, "state {bits}");
        assert_eq!(
            feasible(&admitted, &instance, &spins),
            loss == 0,
            "state {bits}"
        );
    }
}

#[test]
fn feasible_matches_zero_loss() {
    let bytes = params(3, 4, 4, 1);
    let instance = build(&bytes, &lattice_topology(3, 4), nonce_of(2));
    let meta = decode(&instance);
    let admitted = admitted(&bytes).expect("admit");
    let good = feasible_state(&instance, &meta);
    assert!(feasible(&admitted, &instance, &good));
    assert_eq!(energy(&instance, &good), 0);
    // A seeded sample: feasible exactly when the loss is zero.
    let n = good.len();
    let mut seed = 77u64;
    for _ in 0..300 {
        let spins: Vec<i8> = (0..n)
            .map(|_| if next(&mut seed) & 1 == 1 { 1 } else { -1 })
            .collect();
        let loss = direct_loss(&meta, &spins);
        assert_eq!(energy(&instance, &spins), loss);
        assert_eq!(feasible(&admitted, &instance, &spins), loss == 0);
    }
    // One flipped link that touches a labeled vertex breaks a label, however
    // low the energy stays.
    let lattice = SquareLattice::new(4, Boundary::Open).expect("lattice");
    let labeled_link = (0..lattice.link_count())
        .find(|&e| {
            let link = lattice.link(e).expect("link");
            [link.tail, link.head]
                .iter()
                .any(|&v| meta.labels[v as usize] != FREE)
        })
        .expect("a link at a labeled vertex");
    let mut bad = good.clone();
    let slot = meta.positions[labeled_link as usize] as usize;
    bad[slot] = -bad[slot];
    assert!(!feasible(&admitted, &instance, &bad));
    assert!(energy(&instance, &bad) > 0);
    assert_eq!(energy(&instance, &bad), direct_loss(&meta, &bad));
    // A flipped border link touches no labeled vertex.
    let border = (0..lattice.link_count())
        .find(|&e| {
            let link = lattice.link(e).expect("link");
            [link.tail, link.head]
                .iter()
                .all(|&v| meta.labels[v as usize] == FREE)
        })
        .expect("a border link");
    let mut still = good.clone();
    let slot = meta.positions[border as usize] as usize;
    still[slot] = -still[slot];
    assert!(feasible(&admitted, &instance, &still));
    // A witness of the wrong length or with a bad entry is infeasible.
    assert!(!feasible(&admitted, &instance, &good[1..]));
    let mut long = good.clone();
    long.push(1);
    assert!(!feasible(&admitted, &instance, &long));
    let mut zero = good;
    zero[0] = 0;
    assert!(!feasible(&admitted, &instance, &zero));
}

#[test]
fn windows_skip_dead_cells() {
    // z(3, 1): a 5 by 5 cell grid and one line, so a damaged cell is dead.
    let (m, t) = (3u32, 1u32);
    let bytes = params(3, 1, 2, 0);
    let window = |topology: &BaseTopology| {
        let meta = decode(&build(&bytes, topology, nonce_of(3)));
        (meta.c0, meta.r0)
    };
    assert_eq!(window(&lattice_topology(3, 1)), (1, 1));
    // A dead qubit in cell (1, 1): its vertical low corner.
    let dead = seg_id(m, t, 0, 1, 0, 0);
    assert_eq!(window(&damaged(3, 1, &[dead], &[])), (1, 2));
    // A missing coupler in cell (1, 1): its horizontal pair.
    let left = seg_id(m, t, 1, 1, 0, 0);
    let right = seg_id(m, t, 1, 1, 0, 1);
    let cut = (left.min(right), left.max(right));
    assert_eq!(window(&damaged(3, 1, &[], &[cut])), (1, 2));
    // Cell (3, 3) is in every 3 by 3 window, so no window is alive.
    let center = seg_id(m, t, 0, 3, 0, 2);
    let topology = damaged(3, 1, &[center], &[]);
    let admitted = admitted(&bytes).expect("admit");
    assert_eq!(
        bounds(&admitted, &topology),
        Err(Code::TopologyIncompatible)
    );
}

#[test]
fn admission_codes() {
    let topology = lattice_topology(2, 4);
    let code = |bytes: Vec<u8>| match admitted(&bytes) {
        Err(code) => code,
        Ok(admitted) => bounds(&admitted, &topology).expect_err("refused"),
    };
    // pairs above (L - 1)^2 / 2 and nothing else wrong: L = 2 has 1 interior
    // vertex and fits z(2, 4).
    assert_eq!(code(params(2, 4, 2, 1)), Code::TopologyIncompatible);
    // L + 1 above 2m - 1.
    assert_eq!(code(params(2, 4, 3, 0)), Code::TopologyIncompatible);
    // side and pairs outside the schema.
    assert_eq!(code(params(2, 4, 1, 0)), Code::ParamsOutOfBounds);
    assert_eq!(code(params(2, 4, 65, 0)), Code::ParamsOutOfBounds);
    assert_eq!(code(params(2, 4, 2, 2049)), Code::ParamsOutOfBounds);
    assert_eq!(POLARITON_SCHEMA.len(), 4);
    // The boundary case builds.
    assert!(admitted(&params(2, 4, 2, 0)).is_ok());
}

/// The labels drawn by the stream of `nonce`, tries 1 to 4, and the try that
/// passes `check_request`.
fn replay(
    bytes: &[u8],
    side: u16,
    pairs: usize,
    nonce: [u8; 32],
    order: [u8; 32],
) -> Option<(usize, Vec<Option<i8>>)> {
    let admitted = admitted(bytes).expect("admit");
    let mut streams = Streams::new(
        nonce,
        admitted.descriptor().digest(),
        order,
        POLARITON.labels,
    );
    let mut rng = streams.open("charges").expect("charges stream");
    let lattice = SquareLattice::new(side, Boundary::Open).expect("lattice");
    let width = u32::from(side) + 1;
    let interior: Vec<u32> = (1..u32::from(side))
        .flat_map(|y| (1..u32::from(side)).map(move |x| y * width + x))
        .collect();
    for attempt in 1..=4 {
        let mut order = interior.clone();
        shuffle(&mut rng, &mut order).expect("shuffle");
        let mut labels = vec![None; lattice.vertex_count() as usize];
        for &v in &order[..pairs] {
            labels[v as usize] = Some(2);
        }
        for &v in &order[pairs..2 * pairs] {
            labels[v as usize] = Some(-2);
        }
        for &v in &order[2 * pairs..] {
            labels[v as usize] = Some(0);
        }
        let request = Request {
            charges: labels.clone(),
            fixed: vec![],
            fields: vec![],
        };
        if check_request(&lattice, &request).is_ok() {
            return Some((attempt, labels));
        }
    }
    None
}

#[test]
fn charges_are_balanced_and_interior() {
    let bytes = params(3, 4, 4, 1);
    let topology = lattice_topology(3, 4);
    for i in 0..8 {
        let instance = build(&bytes, &topology, nonce_of(i));
        let meta = decode(&instance);
        assert_eq!(meta.side, 4);
        assert_eq!(meta.labels.len(), 25);
        assert_eq!(meta.labels.iter().filter(|&&l| l == 2).count(), 1);
        assert_eq!(meta.labels.iter().filter(|&&l| l == -2).count(), 1);
        for (v, &label) in meta.labels.iter().enumerate() {
            let (x, y) = (v % 5, v / 5);
            if x == 0 || y == 0 || x == 4 || y == 4 {
                assert_eq!(label, FREE, "boundary vertex {v}");
            } else {
                assert!([-2, 0, 2].contains(&label), "interior vertex {v}");
            }
        }
        let (_, expected) =
            replay(&bytes, 4, 1, nonce_of(i), *topology.order_digest()).expect("a draw passes");
        let want: Vec<i8> = expected.iter().map(|l| l.unwrap_or(FREE)).collect();
        assert_eq!(meta.labels, want, "the stream replay");
    }
}

#[test]
fn infeasible_draws_retry_then_give_14() {
    // L = 5 on z(4, 4): 16 interior vertices, all charged, so a draw can
    // cluster charges of one sign and fail check_request.
    let bytes = params(4, 4, 5, 8);
    let topology = lattice_topology(4, 4);
    let mut retried = None;
    let mut exhausted = None;
    for i in 0..4000 {
        let nonce = nonce_of(i);
        match replay(&bytes, 5, 8, nonce, *topology.order_digest()) {
            Some((attempt, _)) if attempt > 1 && retried.is_none() => retried = Some((i, attempt)),
            None if exhausted.is_none() => exhausted = Some(i),
            _ => {}
        }
        if retried.is_some() && exhausted.is_some() {
            break;
        }
    }
    let (i, attempt) = retried.expect("a salt that passes after a failed try");
    let instance = build(&bytes, &topology, nonce_of(i));
    let (_, labels) = replay(&bytes, 5, 8, nonce_of(i), *topology.order_digest()).expect("passes");
    let want: Vec<i8> = labels.iter().map(|l| l.unwrap_or(FREE)).collect();
    assert_eq!(
        decode(&instance).labels,
        want,
        "try {attempt} supplies the labels"
    );
    let i = exhausted.expect("a salt that fails all four tries");
    assert!(matches!(
        try_build(&bytes, &topology, nonce_of(i)),
        Err(SaltFault::Skip(Code::NoInstance))
    ));
}

#[test]
fn every_coupler_is_a_topology_edge() {
    let bytes = params(3, 4, 4, 1);
    let topology = lattice_topology(3, 4);
    let instance = build(&bytes, &topology, nonce_of(4));
    let native: BTreeSet<(u32, u32)> = topology
        .edges()
        .iter()
        .map(|&(a, b)| {
            let (a, b) = (
                topology.node_ids()[a as usize],
                topology.node_ids()[b as usize],
            );
            (a.min(b), a.max(b))
        })
        .collect();
    assert!(!instance.edges().is_empty());
    for &(a, b) in instance.edges() {
        let (a, b) = (
            instance.node_ids()[a as usize],
            instance.node_ids()[b as usize],
        );
        assert!(native.contains(&(a.min(b), a.max(b))), "coupler {a} {b}");
    }
    // The nodes follow topology registration order.
    assert!(instance.node_ids().windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn check_metadata_rejects_each_rule() {
    let bytes = params(3, 4, 4, 1);
    let instance = build(&bytes, &lattice_topology(3, 4), nonce_of(5));
    let meta = decode(&instance);
    let accepts = |candidate: &Instance| {
        (POLARITON.check_metadata)(admitted(&bytes).expect("admit").params(), candidate)
    };
    assert!(accepts(&instance));
    // The same mutation with the energy rebuilt from the mutated labels.
    let with_energy = |mutated: &Meta| {
        let (h, edges, j, offset) = expected_energy(mutated);
        rebuilt(&instance, edges, h, j, offset, encode(mutated))
    };
    // An unmutated rebuild passes, so the helper is sound.
    assert!(accepts(&with_energy(&meta)));
    // A label on a boundary vertex.
    let mut m = meta.clone();
    m.labels[0] = 0;
    assert!(!accepts(&with_energy(&m)));
    // An interior label outside -2, 0, +2.
    let mut m = meta.clone();
    let zero = m.labels.iter().position(|&l| l == 0).expect("a zero label");
    m.labels[zero] = 1;
    assert!(!accepts(&with_energy(&m)));
    // A wrong count of +2 labels.
    let mut m = meta.clone();
    let plus = m.labels.iter().position(|&l| l == 2).expect("a +2 label");
    m.labels[plus] = 0;
    assert!(!accepts(&with_energy(&m)));
    // A wrong count of -2 labels.
    let mut m = meta.clone();
    let minus = m.labels.iter().position(|&l| l == -2).expect("a -2 label");
    m.labels[minus] = 0;
    assert!(!accepts(&with_energy(&m)));
    // The side differs from the parameter.
    let mut m = meta.clone();
    m.side = 3;
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A link position used twice.
    let mut m = meta.clone();
    m.positions[1] = m.positions[0];
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A link position outside the nodes.
    let mut m = meta.clone();
    m.positions[0] = instance.node_ids().len() as u32;
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A window outside the grid.
    let mut m = meta.clone();
    m.c0 = 0;
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // Swapped link positions with the old coefficients.
    let mut m = meta;
    m.positions.swap(2, 3);
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A coefficient that differs from the rebuilt energy.
    let mut h = instance.h().to_vec();
    let at = h.iter().position(|&v| v != 0).expect("a field");
    h[at] += 1;
    assert!(!accepts(&rebuilt(
        &instance,
        instance.edges().to_vec(),
        h,
        instance.j().to_vec(),
        instance.offset_milli(),
        instance.metadata().to_vec(),
    )));
    let mut j = instance.j().to_vec();
    j[0] = -j[0];
    assert!(!accepts(&rebuilt(
        &instance,
        instance.edges().to_vec(),
        instance.h().to_vec(),
        j,
        instance.offset_milli(),
        instance.metadata().to_vec(),
    )));
    assert!(!accepts(&rebuilt(
        &instance,
        instance.edges().to_vec(),
        instance.h().to_vec(),
        instance.j().to_vec(),
        instance.offset_milli() + 1,
        instance.metadata().to_vec(),
    )));
    // A dropped coupler.
    let mut edges = instance.edges().to_vec();
    let mut j = instance.j().to_vec();
    let _ = edges.pop();
    let _ = j.pop();
    assert!(!accepts(&rebuilt(
        &instance,
        edges,
        instance.h().to_vec(),
        j,
        instance.offset_milli(),
        instance.metadata().to_vec(),
    )));
    // Trailing bytes after the sections.
    let mut region = instance.metadata().to_vec();
    region.push(0);
    let parts_ok = Instance::new(
        InstanceParts {
            descriptor_digest: *instance.descriptor_digest(),
            topology_order_digest: *instance.topology_order_digest(),
            nonce: instance.nonce().copied(),
            node_ids: instance.node_ids().to_vec(),
            edges: instance.edges().to_vec(),
            h: instance.h().to_vec(),
            j: instance.j().to_vec(),
            offset_milli: instance.offset_milli(),
            metadata: region,
        },
        &POLARITON_V1.limits,
        POLARITON_V1.metadata_kinds,
        None,
    );
    assert!(parts_ok.is_err() || !accepts(&parts_ok.expect("checked")));
}

#[test]
fn observables_report_static_charges_only() {
    let bytes = params(3, 4, 4, 1);
    let instance = build(&bytes, &lattice_topology(3, 4), nonce_of(6));
    let meta = decode(&instance);
    let admitted = admitted(&bytes).expect("admit");
    let good = feasible_state(&instance, &meta);
    let seen = polariton_observables(admitted.params(), &instance, &good).expect("observables");
    assert_eq!(seen.violations, 0);
    assert_eq!(seen.loss_milli, 0);
    assert_eq!(seen.charges.len(), 25);
    for (&got, &label) in seen.charges.iter().zip(&meta.labels) {
        if label != FREE {
            assert_eq!(got, label);
        }
    }
    let mut bad = good;
    let lattice = SquareLattice::new(4, Boundary::Open).expect("lattice");
    let link = (0..lattice.link_count())
        .find(|&e| {
            let l = lattice.link(e).expect("link");
            meta.labels[l.tail as usize] != FREE && meta.labels[l.head as usize] != FREE
        })
        .expect("a link between labeled vertices");
    let slot = meta.positions[link as usize] as usize;
    bad[slot] = -bad[slot];
    let seen = polariton_observables(admitted.params(), &instance, &bad).expect("observables");
    assert_eq!(seen.violations, 2);
    assert_eq!(seen.loss_milli, direct_loss(&meta, &bad) as u64);
    assert_eq!(seen.loss_milli, energy(&instance, &bad) as u64);
    assert!(polariton_observables(admitted.params(), &instance, &bad[1..]).is_none());
}

#[test]
fn generation_is_repeatable() {
    let bytes = params(3, 4, 4, 1);
    let topology = lattice_topology(3, 4);
    let a = build(&bytes, &topology, nonce_of(7));
    let b = build(&bytes, &topology, nonce_of(7));
    assert_eq!(a, b);
    assert_eq!(a.digest(), b.digest());
    let others: BTreeSet<Vec<i8>> = (0..16)
        .map(|i| decode(&build(&bytes, &topology, nonce_of(i))).labels)
        .collect();
    assert!(others.len() > 1, "the nonce changes the charges");
}

#[test]
fn recipes_are_canonical() {
    let expected = [
        ("l4", "polariton-l4", 4u16, 1u16),
        ("l8", "polariton-l8", 8, 6),
        ("l9", "polariton-l9", 9, 8),
    ];
    assert_eq!(POLARITON_RECIPES.len(), 3);
    for (recipe, (alias, cell, side, pairs)) in POLARITON_RECIPES.iter().zip(expected) {
        assert_eq!(recipe.alias, alias);
        assert_eq!(recipe.cell, cell);
        assert_eq!(recipe.params, params(12, 4, side, pairs).as_slice());
        assert_eq!(pairs, (side - 1) * (side - 1) / 8);
        assert!(admitted(recipe.params).is_ok());
    }
}

#[test]
fn live_recipes_build() {
    let topology = common::live_topology();
    for recipe in &POLARITON_RECIPES {
        let admitted = admitted(recipe.params).expect("admit");
        let bounds = bounds(&admitted, topology).expect("admit topology");
        let mut built = None;
        for i in 0..64 {
            if let Ok(instance) = generate_instance(&admitted, topology, &bounds, nonce_of(i)) {
                built = Some((i, instance));
                break;
            }
        }
        let (i, instance) = built.expect("a salt builds");
        let meta = decode(&instance);
        let side = u32::from(meta.side);
        assert_eq!(instance.node_ids().len() as u32, 2 * side * (side + 1));
        let pairs = (side - 1) * (side - 1) / 8;
        let (attempt, _) = replay(
            recipe.params,
            meta.side,
            pairs as usize,
            nonce_of(i),
            *topology.order_digest(),
        )
        .expect("a draw passes");
        println!(
            "RECIPE {} window ({}, {}) salt {i} try {attempt}",
            recipe.alias, meta.c0, meta.r0
        );
    }
}

#[test]
fn a_side_16_request_has_no_window_on_the_live_topology() {
    // Side 16 builds only on a window without dead qubits. The live z(12, 4)
    // topology has none: its largest alive block of cells is 10 by 10.
    let topology = common::live_topology();
    let bytes = params(12, 4, 16, 28);
    let admitted = admitted(&bytes).expect("admit");
    assert_eq!(bounds(&admitted, topology), Err(Code::TopologyIncompatible));
}
