//! The `quip/flow-static@1` construction: the energy against the direct
//! objective, feasibility, the boundary labels, the draw order, the metadata
//! check, the flow network, admission codes, the limits, and the recipes.

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

use ising_generators::family::{feasible, generate_instance, BaseTopology, SaltFault, Streams};
use ising_generators::flow::{
    flow_network, flow_observables, FlowArc, FLOW, FLOW_RECIPES, FLOW_SCHEMA, FLOW_V1,
};
use ising_generators::ice::{zephyr_lattice, ICE_FIELDS_RECIPES, ICE_FIELDS_V1};
use ising_generators::instance::{Instance, InstanceLimits, InstanceParts};
use ising_generators::rng::{shuffle, uniform};
use ising_generators::routing::{ROUTING_RECIPES, ROUTING_V1};
use ising_generators::square::{charges, check_request, Boundary, Request, SquareLattice};
use ising_generators::Code;

#[expect(dead_code, reason = "this test uses only the live topology loader")]
mod common;

#[path = "common/admit.rs"]
mod admit;
#[path = "common/lattice.rs"]
mod lattice;
#[path = "common/size_bounds.rs"]
mod size_bounds;

use admit::admitted;
use lattice::lattice_topology;
use size_bounds::bounds;

/// The parameters of a case.
#[derive(Clone, Copy, Debug)]
struct Case {
    m: u16,
    t: u8,
    side: u16,
    half: u16,
}

impl Case {
    /// Canonical parameter bytes: format, then `(tag, len, payload)` per
    /// field.
    fn bytes(self) -> Vec<u8> {
        let mut bytes = vec![0x01];
        bytes.extend_from_slice(&[1, 0, 2, 0, 0, 0]);
        bytes.extend_from_slice(&self.m.to_le_bytes());
        bytes.extend_from_slice(&[2, 0, 1, 0, 0, 0, self.t]);
        for (tag, value) in [(3u8, self.side), (4, self.half)] {
            bytes.extend_from_slice(&[tag, 0, 2, 0, 0, 0]);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn topology(self) -> BaseTopology {
        lattice_topology(self.m, self.t)
    }
}

/// L = 2 on z(2, 4).
const L2: Case = Case {
    m: 2,
    t: 4,
    side: 2,
    half: 30,
};

/// L = 3 on z(3, 4).
const L3: Case = Case {
    m: 3,
    t: 4,
    side: 3,
    half: 30,
};

fn nonce_of(i: u32) -> [u8; 32] {
    let mut nonce = [9u8; 32];
    nonce[..4].copy_from_slice(&i.to_le_bytes());
    nonce
}

fn try_build_on(
    bytes: &[u8],
    topology: &BaseTopology,
    nonce: [u8; 32],
) -> Result<Instance, SaltFault> {
    let admitted = admitted(&FLOW_V1, bytes).expect("admit");
    let bounds = bounds(&admitted, topology).expect("admit topology");
    generate_instance(&admitted, topology, &bounds, nonce)
}

fn build(case: Case, nonce: u32) -> Instance {
    let built = try_build_on(&case.bytes(), &case.topology(), nonce_of(nonce))
        .map_err(|fault| format!("{fault:?}"));
    assert!(built.is_ok(), "{case:?}: generate: {built:?}");
    built.expect("generate")
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
    targets: Vec<i8>,
    costs: Vec<u32>,
}

fn decode(what: &str, instance: &Instance) -> Meta {
    let region = instance.metadata();
    assert_eq!(&region[..2], &[3, 0], "{what}: three sections");
    let mut at = 2;
    let mut payloads = Vec::new();
    for kind in [4u16, 5, 9] {
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
    let (charges, incidence, flow) = (payloads[0], payloads[1], payloads[2]);
    assert_eq!((flow.len() - 8) % 5, 0, "{what}: kind 9 length");
    let links = (flow.len() - 8) / 5;
    Meta {
        side: u16_at(charges, 0),
        labels: charges[2..].iter().map(|&b| b as i8).collect(),
        c0: u16_at(incidence, 0),
        r0: u16_at(incidence, 2),
        positions: incidence[4..]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().expect("four")))
            .collect(),
        weight: u64::from_le_bytes(flow[..8].try_into().expect("eight")),
        targets: (0..links).map(|e| flow[8 + 5 * e] as i8).collect(),
        costs: (0..links)
            .map(|e| u32::from_le_bytes(flow[9 + 5 * e..13 + 5 * e].try_into().expect("four")))
            .collect(),
    }
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
    encode_with(meta, &[])
}

/// `encode` with extra bytes after the kind 9 payload.
fn encode_with(meta: &Meta, extra: &[u8]) -> Vec<u8> {
    let [charges, incidence, mut flow] = payloads(meta);
    flow.extend_from_slice(extra);
    region_of(&[(4, charges), (5, incidence), (9, flow)])
}

/// The payloads of kinds 4, 5, and 9.
fn payloads(meta: &Meta) -> [Vec<u8>; 3] {
    let mut charges = meta.side.to_le_bytes().to_vec();
    charges.extend(meta.labels.iter().map(|&l| l as u8));
    let mut incidence = meta.c0.to_le_bytes().to_vec();
    incidence.extend_from_slice(&meta.r0.to_le_bytes());
    for p in &meta.positions {
        incidence.extend_from_slice(&p.to_le_bytes());
    }
    let mut flow = meta.weight.to_le_bytes().to_vec();
    for (&t, &c) in meta.targets.iter().zip(&meta.costs) {
        flow.push(t as u8);
        flow.extend_from_slice(&c.to_le_bytes());
    }
    [charges, incidence, flow]
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
    Instance::new(parts, &InstanceLimits::FAM_37, FLOW_V1.metadata_kinds, None)
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

/// The weight of D5, written from the definition.
fn expected_weight(costs: &[u32]) -> u64 {
    let r: u64 = costs.iter().map(|&c| u64::from(c)).sum();
    let u = r / 2 + 1;
    u + u % 2
}

type Coefficients = (Vec<i32>, Vec<(u32, u32)>, Vec<i32>, i64);

/// The energy written from the definition `O(s) + (U / 2) sum (Q - q)^2`,
/// expanded by hand: `h`, the couplings, and the offset over the instance
/// positions.
fn expected_energy(meta: &Meta) -> Coefficients {
    let lattice = SquareLattice::new(meta.side, Boundary::Open).expect("lattice");
    let nodes = meta.positions.len();
    let u = meta.weight as i64;
    let mut h = vec![0i64; nodes];
    let mut couplers = std::collections::BTreeMap::new();
    let mut offset = 0i64;
    for e in 0..meta.positions.len() {
        let half = i64::from(meta.costs[e] / 2);
        h[meta.positions[e] as usize] -= half * i64::from(meta.targets[e]);
        offset += half;
    }
    for v in 0..lattice.vertex_count() {
        let q = i64::from(meta.labels[v as usize]);
        let at = lattice.incident(v);
        offset += u / 2 * (at.len() as i64 + q * q);
        for (rank, &(e, ie)) in at.iter().enumerate() {
            h[meta.positions[e as usize] as usize] -= u * q * i64::from(ie);
            for &(f, jf) in &at[rank + 1..] {
                let (a, b) = (meta.positions[e as usize], meta.positions[f as usize]);
                *couplers.entry((a.min(b), a.max(b))).or_insert(0i64) +=
                    u * i64::from(ie) * i64::from(jf);
            }
        }
    }
    let edges: Vec<(u32, u32)> = couplers
        .iter()
        .filter(|(_, &v)| v != 0)
        .map(|(&p, _)| p)
        .collect();
    let j: Vec<i32> = couplers
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
    objective: i64,
    squares: i64,
}

/// The energy of `spins` written from the definition: the cost of the links
/// against their target and the charges from `square::charges`.
fn direct(meta: &Meta, spins: &[i8]) -> Direct {
    let lattice = SquareLattice::new(meta.side, Boundary::Open).expect("lattice");
    let link_spins: Vec<i8> = meta.positions.iter().map(|&p| spins[p as usize]).collect();
    let objective = link_spins
        .iter()
        .zip(&meta.targets)
        .zip(&meta.costs)
        .filter(|((s, t), _)| s != t)
        .map(|(_, &c)| i64::from(c))
        .sum();
    let q = charges(&lattice, &link_spins).expect("charges");
    let squares = q
        .iter()
        .zip(&meta.labels)
        .map(|(&got, &label)| (i64::from(got) - i64::from(label)).pow(2))
        .sum();
    Direct { objective, squares }
}

impl Direct {
    fn instance_energy(&self, weight: u64) -> i64 {
        self.objective + weight as i64 / 2 * self.squares
    }

    fn feasible(&self) -> bool {
        self.squares == 0
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
fn feasible_state(meta: &Meta) -> Vec<i8> {
    let n = meta.positions.len();
    let mut seed = 5u64;
    for _ in 0..400 {
        let mut spins: Vec<i8> = (0..n)
            .map(|_| if next(&mut seed) & 1 == 1 { 1 } else { -1 })
            .collect();
        let mut cost = direct(meta, &spins).squares;
        for _ in 0..60 * n {
            if cost == 0 {
                return spins;
            }
            let i = next(&mut seed) as usize % n;
            spins[i] = -spins[i];
            let after = direct(meta, &spins).squares;
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

/// The vertices of degree 3 on the boundary, in vertex order.
fn edge_vertices(side: u16) -> Vec<u32> {
    let width = u32::from(side) + 1;
    let last = u32::from(side);
    (0..width * width)
        .filter(|&v| {
            let (x, y) = (v % width, v / width);
            let border = x == 0 || y == 0 || x == last || y == last;
            let corner = (x == 0 || x == last) && (y == 0 || y == last);
            border && !corner
        })
        .collect()
}

// ---- tests ----

#[test]
fn every_l2_state_matches_the_direct_objective() {
    let instance = build(L2, 1);
    let meta = decode("l2", &instance);
    let bytes = L2.bytes();
    let admitted = admitted(&FLOW_V1, &bytes).expect("admit");
    let lattice = SquareLattice::new(2, Boundary::Open).expect("lattice");
    assert_eq!(instance.node_ids().len(), 12);
    assert_eq!(meta.weight, expected_weight(&meta.costs));
    assert_eq!(meta.weight % 2, 0);
    let (h, edges, j, offset) = expected_energy(&meta);
    assert_eq!(instance.h(), h.as_slice());
    assert_eq!(instance.edges(), edges.as_slice());
    assert_eq!(instance.j(), j.as_slice());
    assert_eq!(instance.offset_milli(), offset);
    let mut feasible_count = 0;
    for bits in 0..4096u64 {
        let spins = state(bits, 12);
        let d = direct(&meta, &spins);
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
        let seen = flow_observables(admitted.params(), &instance, &spins).expect("observables");
        assert_eq!(seen.objective as i64, d.objective, "state {bits}");
        let mut direct_charges = vec![0i8; 9];
        for (e, link) in (0..lattice.link_count()).map(|e| (e, lattice.link(e).expect("link"))) {
            let spin = spins[meta.positions[e as usize] as usize];
            direct_charges[link.head as usize] += spin;
            direct_charges[link.tail as usize] -= spin;
        }
        let mismatches = direct_charges
            .iter()
            .zip(&meta.labels)
            .filter(|(q, l)| q != l)
            .count();
        assert_eq!(seen.charges, direct_charges, "state {bits}: charges");
        assert_eq!(seen.violations as usize, mismatches, "state {bits}");
        assert_eq!(seen.violations == 0, d.feasible(), "state {bits}");
        feasible_count += i32::from(d.feasible());
    }
    assert!(feasible_count > 0, "some state is feasible");
}

#[test]
fn infeasible_states_never_beat_feasible_ones() {
    for nonce in [2, 3, 4] {
        let instance = build(L2, nonce);
        let meta = decode("l2", &instance);
        let mut feasible_max = i64::MIN;
        let mut infeasible_min = i64::MAX;
        let mut objective_min = i64::MAX;
        let mut objective_max = i64::MIN;
        let mut best_feasible = i64::MAX;
        for bits in 0..4096u64 {
            let spins = state(bits, 12);
            let d = direct(&meta, &spins);
            let e = energy(&instance, &spins);
            objective_min = objective_min.min(d.objective);
            objective_max = objective_max.max(d.objective);
            if d.feasible() {
                feasible_max = feasible_max.max(e);
                best_feasible = best_feasible.min(d.objective);
            } else {
                infeasible_min = infeasible_min.min(e);
            }
        }
        assert!(
            infeasible_min > feasible_max,
            "nonce {nonce}: every infeasible state is above every feasible state"
        );
        let r: i64 = meta.costs.iter().map(|&c| i64::from(c)).sum();
        assert!(objective_max - objective_min <= r);
        assert!(2 * meta.weight as i64 > r, "2 U > R");
        let lowest = (0..4096u64)
            .map(|bits| (energy(&instance, &state(bits, 12)), bits))
            .min()
            .expect("states");
        let d = direct(&meta, &state(lowest.1, 12));
        assert!(d.feasible());
        assert_eq!(d.objective, best_feasible);
    }
}

#[test]
fn a_lower_objective_that_breaks_conservation_fails() {
    let bytes = L3.bytes();
    let admitted = admitted(&FLOW_V1, &bytes).expect("admit");
    let instance = build(L3, 3);
    let meta = decode("l3", &instance);
    let good = feasible_state(&meta);
    assert!(feasible(&admitted, &instance, &good));
    let good_direct = direct(&meta, &good);
    // A single flip that lowers the objective but breaks a charge.
    let mut found = 0;
    for node in 0..good.len() {
        let mut bad = good.clone();
        bad[node] = -bad[node];
        let d = direct(&meta, &bad);
        if d.objective < good_direct.objective && !d.feasible() {
            found += 1;
            assert!(!feasible(&admitted, &instance, &bad), "node {node}");
            assert_eq!(energy(&instance, &bad), d.instance_energy(meta.weight));
            assert!(
                energy(&instance, &bad) > energy(&instance, &good),
                "node {node}: the penalty outweighs the objective gain"
            );
        }
    }
    assert!(found > 0, "a flip lowers the objective and breaks a charge");
    // A seeded sample: an infeasible state is never below the feasible one.
    let mut seed = 99u64;
    for _ in 0..2000 {
        let spins: Vec<i8> = (0..good.len())
            .map(|_| if next(&mut seed) & 1 == 1 { 1 } else { -1 })
            .collect();
        let d = direct(&meta, &spins);
        assert_eq!(energy(&instance, &spins), d.instance_energy(meta.weight));
        assert_eq!(feasible(&admitted, &instance, &spins), d.feasible());
        if !d.feasible() {
            assert!(energy(&instance, &spins) > energy(&instance, &good));
        }
    }
    // A witness of the wrong length or with a bad entry is infeasible.
    assert!(!feasible(&admitted, &instance, &good[1..]));
    let mut zero = good;
    zero[0] = 0;
    assert!(!feasible(&admitted, &instance, &zero));
}

/// The failed tries, labels, targets, and costs of a replay.
type Replay = (usize, Vec<i8>, Vec<i8>, Vec<u32>);

/// The draws of the stream of `nonce` for `case`, written with the crate's
/// own stream helpers: the number of failed tries, then the labels, the
/// targets, and the costs. `None` when all four tries fail.
fn replay_tries(case: Case, nonce: [u8; 32], order: [u8; 32]) -> Option<Replay> {
    let bytes = case.bytes();
    let admitted = admitted(&FLOW_V1, &bytes).expect("admit");
    let mut streams = Streams::new(nonce, admitted.descriptor().digest(), order, FLOW.labels);
    let mut rng = streams.open("flow").expect("flow stream");
    let lattice = SquareLattice::new(case.side, Boundary::Open).expect("lattice");
    let mut passed = None;
    for failed in 0..4 {
        let mut edge = edge_vertices(case.side);
        assert_eq!(edge.len(), 4 * (usize::from(case.side) - 1));
        shuffle(&mut rng, &mut edge).expect("shuffle");
        let mut labels = vec![0i8; lattice.vertex_count() as usize];
        for (rank, &v) in edge.iter().enumerate() {
            labels[v as usize] = if rank < edge.len() / 2 { 1 } else { -1 };
        }
        let request = Request {
            charges: labels.iter().map(|&q| Some(q)).collect(),
            fixed: Vec::new(),
            fields: Vec::new(),
        };
        if check_request(&lattice, &request).is_ok() {
            passed = Some((failed, labels));
            break;
        }
    }
    let (failed, labels) = passed?;
    let two = std::num::NonZeroU32::new(2).expect("two");
    let bound = std::num::NonZeroU32::new(u32::from(case.half) + 1).expect("bound");
    let mut targets = Vec::new();
    let mut costs = Vec::new();
    for _ in 0..lattice.link_count() {
        targets.push(if uniform(&mut rng, two) == 1 { 1 } else { -1 });
        costs.push(2 * uniform(&mut rng, bound));
    }
    Some((failed, labels, targets, costs))
}

/// The labels, targets, and costs of the first passing try.
fn replay(case: Case, nonce: [u8; 32], order: [u8; 32]) -> (Vec<i8>, Vec<i8>, Vec<u32>) {
    let (_, labels, targets, costs) = replay_tries(case, nonce, order).expect("a try passes");
    (labels, targets, costs)
}

#[test]
fn boundary_labels_are_balanced() {
    for case in [
        L2,
        L3,
        Case {
            m: 4,
            side: 4,
            ..L3
        },
    ] {
        let topology = case.topology();
        let mut seen = BTreeSet::new();
        for i in 0..10 {
            let instance = build(case, i);
            let meta = decode("labels", &instance);
            let edge = edge_vertices(case.side);
            let plus = edge
                .iter()
                .filter(|&&v| meta.labels[v as usize] == 1)
                .count();
            let minus = edge
                .iter()
                .filter(|&&v| meta.labels[v as usize] == -1)
                .count();
            assert_eq!(plus, 2 * (usize::from(case.side) - 1), "{case:?}");
            assert_eq!(minus, plus);
            let sum: i32 = meta.labels.iter().map(|&q| i32::from(q)).sum();
            assert_eq!(sum, 0);
            for (v, &label) in meta.labels.iter().enumerate() {
                if !edge.contains(&(v as u32)) {
                    assert_eq!(label, 0, "vertex {v} is 0");
                }
            }
            let (labels, _, _) = replay(case, nonce_of(i), *topology.order_digest());
            assert_eq!(meta.labels, labels, "{case:?} nonce {i}");
            let _ = seen.insert(meta.labels);
        }
        assert!(seen.len() > 1, "the nonce changes the labels");
    }
}

#[test]
fn costs_and_targets_replay_the_stream() {
    for case in [L2, L3] {
        let topology = case.topology();
        for i in 0..8 {
            let instance = build(case, i);
            let meta = decode("draws", &instance);
            let (_, targets, costs) = replay(case, nonce_of(i), *topology.order_digest());
            assert_eq!(meta.targets, targets, "{case:?} nonce {i}: targets");
            assert_eq!(meta.costs, costs, "{case:?} nonce {i}: costs");
            assert!(meta.costs.iter().all(|&c| c % 2 == 0 && c <= 2 * 30));
            assert_eq!(meta.weight, expected_weight(&costs));
        }
    }
    let instance = build(L3, 1);
    let meta = decode("draws", &instance);
    assert!(meta.targets.contains(&1) && meta.targets.contains(&-1));
    assert!(meta.costs.iter().any(|&c| c > 0));
}

#[test]
fn zero_cost_links_keep_their_target() {
    // A half cost of 1 gives costs 0 and 2.
    let case = Case { half: 1, ..L3 };
    let bytes = case.bytes();
    let admitted = admitted(&FLOW_V1, &bytes).expect("admit");
    let instance = build(case, 4);
    let meta = decode("zero", &instance);
    let topology = case.topology();
    let (_, targets, costs) = replay(case, nonce_of(4), *topology.order_digest());
    assert_eq!(meta.costs, costs);
    assert_eq!(meta.targets, targets, "the targets of zero cost links stay");
    let zero = meta.costs.iter().position(|&c| c == 0).expect("a zero");
    let paid = meta.costs.iter().position(|&c| c == 2).expect("a paid");
    let accepts = |candidate: &Instance| (FLOW.check_metadata)(admitted.params(), candidate);
    assert!(accepts(&instance));
    // A target that disagrees with a nonzero h fails, with the old fields.
    let mut m = meta.clone();
    m.targets[paid] = -m.targets[paid];
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A zero cost link has no field, so its target is free: the fields stay
    // and the metadata still matches.
    let mut m = meta;
    m.targets[zero] = -m.targets[zero];
    assert!(accepts(&same_fields(&instance, encode(&m))));
    let all_zero = Case { half: 0, ..L3 };
    let flat = build(all_zero, 4);
    let flat_meta = decode("flat", &flat);
    assert!(flat_meta.costs.iter().all(|&c| c == 0));
    assert!(flat_meta.targets.iter().all(|&t| t == 1 || t == -1));
    assert_eq!(flat_meta.weight, 2);
}

#[test]
fn check_metadata_rejects_each_rule() {
    let bytes = L3.bytes();
    let instance = build(L3, 6);
    let meta = decode("rules", &instance);
    let admitted = admitted(&FLOW_V1, &bytes).expect("admit");
    let accepts = |candidate: &Instance| (FLOW.check_metadata)(admitted.params(), candidate);
    assert!(accepts(&instance));
    // The same mutation with the energy rebuilt from the mutated metadata.
    let with_energy = |mutated: &Meta| {
        let (h, edges, j, offset) = expected_energy(mutated);
        rebuilt(&instance, edges, h, j, offset, encode(mutated))
    };
    assert!(accepts(&with_energy(&meta)));
    let edge = edge_vertices(3);
    let plus = edge
        .iter()
        .copied()
        .find(|&v| meta.labels[v as usize] == 1)
        .expect("plus");
    let minus = edge
        .iter()
        .copied()
        .find(|&v| meta.labels[v as usize] == -1)
        .expect("minus");

    // Balanced pairs of invalid labels, each with a matching energy: two
    // corners, two interior vertices, and the edge pair at 0 and at +-3.
    for (x, y, value) in [
        (0usize, 15usize, 1i8),
        (5, 10, 1),
        (plus as usize, minus as usize, 0),
        (plus as usize, minus as usize, 3),
    ] {
        let mut m = meta.clone();
        m.labels[x] = value;
        m.labels[y] = -value;
        assert!(!accepts(&with_energy(&m)), "labels {x}, {y} at {value}");
    }
    // Unequal counts of +1 and -1, with every value valid.
    let mut m = meta.clone();
    m.labels[minus as usize] = 1;
    assert!(!accepts(&with_energy(&m)));
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

    // A weight that breaks D5, with the energy rebuilt for it.
    for delta in [1i64, 2, -2] {
        let mut m = meta.clone();
        m.weight = (m.weight as i64 + delta) as u64;
        assert!(!accepts(&with_energy(&m)), "U {delta:+}");
    }
    // A stored weight that differs from the fields.
    let mut m = meta.clone();
    m.weight += 2;
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A target that is not +-1, with the energy rebuilt for it.
    for target in [0i8, 2] {
        let mut m = meta.clone();
        m.targets[0] = target;
        assert!(!accepts(&with_energy(&m)), "target {target}");
    }
    // An odd cost, and a cost above 2 * cost_half_max, with the energy
    // rebuilt (the weight follows the costs).
    let mut m = meta.clone();
    m.costs[0] = 1;
    m.weight = expected_weight(&m.costs);
    assert!(!accepts(&with_energy(&m)));
    let mut m = meta.clone();
    m.costs[0] = 62;
    m.weight = expected_weight(&m.costs);
    assert!(!accepts(&with_energy(&m)));
    // A cost that changes with the old fields.
    let mut m = meta.clone();
    m.costs[0] = if m.costs[0] == 4 { 6 } else { 4 };
    m.weight = expected_weight(&m.costs);
    assert!(!accepts(&same_fields(&instance, encode(&m))));
    // A coefficient that differs by one, and an offset that differs by one.
    let mut h = instance.h().to_vec();
    h[0] += 1;
    assert!(!accepts(&rebuilt(
        &instance,
        instance.edges().to_vec(),
        h,
        instance.j().to_vec(),
        instance.offset_milli(),
        instance.metadata().to_vec()
    )));
    assert!(!accepts(&rebuilt(
        &instance,
        instance.edges().to_vec(),
        instance.h().to_vec(),
        instance.j().to_vec(),
        instance.offset_milli() + 1,
        instance.metadata().to_vec()
    )));
    // Extended and truncated sections, one payload at a time.
    assert!(!accepts(&same_fields(&instance, encode_with(&meta, &[0]))));
    let parts = payloads(&meta);
    for cut in 0..3 {
        let mut short = parts.clone();
        let _ = short[cut].pop();
        let region = region_of(&[
            (4, short[0].clone()),
            (5, short[1].clone()),
            (9, short[2].clone()),
        ]);
        assert!(!accepts(&same_fields(&instance, region)), "payload {cut}");
    }
    // Only J differs.
    let mut j = instance.j().to_vec();
    j[0] += 1;
    assert!(!accepts(&rebuilt(
        &instance,
        instance.edges().to_vec(),
        instance.h().to_vec(),
        j,
        instance.offset_milli(),
        instance.metadata().to_vec()
    )));
    // A region without the three sections.
    assert!(!accepts(&same_fields(&instance, vec![0, 0])));
}

#[test]
fn flow_network_matches_the_lattice() {
    let bytes = L3.bytes();
    let admitted = admitted(&FLOW_V1, &bytes).expect("admit");
    let lattice = SquareLattice::new(3, Boundary::Open).expect("lattice");
    for nonce in 0..6 {
        let instance = build(L3, nonce);
        let meta = decode("net", &instance);
        let net = flow_network(admitted.params(), &instance).expect("network");
        assert_eq!(net.supplies.len(), 16);
        assert_eq!(net.supplies.iter().map(|&s| i32::from(s)).sum::<i32>(), 0);
        for (v, &supply) in net.supplies.iter().enumerate() {
            assert_eq!(supply, -meta.labels[v]);
        }
        assert_eq!(net.arcs.len(), lattice.link_count() as usize);
        for (e, arc) in net.arcs.iter().enumerate() {
            let link = lattice.link(e as u32).expect("link");
            assert_eq!(
                *arc,
                FlowArc {
                    tail: link.tail,
                    head: link.head,
                    target: meta.targets[e],
                    cost: meta.costs[e],
                }
            );
        }
        // A feasible state routes the supplies: each vertex sends out minus in
        // what its supply says.
        let spins = feasible_state(&meta);
        let links: Vec<i8> = meta.positions.iter().map(|&p| spins[p as usize]).collect();
        let mut net_out = [0i32; 16];
        for (arc, &s) in net.arcs.iter().zip(&links) {
            net_out[arc.tail as usize] += i32::from(s);
            net_out[arc.head as usize] -= i32::from(s);
        }
        for (v, &supply) in net.supplies.iter().enumerate() {
            assert_eq!(net_out[v], i32::from(supply), "vertex {v}");
        }
        let seen = flow_observables(admitted.params(), &instance, &spins).expect("seen");
        let objective: u64 = net
            .arcs
            .iter()
            .zip(&links)
            .filter(|(arc, &s)| s != arc.target)
            .map(|(arc, _)| u64::from(arc.cost))
            .sum();
        assert_eq!(seen.objective, objective);
        assert_eq!(seen.violations, 0);
        assert_eq!(
            seen.reversed as usize,
            net.arcs
                .iter()
                .zip(&links)
                .filter(|(a, &s)| s != a.target)
                .count()
        );
    }
    // A damaged instance has no network.
    let instance = build(L3, 0);
    let mut h = instance.h().to_vec();
    h[0] += 1;
    let bad = rebuilt(
        &instance,
        instance.edges().to_vec(),
        h,
        instance.j().to_vec(),
        instance.offset_milli(),
        instance.metadata().to_vec(),
    );
    assert!(flow_network(admitted.params(), &bad).is_none());
    assert!(flow_observables(admitted.params(), &bad, &[1; 24]).is_none());
    assert!(flow_observables(admitted.params(), &instance, &[1; 3]).is_none());
}

#[test]
fn admission_codes() {
    let with = |case: Case| match admitted(&FLOW_V1, &case.bytes()) {
        Err(code) => code,
        Ok(admitted) => bounds(&admitted, &case.topology()).expect_err("refused"),
    };
    let ok = |case: Case| {
        let bytes = case.bytes();
        let admitted = admitted(&FLOW_V1, &bytes).expect("admit");
        bounds(&admitted, &case.topology()).is_ok()
    };
    assert_eq!(FLOW_SCHEMA.len(), 4);
    assert!(ok(L2));
    assert!(ok(L3));
    assert!(ok(Case { half: 0, ..L3 }));
    assert!(ok(Case { half: 500, ..L3 }));
    // Side 1 is below the schema.
    assert_eq!(with(Case { side: 1, ..L2 }), Code::ParamsOutOfBounds);
    assert_eq!(with(Case { half: 501, ..L3 }), Code::ParamsOutOfBounds);
    assert_eq!(with(Case { side: 65, ..L3 }), Code::ParamsOutOfBounds);
    assert_eq!(with(Case { m: 33, ..L3 }), Code::ParamsOutOfBounds);
    assert_eq!(with(Case { t: 9, ..L3 }), Code::ParamsOutOfBounds);
    // L + 1 above 2m - 1.
    assert_eq!(with(Case { m: 2, ..L3 }), Code::TopologyIncompatible);
    // No window: z(3, 1) has one line, so a dead qubit kills its cell, and
    // cell (3, 3) is in every 3 by 3 window.
    let case = Case {
        m: 3,
        t: 1,
        side: 2,
        half: 5,
    };
    let (nodes, edges) = zephyr_lattice(3, 1).expect("lattice");
    // The segment (u = 0, w = 3, k = 0, p = 2) of z(3, 1).
    let (m, t, u, w, k, p) = (3u32, 1u32, 0u32, 3u32, 0u32, 2u32);
    let center = (((u * (2 * m + 1) + w) * t + k) * 2 + p % 2) * m + p / 2;
    let nodes: Vec<u32> = nodes.into_iter().filter(|&id| id != center).collect();
    let edges: Vec<(u32, u32)> = edges
        .into_iter()
        .filter(|&(a, b)| a != center && b != center)
        .collect();
    let damaged = BaseTopology::new(nodes, &edges).expect("topology");
    let bytes = case.bytes();
    let admitted_case = admitted(&FLOW_V1, &bytes).expect("admit");
    assert!(bounds(&admitted_case, &case.topology()).is_ok());
    assert_eq!(
        bounds(&admitted_case, &damaged),
        Err(Code::TopologyIncompatible)
    );
    // The largest side that fits z(32, 4) is 62. Sides 63 and 64 give code 12
    // however large the topology is.
    for side in [63u16, 64] {
        let big = Case {
            m: 32,
            t: 4,
            side,
            half: 500,
        };
        let bytes = big.bytes();
        let admitted = admitted(&FLOW_V1, &bytes).expect("admit");
        let sizes = bounds(&admitted, &big.topology());
        assert_eq!(sizes, Err(Code::TopologyIncompatible), "side {side}");
    }
}

/// The labelings of the edge vertices of a lattice of side `side` that
/// `check_request` refuses, and the number of labelings.
fn refused_edge_labelings(side: u16) -> (usize, usize) {
    let lattice = SquareLattice::new(side, Boundary::Open).expect("lattice");
    let edge = edge_vertices(side);
    let n = edge.len();
    let (mut refused, mut all) = (0, 0);
    for mask in 0u32..(1 << n) {
        if mask.count_ones() as usize != n / 2 {
            continue;
        }
        let mut labels = vec![Some(0i8); lattice.vertex_count() as usize];
        for (i, &v) in edge.iter().enumerate() {
            labels[v as usize] = Some(if mask >> i & 1 == 1 { 1 } else { -1 });
        }
        let request = Request {
            charges: labels,
            fixed: Vec::new(),
            fields: Vec::new(),
        };
        all += 1;
        refused += usize::from(check_request(&lattice, &request).is_err());
    }
    (refused, all)
}

#[test]
fn infeasible_draws_retry_then_give_14() {
    // D2 requests are not always feasible. At side 5 the labeling that puts
    // every sink on the first eight edge vertices sends 8 units across the 6
    // links between the rows y = 2 and y = 3. Sides 2 to 4 have no refused
    // labeling, and side 5 refuses some.
    assert_eq!(refused_edge_labelings(2), (0, 6));
    assert_eq!(refused_edge_labelings(3), (0, 70));
    assert_eq!(refused_edge_labelings(4), (0, 924));
    let (refused, all) = refused_edge_labelings(5);
    assert_eq!(all, 12870);
    assert!(refused > 0);
    println!("side 5 refuses {refused} of {all} labelings");

    // The retry path: a nonce whose first try is refused builds from a later
    // try, and the labels are those of the replay. Side 5 needs z(4, 4).
    let case = Case {
        m: 4,
        t: 4,
        side: 5,
        half: 30,
    };
    let topology = case.topology();
    let bytes = case.bytes();
    let mut retried = 0;
    for i in 0..4000 {
        let Some((failed, labels, targets, costs)) =
            replay_tries(case, nonce_of(i), *topology.order_digest())
        else {
            panic!("nonce {i}: four tries failed");
        };
        if failed == 0 {
            continue;
        }
        retried += 1;
        let instance = try_build_on(&bytes, &topology, nonce_of(i)).expect("a later try builds");
        let meta = decode("retry", &instance);
        assert_eq!(meta.labels, labels, "nonce {i}: labels of the passing try");
        assert_eq!(meta.targets, targets, "nonce {i}: targets follow the try");
        assert_eq!(meta.costs, costs, "nonce {i}: costs follow the try");
        if retried == 3 {
            break;
        }
    }
    assert!(retried > 0, "some nonce retries");
    // Code 14 needs four refused tries in a row. No nonce reaches it at the
    // sides that build, so the code is a guard that this test cannot reach.
    for case in [L2, L3] {
        let bytes = case.bytes();
        let topology = case.topology();
        for i in 0..40 {
            let built = try_build_on(&bytes, &topology, nonce_of(i));
            assert!(built.is_ok(), "{case:?} nonce {i}: {built:?}");
        }
    }
}

#[test]
fn fixtures_differ_from_ice_fields_and_routing() {
    assert_eq!(FLOW_V1.family_id, "quip/flow-static");
    assert_eq!(FLOW_V1.revision, 1);
    assert_eq!(FLOW_V1.metadata_kinds, &[4, 5, 9]);
    for (id, kinds, params) in [
        (
            ICE_FIELDS_V1.family_id,
            ICE_FIELDS_V1.metadata_kinds,
            ICE_FIELDS_RECIPES[0].params,
        ),
        (
            ROUTING_V1.family_id,
            ROUTING_V1.metadata_kinds,
            ROUTING_RECIPES[0].params,
        ),
    ] {
        assert_ne!(FLOW_V1.family_id, id);
        assert_ne!(FLOW_V1.metadata_kinds, kinds, "{id}");
        assert!(!kinds.contains(&9), "{id}: kind 9 is the flow kind");
        assert!(params.len() > 1);
    }
    let l4 = admitted(&FLOW_V1, FLOW_RECIPES[0].params).expect("admit");
    let fields = admitted(&ICE_FIELDS_V1, ICE_FIELDS_RECIPES[0].params).expect("admit");
    let routing = admitted(&ROUTING_V1, ROUTING_RECIPES[0].params).expect("admit");
    let digests = [
        l4.descriptor().digest(),
        fields.descriptor().digest(),
        routing.descriptor().digest(),
    ];
    assert_eq!(digests.iter().collect::<BTreeSet<_>>().len(), 3);
    // The canonical bytes of a flow instance carry the flow descriptor.
    let instance = build(L2, 1);
    assert_ne!(instance.descriptor_digest(), &fields.descriptor().digest());
    assert_ne!(instance.descriptor_digest(), &routing.descriptor().digest());
    let bytes = instance.canonical_bytes().expect("bytes");
    assert!(bytes.windows(32).any(|w| w == instance.descriptor_digest()));
}

#[test]
fn generation_is_repeatable() {
    let topology = L3.topology();
    let first = build(L3, 7);
    let again = build(L3, 7);
    assert_eq!(first, again);
    assert_eq!(first.digest(), again.digest());
    assert_eq!(
        try_build_on(&L3.bytes(), &topology, nonce_of(7)).expect("builds"),
        first
    );
    let other = build(L3, 8);
    assert_ne!(first.digest(), other.digest());
    let flat = build(Case { half: 0, ..L3 }, 7);
    assert_ne!(first.h(), flat.h());
    assert_eq!(first.node_ids(), flat.node_ids());
}

#[test]
fn recipes_are_canonical() {
    let expected = [
        ("l4", "flow-l4", 4u16),
        ("l6", "flow-l6", 6),
        ("l8", "flow-l8", 8),
        ("l12", "flow-l12", 12),
    ];
    assert_eq!(FLOW_RECIPES.len(), 4);
    for (recipe, (alias, cell, side)) in FLOW_RECIPES.iter().zip(expected) {
        assert_eq!(recipe.alias, alias);
        assert_eq!(recipe.cell, cell);
        let case = Case {
            m: 12,
            t: 4,
            side,
            half: 500,
        };
        assert_eq!(recipe.params, case.bytes().as_slice());
        assert!(admitted(&FLOW_V1, recipe.params).is_ok());
    }
}

/// The salt that builds for each recipe, in recipe order. Every building
/// recipe builds at salt 0.
const BUILDING_SALTS: [u32; 3] = [0, 0, 0];

#[test]
fn live_recipes_build_or_give_12() {
    let topology = common::live_topology();
    for (recipe, i) in FLOW_RECIPES.iter().zip(BUILDING_SALTS) {
        let admitted = admitted(&FLOW_V1, recipe.params).expect("admit");
        let sizes = bounds(&admitted, topology).expect("admit topology");
        let instance =
            generate_instance(&admitted, topology, &sizes, nonce_of(i)).expect("the salt builds");
        let meta = decode("live_recipes", &instance);
        let side = u32::from(meta.side);
        assert_eq!(instance.node_ids().len() as u32, 2 * side * (side + 1));
        assert!((FLOW.check_metadata)(admitted.params(), &instance));
        println!(
            "RECIPE {} window ({}, {}) salt {i}",
            recipe.alias, meta.c0, meta.r0
        );
    }
    // The live topology has no fully alive window above side 9.
    let l12 = admitted(&FLOW_V1, FLOW_RECIPES[3].params).expect("admit");
    assert_eq!(bounds(&l12, topology), Err(Code::TopologyIncompatible));
}

#[test]
fn limits_hold_at_the_largest_admitted_side() {
    let limits = FLOW_V1.limits;
    // The formulas of the `LIMITS` comment at side `l` and half cost `u`.
    let formulas = |l: u64, u: u64| {
        let links = 2 * l * (l + 1);
        let weight = {
            let w = links * u + 1;
            w + w % 2
        };
        let nodes = links;
        let edges = 6 * (l - 1) * (l - 1) + 12 * (l - 1) + 4;
        let coefficient = 2 * weight + u;
        let degrees = 2 * links;
        let labels = 4 * (l - 1);
        let offset = weight / 2 * (degrees + labels) + links * u;
        let metadata = 2 + 3 * 6 + (2 + (l + 1) * (l + 1)) + (4 + 4 * links) + (8 + 5 * links);
        (nodes, edges, coefficient, offset, metadata)
    };
    let (nodes, edges, coefficient, offset, metadata) = formulas(64, 500);
    assert_eq!(nodes, 8320);
    assert_eq!(edges, 24_574);
    assert_eq!(coefficient, 8_320_504);
    assert_eq!(offset, 35_139_536_892);
    assert_eq!(metadata, 79_139);
    assert_eq!(nodes, u64::from(limits.max_nodes()));
    assert_eq!(edges, u64::from(limits.max_edges()));
    assert_eq!(coefficient, u64::from(limits.max_coefficient()));
    assert_eq!(offset, limits.max_offset());
    assert_eq!(metadata, u64::from(limits.max_metadata()));
    // The formulas grow with the side, so side 64 covers every admitted side.
    let mut last = formulas(2, 500);
    for l in 3..=64 {
        let now = formulas(l, 500);
        assert!(now.0 > last.0 && now.1 > last.1 && now.2 > last.2);
        assert!(now.3 > last.3 && now.4 > last.4);
        last = now;
    }
    // The largest side that builds on z(32, 4) is 62. The built instance is
    // inside the limits at the formulas of side 62 (the generator checks the
    // limits in `Instance::new`), and the exact counts agree with the formulas.
    let big = Case {
        m: 32,
        t: 4,
        side: 62,
        half: 500,
    };
    let instance = build(big, 1);
    let meta = decode("side 62", &instance);
    let (nodes, edges, coefficient, offset, metadata) = formulas(62, 500);
    assert_eq!(instance.node_ids().len() as u64, nodes);
    assert_eq!(instance.edges().len() as u64, edges);
    // Pairs at a vertex: 3721 interior vertices with 6, 244 edge vertices with
    // 3, and 4 corners with 1.
    assert_eq!(instance.edges().len(), 6 * 61 * 61 + 3 * 244 + 4);
    assert!(instance
        .h()
        .iter()
        .chain(instance.j())
        .all(|v| u64::from(v.unsigned_abs()) <= coefficient));
    assert!(instance.offset_milli().unsigned_abs() <= offset);
    assert_eq!(instance.metadata().len() as u64, metadata);
    assert_eq!(meta.weight % 2, 0);
    assert!(meta.weight <= (2 * 62 * 63 * 500 + 2));
}
