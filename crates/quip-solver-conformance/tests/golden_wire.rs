//! Byte-level wire pinning.
//!
//! Every other test in this workspace round-trips a message through the same
//! generated code on both sides, so a field renumbered in the `.proto` still
//! passes: encoder and decoder changed together. Nothing catches it until a
//! Python or TypeScript peer built against the old numbering reads garbage.
//!
//! This test compares the generated encoding against hex checked into
//! `vectors/golden_wire.json`. The fixture was produced *from* the generated
//! code, which is the point: it freezes today's layout so tomorrow's proto edit
//! has to be deliberate.

use prost::Message;
use quip_proto::v1::{
    Capabilities, EdgeList, Fatal, Hello, IsingProblem, Job, JobKind, Provenance, Reject,
    RejectReason, Result as JobResult, SamplerMeta, Solution, Status,
};
use quip_solver_conformance::golden_wire;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

fn from_hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks(2)
        .filter_map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}

/// A single-entry map. Prost writes map entries in `HashMap` iteration order,
/// which is not stable across runs, so a fixture may pin at most one entry per
/// map field or the encoding is not canonical.
fn one_entry(k: &str, v: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let _ = m.insert(k.to_owned(), v.to_owned());
    m
}

/// Encode `msg`, record its hex under `name`, and check the fixture's hex
/// decodes back to exactly `msg`.
fn pin<M>(name: &str, msg: &M, into: &mut BTreeMap<String, String>)
where
    M: Message + Default + PartialEq + std::fmt::Debug,
{
    let _ = into.insert(name.to_owned(), to_hex(&msg.encode_to_vec()));

    let fixture = golden_wire();
    let Some(hex) = fixture.messages.get(name) else {
        // The encode comparison below reports the missing entry; decoding
        // nothing here would only add a second, less useful failure.
        return;
    };
    let decoded = M::decode(&from_hex(hex)[..]);
    assert!(
        decoded.is_ok(),
        "the {name} fixture must decode as {}: {:?}",
        std::any::type_name::<M>(),
        decoded.err()
    );
    assert_eq!(
        decoded.ok().as_ref(),
        Some(msg),
        "decoding the {name} fixture must reproduce the message it was made from"
    );
}

fn hello() -> Hello {
    Hello {
        miner_id: "cpu-0".to_owned(),
        session_token: "tok".to_owned(),
        protocol_version: 1,
        backend: "cpu".to_owned(),
        algorithm: "sa".to_owned(),
        supported_kinds: vec![JobKind::IsingSample as i32],
        max_nodes: 4600,
        max_edges: 40_000,
        native_topology_hash: Some(vec![0xAB, 0xCD]),
        features: vec!["streaming".to_owned(), "governor".to_owned()],
    }
}

fn job() -> Job {
    Job {
        job_id: vec![0x01, 0x02, 0x03, 0x04],
        kind: JobKind::IsingSample as i32,
        generation: 7,
        deadline_ms: 1_700_000_000_000,
        ising: Some(IsingProblem {
            graph: Some(quip_proto::v1::ising_problem::Graph::Edges(EdgeList {
                u: vec![0, 1],
                v: vec![1, 2],
            })),
            h_milli_le32: vec![0xE8, 0x03, 0x00, 0x00],
            j_milli_le32: vec![0xF4, 0x01, 0x00, 0x00],
            num_reads: 128,
            num_sweeps: 512,
            anneal_time_us: 20,
        }),
        provenance: Some(Provenance {
            is_pow: true,
            order_id: vec![0xDE, 0xAD],
        }),
    }
}

fn result() -> JobResult {
    JobResult {
        job_id: vec![0x01, 0x02, 0x03, 0x04],
        solutions: vec![Solution {
            // 0x01 = +1, 0xFF = -1
            spins_bytes: vec![0x01, 0xFF, 0x01],
            energy_milli: -1_250,
        }],
        meta: Some(SamplerMeta {
            reads: 128,
            sweeps: 512,
            device_access_time_us: 4_096,
            qpu_access_us: 64,
            extra: one_entry("chain_breaks", "3"),
        }),
    }
}

fn reject() -> Reject {
    Reject {
        job_id: vec![0x01, 0x02, 0x03, 0x04],
        reason: RejectReason::TopologyMismatch as i32,
    }
}

fn status() -> Status {
    Status {
        miner_id: "cpu-0".to_owned(),
        utilization: 0.75,
        jobs_done: 42,
        abandoned_generation: 7,
        sampler_stats: one_entry("queue", "2"),
    }
}

fn capabilities() -> Capabilities {
    Capabilities {
        backend: "cuda".to_owned(),
        algorithm: "sa".to_owned(),
        supported_kinds: vec![JobKind::IsingSample as i32],
        max_nodes: 4096,
        max_edges: 32_768,
        features: vec!["streaming".to_owned(), "governor".to_owned()],
        protocol_version: 1,
        stream_width: 4,
        native_topology_hash: Some(vec![0xAB, 0xCD]),
    }
}

fn fatal() -> Fatal {
    Fatal {
        exit_code: 64,
        reason: "unsupported protocol version".to_owned(),
        restart_required: true,
    }
}

#[test]
fn canonical_encodings_match_the_fixture() {
    let mut generated = BTreeMap::new();
    pin("capabilities", &capabilities(), &mut generated);
    pin("fatal", &fatal(), &mut generated);
    pin("hello", &hello(), &mut generated);
    pin("job", &job(), &mut generated);
    pin("reject", &reject(), &mut generated);
    pin("result", &result(), &mut generated);
    pin("status", &status(), &mut generated);

    assert_eq!(
        generated,
        golden_wire().messages,
        "the wire format moved. If the .proto really changed on purpose, \
         update vectors/golden_wire.json from the left-hand side above AND \
         update the Python and TypeScript fixtures with it — this file is the \
         only thing standing between a renumbered field and a silent \
         cross-language break."
    );
}

#[test]
fn enum_numbers_match_the_fixture() {
    let mut generated: BTreeMap<String, BTreeMap<String, i32>> = BTreeMap::new();

    let job_kinds = [
        JobKind::Unspecified,
        JobKind::IsingSample,
        JobKind::GateCircuit,
    ];
    let _ = generated.insert(
        "JobKind".to_owned(),
        job_kinds
            .iter()
            .map(|k| (k.as_str_name().to_owned(), *k as i32))
            .collect(),
    );

    let reasons = [
        RejectReason::Unspecified,
        RejectReason::UnsupportedKind,
        RejectReason::TooLarge,
        RejectReason::Expired,
        RejectReason::Overloaded,
        RejectReason::ShuttingDown,
        RejectReason::Malformed,
        RejectReason::TopologyMismatch,
        RejectReason::TopologyMissing,
    ];
    let _ = generated.insert(
        "RejectReason".to_owned(),
        reasons
            .iter()
            .map(|r| (r.as_str_name().to_owned(), *r as i32))
            .collect(),
    );

    assert_eq!(
        generated,
        golden_wire().enums,
        "an enum variant changed number. Reordering an enum is source-compatible \
         and wire-incompatible: old peers keep sending the old number and it now \
         means something else."
    );
}

#[test]
fn every_variant_the_fixture_pins_still_parses_from_its_name() {
    // The fixture keys are the proto names, so a rename shows up here rather
    // than as a mystery mismatch in the numbers.
    let wire = golden_wire();
    for name in wire
        .enums
        .get("JobKind")
        .into_iter()
        .flat_map(BTreeMap::keys)
    {
        assert!(
            JobKind::from_str_name(name).is_some(),
            "JobKind::{name} no longer exists under that name"
        );
    }
    for name in wire
        .enums
        .get("RejectReason")
        .into_iter()
        .flat_map(BTreeMap::keys)
    {
        assert!(
            RejectReason::from_str_name(name).is_some(),
            "RejectReason::{name} no longer exists under that name"
        );
    }
}
