//! Lease protocol tests against a real session and deterministic sampler.
use quip_proto::v1::{
    self as wire, coord_msg, miner_msg,
    miner_service_server::{MinerService, MinerServiceServer},
};
use quip_protocol::{
    lease::{verify_lease_result, LeaseSpec, TopologyView},
    target::Target,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    time::{Duration, Instant},
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
}
impl Drop for Session {
    fn drop(&mut self) {
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
    async fn start(zero: bool) -> Self {
        Self::start_with_gate(zero, false).await
    }
    async fn start_with_gate(zero: bool, gated: bool) -> Self {
        Self::start_mode(zero, gated, None).await
    }
    async fn start_mode(zero: bool, gated: bool, mode: Option<&str>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let bin = default_binary();
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
        let local = mode.map(|_| local_binary());
        let mut command = tokio::process::Command::new(local.as_ref().unwrap_or(&bin));
        if let Some(mode) = mode {
            let _ = command
                .args(["--mode", mode])
                .stderr(std::process::Stdio::piped());
        }
        let _ = command
            .args([
                "--quip-coordinator",
                &format!("unix://{}", socket.display()),
                "--log-level",
                "error",
            ])
            .env("QUIP_SESSION_TOKEN", "test")
            .kill_on_drop(true);
        if zero {
            let _ = command.arg("--zero");
        }
        if gated {
            let _ = command.arg("--gate").arg(gate_path);
        }
        let child = command.spawn().unwrap();
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
        };
        let miner_msg::Msg::Hello(hello) = s.recv().await else {
            panic!("expected Hello")
        };
        let caps = hello.capabilities.unwrap();
        assert_eq!(caps.protocol_version, 2);
        assert!(caps
            .supported_kinds
            .contains(&(wire::JobKind::IsingGenerate as i32)));
        if gated {
            assert_eq!(caps.stream_width, 1);
        }
        assert_eq!(
            caps.generators,
            vec![wire::GeneratorAlgorithm::Blake3Chacha8V1 as i32]
        );
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
    async fn blocked_sample(&self) -> tokio::net::UnixStream {
        tokio::time::timeout(Duration::from_secs(10), self.gate.accept())
            .await
            .unwrap()
            .unwrap()
            .0
    }
    async fn ack(&mut self) {
        self.send(coord_msg::Msg::Ping(wire::Ping {})).await;
        assert!(matches!(self.recv().await, miner_msg::Msg::Status(_)));
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
#[expect(clippy::unwrap_used, reason = "test barrier must release the sampler")]
async fn release(stream: &mut tokio::net::UnixStream) {
    use tokio::io::AsyncWriteExt as _;
    stream.write_all(&[1]).await.unwrap();
}
fn topology() -> wire::Topology {
    wire::Topology {
        hash: vec![7; 32],
        nodes: vec![10, 20],
        edges: Some(wire::EdgeList {
            u: vec![10],
            v: vec![20],
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
fn job(count: u64) -> wire::Job {
    wire::Job {
        job_id: b"lease".to_vec(),
        kind: wire::JobKind::IsingGenerate as i32,
        generation: 1,
        generator: Some(wire::IsingProblemGenerator {
            algorithm: wire::GeneratorAlgorithm::Blake3Chacha8V1 as i32,
            topology_hash: vec![7; 32],
            last_proof_block_hash: vec![1; 32],
            miner_account: vec![2; 32],
            base_salt: vec![0; 32],
            salt_start: 10,
            salt_count: count,
        }),
        ..Default::default()
    }
}
#[expect(
    clippy::unwrap_used,
    reason = "verification inputs are fixed valid fixtures"
)]
fn verify(result: &wire::Result) {
    assert_eq!(result.job_id, b"lease");
    assert!(verify_lease_result(
        &job(10_000).generator.unwrap(),
        &TopologyView::from_proto(&topology()).unwrap(),
        &Target::from_proto(&target(i64::MAX)),
        result
    )
    .is_ok());
}
#[expect(clippy::panic, reason = "a missing winner is a test failure")]
async fn first(s: &mut Session) {
    let miner_msg::Msg::Result(r) = s.recv().await else {
        panic!("expected winner")
    };
    verify(&r);
}

#[tokio::test]
async fn a_lease_reports_winners_then_one_lease_done() {
    let mut s = Session::start(false).await;
    s.setup(i64::MAX).await;
    let j = job(5);
    let spec = LeaseSpec::from_proto(j.generator.as_ref().unwrap()).unwrap();
    s.send(coord_msg::Msg::Job(j)).await;
    let mut salts = Vec::new();
    for _ in 0..5 {
        let miner_msg::Msg::Result(r) = s.recv().await else {
            panic!("expected winner")
        };
        verify(&r);
        salts.push(r.salt);
    }
    salts.sort();
    let mut expected = (0..5)
        .map(|i| spec.salt(i).unwrap().to_vec())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(salts, expected);
    assert!(matches!(s.recv().await, miner_msg::Msg::LeaseDone(d) if d.salts_done == 5));
    s.refund().await;
    s.finish().await;
}
#[tokio::test]
async fn a_lease_with_no_winners_still_reports_its_count() {
    let mut s = Session::start(false).await;
    s.setup(i64::MIN).await;
    let j = job(5);
    let spec = LeaseSpec::from_proto(j.generator.as_ref().unwrap()).unwrap();
    let top = TopologyView::from_proto(&topology()).unwrap();
    let best = (0..5)
        .map(|i| {
            let (h, j) = top.draw(spec.nonce(i).unwrap()).unwrap();
            quip_protocol::scoring::energy_from_milli(&[1, 1], &h, &j, &top.edges)
        })
        .min()
        .unwrap();
    s.send(coord_msg::Msg::Job(j)).await;
    assert!(
        matches!(s.recv().await, miner_msg::Msg::LeaseDone(d) if d.salts_done == 5 && d.best_energy_milli == best)
    );
    s.refund().await;
    s.finish().await;
}
#[tokio::test]
async fn malformed_leases_are_rejected_before_drawing() {
    let mut s = Session::start(false).await;
    s.setup(i64::MAX).await;
    for variant in 0..4 {
        let mut j = job(5);
        let g = j.generator.as_mut().unwrap();
        match variant {
            0 => g.salt_count = 0,
            1 => g.salt_start = u64::MAX,
            2 => g.base_salt = vec![0; 31],
            _ => g.algorithm = 0,
        }
        s.send(coord_msg::Msg::Job(j)).await;
        assert!(
            matches!(s.recv().await, miner_msg::Msg::Reject(r) if r.reason == wire::RejectReason::Malformed as i32)
        );
        s.refund().await;
    }
    s.finish().await;
}
#[tokio::test]
async fn a_lease_needs_a_topology_and_a_target() {
    let mut s = Session::start(false).await;
    for (index, reason) in [
        wire::RejectReason::TopologyMissing,
        wire::RejectReason::TargetMissing,
        wire::RejectReason::TargetMissing,
    ]
    .into_iter()
    .enumerate()
    {
        s.send(coord_msg::Msg::Job(job(5))).await;
        assert!(matches!(s.recv().await, miner_msg::Msg::Reject(r) if r.reason == reason as i32));
        s.refund().await;
        if index == 0 {
            s.send(coord_msg::Msg::Topology(topology())).await;
        }
        if index == 1 {
            let mut t = target(i64::MAX);
            t.max_proof_solutions = 0;
            s.send(coord_msg::Msg::SetTarget(t)).await;
        }
    }
    s.finish().await;
}
#[tokio::test]
async fn cancel_stops_a_lease_and_reports_what_finished() {
    let mut s = Session::start(false).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(10_000))).await;
    first(&mut s).await;
    let start = Instant::now();
    s.send(coord_msg::Msg::Cancel(wire::Cancel { max_generation: 1 }))
        .await;
    let mut ack = false;
    let mut done = false;
    let mut refund = false;
    while !(ack && done && refund) {
        match s.recv().await {
            miner_msg::Msg::Status(v) => {
                assert_eq!(v.abandoned_generation, 1);
                ack = true;
            }
            miner_msg::Msg::Result(r) => {
                assert!(!ack);
                verify(&r);
            }
            miner_msg::Msg::LeaseDone(d) => {
                assert!(!done);
                assert!(d.salts_done < 10_000);
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
async fn plain_jobs_interleave_with_a_lease() {
    let mut s = Session::start(false).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(10_000))).await;
    first(&mut s).await;
    for id in 0..3 {
        s.send(coord_msg::Msg::Job(wire::Job {
            job_id: vec![id],
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
    }
    let mut ids = Vec::new();
    while ids.len() < 3 {
        let miner_msg::Msg::Result(r) = s.recv().await else {
            panic!("lease ended before plain jobs")
        };
        if r.job_id == b"lease" {
            verify(&r);
        } else {
            ids.push(r.job_id);
            s.refund().await;
        }
    }
    ids.sort();
    assert_eq!(ids, vec![vec![0], vec![1], vec![2]]);
    s.send(coord_msg::Msg::Cancel(wire::Cancel { max_generation: 1 }))
        .await;
    let mut ack = false;
    let mut done = false;
    let mut refunded = false;
    while !(ack && done && refunded) {
        match s.recv().await {
            miner_msg::Msg::Status(_) => ack = true,
            miner_msg::Msg::LeaseDone(_) => {
                assert!(!done);
                done = true;
            }
            miner_msg::Msg::JobRequest(r) => {
                assert!(done);
                assert_eq!(r.credits, 1);
                refunded = true;
            }
            miner_msg::Msg::Result(r) => {
                assert!(!ack);
                verify(&r);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    s.finish().await;
}
#[tokio::test]
async fn a_new_target_applies_to_later_salts_and_the_topology_is_a_snapshot() {
    let mut s = Session::start_with_gate(false, true).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(40))).await;
    release(&mut s.blocked_sample().await).await;
    first(&mut s).await;
    let mut before_update = s.blocked_sample().await;
    let mut top = topology();
    top.hash = vec![8; 32];
    top.allowed_h_milli = vec![9000];
    s.send(coord_msg::Msg::Topology(top)).await;
    s.ack().await;
    release(&mut before_update).await;
    // This salt was drawn before the update.
    first(&mut s).await;
    // Width one keeps the next draw behind the blocked salt's completion.
    release(&mut s.blocked_sample().await).await;
    let miner_msg::Msg::Result(confirming) = s.recv().await else {
        panic!("expected confirming winner")
    };
    verify(&confirming);
    let spec = LeaseSpec::from_proto(job(40).generator.as_ref().unwrap()).unwrap();
    assert_eq!(
        confirming.salt,
        spec.salt(2).unwrap(),
        "confirmation must be drawn after the acknowledged update"
    );
    let mut after_confirmation = s.blocked_sample().await;
    s.send(coord_msg::Msg::SetTarget(target(i64::MIN))).await;
    s.ack().await;
    release(&mut after_confirmation).await;
    let releases = s.release_remaining();
    // No winner can pass the acknowledged target, including the blocked salt.
    assert!(matches!(s.recv().await, miner_msg::Msg::LeaseDone(d) if d.salts_done == 40));
    s.refund().await;
    s.finish().await;
    releases.abort();
}
#[tokio::test]
async fn zero_read_salts_count_but_never_win() {
    let mut s = Session::start(true).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(5))).await;
    assert!(
        matches!(s.recv().await, miner_msg::Msg::LeaseDone(d) if d.salts_done == 5 && d.best_energy_milli == i64::MAX)
    );
    s.refund().await;
    s.finish().await;
}
#[tokio::test]
async fn shutdown_drains_in_flight_winners_then_lease_done() {
    let mut s = Session::start(false).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(10_000))).await;
    first(&mut s).await;
    s.send(coord_msg::Msg::Shutdown(wire::Shutdown { grace_ms: 2000 }))
        .await;
    let mut done = false;
    while let Some(msg) = tokio::time::timeout(Duration::from_secs(10), s.inbound.message())
        .await
        .unwrap()
        .unwrap()
    {
        match msg.msg.unwrap() {
            miner_msg::Msg::Result(r) => {
                assert!(!done);
                verify(&r);
            }
            miner_msg::Msg::LeaseDone(d) => {
                assert!(!done);
                assert!(d.salts_done < 10_000);
                done = true;
            }
            miner_msg::Msg::JobRequest(r) => {
                assert!(done);
                assert_eq!(r.credits, 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(done);
    s.tx = mpsc::channel(1).0;
    assert!(s.child.wait().await.unwrap().success());
}
#[tokio::test]
async fn an_expired_lease_is_rejected_and_a_deadline_mid_lease_stops_it() {
    let mut s = Session::start(false).await;
    s.setup(i64::MAX).await;
    let mut j = job(5);
    j.deadline_ms = 1;
    s.send(coord_msg::Msg::Job(j)).await;
    assert!(
        matches!(s.recv().await, miner_msg::Msg::Reject(r) if r.reason == wire::RejectReason::Expired as i32)
    );
    s.refund().await;
    let start = Instant::now();
    let mut j = job(10_000);
    j.deadline_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
        + 5000;
    // Model a coordinator task descheduled between constructing and sending a job.
    let _ = tokio::time::timeout(Duration::from_millis(300), std::future::pending::<()>()).await;
    let deadline_ms = j.deadline_ms;
    s.send(coord_msg::Msg::Job(j)).await;
    first(&mut s).await;
    loop {
        match s.recv().await {
            miner_msg::Msg::Result(r) => verify(&r),
            miner_msg::Msg::LeaseDone(d) => {
                assert!(d.salts_done < 10_000);
                assert!(start.elapsed() < Duration::from_secs(7));
                assert!(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis()
                        >= u128::from(deadline_ms)
                );
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    s.refund().await;
    s.finish().await;
}

#[tokio::test]
async fn plain_id_equal_to_an_old_salt_id_keeps_its_metadata_and_credit() {
    let mut s = Session::start_with_gate(false, true).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(2))).await;
    let mut salt = s.blocked_sample().await;
    let mut plain_id = b"lease".to_vec();
    plain_id.extend_from_slice(&0u64.to_le_bytes());
    s.send(coord_msg::Msg::Job(wire::Job {
        job_id: plain_id.clone(),
        kind: wire::JobKind::IsingSample as i32,
        ising: Some(wire::IsingProblem {
            encoding: wire::CoefficientEncoding::I32 as i32,
            scale: 1000,
            h: quip_protocol::wire::encode_i32_le(&[1000]),
            num_reads: 3,
            num_sweeps: 7,
            ..Default::default()
        }),
        ..Default::default()
    }))
    .await;
    s.ack().await;
    release(&mut salt).await;
    let releases = s.release_remaining();
    let mut plain = 0;
    let mut winners = 0;
    let mut done = 0;
    let mut refunds = 0;
    while refunds < 2 {
        match s.recv().await {
            miner_msg::Msg::Result(r) if r.job_id == plain_id => {
                plain += 1;
                assert!(r.salt.is_empty());
                assert!(r.nonce.is_empty());
                let meta = r.meta.unwrap();
                assert_eq!((meta.reads, meta.sweeps), (3, 7));
                assert_eq!(r.solutions.len(), 3);
                assert!(r.solutions.iter().all(|v| v.energy_milli == 1000));
                s.refund().await;
                refunds += 1;
            }
            miner_msg::Msg::Result(r) => {
                verify(&r);
                winners += 1;
            }
            miner_msg::Msg::LeaseDone(d) => {
                assert_eq!(d.job_id, b"lease");
                assert_eq!(d.salts_done, 2);
                done += 1;
                s.refund().await;
                refunds += 1;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!((plain, winners, done, refunds), (1, 2, 1, 2));
    s.finish().await;
    releases.abort();
}

fn local_binary() -> std::path::PathBuf {
    static BIN: OnceLock<std::path::PathBuf> = OnceLock::new();
    BIN.get_or_init(|| build_example("mock_sampler_lease_local"))
        .clone()
}
fn default_binary() -> std::path::PathBuf {
    static BIN: OnceLock<std::path::PathBuf> = OnceLock::new();
    BIN.get_or_init(|| build_example("mock_sampler_lease"))
        .clone()
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

#[tokio::test]
async fn a_local_generator_with_a_correct_draw_wins() {
    let mut s = Session::start_mode(false, false, Some("honest")).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(5))).await;
    let mut salts = std::collections::HashSet::new();
    for _ in 0..5 {
        let miner_msg::Msg::Result(r) = s.recv().await else {
            panic!("expected winner")
        };
        verify(&r);
        assert!(salts.insert(r.salt));
    }
    assert!(matches!(s.recv().await, miner_msg::Msg::LeaseDone(d) if d.salts_done == 5));
    s.refund().await;
    s.finish().await;
}

#[tokio::test]
async fn a_wrong_device_draw_is_a_device_fault() {
    let mut s = Session::start_mode(false, false, Some("wrong-draw")).await;
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(5))).await;
    assert!(matches!(s.recv().await, miner_msg::Msg::Fatal(f)
        if f.restart_required && f.exit_code == 70));
    s.tx = mpsc::channel(1).0;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), s.child.wait())
            .await
            .unwrap()
            .unwrap()
            .code(),
        Some(70)
    );
}

#[tokio::test]
async fn a_sampler_that_ignores_stop_sends_nothing_after_cancel() {
    use tokio::io::AsyncBufReadExt as _;
    let mut s = Session::start_mode(false, false, Some("ignore-stop")).await;
    let mut logs = tokio::io::BufReader::new(s.child.stderr.take().unwrap()).lines();
    s.setup(i64::MAX).await;
    s.send(coord_msg::Msg::Job(job(u64::MAX - 10))).await;
    first(&mut s).await;
    s.send(coord_msg::Msg::Cancel(wire::Cancel { max_generation: 1 }))
        .await;
    loop {
        match s.recv().await {
            miner_msg::Msg::Result(r) => verify(&r),
            miner_msg::Msg::Status(_) => {}
            miner_msg::Msg::LeaseDone(d) => {
                assert!(d.salts_done >= 1);
                break;
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
    s.refund().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(line) = logs.next_line().await.unwrap() {
            if line.contains("push returned LeaseStopped") {
                return;
            }
        }
        panic!("missing stop acknowledgement");
    })
    .await
    .unwrap();
    s.ack().await;
    s.finish().await;
}

#[tokio::test]
async fn capabilities_are_the_same_for_both_paths() {
    let s = Session::start(false).await;
    let default = local_binary().with_file_name("mock_sampler_lease");
    let caps = |bin: &std::path::Path| {
        let output = std::process::Command::new(bin)
            .arg("--capabilities")
            .output()
            .unwrap();
        assert!(output.status.success());
        let mut value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let object = value.as_object_mut().unwrap();
        let _ = object.remove("backend");
        let _ = object.remove("algorithm");
        value
    };
    assert_eq!(caps(&local_binary()), caps(&default));
    s.finish().await;
}
