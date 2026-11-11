//! Request EOF must reach the coordinator before the public runner exits.

use quip_proto::v1::{
    self as wire, coord_msg, miner_msg,
    miner_service_server::{MinerService, MinerServiceServer},
};
use quip_solver_core::{
    run_code, BackendIdentity, CancelToken, CommonArgs, ExitCode, IsingGraph, SampleError,
    SampleParams, Sampler, SamplerResult, StreamJob, StreamOutcome, StreamResult,
};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_stream::wrappers::{ReceiverStream, UnixListenerStream};
use tonic::{Request, Response, Status, Streaming};

type Error = Box<dyn std::error::Error + Send + Sync>;
type Link = (
    Streaming<wire::MinerMsg>,
    mpsc::Sender<Result<wire::CoordMsg, Status>>,
);

struct Coordinator(Mutex<Option<oneshot::Sender<Link>>>);

#[tonic::async_trait]
impl MinerService for Coordinator {
    type SessionStream = ReceiverStream<Result<wire::CoordMsg, Status>>;

    async fn session(
        &self,
        request: Request<Streaming<wire::MinerMsg>>,
    ) -> Result<Response<Self::SessionStream>, Status> {
        let (tx, rx) = mpsc::channel(32);
        self.0
            .lock()
            .await
            .take()
            .ok_or_else(|| Status::internal("unexpected second session"))?
            .send((request.into_inner(), tx))
            .map_err(|_| Status::internal("test ended"))?;
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

struct FinalSampler;

impl Sampler for FinalSampler {
    fn sample(
        &self,
        graph: &IsingGraph,
        _params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        Ok(vec![SamplerResult {
            spins: vec![1; graph.num_nodes()],
            energy_milli: 1000,
        }])
    }

    fn sample_stream(
        &self,
        mut jobs: mpsc::Receiver<StreamJob>,
        out: mpsc::Sender<StreamResult>,
        _cancel: CancelToken,
    ) {
        let mut pending = Vec::new();
        while let Some(job) = jobs.blocking_recv() {
            pending.push(job);
        }
        // The session closes jobs only after Shutdown. Every result therefore
        // enters the outbound writer during shutdown, without a timing delay.
        for job in pending {
            let outcome = StreamOutcome::Completed(self.sample(&job.graph, &job.params));
            if out
                .blocking_send(StreamResult {
                    job_id: job.job_id,
                    outcome,
                    device_access_time_us: 0,
                })
                .is_err()
            {
                return;
            }
        }
    }
}

#[test]
fn miner_process() {
    let Ok(socket) = std::env::var("QUIP_CLOSE_TEST_SOCKET") else {
        return;
    };
    let args = CommonArgs {
        quip_coordinator: Some(socket),
        miner_id: Some("transport-close-test".into()),
        capabilities: false,
        solve: false,
        check: false,
        log_level: "warn".into(),
        sweeps_per_beta: None,
    };
    let identity = BackendIdentity {
        backend: wire::Backend::Mock,
        algorithm: wire::Algorithm::Sa,
        max_nodes: 100,
        max_edges: 200,
        features: &[],
        adapt: quip_solver_core::adapt::AdaptBounds {
            min_sweeps: 1,
            max_sweeps: 1,
            min_reads: 1,
            max_reads: 1,
            reads_solution_min_factor: 1,
            reads_solution_max_factor: 1,
            reads_solution_floor_factor: 0,
        },
        limits: quip_solver_core::ModelLimits::UNSTATED,
    };
    assert_eq!(
        run_code(identity, &args, || Ok(FinalSampler)),
        ExitCode::Clean
    );
}

struct Server {
    task: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
    dir: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn send(
    tx: &mpsc::Sender<Result<wire::CoordMsg, Status>>,
    msg: coord_msg::Msg,
) -> Result<(), Error> {
    tx.send(Ok(wire::CoordMsg { msg: Some(msg) })).await?;
    Ok(())
}

async fn recv(inbound: &mut Streaming<wire::MinerMsg>) -> Result<miner_msg::Msg, Error> {
    inbound
        .message()
        .await?
        .and_then(|message| message.msg)
        .ok_or_else(|| "unexpected request EOF".into())
}

async fn prepare(
    inbound: &mut Streaming<wire::MinerMsg>,
    tx: &mpsc::Sender<Result<wire::CoordMsg, Status>>,
    queued: bool,
) -> Result<(), Error> {
    assert!(matches!(recv(inbound).await?, miner_msg::Msg::Hello(_)));
    send(
        tx,
        coord_msg::Msg::Welcome(wire::Welcome {
            protocol_version: 2,
        }),
    )
    .await?;
    send(
        tx,
        coord_msg::Msg::Configure(wire::Configure {
            queue_depth: 8,
            ..Default::default()
        }),
    )
    .await?;
    assert!(matches!(recv(inbound).await?, miner_msg::Msg::Ready(_)));
    assert!(matches!(
        recv(inbound).await?,
        miner_msg::Msg::JobRequest(_)
    ));
    if queued {
        for id in 0..8u8 {
            send(
                tx,
                coord_msg::Msg::Job(wire::Job {
                    job_id: vec![id],
                    kind: wire::JobKind::IsingSample as i32,
                    ising: Some(wire::IsingProblem {
                        encoding: wire::CoefficientEncoding::I32 as i32,
                        scale: 1000,
                        h: 1000i32.to_le_bytes().to_vec(),
                        num_reads: 1,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )
            .await?;
        }
    }
    Ok(())
}

async fn shutdown_once(queued: bool, iteration: usize) -> Result<(), Error> {
    // A Unix socket path must fit in `sun_path` (108 bytes on Linux), so the
    // socket lives under the short system temporary directory, not the checkout.
    let dir = std::env::temp_dir().join(format!(
        "qtc-{}-{}-{iteration}",
        std::process::id(),
        u8::from(queued)
    ));
    std::fs::create_dir_all(&dir)?;
    let socket = dir.join("s");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let (link_tx, link_rx) = oneshot::channel();
    let _server = Server {
        task: tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(MinerServiceServer::new(Coordinator(Mutex::new(Some(
                    link_tx,
                )))))
                .serve_with_incoming(UnixListenerStream::new(listener)),
        ),
        dir,
    };
    let mut child = tokio::process::Command::new(std::env::current_exe()?)
        .args(["--exact", "miner_process", "--nocapture"])
        .env(
            "QUIP_CLOSE_TEST_SOCKET",
            format!("unix://{}", socket.display()),
        )
        .env("QUIP_SESSION_TOKEN", "test")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let (mut inbound, tx) = link_rx.await?;
    prepare(&mut inbound, &tx, queued).await?;
    send(
        &tx,
        coord_msg::Msg::Shutdown(wire::Shutdown { grace_ms: 2000 }),
    )
    .await?;
    // Close the response immediately. Waiting for request EOF first masks the bug.
    drop(tx);
    let mut results = Vec::new();
    let mut credits = 0;
    let drain = async {
        while let Some(message) = inbound.message().await? {
            match message.msg {
                Some(miner_msg::Msg::Result(result)) => results.push(result.job_id),
                Some(miner_msg::Msg::JobRequest(request)) => credits += request.credits,
                other => return Err(format!("unexpected final message: {other:?}").into()),
            }
        }
        Ok::<_, Error>(())
    }
    .await;
    // Observe the exit even when EOF fails: a clean miner exit is the defect.
    let status = child.wait().await?;
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use tokio::io::AsyncReadExt as _;
        let _ = pipe.read_to_string(&mut stderr).await?;
    }
    if let Err(error) = drain {
        return Err(format!(
            "request EOF failed: {error}; results={}/{}; refunds={}/{}; child={status}; stderr={stderr}",
            results.len(),
            if queued { 8 } else { 0 },
            credits,
            if queued { 8 } else { 0 },
        )
        .into());
    }
    if !status.success() {
        return Err(format!("miner failed: {status}; stderr={stderr}").into());
    }
    results.sort();
    let expected: Vec<Vec<u8>> = if queued {
        (0..8).map(|id| vec![id]).collect()
    } else {
        vec![]
    };
    if results != expected || credits != if queued { 8 } else { 0 } {
        return Err(
            format!("final results={results:?}, credits={credits}, expected={expected:?}").into(),
        );
    }
    Ok(())
}

/// Run `default_iterations` shutdowns, or `QUIP_CLOSE_TEST_ITERATIONS` when it
/// is set, so that a longer campaign needs no code change. The campaign prints
/// its run time, and each failed run prints the count of results and refunds
/// that the coordinator received.
#[expect(
    clippy::print_stderr,
    reason = "report every failure and the campaign totals"
)]
async fn campaign(queued: bool, default_iterations: usize) -> Result<(), Error> {
    let iterations = std::env::var("QUIP_CLOSE_TEST_ITERATIONS")
        .map_or(Ok(default_iterations), |value| value.parse::<usize>())?;
    assert!(iterations > 0);
    let started = std::time::Instant::now();
    let mut failures = 0;
    for iteration in 0..iterations {
        let result =
            tokio::time::timeout(Duration::from_secs(10), shutdown_once(queued, iteration)).await;
        if !matches!(result, Ok(Ok(()))) {
            failures += 1;
            eprintln!("queued={queued} iteration={iteration}: {result:?}");
        }
    }
    eprintln!(
        "queued={queued}: {failures}/{iterations} failures in {:.2}s",
        started.elapsed().as_secs_f64()
    );
    assert_eq!(failures, 0, "request stream must end cleanly in every run");
    Ok(())
}

#[tokio::test]
async fn idle_shutdown_finishes_request_stream() -> Result<(), Error> {
    campaign(false, 20).await
}

#[tokio::test]
async fn shutdown_flushes_final_results_and_finishes_request_stream() -> Result<(), Error> {
    campaign(true, 20).await
}
