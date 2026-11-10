//! The `quip/defects-static@1` construction: the energy against the direct
//! definition, feasibility, the dropped couplings, admission codes, the draw
//! order, the metadata check, and repeatability.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::many_single_char_names,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;

use ising_generators::defects::{
    check_route, defect_observables, RouteError, DEFECTS, DEFECTS_SCHEMA, DEFECTS_V1,
};
use ising_generators::dipolar::{
    quantize, Axis, Calibration, Entry, Geometry, Island, PhysicalStatus, Table,
};
use ising_generators::family::{feasible, generate_instance, BaseTopology, SaltFault, Streams};
use ising_generators::ice::zephyr_lattice;
use ising_generators::instance::{Instance, InstanceLimits, InstanceParts};
use ising_generators::rng::shuffle;
use ising_generators::square::{
    charges, check_request, Axis as LinkAxis, Boundary, Request, SquareLattice,
};
use ising_generators::Code;

#[path = "common/admit.rs"]
mod admit;
#[path = "common/lattice.rs"]
mod lattice;
#[path = "common/size_bounds.rs"]
mod size_bounds;

use admit::admitted;
use lattice::lattice_topology;
use size_bounds::bounds;

/// The labels value of a free vertex.
const FREE: i8 = 127;

/// The parameters of a case.
#[derive(Clone, Copy, Debug)]
struct Case {
    m: u16,
    t: u8,
    side: u16,
    pairs: u16,
    obstacles: u16,
    pins: u16,
    field_x: i32,
    field_y: i32,
}

impl Case {
    /// Canonical parameter bytes: format, then `(tag, len, payload)` per
    /// field.
    fn bytes(self) -> Vec<u8> {
        let mut bytes = vec![0x01];
        bytes.extend_from_slice(&[1, 0, 2, 0, 0, 0]);
        bytes.extend_from_slice(&self.m.to_le_bytes());
        bytes.extend_from_slice(&[2, 0, 1, 0, 0, 0, self.t]);
        for (tag, value) in [
            (3u8, self.side),
            (4, self.pairs),
            (5, self.obstacles),
            (6, self.pins),
        ] {
            bytes.extend_from_slice(&[tag, 0, 2, 0, 0, 0]);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for (tag, value) in [(7u8, self.field_x), (8, self.field_y)] {
            bytes.extend_from_slice(&[tag, 0, 4, 0, 0, 0]);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn topology(self) -> BaseTopology {
        lattice_topology(self.m, self.t)
    }
}

/// L = 2 on z(2, 4), with two pins and a field in both directions.
const L2: Case = Case {
    m: 2,
    t: 4,
    side: 2,
    pairs: 0,
    obstacles: 0,
    pins: 2,
    field_x: 300,
    field_y: -200,
};

/// L = 3 on z(3, 4), with one pair, one obstacle, and two pins.
const L3: Case = Case {
    m: 3,
    t: 4,
    side: 3,
    pairs: 1,
    obstacles: 1,
    pins: 2,
    field_x: 1000,
    field_y: -1000,
};

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
    let admitted = admitted(&DEFECTS_V1, bytes).expect("admit");
    let bounds = bounds(&admitted, topology).expect("admit topology");
    generate_instance(&admitted, topology, &bounds, nonce)
}

fn build(case: Case, nonce: u32) -> Instance {
    let built = try_build(&case.bytes(), &case.topology(), nonce_of(nonce))
        .map_err(|fault| format!("{fault:?}"));
    assert!(built.is_ok(), "{case:?}: generate: {built:?}");
    built.expect("generate")
}

/// The Zephyr id of the segment `(u, w, k, p)` of `z(m, t)`.
fn seg_id(m: u32, t: u32, u: u32, w: u32, k: u32, p: u32) -> u32 {
    (((u * (2 * m + 1) + w) * t + k) * 2 + p % 2) * m + p / 2
}

/// The decoded metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Meta {
    side: u16,
    labels: Vec<i8>,
    c0: u16,
    r0: u16,
    positions: Vec<u32>,
    weight: u64,
    dropped: u64,
    commitment: [u8; 32],
    obstacles: Vec<u32>,
    pins: Vec<u32>,
}

fn decode(what: &str, instance: &Instance) -> Meta {
    let region = instance.metadata();
    assert_eq!(&region[..2], &[3, 0], "{what}: three sections");
    let mut at = 2;
    let mut payloads = Vec::new();
    for kind in [4u16, 5, 8] {
        assert_eq!(
            &region[at..at + 2],
            &kind.to_le_bytes(),
            "{what}: kind {kind}"
        );
        let len = u32::from_le_bytes(region[at + 2..at + 6].try_into().expect("four")) as usize;
        payloads.push(&region[at + 6..at + 6 + len]);
        at += 6 + len;
    }
    assert_eq!(at, region.len(), "{what}: no trailing bytes");
    let u16_at =
        |bytes: &[u8], at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().expect("two"));
    let u32_at =
        |bytes: &[u8], at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four"));
    let u64_at =
        |bytes: &[u8], at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight"));
    let (charges, incidence, defects) = (payloads[0], payloads[1], payloads[2]);
    let ids = |at: &mut usize| {
        let n = usize::from(u16_at(defects, *at));
        let list: Vec<u32> = (0..n).map(|k| u32_at(defects, *at + 2 + 4 * k)).collect();
        *at += 2 + 4 * n;
        list
    };
    let mut cursor = 48;
    let obstacles = ids(&mut cursor);
    let pins = ids(&mut cursor);
    assert_eq!(
        cursor,
        defects.len(),
        "{what}: kind 8 has no trailing bytes"
    );
    Meta {
        side: u16_at(charges, 0),
        labels: charges[2..].iter().map(|&b| b as i8).collect(),
        c0: u16_at(incidence, 0),
        r0: u16_at(incidence, 2),
        positions: incidence[4..]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().expect("four")))
            .collect(),
        weight: u64_at(defects, 0),
        dropped: u64_at(defects, 8),
        commitment: defects[16..48].try_into().expect("32"),
        obstacles,
        pins,
    }
}

/// The payload of kind 8.
fn defects_payload(meta: &Meta) -> Vec<u8> {
    let mut payload = meta.weight.to_le_bytes().to_vec();
    payload.extend_from_slice(&meta.dropped.to_le_bytes());
    payload.extend_from_slice(&meta.commitment);
    for list in [&meta.obstacles, &meta.pins] {
        payload.extend_from_slice(&(list.len() as u16).to_le_bytes());
        for id in list {
            payload.extend_from_slice(&id.to_le_bytes());
        }
    }
    payload
}

fn region_of(sections: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut region = (sections.len() as u16).to_le_bytes().to_vec();
    for (kind, payload) in sections {
        region.extend_from_slice(&kind.to_le_bytes());
        region.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        region.extend_from_slice(payload);
    }
    region
}

fn encode(meta: &Meta) -> Vec<u8> {
    let mut charges = meta.side.to_le_bytes().to_vec();
    charges.extend(meta.labels.iter().map(|&l| l as u8));
    let mut incidence = meta.c0.to_le_bytes().to_vec();
    incidence.extend_from_slice(&meta.r0.to_le_bytes());
    for p in &meta.positions {
        incidence.extend_from_slice(&p.to_le_bytes());
    }
    region_of(&[(4, charges), (5, incidence), (8, defects_payload(meta))])
}

/// A copy of `instance` with new fields and metadata. The protocol limits
/// apply, so a mutated weight can build.
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
        &InstanceLimits::FAM_37,
        DEFECTS_V1.metadata_kinds,
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

/// The table of the lattice, written from the definition of the geometry and
/// not from the generator: link `e` is an island at the link midpoint.
fn reference_table(lattice: &SquareLattice) -> Table {
    let width = u32::from(lattice.side()) + 1;
    let islands: Vec<Island> = (0..lattice.link_count())
        .map(|e| {
            let link = lattice.link(e).expect("link");
            let (x, y) = ((link.tail % width) as u16, (link.tail / width) as u16);
            match link.axis {
                LinkAxis::X => Island {
                    x: 2 * x + 1,
                    y: 2 * y,
                    axis: Axis::PlusX,
                    moment: 1,
                },
                LinkAxis::Y => Island {
                    x: 2 * x,
                    y: 2 * y + 1,
                    axis: Axis::PlusY,
                    moment: 1,
                },
            }
        })
        .collect();
    let geometry = Geometry::new(islands, 1, 1000, 4).expect("geometry");
    let entries = quantize(&geometry).expect("quantize");
    Table::new(geometry, entries, Calibration::Oracle).expect("table")
}

/// Whether links `i` and `j` share a lattice vertex.
fn share(lattice: &SquareLattice, i: u32, j: u32) -> bool {
    let (a, b) = (
        lattice.link(i).expect("link"),
        lattice.link(j).expect("link"),
    );
    [a.tail, a.head]
        .iter()
        .any(|v| *v == b.tail || *v == b.head)
}

/// The kept entries and the sum of `|q|` over the dropped entries.
fn split(lattice: &SquareLattice, table: &Table) -> (Vec<Entry>, u64) {
    let mut kept = Vec::new();
    let mut dropped = 0;
    for entry in table.entries() {
        if share(lattice, entry.i, entry.j) {
            kept.push(*entry);
        } else {
            dropped += u64::from(entry.q.unsigned_abs());
        }
    }
    (kept, dropped)
}

/// The penalty weight from the decision D4, written from the definition.
fn expected_weight(side: u16, kept: &[Entry], case: Case) -> u64 {
    let sum_j: u64 = kept.iter().map(|e| u64::from(e.q.unsigned_abs())).sum();
    let links_per_axis = u64::from(side) * (u64::from(side) + 1);
    let sum_h = links_per_axis
        * (u64::from(case.field_x.unsigned_abs()) + u64::from(case.field_y.unsigned_abs()));
    let range = 2 * (sum_j + sum_h);
    let u = range / 2 + 1;
    u + u % 2
}

/// The instance coefficients written from the definition, with the weight and
/// the request taken from `meta`: `(h, edges, j, offset)`.
type Coefficients = (Vec<i32>, Vec<(u32, u32)>, Vec<i32>, i64);

fn expected_energy(meta: &Meta, kept: &[Entry], case: Case) -> Coefficients {
    let lattice = SquareLattice::new(meta.side, Boundary::Open).expect("lattice");
    let n = meta.positions.len();
    let u = meta.weight as i64;
    let mut h = vec![0i64; n];
    let mut pair: std::collections::BTreeMap<(u32, u32), i64> = std::collections::BTreeMap::new();
    let mut offset = 0i64;
    for e in 0..lattice.link_count() {
        let field = match lattice.link(e).expect("link").axis {
            LinkAxis::X => case.field_x,
            LinkAxis::Y => case.field_y,
        };
        h[meta.positions[e as usize] as usize] -= i64::from(field);
    }
    for entry in kept {
        let (a, b) = (
            meta.positions[entry.i as usize],
            meta.positions[entry.j as usize],
        );
        *pair.entry((a.min(b), a.max(b))).or_default() += i64::from(entry.q);
    }
    for v in 0..lattice.vertex_count() {
        let label = meta.labels[v as usize];
        if label == FREE {
            continue;
        }
        let q = i64::from(label);
        let at = lattice.incident(v);
        offset += u / 2 * (at.len() as i64 + q * q);
        for (a, &(e, ie)) in at.iter().enumerate() {
            let pe = meta.positions[e as usize];
            h[pe as usize] -= u * q * i64::from(ie);
            for &(f, jf) in &at[a + 1..] {
                let pf = meta.positions[f as usize];
                *pair.entry((pe.min(pf), pe.max(pf))).or_default() +=
                    u * i64::from(ie) * i64::from(jf);
            }
        }
    }
    for &e in &meta.pins {
        h[meta.positions[e as usize] as usize] -= u;
        offset += u;
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

/// The pieces of the direct energy of a state.
struct Direct {
    physical: i64,
    label_penalty: i64,
    violated_pins: i64,
    labels_hold: bool,
}

/// The energy of `spins` written from the definition: the physical energy over
/// the kept couplings and the field, the charge penalty from
/// `square::charges`, and the pin count.
fn direct(meta: &Meta, kept: &[Entry], case: Case, spins: &[i8]) -> Direct {
    let lattice = SquareLattice::new(meta.side, Boundary::Open).expect("lattice");
    let link_spins: Vec<i8> = meta.positions.iter().map(|&p| spins[p as usize]).collect();
    let mut physical = 0i64;
    for entry in kept {
        physical += i64::from(entry.q)
            * i64::from(link_spins[entry.i as usize])
            * i64::from(link_spins[entry.j as usize]);
    }
    for e in 0..lattice.link_count() {
        let field = match lattice.link(e).expect("link").axis {
            LinkAxis::X => case.field_x,
            LinkAxis::Y => case.field_y,
        };
        physical -= i64::from(field) * i64::from(link_spins[e as usize]);
    }
    let q = charges(&lattice, &link_spins).expect("charges");
    let squares: i64 = q
        .iter()
        .zip(&meta.labels)
        .filter(|(_, &label)| label != FREE)
        .map(|(&got, &label)| (i64::from(got) - i64::from(label)).pow(2))
        .sum();
    let violated_pins = meta
        .pins
        .iter()
        .filter(|&&e| link_spins[e as usize] != 1)
        .count() as i64;
    Direct {
        physical,
        label_penalty: meta.weight as i64 / 2 * squares,
        violated_pins,
        labels_hold: squares == 0,
    }
}

impl Direct {
    /// A violated pin costs `2 U`, the cost of `U * (1 - s)` at `s = -1`.
    fn instance_energy(&self, weight: u64) -> i64 {
        self.physical + self.label_penalty + 2 * weight as i64 * self.violated_pins
    }

    fn feasible(&self) -> bool {
        self.labels_hold && self.violated_pins == 0
    }
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

/// A feasible state of the instance, found by descent on the violations from
/// random starts.
fn feasible_state(meta: &Meta, kept: &[Entry], case: Case) -> Vec<i8> {
    let n = meta.positions.len();
    let mut seed = 5u64;
    let violations = |spins: &[i8]| {
        let d = direct(meta, kept, case, spins);
        d.label_penalty + d.violated_pins * 4
    };
    for _ in 0..400 {
        let mut spins: Vec<i8> = (0..n)
            .map(|_| if next(&mut seed) & 1 == 1 { 1 } else { -1 })
            .collect();
        let mut cost = violations(&spins);
        for _ in 0..60 * n {
            if cost == 0 {
                return spins;
            }
            let i = next(&mut seed) as usize % n;
            spins[i] = -spins[i];
            let after = violations(&spins);
            if after <= cost {
                cost = after;
            } else {
                spins[i] = -spins[i];
            }
        }
        if cost == 0 {
            return spins;
        }
    }
    panic!("no feasible state found");
}

// ---- tests ----

#[test]
fn every_l2_state_matches_the_direct_energy() {
    let instance = build(L2, 1);
    let meta = decode("l2", &instance);
    let lattice = SquareLattice::new(2, Boundary::Open).expect("lattice");
    let (kept, _) = split(&lattice, &reference_table(&lattice));
    let bytes = L2.bytes();
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    assert_eq!(instance.node_ids().len(), 12);
    assert_eq!(meta.weight, expected_weight(2, &kept, L2));
    assert_eq!(meta.weight, 12982);
    assert_eq!(meta.pins.len(), 2);
    let mut feasible_count = 0;
    for bits in 0..4096u64 {
        let spins = state(bits, 12);
        let d = direct(&meta, &kept, L2, &spins);
        assert_eq!(
            energy(&instance, &spins),
            d.instance_energy(meta.weight),
            "state {bits}"
        );
        assert_eq!(
            feasible(&admitted, &instance, &spins),
            d.feasible(),
            "state {bits}"
        );
        feasible_count += i32::from(d.feasible());
    }
    assert!(feasible_count > 0, "some state is feasible");
}

#[test]
fn infeasible_states_never_beat_feasible_ones() {
    let instance = build(L2, 2);
    let meta = decode("l2", &instance);
    let lattice = SquareLattice::new(2, Boundary::Open).expect("lattice");
    let (kept, _) = split(&lattice, &reference_table(&lattice));
    let mut feasible_max = i64::MIN;
    let mut infeasible_min = i64::MAX;
    let mut physical_min = i64::MAX;
    let mut physical_max = i64::MIN;
    let mut best_feasible = (i64::MAX, 0u64);
    for bits in 0..4096u64 {
        let spins = state(bits, 12);
        let d = direct(&meta, &kept, L2, &spins);
        let e = energy(&instance, &spins);
        physical_min = physical_min.min(d.physical);
        physical_max = physical_max.max(d.physical);
        if d.feasible() {
            feasible_max = feasible_max.max(e);
            if d.physical < best_feasible.0 {
                best_feasible = (d.physical, bits);
            }
        } else {
            infeasible_min = infeasible_min.min(e);
        }
    }
    assert!(
        infeasible_min > feasible_max,
        "every infeasible state is above every feasible state"
    );
    // The weight bound of D4 covers the range of the physical energy.
    assert!(2 * meta.weight as i64 > physical_max - physical_min);
    // The lowest instance energy is the lowest physical energy over the
    // feasible states.
    let lowest = (0..4096u64)
        .map(|bits| (energy(&instance, &state(bits, 12)), bits))
        .min()
        .expect("states");
    let d = direct(&meta, &kept, L2, &state(lowest.1, 12));
    assert!(d.feasible());
    assert_eq!(d.physical, best_feasible.0);
}

#[test]
fn a_lower_physical_energy_with_a_bad_label_fails() {
    let bytes = L3.bytes();
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let instance = build(L3, 3);
    let meta = decode("l3", &instance);
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    let (kept, _) = split(&lattice, &reference_table(&lattice));
    let good = feasible_state(&meta, &kept, L3);
    assert!(feasible(&admitted, &instance, &good));
    let good_direct = direct(&meta, &kept, L3, &good);
    assert!(good_direct.feasible());
    // A single flip that lowers the physical energy but breaks a label.
    let mut found = 0;
    for node in 0..good.len() {
        let mut bad = good.clone();
        bad[node] = -bad[node];
        let d = direct(&meta, &kept, L3, &bad);
        if d.physical < good_direct.physical && !d.feasible() {
            found += 1;
            assert!(!feasible(&admitted, &instance, &bad), "node {node}");
            assert_eq!(energy(&instance, &bad), d.instance_energy(meta.weight));
            assert!(
                energy(&instance, &bad) > energy(&instance, &good),
                "node {node}: the penalty outweighs the physical gain"
            );
        }
    }
    assert!(
        found > 0,
        "a flip lowers the physical energy and breaks a label"
    );
    // A seeded sample: an infeasible state is never below the feasible one.
    let mut seed = 99u64;
    for _ in 0..2000 {
        let spins: Vec<i8> = (0..good.len())
            .map(|_| if next(&mut seed) & 1 == 1 { 1 } else { -1 })
            .collect();
        let d = direct(&meta, &kept, L3, &spins);
        assert_eq!(energy(&instance, &spins), d.instance_energy(meta.weight));
        assert_eq!(feasible(&admitted, &instance, &spins), d.feasible());
        if !d.feasible() {
            assert!(energy(&instance, &spins) > energy(&instance, &good));
        }
    }
    // A pin violation alone also fails, however low the energy.
    let pin = meta.positions[meta.pins[0] as usize] as usize;
    let mut unpinned = good.clone();
    unpinned[pin] = -unpinned[pin];
    assert!(!feasible(&admitted, &instance, &unpinned));
    // A witness of the wrong length or with a bad entry is infeasible.
    assert!(!feasible(&admitted, &instance, &good[1..]));
    let mut zero = good;
    zero[0] = 0;
    assert!(!feasible(&admitted, &instance, &zero));
}

#[test]
fn kept_couplings_share_a_vertex() {
    for case in [L2, L3] {
        let instance = build(case, 4);
        let meta = decode("kept", &instance);
        let lattice = SquareLattice::new(case.side, Boundary::Open).expect("lattice");
        let (kept, _) = split(&lattice, &reference_table(&lattice));
        let kept_pairs: BTreeSet<(u32, u32)> = kept.iter().map(|e| (e.i, e.j)).collect();
        // Link of each instance position.
        let mut link_at = vec![0u32; meta.positions.len()];
        for (e, &p) in meta.positions.iter().enumerate() {
            link_at[p as usize] = e as u32;
        }
        let topology = case.topology();
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
            let (i, j) = (link_at[a as usize], link_at[b as usize]);
            let pair = (i.min(j), i.max(j));
            assert!(
                share(&lattice, i, j),
                "{case:?}: links {i} {j} share a vertex"
            );
            assert!(kept_pairs.contains(&pair), "{case:?}: {pair:?} is kept");
            let (x, y) = (
                instance.node_ids()[a as usize],
                instance.node_ids()[b as usize],
            );
            assert!(
                native.contains(&(x.min(y), x.max(y))),
                "{case:?}: coupler {x} {y} is a hardware coupler"
            );
        }
        // Every kept pair has a coupler unless the charge penalty cancels it.
        let with_edge: BTreeSet<(u32, u32)> = instance
            .edges()
            .iter()
            .map(|&(a, b)| {
                let (i, j) = (link_at[a as usize], link_at[b as usize]);
                (i.min(j), i.max(j))
            })
            .collect();
        assert!(with_edge.is_subset(&kept_pairs));
        assert!(with_edge.len() * 10 >= kept_pairs.len() * 9);
    }
}

#[test]
fn dropped_entries_are_the_parallel_pairs() {
    for case in [L2, L3] {
        let instance = build(case, 5);
        let meta = decode("dropped", &instance);
        let lattice = SquareLattice::new(case.side, Boundary::Open).expect("lattice");
        let table = reference_table(&lattice);
        let (_, dropped) = split(&lattice, &table);
        assert_eq!(dropped, if case.side == 2 { 1000 } else { 2250 });
        assert_eq!(meta.dropped, dropped, "{case:?}: the stored sum");
        assert_eq!(
            meta.commitment,
            table.commitment(),
            "{case:?}: the commitment of the oracle table"
        );
        assert_eq!(table.calibration(), &Calibration::Oracle);
        // Every dropped entry is two parallel links at distance 2 that share
        // no vertex, and every parallel pair at distance 2 is dropped.
        let islands = table.geometry().islands();
        for entry in table.entries() {
            let (a, b) = (islands[entry.i as usize], islands[entry.j as usize]);
            let (dx, dy) = (
                i32::from(a.x) - i32::from(b.x),
                i32::from(a.y) - i32::from(b.y),
            );
            let parallel = a.axis == b.axis;
            let across = match a.axis {
                Axis::PlusX => dx == 0 && dy.abs() == 2,
                _ => dy == 0 && dx.abs() == 2,
            };
            let dropped_pair = !share(&lattice, entry.i, entry.j);
            assert_eq!(dropped_pair, parallel && across, "{case:?}: {entry:?}");
        }
        assert!(table.truncation_bound() > 0);
    }
}

#[test]
fn admission_codes() {
    let with = |case: Case| match admitted(&DEFECTS_V1, &case.bytes()) {
        Err(code) => code,
        Ok(admitted) => bounds(&admitted, &case.topology()).expect_err("refused"),
    };
    let ok = |case: Case| {
        let bytes = case.bytes();
        let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
        bounds(&admitted, &case.topology()).is_ok()
    };
    assert_eq!(DEFECTS_SCHEMA.len(), 8);
    // The boundary cases build.
    assert!(ok(L2));
    assert!(ok(L3));
    assert!(ok(Case { pins: 8, ..L2 }));
    assert!(ok(Case {
        pairs: 2,
        obstacles: 0,
        pins: 12,
        ..L3
    }));
    assert!(ok(Case {
        pairs: 0,
        obstacles: 4,
        ..L3
    }));
    // Side 4 is outside the schema.
    assert_eq!(
        with(Case {
            m: 4,
            side: 4,
            pairs: 0,
            obstacles: 0,
            pins: 0,
            ..L3
        }),
        Code::ParamsOutOfBounds
    );
    // The other schema limits.
    assert_eq!(with(Case { pairs: 3, ..L3 }), Code::ParamsOutOfBounds);
    assert_eq!(with(Case { obstacles: 5, ..L3 }), Code::ParamsOutOfBounds);
    assert_eq!(with(Case { pins: 13, ..L3 }), Code::ParamsOutOfBounds);
    assert_eq!(
        with(Case {
            field_x: 1001,
            ..L3
        }),
        Code::ParamsOutOfBounds
    );
    assert_eq!(
        with(Case {
            field_y: -1001,
            ..L3
        }),
        Code::ParamsOutOfBounds
    );
    // 2 * pairs + obstacles above (L - 1)^2: L = 2 has one interior vertex.
    assert_eq!(
        with(Case {
            pairs: 1,
            obstacles: 0,
            ..L2
        }),
        Code::TopologyIncompatible
    );
    assert_eq!(
        with(Case { obstacles: 2, ..L2 }),
        Code::TopologyIncompatible
    );
    // L = 3 has four interior vertices.
    assert_eq!(
        with(Case {
            pairs: 2,
            obstacles: 1,
            ..L3
        }),
        Code::TopologyIncompatible
    );
    assert_eq!(
        with(Case {
            pairs: 1,
            obstacles: 3,
            ..L3
        }),
        Code::TopologyIncompatible
    );
    // More pins than the 4 L boundary links.
    assert_eq!(with(Case { pins: 9, ..L2 }), Code::TopologyIncompatible);
    // L + 1 above 2m - 1.
    assert_eq!(with(Case { m: 2, ..L3 }), Code::TopologyIncompatible);
    // No window: z(3, 1) has one line, so a dead qubit kills its cell, and
    // cell (3, 3) is in every 3 by 3 window.
    let case = Case {
        m: 3,
        t: 1,
        side: 2,
        pairs: 0,
        obstacles: 0,
        pins: 0,
        field_x: 0,
        field_y: 0,
    };
    let (nodes, edges) = zephyr_lattice(3, 1).expect("lattice");
    let center = seg_id(3, 1, 0, 3, 0, 2);
    let nodes: Vec<u32> = nodes.into_iter().filter(|&id| id != center).collect();
    let edges: Vec<(u32, u32)> = edges
        .into_iter()
        .filter(|&(a, b)| a != center && b != center)
        .collect();
    let damaged = BaseTopology::new(nodes, &edges).expect("topology");
    let bytes = case.bytes();
    let admitted_case = admitted(&DEFECTS_V1, &bytes).expect("admit");
    assert!(bounds(&admitted_case, &case.topology()).is_ok());
    assert_eq!(
        bounds(&admitted_case, &damaged),
        Err(Code::TopologyIncompatible)
    );
}

/// The draws of the stream of `nonce`: the labels, the obstacles, and the
/// pins of the first try, written with the crate's own stream helpers.
fn replay(case: Case, nonce: [u8; 32], order: [u8; 32]) -> (Vec<Option<i8>>, Vec<u32>, Vec<u32>) {
    let bytes = case.bytes();
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let mut streams = Streams::new(nonce, admitted.descriptor().digest(), order, DEFECTS.labels);
    let mut rng = streams.open("charges").expect("charges stream");
    let side = u32::from(case.side);
    let lattice = SquareLattice::new(case.side, Boundary::Open).expect("lattice");
    let width = side + 1;
    let mut interior: Vec<u32> = (1..side)
        .flat_map(|y| (1..side).map(move |x| y * width + x))
        .collect();
    shuffle(&mut rng, &mut interior).expect("shuffle");
    let pairs = usize::from(case.pairs);
    let mut labels = vec![None; lattice.vertex_count() as usize];
    for (rank, &v) in interior.iter().enumerate() {
        labels[v as usize] = Some(if rank < pairs {
            2
        } else if rank < 2 * pairs {
            -2
        } else {
            0
        });
    }
    let mut zeros: Vec<u32> = (0..lattice.vertex_count())
        .filter(|&v| labels[v as usize] == Some(0))
        .collect();
    shuffle(&mut rng, &mut zeros).expect("shuffle");
    let mut obstacles = zeros[..usize::from(case.obstacles)].to_vec();
    obstacles.sort_unstable();
    let border = |v: u32| {
        v.is_multiple_of(width) || v / width == 0 || v % width == side || v / width == side
    };
    let mut links: Vec<u32> = (0..lattice.link_count())
        .filter(|&e| {
            let link = lattice.link(e).expect("link");
            border(link.tail) && border(link.head)
        })
        .collect();
    assert_eq!(links.len(), 4 * usize::from(case.side));
    shuffle(&mut rng, &mut links).expect("shuffle");
    let mut pins = links[..usize::from(case.pins)].to_vec();
    pins.sort_unstable();
    (labels, obstacles, pins)
}

#[test]
fn draws_follow_the_order() {
    let topology = L3.topology();
    let mut seen = BTreeSet::new();
    for i in 0..12 {
        let case = Case {
            obstacles: 1 + (i % 2) as u16,
            pairs: (i % 2) as u16,
            pins: 1 + (i % 12) as u16,
            ..L3
        };
        let instance = build(case, i);
        let meta = decode("draws", &instance);
        let (labels, obstacles, pins) = replay(case, nonce_of(i), *topology.order_digest());
        let want: Vec<i8> = labels.iter().map(|l| l.unwrap_or(FREE)).collect();
        assert_eq!(meta.labels, want, "nonce {i}: labels");
        assert_eq!(meta.obstacles, obstacles, "nonce {i}: obstacles");
        assert_eq!(meta.pins, pins, "nonce {i}: pins");
        assert!(meta.obstacles.windows(2).all(|w| w[0] < w[1]));
        assert!(meta.pins.windows(2).all(|w| w[0] < w[1]));
        for &v in &meta.obstacles {
            assert_eq!(
                meta.labels[v as usize], 0,
                "nonce {i}: obstacle {v} is zero"
            );
        }
        let _ = seen.insert((meta.labels, meta.obstacles, meta.pins));
    }
    assert!(seen.len() > 1, "the nonce changes the draw");
}

/// Every placement of `pairs` vertices of `+2` and of `-2` over the interior
/// vertices of a lattice of side `side`, as label vectors.
fn all_labelings(side: u16, pairs: usize) -> Vec<Vec<Option<i8>>> {
    let lattice = SquareLattice::new(side, Boundary::Open).expect("lattice");
    let width = u32::from(side) + 1;
    let interior: Vec<u32> = (1..u32::from(side))
        .flat_map(|y| (1..u32::from(side)).map(move |x| y * width + x))
        .collect();
    let k = interior.len();
    let mut all = Vec::new();
    for plus in 0..1u32 << k {
        for minus in 0..1u32 << k {
            if plus & minus != 0
                || plus.count_ones() as usize != pairs
                || minus.count_ones() as usize != pairs
            {
                continue;
            }
            let mut labels = vec![None; lattice.vertex_count() as usize];
            for (rank, &v) in interior.iter().enumerate() {
                labels[v as usize] = Some(if plus >> rank & 1 == 1 {
                    2
                } else if minus >> rank & 1 == 1 {
                    -2
                } else {
                    0
                });
            }
            all.push(labels);
        }
    }
    all
}

#[test]
fn infeasible_draws_retry_then_give_14() {
    // No admitted request is infeasible, so code 14 cannot happen for valid
    // parameters. The proof is in the comment of `draw`: a boundary link joins
    // two boundary vertices, so a pin never touches a labeled vertex. The
    // interior links form a directed cycle that adds charge 0 at every
    // interior vertex, and the links to the free boundary vertices give
    // charge +2, 0, or -2. Here the claim is asserted for every labeling that
    // an admitted pair count can draw, with every boundary link pinned. Fewer
    // pins only remove requirements, so this is the worst case.
    for side in [2u16, 3] {
        let lattice = SquareLattice::new(side, Boundary::Open).expect("lattice");
        let width = u32::from(side) + 1;
        let border = |v: u32| {
            v.is_multiple_of(width)
                || v / width == 0
                || v % width == u32::from(side)
                || v / width == u32::from(side)
        };
        let pinned: Vec<(u32, i8)> = (0..lattice.link_count())
            .filter(|&e| {
                let link = lattice.link(e).expect("link");
                border(link.tail) && border(link.head)
            })
            .map(|e| (e, 1))
            .collect();
        assert_eq!(pinned.len(), 4 * usize::from(side));
        let interior = (usize::from(side) - 1).pow(2);
        for pairs in 0..=2usize {
            if 2 * pairs > interior {
                continue;
            }
            let labelings = all_labelings(side, pairs);
            assert!(!labelings.is_empty());
            for labels in labelings {
                let request = Request {
                    charges: labels.clone(),
                    fixed: pinned.clone(),
                    fields: vec![],
                };
                assert_eq!(
                    check_request(&lattice, &request),
                    Ok(()),
                    "side {side} pairs {pairs}: {labels:?}"
                );
            }
        }
    }
    // Every admitted parameter set on the grid builds, for several salts, and
    // none ends in code 14.
    for (m, t, side) in [(2u16, 4u8, 2u16), (3, 4, 3)] {
        let topology = lattice_topology(m, t);
        let interior = (side - 1) * (side - 1);
        for pairs in 0..=2u16 {
            for obstacles in 0..=4u16 {
                for pins in 0..=12u16 {
                    let case = Case {
                        m,
                        t,
                        side,
                        pairs,
                        obstacles,
                        pins,
                        field_x: 250,
                        field_y: -250,
                    };
                    let fits = 2 * pairs + obstacles <= interior && pins <= 4 * side;
                    let bytes = case.bytes();
                    let Ok(admitted_case) = admitted(&DEFECTS_V1, &bytes) else {
                        panic!("{case:?}: the schema admits the grid");
                    };
                    let Ok(sizes) = bounds(&admitted_case, &topology) else {
                        assert!(!fits, "{case:?}: an admitted set is refused");
                        continue;
                    };
                    assert!(fits, "{case:?}: a refused set is admitted");
                    for nonce in 0..3 {
                        let built =
                            generate_instance(&admitted_case, &topology, &sizes, nonce_of(nonce));
                        assert!(built.is_ok(), "{case:?} nonce {nonce}: {:?}", built.err());
                    }
                }
            }
        }
    }
}

#[test]
fn check_metadata_rejects_each_rule() {
    let case = Case { pins: 3, ..L3 };
    let bytes = case.bytes();
    let instance = build(case, 6);
    let meta = decode("rules", &instance);
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    let (kept, _) = split(&lattice, &reference_table(&lattice));
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let accepts = |candidate: &Instance| (DEFECTS.check_metadata)(admitted.params(), candidate);
    assert!(accepts(&instance));
    // The same mutation with the energy rebuilt from the mutated metadata.
    let with_energy = |mutated: &Meta| {
        let (h, edges, j, offset) = expected_energy(mutated, &kept, case);
        rebuilt(&instance, edges, h, j, offset, encode(mutated))
    };
    // An unmutated rebuild passes, so the helper is sound.
    assert!(accepts(&with_energy(&meta)));
    assert_eq!(meta.obstacles.len(), 1);
    assert_eq!(meta.pins.len(), 3);

    // A label on a boundary vertex.
    let mut m = meta.clone();
    m.labels[0] = 0;
    assert!(!accepts(&with_energy(&m)));
    // An interior label outside -2, 0, +2.
    let mut m = meta.clone();
    let zero = m.labels.iter().position(|&l| l == 0).expect("a zero label");
    m.labels[zero] = 1;
    assert!(!accepts(&with_energy(&m)));
    // A wrong count of +2 labels, and of -2 labels.
    for label in [2i8, -2] {
        let mut m = meta.clone();
        let at = m.labels.iter().position(|&l| l == label).expect("a label");
        m.labels[at] = 0;
        assert!(!accepts(&with_energy(&m)));
    }
    // The side differs from the parameter.
    let mut m = meta.clone();
    m.side = 2;
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A link position used twice, and one outside the nodes.
    let mut m = meta.clone();
    m.positions[1] = m.positions[0];
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    let mut m = meta.clone();
    m.positions[0] = instance.node_ids().len() as u32;
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A window outside the grid.
    let mut m = meta.clone();
    m.c0 = 0;
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // Swapped link positions with the old coefficients.
    let mut m = meta.clone();
    m.positions.swap(2, 3);
    assert!(!accepts(&same_fields(&instance, encode(&m))));

    // A weight that breaks D4, with the energy rebuilt for it, odd or even.
    for delta in [1u64, 2] {
        let mut m = meta.clone();
        m.weight += delta;
        assert!(!accepts(&with_energy(&m)), "U + {delta}");
    }
    let mut m = meta.clone();
    m.weight -= 2;
    assert!(!accepts(&with_energy(&m)), "U - 2");
    // The old weight with the old coefficients but a stored weight that
    // differs.
    let mut m = meta.clone();
    m.weight += 2;
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A dropped sum and a table commitment that differ.
    let mut m = meta.clone();
    m.dropped += 1;
    assert!(!accepts(&with_energy(&m)));
    let mut m = meta.clone();
    m.commitment[0] ^= 1;
    assert!(!accepts(&with_energy(&m)));

    // An obstacle count that differs from the parameter.
    let mut m = meta.clone();
    m.obstacles.clear();
    assert!(!accepts(&with_energy(&m)));
    let mut m = meta.clone();
    let other = (0..lattice.vertex_count())
        .find(|&v| m.labels[v as usize] == 0 && !m.obstacles.contains(&v))
        .expect("a second zero vertex");
    m.obstacles.push(other);
    m.obstacles.sort_unstable();
    assert!(!accepts(&with_energy(&m)));
    // A repeated obstacle, with the count right.
    let mut twice = meta.clone();
    twice.obstacles = vec![meta.obstacles[0], meta.obstacles[0]];
    assert!(!accepts(&with_energy(&twice)));
    // An obstacle on a boundary vertex, and on a charged vertex.
    let mut m = meta.clone();
    m.obstacles = vec![0];
    assert!(!accepts(&with_energy(&m)));
    let mut m = meta.clone();
    let charged = m
        .labels
        .iter()
        .position(|&l| l == 2 || l == -2)
        .expect("a charged vertex") as u32;
    m.obstacles = vec![charged];
    assert!(!accepts(&with_energy(&m)));

    // A pin count that differs from the parameter.
    let mut m = meta.clone();
    let _ = m.pins.pop();
    assert!(!accepts(&with_energy(&m)));
    // A pin on a link that is not on the boundary.
    let boundary: Vec<u32> = (0..lattice.link_count())
        .filter(|&e| {
            let link = lattice.link(e).expect("link");
            let width = 4;
            let edge = |v: u32| {
                v.is_multiple_of(width) || v / width == 0 || v % width == 3 || v / width == 3
            };
            edge(link.tail) && edge(link.head)
        })
        .collect();
    let inner = (0..lattice.link_count())
        .find(|e| !boundary.contains(e))
        .expect("an inner link");
    let mut m = meta.clone();
    m.pins[0] = inner;
    m.pins.sort_unstable();
    assert!(!accepts(&with_energy(&m)));
    // A repeated pin and an unsorted pin list, with the count right.
    let mut m = meta.clone();
    m.pins[1] = m.pins[0];
    assert!(!accepts(&with_energy(&m)));
    let mut m = meta.clone();
    m.pins.reverse();
    assert!(!accepts(&with_energy(&m)));
    // A different valid pin set with its own rebuilt energy is a different
    // instance, and it passes: the pins come from the metadata.
    let mut m = meta.clone();
    let free = boundary
        .iter()
        .copied()
        .find(|e| !m.pins.contains(e))
        .expect("an unpinned boundary link");
    m.pins[0] = free;
    m.pins.sort_unstable();
    assert!(accepts(&with_energy(&m)));
    assert!(!accepts(&same_fields(&instance, encode(&m))));

    // A missing section and bytes after the last section.
    let region = encode(&meta);
    let mut short = region.clone();
    short.truncate(2 + 6 + 2 + 16);
    short[..2].copy_from_slice(&[1, 0]);
    assert!(!accepts(&same_fields(&instance, short)));
    let mut trailing = defects_payload(&meta);
    trailing.push(0);
    let mut sections = Vec::new();
    {
        let mut at = 2;
        for kind in [4u16, 5] {
            let len = u32::from_le_bytes(region[at + 2..at + 6].try_into().expect("four")) as usize;
            sections.push((kind, region[at + 6..at + 6 + len].to_vec()));
            at += 6 + len;
        }
    }
    let without_defects = region_of(&sections);
    assert!(!accepts(&same_fields(&instance, without_defects)));
    sections.push((8, trailing));
    assert!(!accepts(&same_fields(&instance, region_of(&sections))));

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
}

#[test]
fn generation_is_repeatable() {
    let topology = L3.topology();
    let first = build(L3, 7);
    let again = build(L3, 7);
    assert_eq!(first, again);
    assert_eq!(first.digest(), again.digest());
    assert_eq!(
        try_build(&L3.bytes(), &topology, nonce_of(7)).expect("builds"),
        first
    );
    // The nonce changes the instance, and the field changes the coefficients.
    let other = build(L3, 8);
    assert_ne!(first.digest(), other.digest());
    let flat = build(
        Case {
            field_x: 0,
            field_y: 0,
            ..L3
        },
        7,
    );
    assert_ne!(first.h(), flat.h());
    assert_eq!(first.node_ids(), flat.node_ids());
}

#[test]
fn limits_are_the_smallest_that_hold() {
    // The largest request at side 3: four obstacles and twelve pins give the
    // longest metadata, and every labeling and field sign gives the largest
    // coefficients and offset.
    let base = Case {
        pairs: 0,
        obstacles: 4,
        pins: 12,
        ..L3
    };
    let instance = build(base, 9);
    let meta = decode("limits", &instance);
    let limits = DEFECTS_V1.limits;
    assert_eq!(instance.metadata().len(), limits.max_metadata() as usize);
    let bytes = base.bytes();
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let sizes = bounds(&admitted, &base.topology()).expect("sizes");
    assert_eq!(sizes.nodes, u64::from(limits.max_nodes()));
    assert_eq!(sizes.edges, u64::from(limits.max_edges()));
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    let (kept, _) = split(&lattice, &reference_table(&lattice));
    let mut max_coefficient = 0u32;
    let mut max_offset = 0u64;
    let mut max_edges = 0usize;
    for (field_x, field_y) in [(1000, 1000), (1000, -1000), (-1000, 1000), (-1000, -1000)] {
        let case = Case {
            field_x,
            field_y,
            ..base
        };
        for pairs in 0..=2 {
            for labels in all_labelings(3, pairs) {
                let mut m = meta.clone();
                m.labels = labels.iter().map(|l| l.unwrap_or(FREE)).collect();
                m.weight = expected_weight(3, &kept, case);
                let (h, edges, j, offset) = expected_energy(&m, &kept, case);
                for value in h.iter().chain(&j) {
                    max_coefficient = max_coefficient.max(value.unsigned_abs());
                }
                max_offset = max_offset.max(offset.unsigned_abs());
                max_edges = max_edges.max(edges.len());
            }
        }
    }
    assert_eq!(max_coefficient, limits.max_coefficient());
    assert_eq!(max_offset, limits.max_offset());
    assert!(max_edges <= limits.max_edges() as usize);
}

// ---- route and observables ----

/// The first built instance of `case` whose metadata satisfies `want`.
fn find(case: Case, want: impl Fn(&Meta) -> bool) -> (Instance, Meta) {
    for nonce in 0..4000 {
        let instance = build(case, nonce);
        let meta = decode("find", &instance);
        if want(&meta) {
            return (instance, meta);
        }
    }
    panic!("{case:?}: no nonce gives the wanted draw");
}

/// The link that joins vertices `u` and `v` of the lattice.
fn link_between(lattice: &SquareLattice, u: u32, v: u32) -> u32 {
    lattice
        .incident(u)
        .iter()
        .map(|&(e, _)| e)
        .find(|&e| {
            let link = lattice.link(e).expect("link");
            link.tail == v || link.head == v
        })
        .expect("adjacent vertices")
}

/// The links of the paths.
fn path_links(lattice: &SquareLattice, route: &[Vec<u32>]) -> Vec<u32> {
    route
        .iter()
        .flat_map(|path| path.windows(2).map(|w| link_between(lattice, w[0], w[1])))
        .collect()
}

/// The reference state with the given links flipped to `-1`, one spin per
/// instance node.
fn flipped(meta: &Meta, links: &[u32]) -> Vec<i8> {
    let mut spins = vec![1i8; meta.positions.len()];
    for &e in links {
        spins[meta.positions[e as usize] as usize] = -1;
    }
    spins
}

/// The vertices of the 3 by 3 lattice that the tests name: `(x, y)` is
/// `4 y + x`. The interior is 5, 6, 9, and 10.
const BOTTOM_LEFT: u32 = 5;
const BOTTOM_RIGHT: u32 = 6;
const TOP_LEFT: u32 = 9;
const TOP_RIGHT: u32 = 10;

/// One pair with `+2` at the bottom left and `-2` at the top right, one
/// obstacle on a zero vertex, and no pins. Returns the instance, its
/// metadata, the open path, and the blocked path.
fn diagonal() -> (Instance, Meta, Vec<u32>, Vec<u32>) {
    let case = Case { pins: 0, ..L3 };
    let (instance, meta) = find(case, |m| {
        m.labels[BOTTOM_LEFT as usize] == 2 && m.labels[TOP_RIGHT as usize] == -2
    });
    let obstacle = meta.obstacles[0];
    assert!(obstacle == BOTTOM_RIGHT || obstacle == TOP_LEFT);
    let open = if obstacle == BOTTOM_RIGHT {
        TOP_LEFT
    } else {
        BOTTOM_RIGHT
    };
    (
        instance,
        meta,
        vec![BOTTOM_LEFT, open, TOP_RIGHT],
        vec![BOTTOM_LEFT, obstacle, TOP_RIGHT],
    )
}

/// Two pairs with `+2` on the left column and `-2` on the right column of the
/// interior, so a valid route joins the rows and a swapped route joins the
/// columns.
fn columns() -> (Instance, Meta) {
    let case = Case {
        pairs: 2,
        obstacles: 0,
        pins: 0,
        ..L3
    };
    find(case, |m| {
        m.labels[BOTTOM_LEFT as usize] == 2
            && m.labels[TOP_LEFT as usize] == 2
            && m.labels[BOTTOM_RIGHT as usize] == -2
            && m.labels[TOP_RIGHT as usize] == -2
    })
}

fn params_of(case: Case) -> Vec<u8> {
    case.bytes()
}

#[test]
fn a_valid_route_passes() {
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    let (instance, meta, open, _) = diagonal();
    let bytes = params_of(Case { pins: 0, ..L3 });
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let route = vec![open.clone()];
    let spins = flipped(&meta, &path_links(&lattice, &route));
    assert!(feasible(&admitted, &instance, &spins));
    assert_eq!(
        check_route(admitted.params(), &instance, &spins, &route),
        Ok(())
    );
    // A path is undirected: the reverse passes too.
    let mut reversed = open;
    reversed.reverse();
    assert_eq!(
        check_route(admitted.params(), &instance, &spins, &[reversed]),
        Ok(())
    );
    // Two paths, one for each pair.
    let (instance, meta) = columns();
    let bytes = params_of(Case {
        pairs: 2,
        obstacles: 0,
        pins: 0,
        ..L3
    });
    let admitted_next = admit::admitted(&DEFECTS_V1, &bytes).expect("admit");
    let route = vec![vec![BOTTOM_LEFT, BOTTOM_RIGHT], vec![TOP_LEFT, TOP_RIGHT]];
    let spins = flipped(&meta, &path_links(&lattice, &route));
    assert!(feasible(&admitted_next, &instance, &spins));
    assert_eq!(
        check_route(admitted_next.params(), &instance, &spins, &route),
        Ok(())
    );
}

#[test]
fn swapped_endpoints_fail() {
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    let (instance, meta) = columns();
    let bytes = params_of(Case {
        pairs: 2,
        obstacles: 0,
        pins: 0,
        ..L3
    });
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    // The swapped route joins the two `+2` vertices and the two `-2` vertices.
    let swapped = vec![vec![BOTTOM_LEFT, TOP_LEFT], vec![BOTTOM_RIGHT, TOP_RIGHT]];
    // The state flips exactly those links, so only the endpoints are wrong.
    let spins = flipped(&meta, &path_links(&lattice, &swapped));
    assert_eq!(
        check_route(admitted.params(), &instance, &spins, &swapped),
        Err(RouteError::Endpoints)
    );
    // The same swapped route against the state of the valid route.
    let valid = vec![vec![BOTTOM_LEFT, BOTTOM_RIGHT], vec![TOP_LEFT, TOP_RIGHT]];
    let good = flipped(&meta, &path_links(&lattice, &valid));
    assert!(feasible(&admitted, &instance, &good));
    assert_eq!(
        check_route(admitted.params(), &instance, &good, &swapped),
        Err(RouteError::Endpoints)
    );
    // A path that ends on a zero vertex, and a pair of paths that share an
    // endpoint.
    let (instance, meta, open, _) = diagonal();
    let bytes = params_of(Case { pins: 0, ..L3 });
    let admitted_next = admit::admitted(&DEFECTS_V1, &bytes).expect("admit");
    let short = vec![vec![open[0], open[1]]];
    let spins = flipped(&meta, &path_links(&lattice, &short));
    assert_eq!(
        check_route(admitted_next.params(), &instance, &spins, &short),
        Err(RouteError::Endpoints)
    );
}

#[test]
fn an_extra_loop_fails() {
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    let (instance, meta, open, _) = diagonal();
    let bytes = params_of(Case { pins: 0, ..L3 });
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let route = vec![open];
    // The outer border loop flips links that touch no labeled vertex. The
    // charges stay as they were, so the state stays feasible.
    let border: Vec<u32> = (0..lattice.link_count())
        .filter(|&e| {
            let link = lattice.link(e).expect("link");
            [link.tail, link.head]
                .iter()
                .all(|&v| v.is_multiple_of(4) || v / 4 == 0 || v % 4 == 3 || v / 4 == 3)
        })
        .collect();
    assert_eq!(border.len(), 12);
    let mut links = path_links(&lattice, &route);
    links.extend(&border);
    let spins = flipped(&meta, &links);
    assert!(
        feasible(&admitted, &instance, &spins),
        "the loop is invisible to the charges"
    );
    assert_eq!(
        check_route(admitted.params(), &instance, &spins, &route),
        Err(RouteError::Difference)
    );
    // The loop cannot be listed as a path: it repeats its first vertex, and
    // without the closing vertex it has no labeled endpoints.
    let ring = vec![0u32, 1, 2, 3, 7, 11, 15, 14, 13, 12, 8, 4];
    let mut closed = ring.clone();
    closed.push(0);
    let with_loop = vec![route[0].clone(), closed];
    assert_eq!(
        check_route(admitted.params(), &instance, &spins, &with_loop),
        Err(RouteError::RepeatedVertex)
    );
    let open_ring = vec![route[0].clone(), ring];
    assert_eq!(
        check_route(admitted.params(), &instance, &spins, &open_ring),
        Err(RouteError::Endpoints)
    );
}

#[test]
fn a_route_through_an_obstacle_fails() {
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    let (instance, meta, open, blocked) = diagonal();
    let bytes = params_of(Case { pins: 0, ..L3 });
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let route = vec![blocked];
    let spins = flipped(&meta, &path_links(&lattice, &route));
    // The blocked path is a feasible state of the same shape, and only the
    // obstacle refuses it.
    assert!(feasible(&admitted, &instance, &spins));
    assert_eq!(
        check_route(admitted.params(), &instance, &spins, &route),
        Err(RouteError::Obstacle)
    );
    let good = flipped(&meta, &path_links(&lattice, std::slice::from_ref(&open)));
    assert_eq!(
        check_route(admitted.params(), &instance, &good, &[open]),
        Ok(())
    );
}

#[test]
fn route_shape_errors_follow_the_order() {
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    let (instance, meta, open, blocked) = diagonal();
    let obstacle = blocked[1];
    let free = open[1];
    let bytes = params_of(Case { pins: 0, ..L3 });
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let valid = vec![open];
    let good = flipped(&meta, &path_links(&lattice, &valid));
    let run =
        |spins: &[i8], route: &[Vec<u32>]| check_route(admitted.params(), &instance, spins, route);
    assert_eq!(run(&good, &valid), Ok(()));

    // Metadata comes first, before the spins.
    let mut bad = decode("order", &instance);
    bad.weight += 2;
    let broken = same_fields(&instance, encode(&bad));
    assert_eq!(
        check_route(admitted.params(), &broken, &good[1..], &[vec![5]]),
        Err(RouteError::Metadata)
    );
    // Spins come before the shape.
    assert_eq!(run(&good[1..], &[vec![5]]), Err(RouteError::Spins));
    let mut zero = good.clone();
    zero[0] = 0;
    assert_eq!(run(&zero, &[vec![5]]), Err(RouteError::Spins));
    // Shape comes before adjacency: the first path is not adjacent.
    assert_eq!(run(&good, &[vec![5, 10], vec![7]]), Err(RouteError::Shape));
    assert_eq!(run(&good, &[vec![5]]), Err(RouteError::Shape));
    assert_eq!(run(&good, &[vec![]]), Err(RouteError::Shape));
    assert_eq!(run(&good, &[vec![5, 16]]), Err(RouteError::Shape));
    // Adjacency comes before repeated vertices: the second path repeats one.
    assert_eq!(
        run(&good, &[vec![5, 10], vec![free, 5, free]]),
        Err(RouteError::NotAdjacent)
    );
    // Repeated vertices come before repeated links: the paths share links.
    assert_eq!(
        run(&good, &[vec![5, free, 5], vec![5, free]]),
        Err(RouteError::RepeatedVertex)
    );
    // Repeated links come before obstacles: both paths use the obstacle link.
    assert_eq!(
        run(&good, &[vec![5, obstacle], vec![obstacle, 5]]),
        Err(RouteError::RepeatedLink)
    );
    // Obstacles come before endpoints: the path ends on the obstacle.
    assert_eq!(run(&good, &[vec![5, obstacle]]), Err(RouteError::Obstacle));
    // Endpoints come before the difference: a route with the right links but
    // a path that ends on a zero vertex, and an uncovered charged vertex.
    assert_eq!(run(&good, &[vec![5, free]]), Err(RouteError::Endpoints));
    assert_eq!(run(&good, &[]), Err(RouteError::Endpoints));
    // Two paths between the same endpoints reuse the endpoints.
    let around = vec![5, 1, 2, 3, 7, 11, 10];
    assert_eq!(
        run(&good, &[vec![5, free, 10], around]),
        Err(RouteError::Endpoints)
    );
    // The difference comes last.
    let flat = vec![1i8; good.len()];
    assert_eq!(run(&flat, &valid), Err(RouteError::Difference));
    let mut extra = good;
    let spare = meta.positions[link_between(&lattice, 0, 1) as usize] as usize;
    extra[spare] = -1;
    assert_eq!(run(&extra, &valid), Err(RouteError::Difference));
}

#[test]
fn a_route_never_changes_feasibility() {
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    // A valid route to a feasible state, and a bad route to the same state.
    let (instance, meta, open, _) = diagonal();
    let bytes = params_of(Case { pins: 0, ..L3 });
    let admitted_diag = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let route = vec![open];
    let good = flipped(&meta, &path_links(&lattice, &route));
    assert!(feasible(&admitted_diag, &instance, &good));
    assert!(check_route(admitted_diag.params(), &instance, &good, &route).is_ok());
    assert!(check_route(admitted_diag.params(), &instance, &good, &[vec![5]]).is_err());
    assert!(
        feasible(&admitted_diag, &instance, &good),
        "a bad route changes nothing"
    );

    // A valid route to an infeasible state: the pair is `+2` at the bottom
    // right and `-2` at the top left. The route through the bottom left is
    // well formed, and its flips match the state, but the charges are wrong.
    let case = Case {
        obstacles: 0,
        pins: 0,
        ..L3
    };
    let (instance, meta) = find(case, |m| {
        m.labels[BOTTOM_RIGHT as usize] == 2 && m.labels[TOP_LEFT as usize] == -2
    });
    let bytes = params_of(case);
    let admitted_anti = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let route = vec![vec![BOTTOM_RIGHT, BOTTOM_LEFT, TOP_LEFT]];
    let spins = flipped(&meta, &path_links(&lattice, &route));
    assert_eq!(
        check_route(admitted_anti.params(), &instance, &spins, &route),
        Ok(())
    );
    assert!(
        !feasible(&admitted_anti, &instance, &spins),
        "the route is evidence only"
    );

    // An empty route is valid for a request without charges, and a pinned
    // link that flips makes the state infeasible and the route wrong.
    let case = Case {
        pairs: 0,
        obstacles: 0,
        pins: 4,
        ..L3
    };
    let instance = build(case, 11);
    let meta = decode("pins", &instance);
    let bytes = params_of(case);
    let admitted_pins = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let reference = vec![1i8; meta.positions.len()];
    assert!(feasible(&admitted_pins, &instance, &reference));
    assert_eq!(
        check_route(admitted_pins.params(), &instance, &reference, &[]),
        Ok(())
    );
    let pinned = flipped(&meta, &meta.pins[..1]);
    assert!(!feasible(&admitted_pins, &instance, &pinned));
    assert_eq!(
        check_route(admitted_pins.params(), &instance, &pinned, &[]),
        Err(RouteError::Difference)
    );
}

#[test]
fn observables_report_insufficient_physical_evidence() {
    let bytes = L3.bytes();
    let admitted = admitted(&DEFECTS_V1, &bytes).expect("admit");
    let instance = build(L3, 12);
    let meta = decode("observables", &instance);
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    let table = reference_table(&lattice);
    let (kept, dropped) = split(&lattice, &table);
    let good = feasible_state(&meta, &kept, L3);
    let seen = defect_observables(admitted.params(), &instance, &good).expect("observables");
    let d = direct(&meta, &kept, L3, &good);
    assert_eq!(seen.physical_energy, d.physical);
    assert_eq!(seen.violated_labels, 0);
    assert_eq!(seen.violated_pins, 0);
    assert_eq!(seen.dropped, dropped);
    assert_eq!(seen.truncation_bound, table.truncation_bound());
    assert!(seen.truncation_bound > 0);
    assert_eq!(
        seen.physical_status,
        PhysicalStatus::InsufficientPhysicalEvidence
    );
    assert_eq!(seen.charges.len(), 16);
    for (&got, &label) in seen.charges.iter().zip(&meta.labels) {
        if label != FREE {
            assert_eq!(got, label);
        }
    }
    // The instance energy of a feasible state is its physical energy plus the
    // constant of the penalty, which is 0 here.
    assert_eq!(energy(&instance, &good), seen.physical_energy);

    // A flipped pin, and a flipped link at a labeled vertex.
    let mut bad = good.clone();
    let pin = meta.positions[meta.pins[0] as usize] as usize;
    bad[pin] = -bad[pin];
    let seen = defect_observables(admitted.params(), &instance, &bad).expect("observables");
    let d = direct(&meta, &kept, L3, &bad);
    assert_eq!(seen.violated_pins, 1);
    assert_eq!(seen.violated_labels, 0);
    assert_eq!(seen.physical_energy, d.physical);
    assert_eq!(energy(&instance, &bad), d.instance_energy(meta.weight));
    let inner = link_between(&lattice, BOTTOM_LEFT, BOTTOM_RIGHT);
    let mut bad = good.clone();
    let slot = meta.positions[inner as usize] as usize;
    bad[slot] = -bad[slot];
    let seen = defect_observables(admitted.params(), &instance, &bad).expect("observables");
    let d = direct(&meta, &kept, L3, &bad);
    assert_eq!(seen.violated_labels, 2);
    assert_eq!(seen.physical_energy, d.physical);
    let wanted: u32 = seen
        .charges
        .iter()
        .zip(&meta.labels)
        .filter(|(&q, &label)| label != FREE && q != label)
        .count() as u32;
    assert_eq!(seen.violated_labels, wanted);

    // No observables for a wrong length, a bad entry, or bad metadata.
    assert!(defect_observables(admitted.params(), &instance, &good[1..]).is_none());
    let mut zero = good.clone();
    zero[0] = 0;
    assert!(defect_observables(admitted.params(), &instance, &zero).is_none());
    let mut mutated = meta;
    mutated.weight += 2;
    let broken = same_fields(&instance, encode(&mutated));
    assert!(defect_observables(admitted.params(), &broken, &good).is_none());
}
