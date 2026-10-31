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
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let bin = build_example(binary);
        let dir = std::path::PathBuf::from(format!(
            "target/t7-tmp/{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
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
            .env("QUIP_SESSION_TOKEN", "test")
            .kill_on_drop(true);
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
