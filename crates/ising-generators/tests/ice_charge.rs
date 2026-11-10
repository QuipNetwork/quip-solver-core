//! Charged ice: tile charges, penalties, and the metadata check.

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code: a failed lookup is a test failure"
)]

use std::collections::BTreeSet;

use ising_generators::descriptor::Value;
use ising_generators::family::{generate_instance, BaseTopology};
use ising_generators::ice::{charge_defects, CHARGED_ICE_V1, CHARGE_UNIT, ICE_CHARGE_RECIPE};
use ising_generators::instance::{Instance, InstanceParts};
use ising_generators::Code;

#[expect(dead_code, reason = "this test uses only the live topology loader")]
mod common;

#[path = "common/admit.rs"]
mod admit;
#[path = "common/lattice.rs"]
mod lattice;
#[path = "common/nonce.rs"]
mod nonce;
#[path = "common/size_bounds.rs"]
mod size_bounds;

use admit::admitted;
use lattice::lattice_topology;
use nonce::nonce_of;
use size_bounds::bounds;

/// Canonical parameter bytes: format, then `(tag, len, payload)` per field.
fn params(m: u16, t: u8, scale: u16, background: u16, charged: u16) -> Vec<u8> {
    let mut bytes = vec![0x01];
    bytes.extend_from_slice(&[1, 0, 2, 0, 0, 0]);
    bytes.extend_from_slice(&m.to_le_bytes());
    bytes.extend_from_slice(&[2, 0, 1, 0, 0, 0, t]);
    for (tag, value) in [(3u8, scale), (4, background), (5, charged)] {
        bytes.extend_from_slice(&[tag, 0, 2, 0, 0, 0]);
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// K4: four nodes, six edges, one tile in adjacency mode.
fn k4() -> BaseTopology {
    let edges = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
    BaseTopology::new(vec![0, 1, 2, 3], &edges).expect("k4")
}

fn build(what: &str, bytes: &[u8], topology: &BaseTopology, nonce: [u8; 32]) -> Instance {
    let built = admitted(&CHARGED_ICE_V1, bytes)
        .map_err(|code| format!("admit: {code}"))
        .and_then(|admitted| {
            let bounds =
                bounds(&admitted, topology).map_err(|code| format!("admit topology: {code}"))?;
            generate_instance(&admitted, topology, &bounds, nonce)
                .map_err(|fault| format!("generate: {fault:?}"))
        });
    assert!(built.is_ok(), "{what}: {built:?}");
    built.expect("generated")
}

fn check(bytes: &[u8], instance: &Instance) -> bool {
    let admitted = admitted(&CHARGED_ICE_V1, bytes).expect("admit");
    let construction = CHARGED_ICE_V1.construction.expect("construction");
    (construction.check_metadata)(admitted.params(), instance)
}

/// The three metadata payloads, in kind order 1, 2, 6.
fn split(what: &str, instance: &Instance) -> [Vec<u8>; 3] {
    let region = instance.metadata();
    assert_eq!(&region[..2], &[3, 0], "{what}: three sections");
    let mut at = 2;
    [1u16, 2, 6].map(|kind| {
        assert_eq!(
            &region[at..at + 2],
            &kind.to_le_bytes(),
            "{what}: section kind {kind}"
        );
        let len = u32::from_le_bytes(region[at + 2..at + 6].try_into().expect("four bytes"));
        let end = at + 6 + len as usize;
        let payload = region[at + 6..end].to_vec();
        at = end;
        payload
    })
}

/// A region from payloads listed with their kinds, in the given order.
fn join(sections: &[(u16, &Vec<u8>)]) -> Vec<u8> {
    let mut region = u16::try_from(sections.len())
        .expect("fits")
        .to_le_bytes()
        .to_vec();
    for (kind, payload) in sections {
        region.extend_from_slice(&kind.to_le_bytes());
        region.extend_from_slice(&u32::try_from(payload.len()).expect("fits").to_le_bytes());
        region.extend_from_slice(payload);
    }
    region
}

/// The tiles, gauges, and charges of a generated instance.
fn decoded(what: &str, instance: &Instance) -> (Vec<[u32; 4]>, Vec<[i64; 4]>, Vec<i64>) {
    let [tiles, gauges, charges] = split(what, instance);
    let word = |at: usize| u32::from_le_bytes(tiles[at..at + 4].try_into().expect("four bytes"));
    let count = word(0) as usize;
    let tiles: Vec<[u32; 4]> = (0..count)
        .map(|t| [0, 1, 2, 3].map(|k| word(4 + 16 * t + 4 * k)))
        .collect();
    let gauges: Vec<[i64; 4]> = gauges[4..]
        .chunks(4)
        .map(|c| [0, 1, 2, 3].map(|k| if c[k] == 0 { 1 } else { -1 }))
        .collect();
    let charges: Vec<i64> = charges[4..]
        .iter()
        .map(|&b| i64::from(i8::from_le_bytes([b])))
        .collect();
    assert_eq!(tiles.len(), gauges.len(), "{what}: gauge count");
    assert_eq!(tiles.len(), charges.len(), "{what}: charge count");
    (tiles, gauges, charges)
}

fn tile_pairs(tile: [u32; 4]) -> [(u32, u32); 6] {
    let [a, b, c, d] = tile;
    [(a, b), (a, c), (a, d), (b, c), (b, d), (c, d)]
}

fn edge_position(instance: &Instance, edge: (u32, u32)) -> usize {
    instance.edges().binary_search(&edge).expect("an edge")
}

/// The mutable fields of an instance.
#[derive(Clone)]
struct Fields {
    h: Vec<i32>,
    j: Vec<i32>,
    offset: i64,
    metadata: Vec<u8>,
}

fn fields(instance: &Instance) -> Fields {
    Fields {
        h: instance.h().to_vec(),
        j: instance.j().to_vec(),
        offset: instance.offset_milli(),
        metadata: instance.metadata().to_vec(),
    }
}

/// A copy of `instance` with new fields.
fn rebuilt(what: &str, instance: &Instance, new: Fields) -> Instance {
    rebuilt_with_kinds(what, instance, new, CHARGED_ICE_V1.metadata_kinds)
}

/// A copy of `instance` with new fields, under the registered kind list `kinds`.
fn rebuilt_with_kinds(what: &str, instance: &Instance, new: Fields, kinds: &[u16]) -> Instance {
    let parts = InstanceParts {
        descriptor_digest: *instance.descriptor_digest(),
        topology_order_digest: *instance.topology_order_digest(),
        nonce: instance.nonce().copied(),
        node_ids: instance.node_ids().to_vec(),
        edges: instance.edges().to_vec(),
        h: new.h,
        j: new.j,
        offset_milli: new.offset,
        metadata: new.metadata,
    };
    let built = Instance::new(parts, &CHARGED_ICE_V1.limits, kinds, None);
    assert!(built.is_ok(), "{what}: rebuilt instance is well formed");
    built.expect("rebuilt instance is well formed")
}

/// `A * (sum - q)^2 / 2`, exact because `sum - q` is even.
fn direct_penalty(scale: i64, gauge: &[i64; 4], spins: [i64; 4], q: i64) -> i64 {
    let sum: i64 = gauge.iter().zip(spins).map(|(g, s)| g * s).sum();
    scale * (sum - q) * (sum - q) / 2
}

/// `sum J s s + sum h s + offset` for a spin state indexed by node position.
fn encoded_energy(instance: &Instance, spins: &[i64]) -> i64 {
    let mut energy = instance.offset_milli();
    for (&(u, v), &j) in instance.edges().iter().zip(instance.j()) {
        energy += i64::from(j) * spins[u as usize] * spins[v as usize];
    }
    for (&h, &s) in instance.h().iter().zip(spins) {
        energy += i64::from(h) * s;
    }
    energy
}

/// A fixed xorshift, so the test needs no random-number dependency.
struct Xorshift(u64);

impl Xorshift {
    fn next_bit(&mut self) -> bool {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 >> 63 == 1
    }

    fn spins(&mut self, n: usize) -> Vec<i64> {
        (0..n)
            .map(|_| if self.next_bit() { 1 } else { -1 })
            .collect()
    }
}

/// A K4 instance with the wanted charge, found among the nonces 0..=255. With
/// `uniform` the four gauges are equal, otherwise they differ.
fn k4_with_gauge(
    bytes_for: impl Fn(u16) -> Vec<u8>,
    want: i64,
    uniform: bool,
) -> (Vec<u8>, Instance) {
    let topology = k4();
    let bytes = if want == 0 {
        bytes_for(0)
    } else {
        bytes_for(256)
    };
    for nonce in 0..=255u8 {
        let instance = build("k4_with_gauge", &bytes, &topology, nonce_of(nonce));
        let (_, gauges, charges) = decoded("k4_with_gauge", &instance);
        let equal = gauges[0].iter().all(|&g| g == gauges[0][0]);
        if charges == [want] && equal == uniform {
            return (bytes, instance);
        }
    }
    panic!("no nonce in 0..=255 gives charge {want} with uniform {uniform}");
}

#[test]
fn every_state_of_one_tile_matches_the_direct_penalty() {
    for scale in [250u16, 7] {
        for want in [0, i64::from(CHARGE_UNIT), -i64::from(CHARGE_UNIT)] {
            for uniform in [true, false] {
                let (bytes, instance) =
                    k4_with_gauge(|charged| params(0, 0, scale, 31, charged), want, uniform);
                assert!(check(&bytes, &instance));
                let (tiles, gauges, _) = decoded(
                    "every_state_of_one_tile_matches_the_direct_penalty",
                    &instance,
                );
                assert_eq!(tiles, vec![[0, 1, 2, 3]]);
                for state in 0..16u32 {
                    let spins: [i64; 4] =
                        [0, 1, 2, 3].map(|k| if state >> k & 1 == 0 { 1 } else { -1 });
                    let direct = direct_penalty(i64::from(scale), &gauges[0], spins, want);
                    assert_eq!(
                        encoded_energy(&instance, &spins),
                        direct,
                        "scale {scale} q {want} uniform {uniform} state {state}"
                    );
                }
            }
        }
    }
}

#[test]
fn encoded_energy_equals_direct_energy_on_a_lattice() {
    let topology = lattice_topology(1, 2);
    let scale = 250u16;
    let bytes = params(1, 2, scale, 31, 128);
    let mut rng = Xorshift(0x9e37_79b9_7f4a_7c15);
    for nonce in 0..4u8 {
        let instance = build(
            "encoded_energy_equals_direct_energy_on_a_lattice",
            &bytes,
            &topology,
            nonce_of(nonce),
        );
        let (tiles, gauges, charges) = decoded(
            "encoded_energy_equals_direct_energy_on_a_lattice",
            &instance,
        );
        let tile_edges: BTreeSet<(u32, u32)> = tiles.iter().flat_map(|&t| tile_pairs(t)).collect();
        for _ in 0..64 {
            let spins = rng.spins(instance.node_ids().len());
            let mut direct = 0;
            for ((tile, gauge), &q) in tiles.iter().zip(&gauges).zip(&charges) {
                let local = tile.map(|pos| spins[pos as usize]);
                direct += direct_penalty(i64::from(scale), gauge, local, q);
            }
            for (&(u, v), &j) in instance.edges().iter().zip(instance.j()) {
                if !tile_edges.contains(&(u, v)) {
                    direct += i64::from(j) * spins[u as usize] * spins[v as usize];
                }
            }
            assert_eq!(encoded_energy(&instance, &spins), direct, "nonce {nonce}");
        }
        assert!(instance.j().iter().all(|&j| j != 0), "no coupling is zero");
    }
}

#[test]
fn generation_is_repeatable() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 250, 31, 128);
    let mut seen = Vec::new();
    for nonce in [nonce_of(1), nonce_of(2)] {
        let first = build("generation_is_repeatable", &bytes, &topology, nonce)
            .canonical_bytes()
            .expect("bytes");
        let again = build("generation_is_repeatable", &bytes, &topology, nonce)
            .canonical_bytes()
            .expect("bytes");
        assert_eq!(first, again);
        seen.push(first);
    }
    assert_ne!(seen[0], seen[1], "the nonce changes the instance");
}

#[test]
fn both_modes_build_valid_instances() {
    let topology = lattice_topology(2, 2);
    let mut counts = Vec::new();
    for (m, t) in [(2, 2), (0, 0)] {
        let bytes = params(m, t, 250, 31, 128);
        let instance = build(
            "both_modes_build_valid_instances",
            &bytes,
            &topology,
            nonce_of(3),
        );
        assert!(check(&bytes, &instance), "mode ({m}, {t})");
        let (tiles, _, _) = decoded("both_modes_build_valid_instances", &instance);
        assert!(!tiles.is_empty());
        counts.push(tiles.len());
        assert_eq!(instance.edges().len(), topology.edges().len(), "all edges");
    }
    assert_eq!(
        counts[0], counts[1],
        "both modes select the same tile count"
    );
}

#[test]
fn a_node_in_two_charged_tiles_sums_its_fields() {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 250, 31, 256);
    let uncharged = params(2, 2, 250, 31, 0);
    let mut shared_nodes = 0;
    for nonce in 0..4u8 {
        let context = format!("nonce {nonce}");
        let instance = build(&context, &bytes, &topology, nonce_of(nonce));
        let (tiles, gauges, charges) = decoded(&context, &instance);
        // The tile count of the same nonce with no charge, an independent
        // source for the offset.
        let (zero_tiles, _, _) = decoded(
            &context,
            &build(&context, &uncharged, &topology, nonce_of(nonce)),
        );
        let mut want = vec![0i64; instance.node_ids().len()];
        let mut slots = vec![0u32; want.len()];
        for ((tile, gauge), &q) in tiles.iter().zip(&gauges).zip(&charges) {
            assert_ne!(q, 0, "{context}: every tile is charged");
            for (&pos, &g) in tile.iter().zip(gauge) {
                want[pos as usize] -= 250 * q * g;
                slots[pos as usize] += 1;
            }
        }
        shared_nodes += slots.iter().filter(|&&s| s == 2).count();
        assert!(
            charges.iter().all(|&q| q.abs() == 2),
            "nonce {nonce}: every charge is 2 or -2"
        );
        let count = i64::try_from(zero_tiles.len()).expect("fits");
        assert_eq!(
            instance.offset_milli(),
            count * 250 * 8 / 2,
            "nonce {nonce}: offset"
        );
        for (at, (&h, &w)) in instance.h().iter().zip(&want).enumerate() {
            assert_eq!(i64::from(h), w, "{context}: node {at}");
            assert!(h.abs() <= 1000, "{context}: node {at}");
        }
        assert!(check(&bytes, &instance), "{context}: check");
    }
    assert!(shared_nodes > 0, "z(2,2) has nodes in two tiles");
}

#[test]
fn a_charged_of_zero_gives_no_charges() {
    let topology = lattice_topology(2, 2);
    for nonce in 0..4u8 {
        let none = params(2, 2, 250, 31, 0);
        let instance = build(
            "a_charged_of_zero_gives_no_charges",
            &none,
            &topology,
            nonce_of(nonce),
        );
        let (tiles, _, charges) = decoded("a_charged_of_zero_gives_no_charges", &instance);
        assert!(charges.iter().all(|&q| q == 0));
        assert!(instance.h().iter().all(|&h| h == 0));
        let count = i64::try_from(tiles.len()).expect("fits");
        assert_eq!(instance.offset_milli(), count * 250 * 4 / 2);
        assert!(check(&none, &instance));
    }
}

/// K4 plus isolated nodes, `nodes` nodes in all.
fn k4_with_isolated(nodes: u32) -> BaseTopology {
    let edges = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
    BaseTopology::new((0..nodes).collect(), &edges).expect("k4 and isolated nodes")
}

/// One admission case: label, parameter bytes, topology, outcome. A row whose
/// parameters fail admission reports that code and never reaches the topology.
type AdmissionRow<'a> = (&'a str, &'a [u8], &'a BaseTopology, Result<(), Code>);

#[test]
fn admission_codes() {
    let lattice = lattice_topology(2, 2);
    let cycle =
        BaseTopology::new(vec![0, 1, 2, 3], &[(0, 1), (1, 2), (2, 3), (3, 0)]).expect("cycle");
    let star_edges: Vec<(u32, u32)> = (1..=33).map(|leaf| (0, leaf)).collect();
    let star = BaseTopology::new((0..34).collect(), &star_edges).expect("star");
    // A 33-leaf star plus a self-loop: admission checks the self-loop before
    // the degree, so the request gets code 12 and never reaches the size
    // bounds.
    let mut looped_edges = star_edges;
    looped_edges.push((0, 0));
    let looped_star =
        BaseTopology::new((0..34).collect(), &looped_edges).expect("star with a loop");
    let loop_only = BaseTopology::new(vec![0, 1, 2, 3], &[(0, 1), (0, 0)]).expect("loop");
    let at_node_limit = k4_with_isolated(131_072);
    let over_node_limit = k4_with_isolated(131_073);
    let mixed = params(2, 0, 250, 31, 128);
    let adjacency = params(0, 0, 250, 31, 128);
    let too_strong = params(2, 2, 251, 31, 128);
    let too_charged = params(2, 2, 250, 31, 257);
    let incompatible = Err(Code::TopologyIncompatible);
    let rows: [AdmissionRow<'_>; 9] = [
        ("mixed shape", &mixed, &lattice, incompatible),
        (
            "adjacency mode on a cycle",
            &adjacency,
            &cycle,
            incompatible,
        ),
        (
            "scale too strong",
            &too_strong,
            &lattice,
            Err(Code::ParamsOutOfBounds),
        ),
        (
            "charged above 256",
            &too_charged,
            &lattice,
            Err(Code::ParamsOutOfBounds),
        ),
        (
            "a 33-leaf star",
            &adjacency,
            &star,
            Err(Code::BoundsExceeded),
        ),
        (
            "a self-loop wins over a degree fault",
            &adjacency,
            &looped_star,
            incompatible,
        ),
        ("a self-loop alone", &adjacency, &loop_only, incompatible),
        ("131072 nodes", &adjacency, &at_node_limit, Ok(())),
        (
            "131073 nodes",
            &adjacency,
            &over_node_limit,
            Err(Code::BoundsExceeded),
        ),
    ];
    for (label, bytes, topology, expected) in rows {
        let outcome = admitted(&CHARGED_ICE_V1, bytes)
            .and_then(|admitted| bounds(&admitted, topology))
            .map(|_| ());
        assert_eq!(outcome, expected, "{label}");
    }
}

/// A valid z(2,2) instance: its parameter bytes and the instance.
fn valid_z22(nonce: u8) -> (Vec<u8>, Instance) {
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, 250, 31, 128);
    let instance = build("valid_z22", &bytes, &topology, nonce_of(nonce));
    assert!(check(&bytes, &instance), "the base instance is valid");
    (bytes, instance)
}

/// The section kind and count rules: each mutation of `instance` breaks one rule.
fn rejects_kind_and_count_faults(bytes: &[u8], instance: &Instance) {
    // Breaks one rule: one registered kind among the three sections is wrong.
    // The kinds stay strictly ascending, so the first section has no other
    // kind to take. The outer kind list is permissive, so only the family
    // comparison can reject.
    let [tiles, gauges, charges] = split("kind mutations", instance);
    let valid = admitted(&CHARGED_ICE_V1, bytes).expect("admit");
    let construction = CHARGED_ICE_V1.construction.expect("construction");
    for wrong in [[1, 3, 6], [1, 2, 3]] {
        let mut fields = fields(instance);
        fields.metadata = join(&[
            (wrong[0], &tiles),
            (wrong[1], &gauges),
            (wrong[2], &charges),
        ]);
        let built =
            rebuilt_with_kinds(&format!("kinds {wrong:?}"), instance, fields, &[1, 2, 3, 6]);
        assert!(
            !(construction.check_metadata)(valid.params(), &built),
            "kinds {wrong:?}"
        );
    }

    // Breaks one rule: a section count is u32::MAX.
    for which in [0usize, 1, 2] {
        let mut payloads = split(&format!("section {which} count"), instance);
        payloads[which][..4].copy_from_slice(&u32::MAX.to_le_bytes());
        let mut bad = fields(instance);
        bad.metadata = join(&[(1, &payloads[0]), (2, &payloads[1]), (6, &payloads[2])]);
        assert!(
            !check(
                bytes,
                &rebuilt(&format!("section {which} count"), instance, bad)
            ),
            "section {which} count"
        );
    }
}

#[test]
fn check_metadata_rejects_each_rule() {
    let (bytes, instance) = valid_z22(10);
    let base = fields(&instance);
    let (tiles, _, _) = decoded("check_metadata_rejects_each_rule", &instance);
    let tile_edges: BTreeSet<(u32, u32)> = tiles.iter().flat_map(|&t| tile_pairs(t)).collect();
    let [tile_payload, gauge_payload, _] = split("check_metadata_rejects_each_rule", &instance);
    // The positive control: each mutation below starts from this instance.
    assert!(
        check(&bytes, &rebuilt("control", &instance, base.clone())),
        "control"
    );

    // Breaks one rule: a tile edge carries the wrong sign.
    let tile_edge = *tile_edges.iter().next().expect("a tile edge");
    let mut bad = base.clone();
    bad.j[edge_position(&instance, tile_edge)] *= -1;
    assert!(!check(&bytes, &rebuilt("tile J", &instance, bad)), "tile J");

    // Breaks one rule: a background edge has the wrong magnitude.
    let background_at = instance
        .edges()
        .iter()
        .position(|e| !tile_edges.contains(e))
        .expect("a background edge");
    let mut bad = base.clone();
    bad.j[background_at] = if bad.j[background_at] > 0 { 32 } else { -32 };
    assert!(
        !check(&bytes, &rebuilt("background J", &instance, bad)),
        "background J"
    );

    // Breaks one rule: a field differs from its formula.
    let mut bad = base.clone();
    bad.h[0] += 1;
    assert!(!check(&bytes, &rebuilt("h", &instance, bad)), "h");

    // Breaks one rule: the offset differs from its formula.
    let mut bad = base.clone();
    bad.offset += 1;
    assert!(!check(&bytes, &rebuilt("offset", &instance, bad)), "offset");

    // Breaks one rule: the kind list is not exactly 1, 2, 6. Instance::new
    // already refuses kinds out of order, so the tile charges are left out.
    let mut bad = base;
    bad.metadata = join(&[(1, &tile_payload), (2, &gauge_payload)]);
    assert!(
        !check(&bytes, &rebuilt("kind list", &instance, bad)),
        "kind list"
    );

    rejects_kind_and_count_faults(&bytes, &instance);
}

/// The fields a verifier expects for these tiles, gauges, and charges, with the
/// tile edge couplings of `instance` kept. Like the family check, it stops at
/// the shortest of the three lists. The metadata is `region`.
fn consistent_fields(
    instance: &Instance,
    scale: i64,
    decoded: (&[[u32; 4]], &[[i64; 4]], &[i64]),
    region: Vec<u8>,
) -> Fields {
    let (tiles, gauges, charges) = decoded;
    let mut h = vec![0_i64; instance.node_ids().len()];
    let mut offset = 0_i64;
    for ((tile, gauge), &q) in tiles.iter().zip(gauges).zip(charges) {
        for (&pos, &g) in tile.iter().zip(gauge) {
            h[pos as usize] -= scale * q * g;
        }
        offset += scale * (4 + q * q) / 2;
    }
    Fields {
        h: h.into_iter()
            .map(|v| i32::try_from(v).expect("fits"))
            .collect(),
        j: instance.j().to_vec(),
        offset,
        metadata: region,
    }
}

/// A section payload that holds `count` and then `bytes`.
fn counted(count: usize, bytes: &[u8]) -> Vec<u8> {
    let mut payload = u32::try_from(count).expect("fits").to_le_bytes().to_vec();
    payload.extend_from_slice(bytes);
    payload
}

#[test]
fn each_parser_rule_rejects_on_its_own() {
    let scale = 100;
    let topology = lattice_topology(2, 2);
    let bytes = params(2, 2, scale, 31, 128);
    let instance = build("parser", &bytes, &topology, nonce_of(10));
    let (tiles, gauges, charges) = decoded("parser", &instance);
    let count = tiles.len();
    assert!(count >= 2, "the instance has several tiles");
    let [tile_payload, gauge_payload, charge_payload] = split("parser", &instance);
    let scale = i64::from(scale);
    let region = |kinds: [u16; 3], gauges: &Vec<u8>, charges: &Vec<u8>| {
        join(&[
            (kinds[0], &tile_payload),
            (kinds[1], gauges),
            (kinds[2], charges),
        ])
    };

    // The positive control: the same helper accepts the unchanged data.
    let fields_of = |gauges_in: &[[i64; 4]], charges_in: &[i64], region: Vec<u8>| {
        consistent_fields(&instance, scale, (&tiles, gauges_in, charges_in), region)
    };
    let good = fields_of(&gauges, &charges, instance.metadata().to_vec());
    let accepts = |fields: Fields, what: &str| check(&bytes, &rebuilt(what, &instance, fields));
    assert!(accepts(good, "control"), "control");
    let reject = |fields: Fields, why: &str| assert!(!accepts(fields, why), "{why}");

    // A charge outside {0, 2, -2}, with h and the offset computed from it.
    for value in [4_i64, 1] {
        let mut wrong = charges.clone();
        wrong[0] = value;
        let payload = counted(
            count,
            &wrong
                .iter()
                .map(|&q| q.to_le_bytes()[0])
                .collect::<Vec<_>>(),
        );
        let region = region([1, 2, 6], &gauge_payload, &payload);
        reject(
            fields_of(&gauges, &wrong, region),
            &format!("charge {value}"),
        );
    }

    // A charge list with a count that matches its length and differs from the tiles.
    let bytes_of = |list: &[i64]| {
        list.iter()
            .map(|&q| q.to_le_bytes()[0])
            .collect::<Vec<u8>>()
    };
    let mut shorter = charges.clone();
    let _last = shorter.pop();
    let payload = counted(shorter.len(), &bytes_of(&shorter));
    let region_short = region([1, 2, 6], &gauge_payload, &payload);
    reject(
        fields_of(&gauges, &shorter, region_short),
        "shorter charge list",
    );
    let mut longer = charges.clone();
    longer.push(0);
    let payload = counted(longer.len(), &bytes_of(&longer));
    let region_long = region([1, 2, 6], &gauge_payload, &payload);
    reject(
        fields_of(&gauges, &longer, region_long),
        "longer charge list",
    );

    // The count equals the tiles, and valid bytes trail the list.
    let mut trailing = bytes_of(&charges);
    trailing.push(0);
    let payload = counted(count, &trailing);
    let region_trail = region([1, 2, 6], &gauge_payload, &payload);
    reject(
        fields_of(&gauges, &charges, region_trail),
        "charge bytes after the list",
    );

    // An invalid gauge byte, replacing a byte that holds -1 so a lenient
    // parser would read the same gauge.
    let mut invalid = gauge_payload;
    let at = 4 + invalid[4..]
        .iter()
        .position(|&b| b == 1)
        .expect("a -1 gauge");
    invalid[at] = 2;
    let region_invalid = region([1, 2, 6], &invalid, &charge_payload);
    reject(fields_of(&gauges, &charges, region_invalid), "gauge byte 2");

    // A gauge list with a count that matches its length and differs from the tiles.
    let mut fewer = gauges.clone();
    let _last = fewer.pop();
    let fewer_bytes: Vec<u8> = fewer.iter().flatten().map(|&g| u8::from(g < 0)).collect();
    let payload = counted(fewer.len(), &fewer_bytes);
    let region_fewer = region([1, 2, 6], &payload, &charge_payload);
    reject(
        fields_of(&fewer, &charges, region_fewer),
        "shorter gauge list",
    );
    let mut more = gauges.clone();
    more.push([1; 4]);
    let more_bytes: Vec<u8> = more.iter().flatten().map(|&g| u8::from(g < 0)).collect();
    let payload = counted(more.len(), &more_bytes);
    let region_more = region([1, 2, 6], &payload, &charge_payload);
    reject(fields_of(&more, &charges, region_more), "longer gauge list");

    // The count equals the tiles, and valid bytes trail the list.
    let mut trailing: Vec<u8> = gauges.iter().flatten().map(|&g| u8::from(g < 0)).collect();
    trailing.extend_from_slice(&[0; 4]);
    let payload = counted(count, &trailing);
    let region_trail = region([1, 2, 6], &payload, &charge_payload);
    reject(
        fields_of(&gauges, &charges, region_trail),
        "gauge bytes after the list",
    );
}

#[test]
fn charge_defects_count_mismatched_tiles() {
    for want in [0, i64::from(CHARGE_UNIT), -i64::from(CHARGE_UNIT)] {
        let (bytes, instance) =
            k4_with_gauge(|charged| params(0, 0, 250, 31, charged), want, false);
        let admitted = admitted(&CHARGED_ICE_V1, &bytes).expect("admit");
        let params = admitted.params();
        let (_, gauges, _) = decoded("charge_defects_count_mismatched_tiles", &instance);
        let ground: Vec<i8> = (0..16u32)
            .map(|state| [0, 1, 2, 3].map(|k| if state >> k & 1 == 0 { 1i8 } else { -1 }))
            .find(|spins| {
                let sum: i64 = gauges[0]
                    .iter()
                    .zip(spins)
                    .map(|(g, &s)| g * i64::from(s))
                    .sum();
                sum == want
            })
            .expect("a state with the charge")
            .to_vec();
        assert_eq!(
            charge_defects(params, &instance, &ground),
            Some(0),
            "q {want}"
        );
        let mut flipped = ground.clone();
        flipped[2] = -flipped[2];
        assert_eq!(
            charge_defects(params, &instance, &flipped),
            Some(1),
            "q {want}"
        );
        assert_eq!(charge_defects(params, &instance, &ground[..3]), None);
        let mut zero = ground.clone();
        zero[0] = 0;
        assert_eq!(charge_defects(params, &instance, &zero), None);
    }

    // An instance that fails the family validation gives no count, though
    // every section decodes: a wrong offset, field, or tile coupling.
    let (bytes, instance) = valid_z22(12);
    let valid = admitted(&CHARGED_ICE_V1, &bytes).expect("admit");
    let spins = vec![1i8; instance.node_ids().len()];
    let (tiles, _, _) = decoded("charge_defects_count_mismatched_tiles", &instance);
    let tile_edge = tile_pairs(tiles[0])[0];
    let mut offset = fields(&instance);
    offset.offset += 1;
    let mut field = fields(&instance);
    field.h[0] += 1;
    let mut coupling = fields(&instance);
    coupling.j[edge_position(&instance, tile_edge)] *= -1;
    for (why, new) in [("offset", offset), ("h", field), ("tile J", coupling)] {
        let bad = rebuilt("charge_defects_count_mismatched_tiles", &instance, new);
        assert!(!check(&bytes, &bad), "{why} is invalid");
        assert_eq!(charge_defects(valid.params(), &bad, &spins), None, "{why}");
    }

    // A malformed instance: the tile charges are missing.
    let (bytes, instance) = valid_z22(12);
    let [tiles, gauges, _] = split("charge_defects_count_mismatched_tiles", &instance);
    let mut bad = fields(&instance);
    bad.metadata = join(&[(1, &tiles), (2, &gauges)]);
    let bad = rebuilt("charge_defects_count_mismatched_tiles", &instance, bad);
    let admitted = admitted(&CHARGED_ICE_V1, &bytes).expect("admit");
    let spins = vec![1i8; bad.node_ids().len()];
    assert_eq!(charge_defects(admitted.params(), &bad, &spins), None);
    assert!(charge_defects(admitted.params(), &instance, &spins).is_some());
}

#[test]
fn recipe_params_are_canonical() {
    assert_eq!(ICE_CHARGE_RECIPE.alias, "ice-charge");
    assert_eq!(ICE_CHARGE_RECIPE.cell, "ice-charge");
    assert_eq!(ICE_CHARGE_RECIPE.params, params(12, 4, 250, 31, 128));
    let admitted =
        admitted(&CHARGED_ICE_V1, ICE_CHARGE_RECIPE.params).expect("the recipe is admitted");
    assert_eq!(admitted.params().get(3), Some(Value::Unsigned(250)));
    assert_eq!(admitted.params().get(4), Some(Value::Unsigned(31)));
    assert_eq!(admitted.params().get(5), Some(Value::Unsigned(128)));
}

#[test]
fn live_recipe_builds() {
    let topology = common::live_topology();
    let instance = build(
        "live_recipe_builds",
        ICE_CHARGE_RECIPE.params,
        topology,
        nonce_of(1),
    );
    let (tiles, _, _) = decoded("live_recipe_builds", &instance);
    assert_eq!(tiles.len(), 1785);
    assert!(instance.h().iter().all(|&h| h.abs() <= 1000));
    assert!(instance.j().iter().all(|&j| j != 0), "no coupling is zero");
    assert!(check(ICE_CHARGE_RECIPE.params, &instance));
}
