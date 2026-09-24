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
    Algorithm, Backend, Capabilities, CoefficientEncoding, EdgeList, Fatal, GeneratorAlgorithm,
    Hello, IsingProblem, IsingProblemGenerator, Job, JobKind, LeaseDone, Provenance, Reject,
    RejectReason, Result as JobResult, SamplerMeta, SetTarget, Solution, Status, Topology,
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
    if *hex != to_hex(&msg.encode_to_vec()) {
        // Report all changed encodings together in the map assertion below.
        return;
    }
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
        capabilities: Some(Capabilities {
            protocol_version: 2,
            backend: Backend::Cpu as i32,
            algorithm: Algorithm::Sa as i32,
            supported_kinds: vec![JobKind::IsingSample as i32],
            max_nodes: 4600,
            max_edges: 40_000,
            native_topology_hash: Some(vec![0xAB, 0xCD]),
            features: vec!["streaming".to_owned(), "governor".to_owned()],
            stream_width: 4,
            encodings: vec![CoefficientEncoding::I32 as i32],
            generators: vec![GeneratorAlgorithm::Blake3Chacha8V1 as i32],
        }),
    }
}

fn job() -> Job {
    Job {
        generator: None,
        job_id: vec![0x01, 0x02, 0x03, 0x04],
        kind: JobKind::IsingSample as i32,
        generation: 7,
        deadline_ms: 1_700_000_000_000,
        ising: Some(IsingProblem {
            graph: Some(quip_proto::v1::ising_problem::Graph::Edges(EdgeList {
                u: vec![0, 1],
                v: vec![1, 2],
            })),
            encoding: CoefficientEncoding::I32 as i32,
            scale: 1000,
            h: vec![0xE8, 0x03, 0x00, 0x00],
            j: vec![0xF4, 0x01, 0x00, 0x00],
            num_reads: 128,
            num_sweeps: 512,
            anneal_time_us: 20,
            ..Default::default()
        }),
        provenance: Some(Provenance {
            is_pow: true,
            order_id: vec![0xDE, 0xAD],
        }),
    }
}

fn result() -> JobResult {
    JobResult {
        salt: vec![0x31; 32],
        nonce: vec![0x42; 32],
        job_id: vec![0x01, 0x02, 0x03, 0x04],
        solutions: vec![Solution {
            // +1, -1, +1, packed least-significant bit first.
            spins: vec![0x05],
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

fn job_generate() -> Job {
    Job {
        job_id: vec![5, 6],
        kind: JobKind::IsingGenerate as i32,
        generation: 8,
        deadline_ms: 1_700_000_000_001,
        generator: Some(IsingProblemGenerator {
            algorithm: GeneratorAlgorithm::Blake3Chacha8V1 as i32,
            topology_hash: vec![0xab, 0xcd],
            last_proof_block_hash: vec![0x11; 32],
            miner_account: vec![0x22; 32],
            base_salt: vec![0x33; 32],
            salt_start: 256,
            salt_count: 16,
        }),
        ..Default::default()
    }
}

fn lease_done() -> LeaseDone {
    LeaseDone {
        job_id: vec![5, 6],
        salts_done: 16,
        best_energy_milli: -1250,
    }
}

fn topology() -> Topology {
    Topology {
        hash: vec![0xab, 0xcd],
        nodes: vec![40, 7, 90],
        edges: Some(EdgeList {
            u: vec![40, 7],
            v: vec![7, 90],
        }),
        allowed_h_milli: vec![-1000, 1000],
        allowed_j_milli: vec![-500, 500],
    }
}

fn set_target() -> SetTarget {
    SetTarget {
        max_energy_milli: -1000,
        min_solutions: 2,
        min_diversity_milli: 200,
        num_reads: 128,
        num_sweeps: 512,
        anneal_time_us: 20,
        max_proof_solutions: 32,
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
        encodings: vec![CoefficientEncoding::I32 as i32],
        generators: vec![GeneratorAlgorithm::Blake3Chacha8V1 as i32],
        backend: Backend::Cuda as i32,
        algorithm: Algorithm::Sa as i32,
        supported_kinds: vec![JobKind::IsingSample as i32],
        max_nodes: 4096,
        max_edges: 32_768,
        features: vec!["streaming".to_owned(), "governor".to_owned()],
        protocol_version: 2,
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
    pin("job_generate", &job_generate(), &mut generated);
    pin("lease_done", &lease_done(), &mut generated);
    pin("topology", &topology(), &mut generated);
    pin("set_target", &set_target(), &mut generated);
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
    macro_rules! pin_enum {
        ($ty:ident, [$($variant:ident),+ $(,)?]) => {
            let values = [$($ty::$variant),+];
            let _ = generated.insert(stringify!($ty).to_owned(), values.iter().map(|v| (v.as_str_name().to_owned(), *v as i32)).collect());
        };
    }
    pin_enum!(
        JobKind,
        [Unspecified, IsingSample, GateCircuit, IsingGenerate]
    );
    pin_enum!(
        RejectReason,
        [
            Unspecified,
            UnsupportedKind,
            TooLarge,
            Expired,
            Overloaded,
            ShuttingDown,
            Malformed,
            TopologyMismatch,
            TopologyMissing,
            TargetMissing
        ]
    );
    pin_enum!(
        Backend,
        [Unspecified, Cpu, Cuda, Metal, Ane, DwaveQpu, Exec, Mock]
    );
    pin_enum!(
        Algorithm,
        [
            Unspecified,
            Sa,
            Gibbs,
            QuantumAnneal,
            Fsa,
            Msa,
            Flatiron,
            Mps,
            Mfa,
            Sb,
            Bsb,
            Gbsb,
            Gdsb,
            Ggdsb,
            Hbsb,
            Hdsb,
            Sbqa,
            Tedsb,
            External
        ]
    );
    pin_enum!(
        CoefficientEncoding,
        [Unspecified, I32, I16, I8, F16, F32, F64]
    );
    pin_enum!(GeneratorAlgorithm, [Unspecified, Blake3Chacha8V1]);
    assert_eq!(generated, golden_wire().enums, "enum numbers changed");
}

#[test]
fn every_variant_the_fixture_pins_still_parses_from_its_name() {
    let wire = golden_wire();
    for (kind, names) in &wire.enums {
        for name in names.keys() {
            let parsed = match kind.as_str() {
                "JobKind" => JobKind::from_str_name(name).is_some(),
                "RejectReason" => RejectReason::from_str_name(name).is_some(),
                "Backend" => Backend::from_str_name(name).is_some(),
                "Algorithm" => Algorithm::from_str_name(name).is_some(),
                "CoefficientEncoding" => CoefficientEncoding::from_str_name(name).is_some(),
                "GeneratorAlgorithm" => GeneratorAlgorithm::from_str_name(name).is_some(),
                other => panic!("unexpected enum {other}"),
            };
            assert!(parsed, "{kind}::{name} no longer exists");
        }
    }
}
