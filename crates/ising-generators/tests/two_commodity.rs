//! The `quip/two-commodity-routing@1` construction: the street template,
//! terminals and costs, the energy, admission codes, the metadata check, and
//! the defect diagnostic.
//!
//! The gadget, pruning, flow, and chain strength cap tests that need the
//! private builders are unit tests in `src/routing.rs`.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::items_after_statements,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;
use std::num::NonZeroU32;

use ising_generators::descriptor::{admit, params_digest, Admitted, AdvertisedLimits, SizeBounds};
use ising_generators::family::{
    admit_topology, generate_instance, BaseTopology, SaltFault, Streams,
};
use ising_generators::ice::zephyr_lattice;
use ising_generators::instance::{Instance, InstanceParts};
use ising_generators::rng::{shuffle, uniform};
use ising_generators::routing::{
    routing_defects, ROUTING, ROUTING_RECIPES, ROUTING_SCHEMA, ROUTING_V1,
};
use ising_generators::Code;

#[expect(dead_code, reason = "this test uses only the live topology loader")]
mod common;

const CONTRACTS: &[ising_generators::descriptor::Contract] = &[ROUTING_V1];

/// The twelve native ids of the 2 by 2 street block of `z(2, 4)` that holds
/// four arcs in a directed cycle: the spins of cells (1, 2), (2, 2), (2, 3),
/// and (1, 3).
const TINY_IDS: [u32; 12] = [17, 21, 25, 27, 33, 41, 114, 117, 118, 122, 130, 134];

/// Canonical parameter bytes: format, then `(tag, len, payload)` per field.
fn params(m: u16, t: u8, divisor: u16) -> Vec<u8> {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(&[1, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&m.to_le_bytes());
    bytes.extend_from_slice(&[2, 0, 1, 0, 0, 0, t]);
    bytes.extend_from_slice(&[3, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&divisor.to_le_bytes());
    bytes
}

fn admitted(bytes: &[u8]) -> Result<Admitted<'_>, Code> {
    admit(
        CONTRACTS,
        b"quip/two-commodity-routing",
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

/// The subgraph of `z(m, t)` induced by `ids`.
fn induced(m: u16, t: u8, ids: &[u32]) -> BaseTopology {
    let (nodes, edges) = zephyr_lattice(m, t).expect("lattice");
    let keep: BTreeSet<u32> = ids.iter().copied().collect();
    let nodes: Vec<u32> = nodes.into_iter().filter(|id| keep.contains(id)).collect();
    let edges: Vec<(u32, u32)> = edges
        .into_iter()
        .filter(|(a, b)| keep.contains(a) && keep.contains(b))
        .collect();
    BaseTopology::new(nodes, &edges).expect("topology")
}

fn tiny_topology() -> BaseTopology {
    induced(2, 4, &TINY_IDS)
}

fn nonce_of(i: u8) -> [u8; 32] {
    [i; 32]
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

/// An arc of the decoded metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Arc {
    tail: u32,
    head: u32,
    x1: u32,
    chain: [u32; 2],
    cost: [u16; 2],
}

/// The decoded routing section.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Meta {
    cells: Vec<(u16, u16)>,
    supply: Vec<[i8; 2]>,
    arcs: Vec<Arc>,
    strength: u16,
}

fn decode(instance: &Instance) -> Meta {
    let region = instance.metadata();
    assert_eq!(&region[..4], &[1, 0, 7, 0], "one section of kind 7");
    let length = u32::from_le_bytes(region[4..8].try_into().expect("four bytes")) as usize;
    assert_eq!(region.len(), 8 + length);
    let payload = &region[8..];
    let u32_at = |at: usize| u32::from_le_bytes(payload[at..at + 4].try_into().expect("four"));
    let u16_at = |at: usize| u16::from_le_bytes(payload[at..at + 2].try_into().expect("two"));
    let n = u32_at(0) as usize;
    let mut cells = Vec::new();
    let mut supply = Vec::new();
    for k in 0..n {
        let at = 4 + 6 * k;
        cells.push((u16_at(at), u16_at(at + 2)));
        supply.push([payload[at + 4] as i8, payload[at + 5] as i8]);
    }
    let arcs_at = 4 + 6 * n;
    let a = u32_at(arcs_at) as usize;
    let mut arcs = Vec::new();
    for k in 0..a {
        let at = arcs_at + 4 + 24 * k;
        arcs.push(Arc {
            tail: u32_at(at),
            head: u32_at(at + 4),
            x1: u32_at(at + 8),
            chain: [u32_at(at + 12), u32_at(at + 16)],
            cost: [u16_at(at + 20), u16_at(at + 22)],
        });
    }
    let strength = u16_at(arcs_at + 4 + 24 * a);
    assert_eq!(payload.len(), arcs_at + 4 + 24 * a + 2);
    Meta {
        cells,
        supply,
        arcs,
        strength,
    }
}

fn encode(meta: &Meta) -> Vec<u8> {
    let mut payload = (meta.cells.len() as u32).to_le_bytes().to_vec();
    for (&(c, r), supply) in meta.cells.iter().zip(&meta.supply) {
        payload.extend_from_slice(&c.to_le_bytes());
        payload.extend_from_slice(&r.to_le_bytes());
        payload.extend(supply.iter().map(|&b| b as u8));
    }
    payload.extend_from_slice(&(meta.arcs.len() as u32).to_le_bytes());
    for arc in &meta.arcs {
        for word in [arc.tail, arc.head, arc.x1, arc.chain[0], arc.chain[1]] {
            payload.extend_from_slice(&word.to_le_bytes());
        }
        for cost in arc.cost {
            payload.extend_from_slice(&cost.to_le_bytes());
        }
    }
    payload.extend_from_slice(&meta.strength.to_le_bytes());
    section(&payload)
}

/// A metadata region with one kind 7 section.
fn section(payload: &[u8]) -> Vec<u8> {
    let mut region = vec![1, 0, 7, 0];
    region.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    region.extend_from_slice(payload);
    region
}

/// A copy of `instance` with new fields and metadata.
fn rebuilt(instance: &Instance, h: Vec<i32>, j: Vec<i32>, metadata: Vec<u8>) -> Instance {
    let parts = InstanceParts {
        descriptor_digest: *instance.descriptor_digest(),
        topology_order_digest: *instance.topology_order_digest(),
        nonce: instance.nonce().copied(),
        node_ids: instance.node_ids().to_vec(),
        edges: instance.edges().to_vec(),
        h,
        j,
        offset_milli: instance.offset_milli(),
        metadata,
    };
    Instance::new(parts, &ROUTING_V1.limits, ROUTING_V1.metadata_kinds, None)
        .expect("rebuilt instance is well formed")
}

fn with_metadata(instance: &Instance, metadata: Vec<u8>) -> Instance {
    rebuilt(
        instance,
        instance.h().to_vec(),
        instance.j().to_vec(),
        metadata,
    )
}

fn check(bytes: &[u8], instance: &Instance) -> bool {
    let admitted = admitted(bytes).expect("admit");
    let construction = ROUTING_V1.construction.expect("construction");
    (construction.check_metadata)(admitted.params(), instance)
}

/// The commodity `k` spin of `arc` inside the K4 of node `v`.
fn spin_at(arc: &Arc, v: u32, k: usize) -> u32 {
    if k == 0 {
        arc.x1
    } else if v == arc.tail.min(arc.head) {
        arc.chain[0]
    } else {
        arc.chain[1]
    }
}

/// The energy of the instance at the spins `state` (`+1` or `-1`), in milli.
fn instance_energy(instance: &Instance, state: &[i64]) -> i64 {
    let mut total = instance.offset_milli();
    for (&h, &s) in instance.h().iter().zip(state) {
        total += i64::from(h) * s;
    }
    for (&(u, v), &j) in instance.edges().iter().zip(instance.j()) {
        total += i64::from(j) * state[u as usize] * state[v as usize];
    }
    total
}

/// The structured energy of `meta` at the spins `state`, in milli: the
/// conservation squares, the capacity and cost terms in `x = (1 - s) / 2`
/// form, and the chain couplers in spin form.
fn structured_energy(meta: &Meta, state: &[i64]) -> i64 {
    let x = |spin: u32| (1 - state[spin as usize]) / 2;
    let mut total = 0;
    for (v, supply) in (0u32..).zip(&meta.supply) {
        for (k, &b) in supply.iter().enumerate() {
            let mut net = 0;
            for arc in &meta.arcs {
                if arc.tail == v {
                    net += x(spin_at(arc, v, k));
                }
                if arc.head == v {
                    net -= x(spin_at(arc, v, k));
                }
            }
            total += 248 * (net - i64::from(b)).pow(2);
        }
    }
    for arc in &meta.arcs {
        let (one, two) = (x(arc.x1), x(arc.chain[0]));
        total += 496 * one * two;
        total += i64::from(arc.cost[0]) * one + i64::from(arc.cost[1]) * two;
        total -=
            i64::from(meta.strength) * state[arc.chain[0] as usize] * state[arc.chain[1] as usize];
    }
    total
}

/// A deterministic pseudo-random spin state.
fn random_state(n: usize, seed: u64) -> Vec<i64> {
    let mut word = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    (0..n)
        .map(|_| {
            word ^= word << 13;
            word ^= word >> 7;
            word ^= word << 17;
            if word & 1 == 0 {
                1
            } else {
                -1
            }
        })
        .collect()
}

/// The adjacency of an instance, as ordered pairs in both directions.
fn spin_pairs(topology: &BaseTopology) -> BTreeSet<(u32, u32)> {
    let ids = topology.node_ids();
    topology
        .edges()
        .iter()
        .flat_map(|&(u, v)| {
            let (a, b) = (ids[u as usize], ids[v as usize]);
            [(a, b), (b, a)]
        })
        .collect()
}

/// The unit-capacity maximum flow from `sources` to `sinks`, by repeated
/// depth-first augmentation on a capacity matrix.
fn max_flow(n: usize, arcs: &[Arc], sources: &[usize], sinks: &[usize]) -> usize {
    let size = n + 2;
    let mut capacity = vec![0i32; size * size];
    for arc in arcs {
        capacity[arc.tail as usize * size + arc.head as usize] += 1;
    }
    for &s in sources {
        capacity[n * size + s] += 1;
    }
    for &t in sinks {
        capacity[t * size + n + 1] += 1;
    }
    fn augment(
        capacity: &mut [i32],
        size: usize,
        at: usize,
        sink: usize,
        seen: &mut [bool],
    ) -> bool {
        if at == sink {
            return true;
        }
        seen[at] = true;
        for next in 0..size {
            if !seen[next]
                && capacity[at * size + next] > 0
                && augment(capacity, size, next, sink, seen)
            {
                capacity[at * size + next] -= 1;
                capacity[next * size + at] += 1;
                return true;
            }
        }
        false
    }
    let mut flow = 0;
    loop {
        let mut seen = vec![false; size];
        if !augment(&mut capacity, size, n, n + 1, &mut seen) {
            return flow;
        }
        flow += 1;
    }
}

/// The terminals of one try: the supply of each node and commodity.
fn replay_supply(order: &[u32], pairs: usize) -> Vec<[i8; 2]> {
    let mut supply = vec![[0i8; 2]; order.len()];
    for k in 0..2 {
        for i in 0..pairs {
            supply[order[2 * pairs * k + 2 * i] as usize][k] = 1;
            supply[order[2 * pairs * k + 2 * i + 1] as usize][k] = -1;
        }
    }
    supply
}

/// Whether both commodities of `supply` route alone over the arcs of `meta`.
fn routes_alone(meta: &Meta, supply: &[[i8; 2]]) -> bool {
    (0..2).all(|k| {
        let nodes = |wanted: i8| -> Vec<usize> {
            (0..supply.len())
                .filter(|&v| supply[v][k] == wanted)
                .collect()
        };
        let (sources, sinks) = (nodes(1), nodes(-1));
        max_flow(supply.len(), &meta.arcs, &sources, &sinks) == sources.len()
    })
}

#[test]
fn the_contract_is_pinned() {
    let limits = ROUTING_V1.limits;
    assert_eq!(limits.max_nodes(), 131_072);
    assert_eq!(limits.max_edges(), 2_097_152);
    assert_eq!(limits.max_coefficient(), 1_000);
    assert_eq!(limits.max_offset(), 1_099_511_627_776);
    assert_eq!(limits.max_metadata(), 1_048_576);
    assert_eq!(ROUTING_V1.family_id, "quip/two-commodity-routing");
    assert_eq!(ROUTING_V1.revision, 1);
    assert_eq!(ROUTING_V1.metadata_kinds, &[7]);
    assert_eq!(ROUTING.labels, &["terminal", "cost"]);
    let fields: Vec<(u16, i128, i128)> = ROUTING_SCHEMA
        .iter()
        .map(|spec| (spec.tag, spec.min, spec.max))
        .collect();
    assert_eq!(fields, [(1, 1, 32), (2, 3, 8), (3, 4, 65535)]);
}

#[test]
fn template_follows_the_street_pattern() {
    let topology = lattice_topology(4, 4);
    let instance = build(&params(4, 4, 8), &topology, nonce_of(1));
    let meta = decode(&instance);
    assert!(meta.arcs.len() > 10);
    let n = meta.cells.len();
    let mut out = vec![0usize; n];
    let mut inward = vec![0usize; n];
    for arc in &meta.arcs {
        let tail = meta.cells[arc.tail as usize];
        let head = meta.cells[arc.head as usize];
        if tail.1 == head.1 {
            let step = i32::from(head.0) - i32::from(tail.0);
            assert_eq!(
                step,
                if tail.1.is_multiple_of(2) { 1 } else { -1 },
                "row {tail:?}"
            );
        } else {
            assert_eq!(tail.0, head.0, "a vertical arc stays in its column");
            let step = i32::from(head.1) - i32::from(tail.1);
            assert_eq!(
                step,
                if tail.0.is_multiple_of(2) { 1 } else { -1 },
                "column {tail:?}"
            );
        }
        out[arc.tail as usize] += 1;
        inward[arc.head as usize] += 1;
    }
    assert!(out.iter().chain(&inward).all(|&degree| degree <= 2));
    // Strongly connected: every node reaches node 0 and node 0 reaches every
    // node.
    for forward in [true, false] {
        let mut seen = vec![false; n];
        let mut stack = vec![0usize];
        seen[0] = true;
        while let Some(v) = stack.pop() {
            for arc in &meta.arcs {
                let (from, to) = if forward {
                    (arc.tail as usize, arc.head as usize)
                } else {
                    (arc.head as usize, arc.tail as usize)
                };
                if from == v && !seen[to] {
                    seen[to] = true;
                    stack.push(to);
                }
            }
        }
        assert!(seen.iter().all(|&s| s), "forward {forward}");
    }
}

#[test]
fn spin_roles_are_disjoint() {
    let topology = lattice_topology(4, 4);
    let instance = build(&params(4, 4, 8), &topology, nonce_of(2));
    let meta = decode(&instance);
    let spins: Vec<u32> = meta
        .arcs
        .iter()
        .flat_map(|arc| [arc.x1, arc.chain[0], arc.chain[1]])
        .collect();
    let distinct: BTreeSet<u32> = spins.iter().copied().collect();
    assert_eq!(distinct.len(), spins.len(), "every spin is used once");
    assert_eq!(spins.len(), instance.node_ids().len(), "and is a node");
    assert!(distinct.iter().copied().eq(0..spins.len() as u32));
    // The chain and the capacity couplers are topology edges.
    let pairs = spin_pairs(&topology);
    let id = |pos: u32| instance.node_ids()[pos as usize];
    for arc in &meta.arcs {
        assert!(pairs.contains(&(id(arc.chain[0]), id(arc.chain[1]))));
        assert!(pairs.contains(&(id(arc.x1), id(arc.chain[0]))));
    }
}

/// After pruning, the spins of every two arcs at a cell share a coupler, for
/// each commodity.
#[test]
fn pruning_leaves_no_conflicts() {
    let topology = lattice_topology(4, 4);
    let instance = build(&params(4, 4, 8), &topology, nonce_of(3));
    let meta = decode(&instance);
    let pairs = spin_pairs(&topology);
    let id = |pos: u32| instance.node_ids()[pos as usize];
    for v in 0..meta.cells.len() as u32 {
        let at: Vec<&Arc> = meta
            .arcs
            .iter()
            .filter(|arc| arc.tail == v || arc.head == v)
            .collect();
        for k in 0..2 {
            for (i, first) in at.iter().enumerate() {
                for second in &at[i + 1..] {
                    let pair = (id(spin_at(first, v, k)), id(spin_at(second, v, k)));
                    assert!(pairs.contains(&pair), "cell {v} commodity {k}");
                }
            }
        }
    }
}

#[test]
fn terminals_are_balanced_and_distinct() {
    let topology = lattice_topology(3, 4);
    for divisor in [8u16, 12] {
        let bytes = params(3, 4, divisor);
        for i in 0..6 {
            let instance = build(&bytes, &topology, nonce_of(i));
            let meta = decode(&instance);
            let pairs = meta.cells.len() / usize::from(divisor);
            assert!(pairs >= 1);
            for k in 0..2 {
                let count = |wanted: i8| meta.supply.iter().filter(|s| s[k] == wanted).count();
                assert_eq!(count(1), pairs, "sources of commodity {k}");
                assert_eq!(count(-1), pairs, "sinks of commodity {k}");
            }
            assert!(meta.supply.iter().all(|s| s[0] == 0 || s[1] == 0));
        }
    }
}

#[test]
fn infeasible_terminals_retry_then_give_14() {
    let topology = lattice_topology(3, 4);
    let bytes = params(3, 4, 4);
    let admitted = admitted(&bytes).expect("admit");
    let digest = admitted.descriptor().digest();
    // The template does not depend on the nonce.
    let template = decode(&build(&bytes, &topology, nonce_of(0)));
    let n = template.cells.len();
    let pairs = n / 4;
    let (mut first_try, mut later_try, mut exhausted, mut fifth_would_pass) = (0, 0, 0, 0);
    let mut latest = 0;
    for byte in 0..=255u8 {
        let nonce = nonce_of(byte);
        let mut streams = Streams::new(nonce, digest, *topology.order_digest(), ROUTING.labels);
        let mut stream = streams.open("terminal").expect("terminal stream");
        let tries: Vec<Vec<[i8; 2]>> = (0..5)
            .map(|_| {
                let mut order: Vec<u32> = (0..n as u32).collect();
                shuffle(&mut stream, &mut order).expect("shuffle");
                replay_supply(&order, pairs)
            })
            .collect();
        let feasible: Vec<bool> = tries.iter().map(|t| routes_alone(&template, t)).collect();
        let first_good = feasible.iter().position(|&ok| ok);
        match try_build(&bytes, &topology, nonce) {
            Ok(instance) => {
                let used = first_good.expect("a feasible try within five");
                assert!(used < 4, "nonce {byte}: success needs a try within four");
                assert_eq!(decode(&instance).supply, tries[used], "nonce {byte}");
                if used == 0 {
                    first_try += 1;
                } else {
                    later_try += 1;
                }
                latest = latest.max(used);
            }
            Err(SaltFault::Skip(Code::NoInstance)) => {
                assert!(feasible[..4].iter().all(|&ok| !ok), "nonce {byte}");
                exhausted += 1;
                if feasible[4] {
                    fifth_would_pass += 1;
                }
            }
            Err(other) => panic!("nonce {byte}: {other:?}"),
        }
    }
    assert!(first_try > 0, "some nonce routes on the first try");
    assert!(later_try > 0, "some nonce routes on a later try");
    assert_eq!(latest, 3, "a build on the fourth try exists");
    assert!(exhausted > 0, "some nonce fails four tries");
    assert!(fifth_would_pass > 0, "the fifth try is never drawn");
}

#[test]
fn costs_are_62_or_124_from_the_cost_stream() {
    let topology = lattice_topology(3, 4);
    let bytes = params(3, 4, 8);
    let nonce = nonce_of(5);
    let instance = build(&bytes, &topology, nonce);
    let meta = decode(&instance);
    let mut streams = Streams::new(
        nonce,
        *instance.descriptor_digest(),
        *topology.order_digest(),
        ROUTING.labels,
    );
    let mut stream = streams.open("cost").expect("cost stream");
    let two = NonZeroU32::new(2).expect("two");
    for arc in &meta.arcs {
        for k in 0..2 {
            let expected = if uniform(&mut stream, two) == 0 {
                62
            } else {
                124
            };
            assert_eq!(arc.cost[k], expected);
        }
    }
    let all: BTreeSet<u16> = meta.arcs.iter().flat_map(|arc| arc.cost).collect();
    assert_eq!(all, BTreeSet::from([62, 124]));
}

#[test]
fn chain_strength_follows_the_rule() {
    // The chain strength cap of 1000 cannot trigger: the unit test
    // `chain_strength_never_reaches_the_cap` in `src/routing.rs` checks every
    // street cell configuration.
    let topology = lattice_topology(4, 4);
    let bytes = params(4, 4, 8);
    for i in 0..4 {
        let instance = build(&bytes, &topology, nonce_of(i));
        let meta = decode(&instance);
        let n = instance.node_ids().len();
        let mut local = vec![0i64; n];
        for (&(u, v), &j) in instance.edges().iter().zip(instance.j()) {
            let chain_pair = meta.arcs.iter().any(|arc| {
                (
                    arc.chain[0].min(arc.chain[1]),
                    arc.chain[0].max(arc.chain[1]),
                ) == (u, v)
            });
            if !chain_pair {
                local[u as usize] += i64::from(j).abs();
                local[v as usize] += i64::from(j).abs();
            }
        }
        let worst = meta
            .arcs
            .iter()
            .flat_map(|arc| arc.chain)
            .map(|spin| local[spin as usize] + i64::from(instance.h()[spin as usize]).abs())
            .max()
            .expect("an arc");
        assert_eq!(i64::from(meta.strength), worst + 1);
        assert!(meta.strength <= 1000);
        for arc in &meta.arcs {
            let key = (
                arc.chain[0].min(arc.chain[1]),
                arc.chain[0].max(arc.chain[1]),
            );
            let at = instance.edges().binary_search(&key).expect("chain coupler");
            assert_eq!(instance.j()[at], -i32::from(meta.strength));
        }
    }
}

#[test]
fn instance_energy_matches_the_structured_energy() {
    // Every state of the tiny instance.
    let tiny = tiny_topology();
    let bytes = params(2, 4, 4);
    for i in 0..3 {
        let instance = build(&bytes, &tiny, nonce_of(i));
        let meta = decode(&instance);
        assert_eq!(instance.node_ids().len(), 12);
        for bits in 0..1u32 << 12 {
            let state: Vec<i64> = (0..12)
                .map(|k| 1 - 2 * i64::from((bits >> k) & 1))
                .collect();
            assert_eq!(
                instance_energy(&instance, &state),
                structured_energy(&meta, &state),
                "nonce {i} state {bits}"
            );
        }
    }
    // Random states of larger instances.
    let topology = lattice_topology(3, 4);
    for divisor in [6u16, 8] {
        let instance = build(&params(3, 4, divisor), &topology, nonce_of(9));
        let meta = decode(&instance);
        for seed in 0..64 {
            let state = random_state(instance.node_ids().len(), seed);
            assert_eq!(
                instance_energy(&instance, &state),
                structured_energy(&meta, &state),
                "divisor {divisor} seed {seed}"
            );
        }
    }
}

/// The spin state of the lowest energy of a tiny instance.
fn ground_state(instance: &Instance) -> Vec<i64> {
    let n = instance.node_ids().len();
    let state_of =
        |bits: u32| -> Vec<i64> { (0..n).map(|k| 1 - 2 * i64::from((bits >> k) & 1)).collect() };
    let best = (0..1u32 << n)
        .min_by_key(|&bits| instance_energy(instance, &state_of(bits)))
        .expect("a state");
    state_of(best)
}

#[test]
fn defects_find_broken_chains_and_capacity() {
    let tiny = tiny_topology();
    let bytes = params(2, 4, 4);
    let admitted = admitted(&bytes).expect("admit");
    let defects = |instance: &Instance, state: &[i64]| {
        let spins: Vec<i8> = state.iter().map(|&s| s as i8).collect();
        routing_defects(admitted.params(), instance, &spins)
    };
    // Find a nonce whose terminals the two commodities route without
    // sharing an arc, so the lowest state has no defect.
    let (instance, ground) = (0..32u8)
        .map(|i| build(&bytes, &tiny, nonce_of(i)))
        .map(|instance| {
            let ground = ground_state(&instance);
            (instance, ground)
        })
        .find(|(instance, ground)| {
            defects(instance, ground).is_some_and(|d| {
                d.capacity_conflicts == 0 && d.conservation_violations == 0 && d.broken_chains == 0
            })
        })
        .expect("a nonce with a defect-free lowest state");
    let meta = decode(&instance);
    let clean = defects(&instance, &ground).expect("defects");
    assert!(clean.used_arcs[0] >= 1 && clean.used_arcs[1] >= 1);

    // One flipped chain spin is one broken chain.
    let mut broken = ground.clone();
    let spin = meta.arcs[0].chain[1] as usize;
    broken[spin] = -broken[spin];
    let found = defects(&instance, &broken).expect("defects");
    assert_eq!(found.broken_chains, 1);
    assert_eq!(found.capacity_conflicts, 0);
    assert_eq!(found.used_arcs, clean.used_arcs);

    // An arc that commodity 1 uses and commodity 2 now uses too.
    let used = meta
        .arcs
        .iter()
        .find(|arc| ground[arc.x1 as usize] == -1)
        .expect("an arc of commodity 1");
    let mut doubled = ground.clone();
    doubled[used.chain[0] as usize] = -1;
    doubled[used.chain[1] as usize] = -1;
    let found = defects(&instance, &doubled).expect("defects");
    assert_eq!(found.capacity_conflicts, 1);
    assert_eq!(found.broken_chains, 0);
    assert_eq!(found.used_arcs[1], clean.used_arcs[1] + 1);
    assert!(found.conservation_violations > 0);

    // Refusals.
    assert!(defects(&instance, &ground[1..]).is_none(), "wrong length");
    let mut zero = ground.clone();
    zero[0] = 0;
    assert!(defects(&instance, &zero).is_none(), "a value outside ±1");
    let mut wrong = meta.clone();
    wrong.arcs[0].cost[0] = 63;
    let bad = with_metadata(&instance, encode(&wrong));
    assert!(defects(&bad, &ground).is_none(), "an invalid instance");
}

#[test]
fn admission_codes() {
    let tiny = tiny_topology();
    let admit_tiny = |bytes: &[u8]| bounds(&admitted(bytes).expect("admit"), &tiny);
    assert!(admit_tiny(&params(2, 4, 4)).is_ok());

    // Parameter bounds: code 9.
    assert_eq!(
        admitted(&params(2, 4, 3)).err(),
        Some(Code::ParamsOutOfBounds)
    );
    assert_eq!(
        admitted(&params(2, 2, 4)).err(),
        Some(Code::ParamsOutOfBounds)
    );
    assert_eq!(
        admitted(&params(0, 4, 4)).err(),
        Some(Code::ParamsOutOfBounds)
    );
    assert_eq!(
        admitted(&params(33, 4, 4)).err(),
        Some(Code::ParamsOutOfBounds)
    );

    // No arc: code 12. Zephyr nodes without edges give no arc.
    let (nodes, _) = zephyr_lattice(2, 4).expect("lattice");
    let bare = BaseTopology::new(nodes, &[]).expect("topology");
    assert_eq!(
        bounds(&admitted(&params(2, 4, 4)).expect("admit"), &bare),
        Err(Code::TopologyIncompatible),
        "no arc"
    );
    // A self-loop: code 12.
    let looped = BaseTopology::new(vec![17, 21], &[(17, 17), (17, 21)]).expect("topology");
    assert_eq!(
        bounds(&admitted(&params(2, 4, 4)).expect("admit"), &looped),
        Err(Code::TopologyIncompatible),
        "self-loop"
    );
    // A pair that is not a Zephyr coupler, and an id that is not a node of
    // the lattice: code 12.
    let (nodes, edges) = zephyr_lattice(2, 4).expect("lattice");
    let mut extra = edges;
    extra.push((0, nodes.len() as u32 - 1));
    let crossed = BaseTopology::new(nodes, &extra).expect("topology");
    assert_eq!(
        bounds(&admitted(&params(2, 4, 4)).expect("admit"), &crossed),
        Err(Code::TopologyIncompatible),
        "not a coupler"
    );
    // The lattice of a larger grid is not a subgraph of the named grid.
    assert_eq!(
        bounds(
            &admitted(&params(2, 4, 4)).expect("admit"),
            &lattice_topology(3, 4)
        ),
        Err(Code::TopologyIncompatible),
        "a larger grid"
    );

    // Size bounds above an advertised limit: code 13.
    let tiny_bytes = params(2, 4, 4);
    let admitted_tiny = admitted(&tiny_bytes).expect("admit");
    for limits in [
        AdvertisedLimits {
            max_nodes: 11,
            ..AdvertisedLimits::default()
        },
        AdvertisedLimits {
            max_edges: 30,
            ..AdvertisedLimits::default()
        },
        AdvertisedLimits {
            max_degree: 5,
            ..AdvertisedLimits::default()
        },
    ] {
        assert_eq!(
            admit_topology(&admitted_tiny, &tiny, tiny.order_digest(), &limits),
            Err(Code::BoundsExceeded),
            "{limits:?}"
        );
    }
    let sized = admit_tiny(&params(2, 4, 4)).expect("bounds");
    assert_eq!((sized.nodes, sized.edges), (12, 31));
}

#[test]
fn generation_is_repeatable() {
    let topology = lattice_topology(3, 4);
    let bytes = params(3, 4, 8);
    let mut bodies = Vec::new();
    for nonce in [nonce_of(1), nonce_of(2)] {
        let first = build(&bytes, &topology, nonce);
        let again = build(&bytes, &topology, nonce);
        assert_eq!(first.canonical_bytes(), again.canonical_bytes());
        assert!(check(&bytes, &first));
        bodies.push((
            first.j().to_vec(),
            first.h().to_vec(),
            first.metadata().to_vec(),
        ));
    }
    assert_ne!(bodies[0], bodies[1], "the nonce changes the body");
}

#[test]
fn check_metadata_rejects_each_rule() {
    let topology = lattice_topology(3, 4);
    let bytes = params(3, 4, 8);
    let instance = build(&bytes, &topology, nonce_of(6));
    let meta = decode(&instance);
    assert!(check(&bytes, &instance), "positive control");
    assert_eq!(encode(&meta), instance.metadata(), "the codec round-trips");

    let rejects = |what: &str, edit: &dyn Fn(&mut Meta)| {
        let mut edited = meta.clone();
        edit(&mut edited);
        let bad = with_metadata(&instance, encode(&edited));
        assert!(!check(&bytes, &bad), "{what}");
    };
    let source_1 = meta
        .supply
        .iter()
        .position(|s| s[0] == 1)
        .expect("a source");
    let source_2 = meta
        .supply
        .iter()
        .position(|s| s[1] == 1)
        .expect("a source");
    rejects("a missing source", &|m| m.supply[source_1][0] = 0);
    rejects("a supply of 2", &|m| m.supply[source_1][0] = 2);
    rejects("a node in both commodities", &|m| {
        m.supply[source_1][1] = 1;
        m.supply[source_2][1] = 0;
    });
    rejects("a cost of 63", &|m| m.arcs[0].cost[0] = 63);
    rejects("a cost of 0", &|m| m.arcs[3].cost[1] = 0);
    rejects("a spin used twice", &|m| m.arcs[1].x1 = m.arcs[0].x1);
    rejects("a chain spin used twice", &|m| {
        m.arcs[1].chain[1] = m.arcs[0].chain[0];
    });
    rejects("a spin out of range", &|m| m.arcs[0].x1 = u32::MAX);
    rejects("a chain strength of one more", &|m| m.strength += 1);
    rejects("a chain strength of one less", &|m| m.strength -= 1);
    rejects("a chain strength of 0", &|m| m.strength = 0);
    rejects("a chain strength above 1000", &|m| m.strength = 1001);
    rejects("an arc against the street", &|m| {
        let arc = &mut m.arcs[0];
        std::mem::swap(&mut arc.tail, &mut arc.head);
    });
    rejects("an arc between cells that are not neighbours", &|m| {
        m.arcs[0].head = (m.arcs[0].tail + 2) % m.cells.len() as u32;
    });
    rejects("an arc from a cell to itself", &|m| {
        m.arcs[0].head = m.arcs[0].tail;
    });
    rejects("a repeated cell pair", &|m| {
        let first = m.arcs[0];
        m.arcs[1].tail = first.tail;
        m.arcs[1].head = first.head;
    });
    rejects("a node index out of range", &|m| m.arcs[0].tail = u32::MAX);
    rejects("cells out of order", &|m| m.cells.swap(0, 1));
    rejects("a repeated cell", &|m| m.cells[1] = m.cells[0]);
    rejects("a cell outside the grid", &|m| m.cells[0] = (0, 1));
    rejects("a cell past the grid", &|m| {
        let last = m.cells.len() - 1;
        m.cells[last] = (6, 1);
    });
    rejects("an arc dropped", &|m| {
        let _ = m.arcs.pop();
    });
    rejects("no arc", &|m| m.arcs.clear());
    rejects("no node", &|m| {
        m.cells.clear();
        m.supply.clear();
        m.arcs.clear();
    });

    // A coefficient that differs from the rebuilt energy.
    let mut h = instance.h().to_vec();
    h[0] += 1;
    let moved = rebuilt(
        &instance,
        h,
        instance.j().to_vec(),
        instance.metadata().to_vec(),
    );
    assert!(!check(&bytes, &moved), "a field off by one");
    let mut j = instance.j().to_vec();
    j[0] += 1;
    let moved = rebuilt(
        &instance,
        instance.h().to_vec(),
        j,
        instance.metadata().to_vec(),
    );
    assert!(!check(&bytes, &moved), "a coupling off by one");
    let pair = meta.arcs[0].chain;
    let at = instance
        .edges()
        .binary_search(&(pair[0].min(pair[1]), pair[0].max(pair[1])))
        .expect("chain coupler");
    let mut j = instance.j().to_vec();
    j[at] -= 1;
    let moved = rebuilt(
        &instance,
        instance.h().to_vec(),
        j,
        instance.metadata().to_vec(),
    );
    assert!(!check(&bytes, &moved), "a chain coupler off by one");

    // The section layout.
    let region = instance.metadata().to_vec();
    let mut trailing = region.clone();
    trailing.push(0);
    assert!(
        Instance::new(
            InstanceParts {
                descriptor_digest: *instance.descriptor_digest(),
                topology_order_digest: *instance.topology_order_digest(),
                nonce: instance.nonce().copied(),
                node_ids: instance.node_ids().to_vec(),
                edges: instance.edges().to_vec(),
                h: instance.h().to_vec(),
                j: instance.j().to_vec(),
                offset_milli: instance.offset_milli(),
                metadata: trailing,
            },
            &ROUTING_V1.limits,
            ROUTING_V1.metadata_kinds,
            None
        )
        .is_err(),
        "trailing bytes are code 16 at the instance layer"
    );
    // A payload with a byte too many or too few.
    for edit in [true, false] {
        let mut payload = region[8..].to_vec();
        if edit {
            payload.push(0);
        } else {
            let _ = payload.pop();
        }
        let bad = with_metadata(&instance, section(&payload));
        assert!(!check(&bytes, &bad), "payload length edit {edit}");
    }
    let empty = with_metadata(&instance, vec![0, 0]);
    assert!(!check(&bytes, &empty), "no section");
}

#[test]
fn oversized_counts_are_rejected_without_allocation() {
    let topology = lattice_topology(3, 4);
    let bytes = params(3, 4, 8);
    let instance = build(&bytes, &topology, nonce_of(11));
    assert!(check(&bytes, &instance));
    let region = instance.metadata().to_vec();
    let n = decode(&instance).cells.len();
    // The node count is at payload offset 0 and the arc count follows the
    // node records. The payload starts after the 8-byte section head.
    for at in [8, 8 + 4 + 6 * n] {
        let mut damaged = region.clone();
        damaged[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let bad = with_metadata(&instance, damaged);
        assert!(!check(&bytes, &bad), "count at {at}");
    }
    // A short payload that names a huge node count.
    let mut payload = u32::MAX.to_le_bytes().to_vec();
    payload.extend_from_slice(&[0; 12]);
    let bad = with_metadata(&instance, section(&payload));
    assert!(!check(&bytes, &bad), "a short payload");
}

#[test]
fn recipe_is_canonical() {
    assert_eq!(ROUTING_RECIPES.len(), 1);
    let recipe = &ROUTING_RECIPES[0];
    assert_eq!(recipe.alias, "r32");
    assert_eq!(recipe.cell, "two-commodity-routing");
    assert_eq!(recipe.params, params(12, 4, 32));
    assert!(admitted(recipe.params).is_ok());
}

#[test]
fn live_template_matches_the_research_template() {
    let topology = common::live_topology();
    let recipe = &ROUTING_RECIPES[0];
    let instance = build(recipe.params, &topology, nonce_of(0));
    let meta = decode(&instance);
    assert_eq!(meta.cells.len(), 505, "template nodes");
    assert_eq!(meta.arcs.len(), 880, "template arcs");
    assert_eq!(instance.node_ids().len(), 3 * 880);
    let mut out = vec![0usize; 505];
    let mut inward = vec![0usize; 505];
    for arc in &meta.arcs {
        out[arc.tail as usize] += 1;
        inward[arc.head as usize] += 1;
    }
    let mut histogram = std::collections::BTreeMap::new();
    for (o, i) in out.iter().zip(&inward) {
        *histogram.entry((*o, *i)).or_insert(0usize) += 1;
    }
    assert_eq!(
        histogram,
        std::collections::BTreeMap::from([((1, 1), 44), ((1, 2), 86), ((2, 1), 86), ((2, 2), 289)])
    );
    assert_eq!(meta.strength, 931);
    assert_eq!(meta.supply.iter().filter(|s| s[0] == 1).count(), 505 / 32);
    assert!(check(recipe.params, &instance));
}
