//! Family lease tests against a real session and deterministic samplers.
//!
//! The legacy pins below describe the lease path as it was at 6d272cd. They
//! must keep passing, unedited, while family leases are added.
use prost::Message as _;
use quip_proto::v1::{
    self as wire, coord_msg, miner_msg,
    miner_service_server::{MinerService, MinerServiceServer},
};
use quip_protocol::{
    lease::{verify_lease_result, TopologyView},
    target::Target,
};
use std::{
    fmt::Write as _,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::sync::{mpsc, Mutex};
use tokio_stream::wrappers::{ReceiverStream, UnixListenerStream};
use tonic::{Request, Response, Status, Streaming};

type Link = (
    Streaming<wire::MinerMsg>,
    mpsc::Sender<Result<wire::CoordMsg, Status>>,
);

struct Coordinator(Mutex<Option<tokio::sync::oneshot::Sender<Link>>>);
#[tonic::async_trait]
#[expect(
    clippy::unwrap_used,
    reason = "test coordinator accepts exactly one connection"
)]
impl MinerService for Coordinator {
    type SessionStream = ReceiverStream<Result<wire::CoordMsg, Status>>;
    async fn session(
        &self,
        req: Request<Streaming<wire::MinerMsg>>,
    ) -> Result<Response<Self::SessionStream>, Status> {
        let (tx, rx) = mpsc::channel(32);
        self.0
            .lock()
            .await
            .take()
            .unwrap()
            .send((req.into_inner(), tx))
            .map_err(|_| Status::internal("test ended"))?;
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
struct Session {
    inbound: Streaming<wire::MinerMsg>,
    tx: mpsc::Sender<Result<wire::CoordMsg, Status>>,
    child: tokio::process::Child,
    server: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    dir: std::path::PathBuf,
    gate: std::sync::Arc<tokio::net::UnixListener>,
    logs: std::sync::Arc<(std::sync::Mutex<Vec<String>>, tokio::sync::Notify)>,
}

#[expect(
    clippy::unwrap_used,
    reason = "the stderr reader stops if the test log buffer is poisoned"
)]
fn capture_stderr(
    child: &mut tokio::process::Child,
) -> std::sync::Arc<(std::sync::Mutex<Vec<String>>, tokio::sync::Notify)> {
    let logs = std::sync::Arc::new((
        std::sync::Mutex::new(Vec::new()),
        tokio::sync::Notify::new(),
    ));
    if let Some(pipe) = child.stderr.take() {
        let logs = std::sync::Arc::clone(&logs);
        drop(tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt as _;
            let mut lines = tokio::io::BufReader::new(pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                logs.0.lock().unwrap().push(line);
                logs.1.notify_waiters();
            }
        }));
    }
    logs
}

#[expect(
    clippy::print_stderr,
    reason = "show captured child logs when a lease test fails"
)]
impl Drop for Session {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Ok(lines) = self.logs.0.lock() {
                eprintln!("miner stderr:\n{}", lines.join("\n"));
            }
        }
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "fixture failures must fail the session test immediately"
)]
impl Session {
    async fn start(binary: &str) -> Self {
        Self::start_with_gate(binary, false).await
    }
    async fn start_with_gate(binary: &str, gated: bool) -> Self {
        Self::start_with(binary, gated, &[]).await
    }
    async fn start_with(binary: &str, gated: bool, extra_args: &[&str]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let bin = build_example(binary);
        let dir = std::path::PathBuf::from(format!(
            "target/t7-tmp/{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let gate_path = dir.join("g");
        let gate = std::sync::Arc::new(tokio::net::UnixListener::bind(&gate_path).unwrap());
        let socket = dir.join("s");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (link_tx, link_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(MinerServiceServer::new(Coordinator(Mutex::new(Some(
                    link_tx,
                )))))
                .serve_with_incoming(UnixListenerStream::new(listener)),
        );
        let mut command = tokio::process::Command::new(bin);
        let _ = command
            .stderr(std::process::Stdio::piped())
            .args([
                "--quip-coordinator",
                &format!("unix://{}", socket.display()),
                "--log-level",
                "error",
            ])
            .args(extra_args)
            .env("QUIP_SESSION_TOKEN", "test")
            .kill_on_drop(true);
        if gated {
            let _ = command.arg("--gate").arg(gate_path);
        }
        let mut child = command.spawn().unwrap();
        let logs = capture_stderr(&mut child);
        let (inbound, tx) = tokio::time::timeout(Duration::from_secs(30), link_rx)
            .await
            .unwrap()
            .unwrap();
        let mut s = Self {
            inbound,
            tx,
            child,
            server,
            dir,
            gate,
            logs,
        };
        let miner_msg::Msg::Hello(hello) = s.recv().await else {
            panic!("expected Hello")
        };
        let caps = hello.capabilities.unwrap();
        assert_eq!(caps.protocol_version, 2);
        assert!(caps
            .supported_kinds
            .contains(&(wire::JobKind::IsingGenerate as i32)));
        s.send(coord_msg::Msg::Welcome(wire::Welcome {
            protocol_version: 2,
        }))
        .await;
        s.send(coord_msg::Msg::Configure(wire::Configure {
            queue_depth: 8,
            ..Default::default()
        }))
        .await;
        assert!(matches!(s.recv().await, miner_msg::Msg::Ready(_)));
        assert!(matches!(s.recv().await, miner_msg::Msg::JobRequest(_)));
        s
    }
    async fn wait_for_log(&self, needle: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let notified = self.logs.1.notified();
                if self
                    .logs
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|l| l.contains(needle))
                {
                    return;
                }
                notified.await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("miner never logged {needle:?}"));
    }
    async fn blocked_sample(&self) -> tokio::net::UnixStream {
        tokio::time::timeout(Duration::from_secs(10), self.gate.accept())
            .await
            .unwrap()
            .unwrap()
            .0
    }
    fn release_remaining(&self) -> tokio::task::JoinHandle<()> {
        let gate = std::sync::Arc::clone(&self.gate);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = gate.accept().await.unwrap();
                release(&mut stream).await;
            }
        })
    }
    async fn ack(&mut self) {
        self.send(coord_msg::Msg::Ping(wire::Ping {})).await;
        assert!(matches!(self.recv().await, miner_msg::Msg::Status(_)));
    }
    async fn send(&self, msg: coord_msg::Msg) {
        self.tx
            .send(Ok(wire::CoordMsg { msg: Some(msg) }))
            .await
            .unwrap();
    }
    async fn recv(&mut self) -> miner_msg::Msg {
        tokio::time::timeout(Duration::from_secs(10), self.inbound.message())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .msg
            .unwrap()
    }
    async fn setup(&self, ceiling: i64) {
        self.send(coord_msg::Msg::Topology(topology())).await;
        self.send(coord_msg::Msg::SetTarget(target(ceiling))).await;
    }
    async fn refund(&mut self) {
        assert!(matches!(self.recv().await, miner_msg::Msg::JobRequest(r) if r.credits == 1));
    }
    async fn finish(mut self) {
        self.send(coord_msg::Msg::Shutdown(wire::Shutdown { grace_ms: 2000 }))
            .await;
        assert!(
            tokio::time::timeout(Duration::from_secs(10), self.inbound.message())
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        self.tx = mpsc::channel(1).0;
        assert!(
            tokio::time::timeout(Duration::from_secs(10), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}
#[expect(clippy::unwrap_used, reason = "test fixture builds must succeed")]
fn build_example(name: &str) -> std::path::PathBuf {
    assert!(std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "quip-solver-core", "--example", name])
        .status()
        .unwrap()
        .success());
    let mut path = std::env::current_exe().unwrap();
    let _ = path.pop();
    let _ = path.pop();
    path.join("examples").join(name)
}

/// `T_SPARSE` (core.json `topologies[sparse]`), sent with the wire hash `[7; 32]`.
fn topology() -> wire::Topology {
    let edges = [
        (907, 41),
        (41, 0),
        (0, 4_294_967_295),
        (4_294_967_295, 12),
        (12, 3),
        (3, 907),
        (41, 12),
    ];
    wire::Topology {
        hash: vec![7; 32],
        nodes: vec![907, 41, 0, 4_294_967_295, 12, 3],
        edges: Some(wire::EdgeList {
            u: edges.iter().map(|e| e.0).collect(),
            v: edges.iter().map(|e| e.1).collect(),
        }),
        allowed_h_milli: vec![-1000, 0, 1000],
        allowed_j_milli: vec![-1000, 1000],
    }
}
fn target(ceiling: i64) -> wire::SetTarget {
    wire::SetTarget {
        max_energy_milli: ceiling,
        min_solutions: 1,
        min_diversity_milli: 0,
        max_proof_solutions: 32,
        num_reads: 1,
        num_sweeps: 1,
        ..Default::default()
    }
}
/// A legacy lease with the core.json `leases[family-small]` inputs.
fn job(count: u64) -> wire::Job {
    wire::Job {
        job_id: b"lease".to_vec(),
        kind: wire::JobKind::IsingGenerate as i32,
        generation: 1,
        generator: Some(wire::IsingProblemGenerator {
            algorithm: wire::GeneratorAlgorithm::Blake3Chacha8V1 as i32,
            topology_hash: vec![7; 32],
            last_proof_block_hash: vec![0x11; 32],
            miner_account: vec![0x22; 32],
            base_salt: [vec![0; 8], vec![0x33; 24]].concat(),
            salt_start: 5,
            salt_count: count,
            ..Default::default()
        }),
        ..Default::default()
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}
#[expect(
    clippy::unwrap_used,
    reason = "verification inputs are fixed valid fixtures"
)]
fn verify(result: &wire::Result) {
    assert_eq!(result.job_id, b"lease");
    assert!(verify_lease_result(
        &job(3).generator.unwrap(),
        &TopologyView::from_proto(&topology()).unwrap(),
        &Target::from_proto(&target(i64::MAX)),
        result
    )
    .is_ok());
}

#[tokio::test]
async fn legacy_lease_results_are_pinned() {
    let mut s = Session::start("mock_sampler_lease").await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(3))).await;
    let mut results = Vec::new();
    for _ in 0..3 {
        let miner_msg::Msg::Result(r) = s.recv().await else {
            panic!("expected winner")
        };
        verify(&r);
        results.push(r);
    }
    results.sort_by(|a, b| a.salt.cmp(&b.salt));
    let pinned = results
        .iter()
        .map(|r| {
            let solutions = wire::Result {
                solutions: r.solutions.clone(),
                ..Default::default()
            };
            (hex(&r.salt), hex(&r.nonce), hex(&solutions.encode_to_vec()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        pinned,
        PINNED_RESULTS.map(|(a, b, c)| (a.into(), b.into(), c.into()))
    );
    assert!(
        matches!(s.recv().await, miner_msg::Msg::LeaseDone(d) if d.salts_done == 3 && d.best_energy_milli == PINNED_BEST_ENERGY_MILLI)
    );
    s.refund().await;
    s.finish().await;
}

#[tokio::test]
async fn legacy_reject_reasons_are_pinned() {
    let mut s = Session::start("mock_sampler_lease").await;
    s.setup(i64::MAX).await;
    for variant in 0..4 {
        let mut j = job(3);
        let g = j.generator.as_mut().unwrap();
        match variant {
            0 => g.algorithm = 0,
            1 => g.algorithm = 7,
            2 => g.algorithm = 2,
            _ => g.family = Some(wire::FamilyDescriptor::default()),
        }
        s.send(coord_msg::Msg::Job(j)).await;
        assert!(
            matches!(s.recv().await, miner_msg::Msg::Reject(r) if r.reason == wire::RejectReason::Malformed as i32),
            "variant {variant}"
        );
        s.refund().await;
    }
    s.finish().await;
}

const PINNED_BEST_ENERGY_MILLI: i64 = -5000;
const PINNED_RESULTS: [(&str, &str, &str); 3] = [
    (
        "0500000000000000333333333333333333333333333333333333333333333333",
        "f0e3ae9a44bc0fad366f72552eade4d81ff343e79aba6744480a8aca061ebdd5",
        "120e10e0e0ffffffffffffff011a013f",
    ),
    (
        "0600000000000000333333333333333333333333333333333333333333333333",
        "c859a7aecf125a88c46377b8e818b4953656699ee8851962e44a065baae799c0",
        "120e10f8d8ffffffffffffff011a013f",
    ),
    (
        "0700000000000000333333333333333333333333333333333333333333333333",
        "8cf62329d6f6f2ecc9315d7a4d94c9a06fdc794e0ed86d8cef6aff9172ca13b0",
        "120610e8071a013f",
    ),
];

// Family leases (SC-5). The expected instances come from
// `generate_instance` on the inputs the lease names.

#[path = "../examples/support/layered_family.rs"]
mod layered;

use quip_protocol::lease::LeaseSpec;
use quip_solver_core::ising_generators::{
    descriptor::{admit, params_digest, topology_order_digest, AdvertisedLimits, Contract},
    family::{admit_topology, generate_instance, BaseTopology, SaltFault},
    instance::{Instance, InstanceLimits},
};
use serde_json::Value;

const TIGHT_LIMITS: InstanceLimits = match InstanceLimits::new(
    16_777_216,
    67_108_864,
    2000,
    72_057_594_037_927_936,
    67_108_864,
) {
    Some(limits) => limits,
    None => InstanceLimits::FAM_37,
};
static UNIFORM_V1: Contract = Contract {
    family_id: "vectors/uniform-field",
    revision: 1,
    schema: layered::UNIFORM_FIELD,
    metadata_kinds: &[1],
    limits: TIGHT_LIMITS,
    size_bounds: layered::example_size_bounds,
    construction: Some(&layered::LAYERED),
};
static RESTRICTED_V1: Contract = Contract {
    family_id: "vectors/restricted-field",
    ..UNIFORM_V1
};
const P_SPEC: [u8; 19] = [
    0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0x24, 0xfa, 0xff, 0xff,
];
const P_L2_F0: [u8; 19] = [0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0];
const P_L3_F0: [u8; 19] = [0x01, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0];
const P_MALFORMED: [u8; 1] = [0x02];
/// The `mock_sampler_family_lease` advertised limits (FAM-47).
const ADVERTISED: AdvertisedLimits = AdvertisedLimits {
    max_nodes: 0,
    max_edges: 0,
    max_degree: 4,
};

/// Salt indices in `0..64` that `generate_instance` skips for a `family-small`
/// lease of 64 salts with `P_SPEC` on `T_SPARSE`. Captured once from
/// `generate_instance` as first implemented (Task 5); never edit.
const SKIPPED_64: [u64; 25] = [
    0, 3, 9, 13, 18, 19, 26, 28, 29, 34, 35, 37, 39, 40, 41, 43, 44, 45, 49, 51, 53, 56, 57, 59, 62,
];
/// The first index in `SKIPPED_64`.
const SKIPPED_ONE: u64 = 0;

#[expect(clippy::unwrap_used, reason = "core.json is a fixed valid fixture")]
fn core() -> Value {
    serde_json::from_str(quip_solver_conformance::FAMILY_CORE).unwrap()
}
#[expect(clippy::unwrap_used, reason = "fixture hex is valid")]
fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2).unwrap(), 16).unwrap())
        .collect()
}
#[expect(
    clippy::unwrap_used,
    reason = "a missing fixture case is a test failure"
)]
fn case<'a>(core: &'a Value, table: &str, name: &str) -> &'a Value {
    core[table]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()
}
/// Every case of a core.json table.
#[expect(clippy::unwrap_used, reason = "core.json is a fixed valid fixture")]
fn cases<'a>(core: &'a Value, table: &str) -> &'a [Value] {
    core.get(table).and_then(Value::as_array).unwrap()
}
/// The raw wire value of a reject reason named in core.json.
#[expect(clippy::unwrap_used, reason = "every core.json reason is in the table")]
fn wire_reason(core: &Value, name: &str) -> i32 {
    let row = core["wire_reasons"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["reason"] == name)
        .unwrap();
    i32::try_from(row["value"].as_i64().unwrap()).unwrap()
}
/// A core.json topology, sent with the wire hash and allowed values of `T_SPARSE`.
#[expect(clippy::unwrap_used, reason = "core.json is a fixed valid fixture")]
fn vector_topology(name: &str) -> wire::Topology {
    let core = core();
    let t = case(&core, "topologies", name);
    let id = |v: &Value| u32::try_from(v.as_u64().unwrap()).unwrap();
    let edges = t["edges"].as_array().unwrap();
    wire::Topology {
        nodes: t["nodes"].as_array().unwrap().iter().map(id).collect(),
        edges: Some(wire::EdgeList {
            u: edges.iter().map(|e| id(&e[0])).collect(),
            v: edges.iter().map(|e| id(&e[1])).collect(),
        }),
        ..topology()
    }
}
/// `T_SPARSE` plus the edge `[41, 907]` (code 12).
#[expect(clippy::unwrap_used, reason = "T_SPARSE has an edge list")]
fn doubled() -> wire::Topology {
    let mut t = topology();
    let edges = t.edges.as_mut().unwrap();
    edges.u.push(41);
    edges.v.push(907);
    t
}
#[expect(clippy::unwrap_used, reason = "fixture topologies are valid")]
fn base(t: &wire::Topology) -> BaseTopology {
    let edges = t.edges.as_ref().unwrap();
    let pairs = edges
        .u
        .iter()
        .copied()
        .zip(edges.v.iter().copied())
        .collect::<Vec<_>>();
    BaseTopology::new(t.nodes.clone(), &pairs).unwrap()
}
#[expect(clippy::unwrap_used, reason = "fixture topologies are small")]
fn order_digest(t: &wire::Topology) -> Vec<u8> {
    let b = base(t);
    let edges = t.edges.as_ref().unwrap();
    let pairs = edges
        .u
        .iter()
        .copied()
        .zip(edges.v.iter().copied())
        .collect::<Vec<_>>();
    let digest = topology_order_digest(b.node_ids(), &pairs).unwrap();
    assert_eq!(&digest, b.order_digest());
    digest.to_vec()
}
/// A family lease with the `family-small` inputs on `T_SPARSE`.
#[expect(clippy::unwrap_used, reason = "the legacy job has a generator")]
fn family_job(count: u64, contract: &Contract, params: &[u8]) -> wire::Job {
    let mut j = job(count);
    let g = j.generator.as_mut().unwrap();
    g.algorithm = wire::GeneratorAlgorithm::FamilyChacha8V1 as i32;
    g.family = Some(wire::FamilyDescriptor {
        family_id: contract.family_id.into(),
        revision: contract.revision,
        params: params.to_vec(),
        params_digest: params_digest(params).to_vec(),
    });
    g.topology_order_digest = order_digest(&topology());
    j
}
/// The canonical instance the coordinator regenerates for `nonce`.
#[expect(
    clippy::unwrap_used,
    reason = "the inputs passed admission in the session"
)]
fn regenerate(
    contract: &'static Contract,
    params: &[u8],
    t: &wire::Topology,
    nonce: [u8; 32],
) -> Result<Instance, SaltFault> {
    let b = base(t);
    let admitted = admit(
        std::slice::from_ref(contract),
        contract.family_id.as_bytes(),
        contract.revision,
        params,
        &params_digest(params),
    )
    .unwrap();
    let bounds = admit_topology(&admitted, &b, b.order_digest(), &ADVERTISED).unwrap();
    generate_instance(&admitted, &b, &bounds, nonce)
}
/// The FAM-33 energy of the all-`+1` spins, `offset` included.
fn all_up_energy(instance: &Instance) -> i64 {
    let edges = instance
        .edges()
        .iter()
        .map(|&(u, v)| (u as usize, v as usize))
        .collect::<Vec<_>>();
    let spins = vec![1i8; instance.node_ids().len()];
    instance.offset_milli()
        + quip_protocol::scoring::energy_from_milli(&spins, instance.h(), instance.j(), &edges)
}
/// Check one family `Result` against its regenerated instance. Returns its energy.
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "a Result that does not match its instance is a test failure"
)]
fn check_result(
    r: &wire::Result,
    contract: &'static Contract,
    params: &[u8],
    t: &wire::Topology,
) -> i64 {
    let nonce: [u8; 32] = r.nonce.as_slice().try_into().unwrap();
    let Ok(instance) = regenerate(contract, params, t, nonce) else {
        panic!("a skipped salt was reported: nonce {}", hex(&r.nonce))
    };
    let energy = all_up_energy(&instance);
    let n = instance.node_ids().len();
    let packed = quip_protocol::wire::encode_spins_packed(&vec![1i8; n]);
    assert_eq!(r.solutions.len(), 1);
    for solution in &r.solutions {
        assert_eq!(solution.spins.len(), n.div_ceil(8));
        assert_eq!(solution.spins, packed);
        assert_eq!(solution.energy_milli, energy, "nonce {}", hex(&r.nonce));
    }
    energy
}
fn spec(j: &wire::Job) -> Option<LeaseSpec> {
    LeaseSpec::from_proto(j.generator.as_ref()?).ok()
}
/// The salt index of a reported salt.
#[expect(clippy::unwrap_used, reason = "every reported salt is in its lease")]
fn index_of(j: &wire::Job, r: &wire::Result) -> u64 {
    spec(j)
        .unwrap()
        .index_of(&r.salt.as_slice().try_into().unwrap())
        .unwrap()
}
/// Read every reply up to the lease summary. Returns the Results and the summary.
#[expect(clippy::panic, reason = "an unexpected message is a test failure")]
async fn drain_lease(s: &mut Session) -> (Vec<wire::Result>, wire::LeaseDone) {
    let mut results = Vec::new();
    loop {
        match s.recv().await {
            miner_msg::Msg::Result(r) => results.push(r),
            miner_msg::Msg::LeaseDone(d) => return (results, d),
            other => panic!("unexpected {other:?}"),
        }
    }
}
#[expect(clippy::panic, reason = "a missing reject is a test failure")]
async fn expect_reject(s: &mut Session, reason: i32, what: &str) {
    match s.recv().await {
        miner_msg::Msg::Reject(r) => assert_eq!(r.reason, reason, "{what}"),
        other => panic!("{what}: expected a reject, got {other:?}"),
    }
    s.refund().await;
}

#[tokio::test]
async fn lease_admission_vectors_hold_in_the_session() {
    let core = core();
    let mut s = Session::start("mock_sampler_family_lease").await;
    for case in cases(&core, "lease_admission") {
        let name = case["name"].as_str().unwrap();
        let faults = case["faults"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap())
            .collect::<Vec<_>>();
        let cached = if faults.contains(&"code:12") {
            doubled()
        } else {
            topology()
        };
        s.send(coord_msg::Msg::Topology(cached.clone())).await;
        s.send(coord_msg::Msg::SetTarget(target(i64::MAX))).await;
        let mut j = family_job(4, &UNIFORM_V1, &P_SPEC);
        let g = j.generator.as_mut().unwrap();
        g.topology_order_digest = order_digest(&cached);
        let set_params = |g: &mut wire::IsingProblemGenerator, contract: &Contract, p: &[u8]| {
            let family = g.family.as_mut().unwrap();
            contract.family_id.clone_into(&mut family.family_id);
            family.params = p.to_vec();
            family.params_digest = params_digest(p).to_vec();
        };
        for fault in &faults {
            match *fault {
                "session:expired" => j.deadline_ms = 1,
                "field:salt-count-zero" => g.salt_count = 0,
                "field:unknown-generator" => g.algorithm = 7,
                "code:1" => g.family = None,
                "code:2" => g.algorithm = wire::GeneratorAlgorithm::Blake3Chacha8V1 as i32,
                "code:3" => "Quip/X".clone_into(&mut g.family.as_mut().unwrap().family_id),
                "code:5" => "vectors/none".clone_into(&mut g.family.as_mut().unwrap().family_id),
                "code:7" => {
                    let family = g.family.as_mut().unwrap();
                    family.params = P_MALFORMED.to_vec();
                    family.params_digest = params_digest(&P_MALFORMED).to_vec();
                }
                "code:10" => set_params(g, &RESTRICTED_V1, &P_L2_F0),
                "session:topology-hash-mismatch" => g.topology_hash = vec![9; 32],
                "code:11" => {
                    g.topology_order_digest =
                        order_digest(&vector_topology("sparse-edges-reordered"));
                }
                "code:12" => {}
                "code:13" => {
                    let id = g.family.as_ref().unwrap().family_id.clone();
                    let contract = if id == RESTRICTED_V1.family_id {
                        &RESTRICTED_V1
                    } else {
                        &UNIFORM_V1
                    };
                    set_params(g, contract, &P_L3_F0);
                }
                other => panic!("{name}: no mutation for {other}"),
            }
        }
        s.send(coord_msg::Msg::Job(j)).await;
        let expected = wire_reason(&core, case["expected_wire"].as_str().unwrap());
        expect_reject(&mut s, expected, name).await;
    }
    s.finish().await;
}

#[tokio::test]
async fn family_lease_field_lengths_follow_the_vectors() {
    let core = core();
    let mut s = Session::start("mock_sampler_family_lease").await;
    s.setup(i64::MAX).await;
    let mut admitted = 0;
    for case in cases(&core, "field_lengths") {
        let field = case["field"].as_str().unwrap();
        let name = case["name"].as_str().unwrap();
        let length = usize::try_from(case["length"].as_u64().unwrap()).unwrap();
        let resize = |bytes: &mut Vec<u8>| bytes.resize(length, 0);
        let mut j = family_job(1, &UNIFORM_V1, &P_SPEC);
        let g = j.generator.as_mut().unwrap();
        match field {
            "FamilyDescriptor.params_digest" => {
                resize(&mut g.family.as_mut().unwrap().params_digest);
            }
            "IsingProblemGenerator.topology_order_digest" => resize(&mut g.topology_order_digest),
            _ => continue,
        }
        s.send(coord_msg::Msg::Job(j)).await;
        if case["expected_code"].is_null() {
            let (_, done) = drain_lease(&mut s).await;
            assert_eq!(done.salts_done, 1, "{name}");
            s.refund().await;
            admitted += 1;
        } else {
            expect_reject(&mut s, wire::RejectReason::Malformed as i32, name).await;
        }
    }
    assert_eq!(admitted, 2, "one admitted case per field");
    s.finish().await;
}

#[tokio::test]
async fn family_leases_use_the_vector_salts_and_nonces() {
    let core = core();
    let mut s = Session::start("mock_sampler_family_lease").await;
    s.setup(i64::MAX).await;
    for lease in cases(&core, "leases") {
        let name = lease["name"].as_str().unwrap();
        let number = |key: &str| lease[key].as_str().unwrap().parse::<u64>().unwrap();
        let mut j = family_job(1, &UNIFORM_V1, &P_SPEC);
        let g = j.generator.as_mut().unwrap();
        g.last_proof_block_hash = unhex(lease["last_proof_block_hash"].as_str().unwrap());
        g.miner_account = unhex(lease["miner_account"].as_str().unwrap());
        g.base_salt = unhex(lease["base_salt"].as_str().unwrap());
        g.salt_start = number("salt_start");
        g.salt_count = number("salt_count");
        let count = g.salt_count;
        s.send(coord_msg::Msg::Job(j)).await;
        if !lease["admitted"].as_bool().unwrap() {
            let expected = wire_reason(&core, lease["wire_reason"].as_str().unwrap());
            expect_reject(&mut s, expected, name).await;
            continue;
        }
        let pairs = lease["salts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    unhex(p["salt"].as_str().unwrap()),
                    unhex(p["nonce"].as_str().unwrap()),
                )
            })
            .collect::<Vec<_>>();
        let (results, done) = drain_lease(&mut s).await;
        for r in &results {
            assert!(
                pairs.contains(&(r.salt.clone(), r.nonce.clone())),
                "{name}: salt {} nonce {}",
                hex(&r.salt),
                hex(&r.nonce)
            );
            let _ = check_result(r, &UNIFORM_V1, &P_SPEC, &topology());
        }
        assert_eq!(done.salts_done, count, "{name}");
        s.refund().await;
    }
    s.finish().await;
}

#[tokio::test]
async fn a_family_lease_reports_regenerated_instances_then_one_lease_done() {
    let mut s = Session::start("mock_sampler_family_lease").await;
    s.setup(i64::MAX).await;
    let j = family_job(4, &UNIFORM_V1, &P_SPEC);
    s.send(coord_msg::Msg::Job(j.clone())).await;
    let (results, done) = drain_lease(&mut s).await;
    let spec = spec(&j).unwrap();
    let expected = (0..4)
        .filter_map(|i| regenerate(&UNIFORM_V1, &P_SPEC, &topology(), spec.nonce(i).unwrap()).ok())
        .map(|instance| all_up_energy(&instance))
        .collect::<Vec<_>>();
    assert!(
        !expected.is_empty(),
        "the fixture must report at least one salt"
    );
    assert_eq!(results.len(), expected.len());
    let mut best = i64::MAX;
    for r in &results {
        assert_eq!(r.job_id, b"lease");
        assert_eq!(r.solutions.first().unwrap().spins.len(), 2, "ceil(12 / 8)");
        best = best.min(check_result(r, &UNIFORM_V1, &P_SPEC, &topology()));
    }
    assert_eq!(done.salts_done, 4);
    assert_eq!(done.best_energy_milli, best);
    assert_eq!(Some(best), expected.iter().copied().min());
    s.refund().await;
    s.finish().await;
}

#[tokio::test]
async fn skipped_salts_count_and_send_no_result() {
    let j = family_job(64, &UNIFORM_V1, &P_SPEC);
    let spec = spec(&j).unwrap();
    let skipped = (0..64)
        .filter(|&i| {
            matches!(
                regenerate(&UNIFORM_V1, &P_SPEC, &topology(), spec.nonce(i).unwrap()),
                Err(SaltFault::Skip(_))
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(skipped, SKIPPED_64);
    assert_eq!(skipped.first(), Some(&SKIPPED_ONE));
    let mut s = Session::start("mock_sampler_family_lease").await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(j.clone())).await;
    let (results, done) = drain_lease(&mut s).await;
    let mut reported = results
        .iter()
        .map(|r| {
            let _ = check_result(r, &UNIFORM_V1, &P_SPEC, &topology());
            index_of(&j, r)
        })
        .collect::<Vec<_>>();
    reported.sort_unstable();
    let expected = (0..64)
        .filter(|i| !SKIPPED_64.contains(i))
        .collect::<Vec<_>>();
    assert_eq!(reported, expected);
    assert_eq!(done.salts_done, 64);
    s.refund().await;
    s.finish().await;
}

/// Run a 64-salt family lease on `mode`. Returns the Results by salt, and the summary.
async fn run_64(mode: &str) -> (Vec<wire::Result>, wire::LeaseDone) {
    let j = family_job(64, &UNIFORM_V1, &P_SPEC);
    let mut s = Session::start_with("mock_sampler_family_lease", false, &["--mode", mode]).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(j.clone())).await;
    let (mut results, done) = drain_lease(&mut s).await;
    results.sort_by_key(|r| index_of(&j, r));
    s.refund().await;
    s.finish().await;
    (results, done)
}

#[tokio::test]
async fn a_local_family_generator_matches_the_session_path() {
    let (local, local_done) = run_64("local").await;
    let (default, default_done) = run_64("default").await;
    assert!(!local.is_empty());
    assert_eq!(local.len(), default.len());
    for (l, d) in local.iter().zip(&default) {
        assert_eq!(l.salt, d.salt);
        assert_eq!(l.nonce, d.nonce);
        assert_eq!(l.solutions, d.solutions);
    }
    assert_eq!(local_done.salts_done, 64);
    assert_eq!(local_done.salts_done, default_done.salts_done);
    assert_eq!(local_done.best_energy_milli, default_done.best_energy_milli);
}

#[tokio::test]
async fn a_local_generator_sees_the_admitted_inputs() {
    let mut s = Session::start_with("mock_sampler_family_lease", false, &["--mode", "local"]).await;
    s.setup(i64::MAX).await;
    let j = family_job(4, &UNIFORM_V1, &P_SPEC);
    s.send(coord_msg::Msg::Job(j)).await;
    let digest = hex(&order_digest(&topology()));
    s.wait_for_log(&format!(
        "family lease inputs: family_id=vectors/uniform-field revision=1 order_digest={digest}"
    ))
    .await;
    let _ = drain_lease(&mut s).await;
    s.refund().await;
    s.finish().await;
}

#[tokio::test]
async fn a_lease_of_one_skipped_salt_still_finishes() {
    let mut s = Session::start("mock_sampler_family_lease").await;
    s.setup(i64::MAX).await;
    let mut j = family_job(1, &UNIFORM_V1, &P_SPEC);
    j.generator.as_mut().unwrap().salt_start = 5 + SKIPPED_ONE;
    s.send(coord_msg::Msg::Job(j)).await;
    let (results, done) = drain_lease(&mut s).await;
    assert!(results.is_empty());
    assert_eq!((done.salts_done, done.best_energy_milli), (1, i64::MAX));
    s.refund().await;
    // Exactly one refund: the next reply answers this Ping.
    s.ack().await;
    s.finish().await;
}

#[tokio::test]
async fn a_topology_change_during_a_lease_does_not_change_its_salts() {
    let mut s = Session::start_with_gate("mock_sampler_family_lease", true).await;
    s.setup(i64::MAX).await;
    let j = family_job(8, &UNIFORM_V1, &P_SPEC);
    s.send(coord_msg::Msg::Job(j.clone())).await;
    let mut held = s.blocked_sample().await;
    let reordered = vector_topology("sparse-edges-reordered");
    s.send(coord_msg::Msg::Topology(reordered.clone())).await;
    s.ack().await;
    release(&mut held).await;
    let releases = s.release_remaining();
    let (results, done) = drain_lease(&mut s).await;
    assert_eq!(done.salts_done, 8);
    assert!(!results.is_empty());
    let mut differs = false;
    for r in &results {
        let energy = check_result(r, &UNIFORM_V1, &P_SPEC, &topology());
        let nonce: [u8; 32] = r.nonce.as_slice().try_into().unwrap();
        differs |= regenerate(&UNIFORM_V1, &P_SPEC, &reordered, nonce)
            .map_or(true, |other| all_up_energy(&other) != energy);
    }
    assert!(differs, "the reordered topology must give other instances");
    s.refund().await;
    // The session now caches T_REORDERED: the T_SPARSE digest is code 11.
    s.send(coord_msg::Msg::Job(family_job(1, &UNIFORM_V1, &P_SPEC)))
        .await;
    expect_reject(
        &mut s,
        wire::RejectReason::TopologyMismatch as i32,
        "T_SPARSE digest",
    )
    .await;
    s.finish().await;
    releases.abort();
}

#[tokio::test]
async fn a_second_lease_does_not_change_an_admitted_lease() {
    let mut s = Session::start_with_gate("mock_sampler_family_lease", true).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(family_job(8, &UNIFORM_V1, &P_SPEC)))
        .await;
    let mut held = s.blocked_sample().await;
    let mut b = family_job(8, &UNIFORM_V1, &P_L2_F0);
    b.job_id = b"lease-b".to_vec();
    s.send(coord_msg::Msg::Job(b)).await;
    s.ack().await;
    release(&mut held).await;
    let releases = s.release_remaining();
    let (mut done, mut refunds, mut seen) = (Vec::new(), 0, [0, 0]);
    while done.len() < 2 || refunds < 2 {
        match s.recv().await {
            miner_msg::Msg::Result(r) if r.job_id == b"lease" => {
                let _ = check_result(&r, &UNIFORM_V1, &P_SPEC, &topology());
                seen[0] += 1;
            }
            miner_msg::Msg::Result(r) if r.job_id == b"lease-b" => {
                let _ = check_result(&r, &UNIFORM_V1, &P_L2_F0, &topology());
                seen[1] += 1;
            }
            miner_msg::Msg::LeaseDone(d) => {
                assert_eq!(d.salts_done, 8);
                done.push(d.job_id);
            }
            miner_msg::Msg::JobRequest(r) => {
                assert_eq!(r.credits, 1);
                refunds += 1;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    done.sort();
    assert_eq!(done, vec![b"lease".to_vec(), b"lease-b".to_vec()]);
    assert!(seen[0] > 0 && seen[1] > 0, "both leases report salts");
    s.finish().await;
    releases.abort();
}

#[tokio::test]
async fn a_rejected_family_lease_keeps_the_session_and_its_credit() {
    let mut s = Session::start("mock_sampler_family_lease").await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(family_job(4, &RESTRICTED_V1, &P_L2_F0)))
        .await;
    expect_reject(
        &mut s,
        wire::RejectReason::FamilyUnsupported as i32,
        "code 10",
    )
    .await;
    s.send(coord_msg::Msg::Job(job(3))).await;
    let (results, done) = drain_lease(&mut s).await;
    assert_eq!(results.len(), 3);
    results.iter().for_each(verify);
    assert_eq!(done.salts_done, 3);
    s.refund().await;
    s.send(coord_msg::Msg::Job(wire::Job {
        job_id: vec![1],
        kind: wire::JobKind::IsingSample as i32,
        ising: Some(wire::IsingProblem {
            encoding: wire::CoefficientEncoding::I32 as i32,
            scale: 1000,
            h: quip_protocol::wire::encode_i32_le(&[1000]),
            num_reads: 1,
            ..Default::default()
        }),
        ..Default::default()
    }))
    .await;
    match s.recv().await {
        miner_msg::Msg::Result(r) => assert_eq!(r.job_id, vec![1]),
        other => panic!("expected the plain Result, got {other:?}"),
    }
    s.refund().await;
    // One refund per job: the next reply answers this Ping.
    s.ack().await;
    s.finish().await;
}

#[tokio::test]
async fn cancel_stops_a_family_lease_and_reports_what_finished() {
    let mut s = Session::start("mock_sampler_family_lease").await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(family_job(
        10_000,
        &UNIFORM_V1,
        &P_L2_F0,
    )))
    .await;
    let miner_msg::Msg::Result(first) = s.recv().await else {
        panic!("expected a Result")
    };
    let _ = check_result(&first, &UNIFORM_V1, &P_L2_F0, &topology());
    let start = std::time::Instant::now();
    s.send(coord_msg::Msg::Cancel(wire::Cancel { max_generation: 1 }))
        .await;
    let (mut ack, mut done, mut refund) = (false, false, false);
    while !(ack && done && refund) {
        match s.recv().await {
            miner_msg::Msg::Status(v) => {
                assert_eq!(v.abandoned_generation, 1);
                ack = true;
            }
            miner_msg::Msg::Result(r) => {
                assert!(!ack);
                let _ = check_result(&r, &UNIFORM_V1, &P_L2_F0, &topology());
            }
            miner_msg::Msg::LeaseDone(d) => {
                assert!(!done);
                assert!(d.salts_done >= 1 && d.salts_done < 10_000);
                assert!(start.elapsed() < Duration::from_secs(1));
                done = true;
            }
            miner_msg::Msg::JobRequest(r) => {
                assert!(done);
                assert_eq!(r.credits, 1);
                refund = true;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    s.finish().await;
}

#[tokio::test]
async fn a_deadline_stops_a_family_lease() {
    let mut s = Session::start_with_gate("mock_sampler_family_lease", true).await;
    s.setup(i64::MAX).await;
    let mut lease = family_job(40, &UNIFORM_V1, &P_SPEC);
    lease.deadline_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
        + 500;
    s.send(coord_msg::Msg::Job(lease)).await;
    let mut held = s.blocked_sample().await;
    let (results, done) = drain_lease(&mut s).await;
    assert!(results.is_empty(), "the held salt never finished");
    // Salt 0 is skipped before the held salt; later skips may count too.
    let skipped_below_40 = SKIPPED_64.iter().filter(|&&i| i < 40).count();
    assert!(done.salts_done >= 1);
    assert!(done.salts_done <= u64::try_from(skipped_below_40).unwrap());
    s.refund().await;
    release(&mut held).await;
    // Salts handed over behind the held one belong to a reported lease. They
    // add no Result, summary, or refund.
    let releases = s.release_remaining();
    s.ack().await;
    s.finish().await;
    releases.abort();
}

#[tokio::test]
async fn a_generic_solver_answers_a_family_lease_with_code_5() {
    let mut s = Session::start("mock_sampler_lease").await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(family_job(4, &UNIFORM_V1, &P_SPEC)))
        .await;
    expect_reject(
        &mut s,
        wire::RejectReason::FamilyUnsupported as i32,
        "FAM-54",
    )
    .await;
    s.finish().await;
}

#[tokio::test]
async fn legacy_leases_are_unchanged_on_a_family_binary() {
    let mut s = Session::start("mock_sampler_family_lease").await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(3))).await;
    let (mut results, done) = drain_lease(&mut s).await;
    assert_eq!(results.len(), 3);
    results.iter().for_each(verify);
    results.sort_by(|a, b| a.salt.cmp(&b.salt));
    let pinned = results
        .iter()
        .map(|r| {
            let solutions = wire::Result {
                solutions: r.solutions.clone(),
                ..Default::default()
            };
            (hex(&r.salt), hex(&r.nonce), hex(&solutions.encode_to_vec()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        pinned,
        PINNED_RESULTS.map(|(a, b, c)| (a.into(), b.into(), c.into()))
    );
    assert_eq!(
        (done.salts_done, done.best_energy_milli),
        (3, PINNED_BEST_ENERGY_MILLI)
    );
    s.refund().await;
    s.finish().await;
}
#[expect(clippy::unwrap_used, reason = "test barrier must release the sampler")]
async fn release(stream: &mut tokio::net::UnixStream) {
    use tokio::io::AsyncWriteExt as _;
    stream.write_all(&[1]).await.unwrap();
}
